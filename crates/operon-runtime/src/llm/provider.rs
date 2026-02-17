use anyhow::Result;
use async_trait::async_trait;

use super::types::{GenerateConfig, GenerateResponse, Message, StreamChunk, ToolSchema};

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

    /// Generate a streaming response from the LLM.
    /// Default impl wraps non-streaming generate() as a single-shot stream.
    async fn generate_stream(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &GenerateConfig,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>> {
        let response = self.generate(messages, tools, config).await?;
        Ok(response_to_stream(response))
    }

    /// Whether this provider supports vision (image content)
    fn supports_vision(&self) -> bool;

    /// Provider model name for logging/tracking
    fn model_name(&self) -> &str;
}

/// Build a fallback stream from a GenerateResponse (for non-streaming providers)
pub fn response_to_stream(response: GenerateResponse) -> tokio::sync::mpsc::Receiver<StreamChunk> {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(async move {
        let text = response.content.extract_text();
        if !text.is_empty() {
            let _ = tx.send(StreamChunk::TextDelta(text)).await;
        }
        for tc in response.content.extract_tool_calls() {
            let _ = tx
                .send(StreamChunk::ToolCallStart {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                })
                .await;
            let input_str = tc.input.to_string();
            if input_str != "null" {
                let _ = tx
                    .send(StreamChunk::ToolCallDelta {
                        id: tc.id.clone(),
                        input_delta: input_str,
                    })
                    .await;
            }
        }
        let _ = tx
            .send(StreamChunk::Done {
                stop_reason: response.stop_reason,
                usage: response.usage,
            })
            .await;
    });
    rx
}
