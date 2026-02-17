use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::sync::RwLock;

use super::provider::LLMProvider;
use super::types::*;

const MAX_RETRIES: usize = 3;
const BASE_BACKOFF_MS: u64 = 500;

/// Provider chain with failover support
/// Tries providers in order, tracks failures, retries with exponential backoff
pub struct ProviderChain {
    providers: Vec<Arc<dyn LLMProvider>>,
    failure_counts: Arc<RwLock<HashMap<String, AtomicUsize>>>,
    max_failures: usize,
}

impl ProviderChain {
    pub fn new(providers: Vec<Arc<dyn LLMProvider>>) -> Self {
        Self {
            providers,
            failure_counts: Arc::new(RwLock::new(HashMap::new())),
            max_failures: 5,
        }
    }

    pub fn with_max_failures(mut self, max: usize) -> Self {
        self.max_failures = max;
        self
    }

    /// Get available providers (not exceeded max failures)
    async fn available_providers(&self) -> Vec<Arc<dyn LLMProvider>> {
        let counts = self.failure_counts.read().await;
        self.providers
            .iter()
            .filter(|p| {
                counts
                    .get(p.model_name())
                    .map(|c| c.load(Ordering::Relaxed) < self.max_failures)
                    .unwrap_or(true)
            })
            .cloned()
            .collect()
    }

    /// Track a failure for a provider
    async fn track_failure(&self, model_name: &str) {
        let mut counts = self.failure_counts.write().await;
        counts
            .entry(model_name.to_string())
            .or_insert_with(|| AtomicUsize::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Reset failure count for a provider (on success)
    async fn reset_failures(&self, model_name: &str) {
        let counts = self.failure_counts.read().await;
        if let Some(count) = counts.get(model_name) {
            count.store(0, Ordering::Relaxed);
        }
    }
}

#[async_trait]
impl LLMProvider for ProviderChain {
    async fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &GenerateConfig,
    ) -> Result<GenerateResponse> {
        let available = self.available_providers().await;

        if available.is_empty() {
            return Err(anyhow!("All LLM providers have exceeded failure threshold"));
        }

        let mut last_error = None;

        for provider in &available {
            let mut last_error_msg = String::new();
            for retry in 0..MAX_RETRIES {
                if retry > 0 {
                    let backoff = if !last_error_msg.is_empty() {
                        parse_retry_delay(&last_error_msg)
                    } else {
                        Duration::from_millis(BASE_BACKOFF_MS * 2u64.pow(retry as u32))
                    };
                    tracing::info!(
                        provider = provider.model_name(),
                        retry,
                        backoff_ms = backoff.as_millis() as u64,
                        "Retrying LLM request"
                    );
                    tokio::time::sleep(backoff).await;
                }

                match provider.generate(messages, tools, config).await {
                    Ok(response) => {
                        self.reset_failures(provider.model_name()).await;

                        if retry > 0 || !std::ptr::eq(provider.as_ref(), available[0].as_ref()) {
                            tracing::info!(
                                provider = provider.model_name(),
                                "LLM request succeeded after failover"
                            );
                        }

                        return Ok(response);
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        last_error_msg = err_str.clone();
                        tracing::warn!(
                            provider = provider.model_name(),
                            error = %err_str,
                            retry,
                            "LLM request failed"
                        );

                        // Only retry on retryable errors (rate limit, server error)
                        if is_retryable(&err_str) {
                            last_error = Some(e);
                            continue;
                        } else {
                            // Non-retryable error, try next provider
                            self.track_failure(provider.model_name()).await;
                            last_error = Some(e);
                            break;
                        }
                    }
                }
            }

            // Exhausted retries for this provider
            self.track_failure(provider.model_name()).await;
        }

        Err(last_error.unwrap_or_else(|| anyhow!("All LLM providers failed")))
    }

    async fn generate_stream(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &GenerateConfig,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>> {
        let available = self.available_providers().await;

        if available.is_empty() {
            return Err(anyhow!("All LLM providers have exceeded failure threshold"));
        }

        // Try each available provider (no retry for streaming - reconnect is complex)
        let mut last_error = None;
        for provider in &available {
            match provider.generate_stream(messages, tools, config).await {
                Ok(rx) => {
                    self.reset_failures(provider.model_name()).await;
                    return Ok(rx);
                }
                Err(e) => {
                    tracing::warn!(
                        provider = provider.model_name(),
                        error = %e,
                        "Streaming request failed, trying next provider"
                    );
                    self.track_failure(provider.model_name()).await;
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("All LLM providers failed for streaming")))
    }

    fn supports_vision(&self) -> bool {
        self.providers.iter().any(|p| p.supports_vision())
    }

    fn model_name(&self) -> &str {
        self.providers
            .first()
            .map(|p| p.model_name())
            .unwrap_or("chain")
    }
}

/// Parse Retry-After delay from error message
fn parse_retry_delay(error: &str) -> Duration {
    // Check for "retry-after: N" in error text
    if let Some(idx) = error.to_lowercase().find("retry-after") {
        let rest = &error[idx..];
        if let Some(secs) = rest.split_whitespace().find_map(|s| {
            s.trim_matches(|c: char| !c.is_ascii_digit())
                .parse::<u64>()
                .ok()
        }) {
            return Duration::from_secs(secs.min(300)); // Cap at 5 min
        }
    }
    Duration::from_millis(BASE_BACKOFF_MS) // Fallback
}

/// Check if error message indicates a retryable condition
fn is_retryable(error: &str) -> bool {
    error.contains("429")
        || error.contains("529")
        || error.contains("500")
        || error.contains("502")
        || error.contains("503")
        || error.contains("rate limit")
        || error.contains("overloaded")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock provider for testing failover
    struct MockProvider {
        name: String,
        should_fail: bool,
        retryable: bool,
    }

    #[async_trait]
    impl LLMProvider for MockProvider {
        async fn generate(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &GenerateConfig,
        ) -> Result<GenerateResponse> {
            if self.should_fail {
                if self.retryable {
                    Err(anyhow!("API error (429): rate limit exceeded"))
                } else {
                    Err(anyhow!("API error (401): unauthorized"))
                }
            } else {
                Ok(GenerateResponse {
                    content: Content::Text {
                        text: format!("Response from {}", self.name),
                    },
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                    model: self.name.clone(),
                })
            }
        }

        fn supports_vision(&self) -> bool {
            false
        }

        fn model_name(&self) -> &str {
            &self.name
        }
    }

    #[tokio::test]
    async fn test_first_provider_success() {
        let chain = ProviderChain::new(vec![
            Arc::new(MockProvider {
                name: "primary".into(),
                should_fail: false,
                retryable: false,
            }),
            Arc::new(MockProvider {
                name: "fallback".into(),
                should_fail: false,
                retryable: false,
            }),
        ]);

        let resp = chain
            .generate(&[Message::user("Hi")], &[], &GenerateConfig::default())
            .await
            .unwrap();

        assert_eq!(resp.content.extract_text(), "Response from primary");
    }

    #[tokio::test]
    async fn test_failover_to_second_provider() {
        let chain = ProviderChain::new(vec![
            Arc::new(MockProvider {
                name: "primary".into(),
                should_fail: true,
                retryable: false,
            }),
            Arc::new(MockProvider {
                name: "fallback".into(),
                should_fail: false,
                retryable: false,
            }),
        ]);

        let resp = chain
            .generate(&[Message::user("Hi")], &[], &GenerateConfig::default())
            .await
            .unwrap();

        assert_eq!(resp.content.extract_text(), "Response from fallback");
    }

    #[tokio::test]
    async fn test_all_providers_fail() {
        let chain = ProviderChain::new(vec![Arc::new(MockProvider {
            name: "only".into(),
            should_fail: true,
            retryable: false,
        })]);

        let result = chain
            .generate(&[Message::user("Hi")], &[], &GenerateConfig::default())
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stream_failover() {
        let chain = ProviderChain::new(vec![Arc::new(MockProvider {
            name: "primary".into(),
            should_fail: false,
            retryable: false,
        })]);

        // Default generate_stream uses fallback (wraps generate)
        let rx = chain
            .generate_stream(&[Message::user("Hi")], &[], &GenerateConfig::default())
            .await
            .unwrap();

        // Should receive at least TextDelta + Done
        let mut chunks = Vec::new();
        let mut rx = rx;
        while let Some(chunk) = rx.recv().await {
            chunks.push(chunk);
        }
        assert!(chunks.len() >= 2);
        assert!(matches!(&chunks[0], StreamChunk::TextDelta(_)));
        assert!(matches!(chunks.last().unwrap(), StreamChunk::Done { .. }));
    }
}
