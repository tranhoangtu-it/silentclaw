pub mod anthropic;
pub mod failover;
pub mod openai;
pub mod provider;
pub mod streaming;
pub mod types;

pub use anthropic::AnthropicClient;
pub use failover::ProviderChain;
pub use openai::OpenAIClient;
pub use provider::LLMProvider;
pub use streaming::{parse_anthropic_sse, parse_openai_sse};
pub use types::{
    Content, GenerateConfig, GenerateResponse, Message, ModelInfo, Role, StopReason, StreamChunk,
    ToolCall, ToolResult, ToolSchema, Usage,
};
