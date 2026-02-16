use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

/// Async Tool trait
/// Note: Uses async_trait for trait object compatibility with DashMap storage
#[async_trait]
pub trait Tool: Send + Sync {
    /// Execute tool with input, returns result
    async fn execute(&self, input: Value) -> Result<Value>;

    /// Tool name for registration
    fn name(&self) -> &str;
}
