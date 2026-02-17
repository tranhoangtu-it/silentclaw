use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Message role in conversation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// Content within a message - text, image, tool call, or tool result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    Text {
        text: String,
    },
    Image {
        data: Vec<u8>,
        mime: String,
    },
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    /// Mixed content blocks (assistant can return text + tool calls)
    Mixed {
        parts: Vec<Content>,
    },
}

/// Tool call request from LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// Tool execution result sent back to LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_use_id: String,
    /// Original function name (needed by Gemini's functionResponse.name)
    #[serde(default)]
    pub name: String,
    pub output: String,
    pub is_error: bool,
}

/// Conversation message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Content,
}

impl Message {
    pub fn system(text: &str) -> Self {
        Self {
            role: Role::System,
            content: Content::Text {
                text: text.to_string(),
            },
        }
    }

    pub fn user(text: &str) -> Self {
        Self {
            role: Role::User,
            content: Content::Text {
                text: text.to_string(),
            },
        }
    }

    pub fn assistant(content: Content) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }

    pub fn tool_result(tool_use_id: &str, name: &str, output: &str, is_error: bool) -> Self {
        Self {
            role: Role::User,
            content: Content::ToolResult(ToolResult {
                tool_use_id: tool_use_id.to_string(),
                name: name.to_string(),
                output: output.to_string(),
                is_error,
            }),
        }
    }
}

/// Tool schema for LLM function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    /// JSON Schema for tool parameters
    pub input_schema: Value,
}

/// Why LLM stopped generating
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
}

/// Token usage info
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens += rhs.input_tokens;
        self.output_tokens += rhs.output_tokens;
    }
}

impl Usage {
    pub fn total(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }
}

/// LLM generation response
#[derive(Debug, Clone)]
pub struct GenerateResponse {
    pub content: Content,
    pub stop_reason: StopReason,
    pub usage: Usage,
    pub model: String,
}

/// Config for LLM generation request
#[derive(Debug, Clone)]
pub struct GenerateConfig {
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub system_prompt: Option<String>,
}

impl Default for GenerateConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            max_tokens: 4096,
            temperature: 0.7,
            system_prompt: None,
        }
    }
}

/// Streaming chunk from LLM
#[derive(Debug, Clone)]
pub enum StreamChunk {
    /// Text delta
    TextDelta(String),
    /// Tool call start
    ToolCallStart { id: String, name: String },
    /// Tool call input delta (partial JSON)
    ToolCallDelta { id: String, input_delta: String },
    /// Generation complete
    Done {
        stop_reason: StopReason,
        usage: Usage,
    },
}

/// Model capability metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub provider: String,
    pub context_window: u32,
    pub supports_vision: bool,
    pub supports_streaming: bool,
    pub max_output_tokens: u32,
}

impl ModelInfo {
    pub fn anthropic_sonnet() -> Self {
        Self {
            name: "claude-sonnet-4-20250514".to_string(),
            provider: "anthropic".to_string(),
            context_window: 200_000,
            supports_vision: true,
            supports_streaming: true,
            max_output_tokens: 8_192,
        }
    }

    pub fn openai_gpt4o() -> Self {
        Self {
            name: "gpt-4o".to_string(),
            provider: "openai".to_string(),
            context_window: 128_000,
            supports_vision: true,
            supports_streaming: true,
            max_output_tokens: 16_384,
        }
    }

    pub fn gemini_flash() -> Self {
        Self {
            name: "gemini-2.0-flash".to_string(),
            provider: "gemini".to_string(),
            context_window: 1_048_576,
            supports_vision: true,
            supports_streaming: true,
            max_output_tokens: 8_192,
        }
    }
}

impl Content {
    /// Extract all tool calls from content
    pub fn extract_tool_calls(&self) -> Vec<&ToolCall> {
        match self {
            Content::ToolCall(tc) => vec![tc],
            Content::Mixed { parts } => parts
                .iter()
                .filter_map(|p| match p {
                    Content::ToolCall(tc) => Some(tc),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        }
    }

    /// Extract text from content
    pub fn extract_text(&self) -> String {
        match self {
            Content::Text { text } => text.clone(),
            Content::Mixed { parts } => parts
                .iter()
                .filter_map(|p| match p {
                    Content::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        }
    }
}
