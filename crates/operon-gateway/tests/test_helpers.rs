//! Shared test helpers: mock LLM provider, test AppState factory.
#![allow(dead_code)] // helpers used across multiple test crates

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use operon_runtime::llm::{
    Content, GenerateConfig, GenerateResponse, LLMProvider, Message, StopReason, StreamChunk,
    ToolSchema, Usage,
};
use operon_runtime::Runtime;

use operon_gateway::{AppState, AuthConfig, RateLimiter, SessionManager};

/// Mock LLM provider that returns canned responses (no network)
pub struct MockLLMProvider;

#[async_trait]
impl LLMProvider for MockLLMProvider {
    async fn generate(
        &self,
        _messages: &[Message],
        _tools: &[ToolSchema],
        _config: &GenerateConfig,
    ) -> Result<GenerateResponse> {
        Ok(GenerateResponse {
            content: Content::Text {
                text: "mock response".to_string(),
            },
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
            },
            model: "mock".to_string(),
        })
    }

    async fn generate_stream(
        &self,
        _messages: &[Message],
        _tools: &[ToolSchema],
        _config: &GenerateConfig,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            let _ = tx.send(StreamChunk::TextDelta("mock".into())).await;
            let _ = tx
                .send(StreamChunk::Done {
                    stop_reason: StopReason::EndTurn,
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                    },
                })
                .await;
        });
        Ok(rx)
    }

    fn supports_vision(&self) -> bool {
        false
    }

    fn model_name(&self) -> &str {
        "mock"
    }
}

/// Build a test AppState with no auth requirement and generous rate limits.
pub fn make_test_state() -> AppState {
    let db_path = format!(
        "/tmp/silentclaw-gw-test-{}.db",
        uuid::Uuid::new_v4()
    );
    let runtime =
        Arc::new(Runtime::with_db(&db_path, true, Duration::from_secs(30)).unwrap());
    let provider: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider);
    let session_manager = Arc::new(SessionManager::new(provider, runtime));

    AppState {
        session_manager,
        auth_config: Arc::new(AuthConfig::new(None)), // no auth
        rate_limiter: Arc::new(RateLimiter::new(1000)),
        allowed_origins: vec![],
    }
}

/// Build a test AppState with auth enabled using given token.
pub fn make_auth_test_state(token: &str) -> AppState {
    let db_path = format!(
        "/tmp/silentclaw-gw-test-{}.db",
        uuid::Uuid::new_v4()
    );
    let runtime =
        Arc::new(Runtime::with_db(&db_path, true, Duration::from_secs(30)).unwrap());
    let provider: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider);
    let session_manager = Arc::new(SessionManager::new(provider, runtime));

    AppState {
        session_manager,
        auth_config: Arc::new(AuthConfig::new(Some(token.to_string()))),
        rate_limiter: Arc::new(RateLimiter::new(1000)),
        allowed_origins: vec![],
    }
}

/// Build a test AppState with a tight rate limit.
pub fn make_ratelimit_test_state(max_rpm: u32) -> AppState {
    let db_path = format!(
        "/tmp/silentclaw-gw-test-{}.db",
        uuid::Uuid::new_v4()
    );
    let runtime =
        Arc::new(Runtime::with_db(&db_path, true, Duration::from_secs(30)).unwrap());
    let provider: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider);
    let session_manager = Arc::new(SessionManager::new(provider, runtime));

    AppState {
        session_manager,
        auth_config: Arc::new(AuthConfig::new(None)),
        rate_limiter: Arc::new(RateLimiter::new(max_rpm)),
        allowed_origins: vec![],
    }
}
