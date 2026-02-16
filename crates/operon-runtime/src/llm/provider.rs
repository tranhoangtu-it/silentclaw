use anyhow::Result;
use async_trait::async_trait;

use super::types::{GenerateConfig, GenerateResponse, Message, ToolSchema};

/// LLM provider trait - abstraction over Anthropic, OpenAI, etc.
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Generate a response from the LLM (non-streaming)
    async fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &GenerateConfig,
    ) -> Result<GenerateResponse>;

    /// Whether this provider supports vision (image content)
    fn supports_vision(&self) -> bool;

    /// Provider model name for logging/tracking
    fn model_name(&self) -> &str;
}
