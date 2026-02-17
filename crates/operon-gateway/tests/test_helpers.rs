//! Shared test helpers: mock LLM provider, test AppState factory.
#![allow(dead_code)] // helpers used across multiple test crates

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use axum::extract::ConnectInfo;
use axum::http::Request;
use operon_runtime::llm::{
    Content, GenerateConfig, GenerateResponse, LLMProvider, Message, StopReason, StreamChunk,
    ToolSchema, Usage,
};
use operon_runtime::Runtime;

use operon_gateway::{AppState, AuthConfig, RateLimiter, SessionManager};

/// Add ConnectInfo extension to a request (required by rate limiter middleware).
pub fn with_connect_info<B>(mut req: Request<B>) -> Request<B> {
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    req
}

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

/// Build runtime with tempdir-backed DB (auto-cleaned on drop).
fn make_test_runtime() -> (Arc<Runtime>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let runtime = Arc::new(
        Runtime::with_db(db_path.to_str().unwrap(), true, Duration::from_secs(30)).unwrap(),
    );
    (runtime, dir)
}

/// Build a test AppState with no auth requirement and generous rate limits.
/// Returns (AppState, TempDir) — caller must keep `_dir` alive for DB lifetime.
pub fn make_test_state() -> (AppState, tempfile::TempDir) {
    let (runtime, dir) = make_test_runtime();
    let provider: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider);
    let session_manager = Arc::new(SessionManager::new(provider, runtime));

    (
        AppState {
            session_manager,
            auth_config: Arc::new(AuthConfig::new(None)),
            rate_limiter: Arc::new(RateLimiter::new(1000)),
            allowed_origins: vec![],
        },
        dir,
    )
}

/// Build a test AppState with auth enabled using given token.
pub fn make_auth_test_state(token: &str) -> (AppState, tempfile::TempDir) {
    let (runtime, dir) = make_test_runtime();
    let provider: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider);
    let session_manager = Arc::new(SessionManager::new(provider, runtime));

    (
        AppState {
            session_manager,
            auth_config: Arc::new(AuthConfig::new(Some(token.to_string()))),
            rate_limiter: Arc::new(RateLimiter::new(1000)),
            allowed_origins: vec![],
        },
        dir,
    )
}

/// Build a test AppState with a tight rate limit.
pub fn make_ratelimit_test_state(max_rpm: u32) -> (AppState, tempfile::TempDir) {
    let (runtime, dir) = make_test_runtime();
    let provider: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider);
    let session_manager = Arc::new(SessionManager::new(provider, runtime));

    (
        AppState {
            session_manager,
            auth_config: Arc::new(AuthConfig::new(None)),
            rate_limiter: Arc::new(RateLimiter::new(max_rpm)),
            allowed_origins: vec![],
        },
        dir,
    )
}
