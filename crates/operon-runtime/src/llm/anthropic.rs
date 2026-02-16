use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::{Client, ClientBuilder};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

use super::provider::LLMProvider;
use super::types::*;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";

/// Anthropic Messages API client
pub struct AnthropicClient {
    client: Client,
    api_key: String,
    model: String,
}

impl AnthropicClient {
    pub fn new(api_key: &str) -> Self {
        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            client,
            api_key: api_key.to_string(),
            model: DEFAULT_MODEL.to_string(),
        }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }

    /// Build Anthropic API request body from messages and tools
    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &GenerateConfig,
    ) -> Value {
        let model = if config.model.is_empty() {
            &self.model
        } else {
            &config.model
        };

        let mut body = json!({
            "model": model,
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
        });

        // Extract system prompt
        if let Some(ref sys) = config.system_prompt {
            body["system"] = json!(sys);
        }

        // Convert messages to Anthropic format (skip system messages)
        let api_messages: Vec<Value> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| self.message_to_api(m))
            .collect();
        body["messages"] = json!(api_messages);

        // Add tools if provided
        if !tools.is_empty() {
            let api_tools: Vec<Value> = tools.iter().map(|t| self.tool_to_api(t)).collect();
            body["tools"] = json!(api_tools);
        }

        body
    }

    /// Convert internal Message to Anthropic API format
    fn message_to_api(&self, msg: &Message) -> Value {
        let role = match msg.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "user", // filtered out above, fallback
        };

        let content = match &msg.content {
            Content::Text { text } => json!([{"type": "text", "text": text}]),
            Content::ToolCall(tc) => {
                json!([{
                    "type": "tool_use",
                    "id": tc.id,
                    "name": tc.name,
                    "input": tc.input,
                }])
            }
            Content::ToolResult(tr) => {
                json!([{
                    "type": "tool_result",
                    "tool_use_id": tr.tool_use_id,
                    "content": tr.output,
                    "is_error": tr.is_error,
                }])
            }
            Content::Mixed { parts } => {
                let blocks: Vec<Value> = parts
                    .iter()
                    .map(|p| match p {
                        Content::Text { text } => json!({"type": "text", "text": text}),
                        Content::ToolCall(tc) => json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.input,
                        }),
                        _ => json!({"type": "text", "text": ""}),
                    })
                    .collect();
                json!(blocks)
            }
            Content::Image { data, mime } => {
                use base64::Engine;
                let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                json!([{
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": mime,
                        "data": encoded,
                    }
                }])
            }
        };

        json!({ "role": role, "content": content })
    }

    /// Convert ToolSchema to Anthropic API format
    fn tool_to_api(&self, tool: &ToolSchema) -> Value {
        json!({
            "name": tool.name,
            "description": tool.description,
            "input_schema": tool.input_schema,
        })
    }

    /// Parse Anthropic API response into GenerateResponse
    fn parse_response(&self, body: &ApiResponse) -> Result<GenerateResponse> {
        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        for block in &body.content {
            match block.block_type.as_str() {
                "text" => {
                    if let Some(ref text) = block.text {
                        text_parts.push(Content::Text {
                            text: text.clone(),
                        });
                    }
                }
                "tool_use" => {
                    tool_calls.push(Content::ToolCall(ToolCall {
                        id: block.id.clone().unwrap_or_default(),
                        name: block.name.clone().unwrap_or_default(),
                        input: block.input.clone().unwrap_or(Value::Null),
                    }));
                }
                _ => {}
            }
        }

        // Combine text and tool calls
        let mut parts = text_parts;
        parts.extend(tool_calls);

        let content = if parts.len() == 1 {
            parts.into_iter().next().unwrap()
        } else {
            Content::Mixed { parts }
        };

        let stop_reason = match body.stop_reason.as_deref() {
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            _ => StopReason::EndTurn,
        };

        Ok(GenerateResponse {
            content,
            stop_reason,
            usage: Usage {
                input_tokens: body.usage.input_tokens,
                output_tokens: body.usage.output_tokens,
            },
            model: body.model.clone(),
        })
    }
}

#[async_trait]
impl LLMProvider for AnthropicClient {
    async fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &GenerateConfig,
    ) -> Result<GenerateResponse> {
        let body = self.build_request_body(messages, tools, config);

        let response = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Anthropic API error ({}): {}",
                status,
                error_body
            ));
        }

        let api_response: ApiResponse = response.json().await?;
        self.parse_response(&api_response)
    }

    fn supports_vision(&self) -> bool {
        true
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// Anthropic API response structures
#[derive(Debug, Deserialize)]
struct ApiResponse {
    model: String,
    content: Vec<ContentBlock>,
    stop_reason: Option<String>,
    usage: ApiUsage,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ApiUsage {
    input_tokens: u32,
    output_tokens: u32,
}

/// Check if an error is retryable (rate limit, server error)
pub fn is_retryable_status(status: u16) -> bool {
    status == 429 || status == 529 || (500..600).contains(&status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request_body() {
        let client = AnthropicClient::new("test-key");
        let messages = vec![Message::user("Hello")];
        let config = GenerateConfig {
            system_prompt: Some("You are helpful".into()),
            ..Default::default()
        };

        let body = client.build_request_body(&messages, &[], &config);

        assert_eq!(body["system"], "You are helpful");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["max_tokens"], 4096);
    }

    #[test]
    fn test_parse_response_text() {
        let client = AnthropicClient::new("test-key");
        let api_resp = ApiResponse {
            model: "claude-sonnet-4-20250514".into(),
            content: vec![ContentBlock {
                block_type: "text".into(),
                text: Some("Hello!".into()),
                id: None,
                name: None,
                input: None,
            }],
            stop_reason: Some("end_turn".into()),
            usage: ApiUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        };

        let resp = client.parse_response(&api_resp).unwrap();
        assert_eq!(resp.content.extract_text(), "Hello!");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn test_parse_response_tool_use() {
        let client = AnthropicClient::new("test-key");
        let api_resp = ApiResponse {
            model: "claude-sonnet-4-20250514".into(),
            content: vec![
                ContentBlock {
                    block_type: "text".into(),
                    text: Some("Let me check.".into()),
                    id: None,
                    name: None,
                    input: None,
                },
                ContentBlock {
                    block_type: "tool_use".into(),
                    text: None,
                    id: Some("toolu_123".into()),
                    name: Some("shell".into()),
                    input: Some(json!({"cmd": "date"})),
                },
            ],
            stop_reason: Some("tool_use".into()),
            usage: ApiUsage {
                input_tokens: 20,
                output_tokens: 15,
            },
        };

        let resp = client.parse_response(&api_resp).unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        let calls = resp.content.extract_tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
    }
}
