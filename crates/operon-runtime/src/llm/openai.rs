use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::{Client, ClientBuilder};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

use super::provider::LLMProvider;
use super::types::*;

const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";
const DEFAULT_MODEL: &str = "gpt-4o";

/// OpenAI Chat Completions API client
pub struct OpenAIClient {
    client: Client,
    api_key: String,
    model: String,
    /// Custom base URL for OpenAI-compatible APIs (e.g., local LLM)
    base_url: Option<String>,
}

impl OpenAIClient {
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
            base_url: None,
        }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }

    pub fn with_base_url(mut self, url: &str) -> Self {
        self.base_url = Some(url.to_string());
        self
    }

    fn api_url(&self) -> &str {
        self.base_url.as_deref().unwrap_or(OPENAI_API_URL)
    }

    /// Build OpenAI API request body
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

        let api_messages: Vec<Value> = self.build_messages(messages, config);

        let mut body = json!({
            "model": model,
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "messages": api_messages,
        });

        if !tools.is_empty() {
            let api_tools: Vec<Value> = tools.iter().map(|t| self.tool_to_api(t)).collect();
            body["tools"] = json!(api_tools);
        }

        body
    }

    /// Build OpenAI messages array (system prompt + conversation)
    fn build_messages(&self, messages: &[Message], config: &GenerateConfig) -> Vec<Value> {
        let mut api_msgs = Vec::new();

        // Add system prompt if provided
        if let Some(ref sys) = config.system_prompt {
            api_msgs.push(json!({"role": "system", "content": sys}));
        }

        for msg in messages {
            match (&msg.role, &msg.content) {
                (Role::System, Content::Text { text }) => {
                    api_msgs.push(json!({"role": "system", "content": text}));
                }
                (Role::User, Content::Text { text }) => {
                    api_msgs.push(json!({"role": "user", "content": text}));
                }
                (Role::User, Content::Image { data, mime }) => {
                    use base64::Engine;
                    let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                    api_msgs.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "image_url",
                            "image_url": {
                                "url": format!("data:{};base64,{}", mime, encoded)
                            }
                        }]
                    }));
                }
                (Role::User, Content::ToolResult(tr)) => {
                    api_msgs.push(json!({
                        "role": "tool",
                        "tool_call_id": tr.tool_use_id,
                        "content": tr.output,
                    }));
                }
                (Role::Assistant, Content::Text { text }) => {
                    api_msgs.push(json!({"role": "assistant", "content": text}));
                }
                (Role::Assistant, Content::ToolCall(tc)) => {
                    api_msgs.push(json!({
                        "role": "assistant",
                        "tool_calls": [{
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.input.to_string(),
                            }
                        }]
                    }));
                }
                (Role::Assistant, Content::Mixed { parts }) => {
                    let mut text_content = String::new();
                    let mut tool_calls_json = Vec::new();

                    for part in parts {
                        match part {
                            Content::Text { text } => text_content.push_str(text),
                            Content::ToolCall(tc) => {
                                tool_calls_json.push(json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": tc.input.to_string(),
                                    }
                                }));
                            }
                            _ => {}
                        }
                    }

                    let mut msg_json = json!({"role": "assistant"});
                    if !text_content.is_empty() {
                        msg_json["content"] = json!(text_content);
                    }
                    if !tool_calls_json.is_empty() {
                        msg_json["tool_calls"] = json!(tool_calls_json);
                    }
                    api_msgs.push(msg_json);
                }
                _ => {}
            }
        }

        api_msgs
    }

    /// Convert ToolSchema to OpenAI function calling format
    fn tool_to_api(&self, tool: &ToolSchema) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.input_schema,
            }
        })
    }

    /// Parse OpenAI API response
    fn parse_response(&self, body: &ApiResponse) -> Result<GenerateResponse> {
        let choice = body
            .choices
            .first()
            .ok_or_else(|| anyhow!("No choices in OpenAI response"))?;

        let msg = &choice.message;

        // Build content from response
        let mut parts = Vec::new();

        if let Some(ref text) = msg.content {
            if !text.is_empty() {
                parts.push(Content::Text { text: text.clone() });
            }
        }

        if let Some(ref tool_calls) = msg.tool_calls {
            for tc in tool_calls {
                let input: Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or(Value::Null);
                parts.push(Content::ToolCall(ToolCall {
                    id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    input,
                }));
            }
        }

        let content = if parts.len() == 1 {
            parts.into_iter().next().unwrap()
        } else if parts.is_empty() {
            Content::Text {
                text: String::new(),
            }
        } else {
            Content::Mixed { parts }
        };

        let stop_reason = match choice.finish_reason.as_deref() {
            Some("tool_calls") => StopReason::ToolUse,
            Some("length") => StopReason::MaxTokens,
            _ => StopReason::EndTurn,
        };

        let usage = body
            .usage
            .as_ref()
            .map(|u| Usage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
            })
            .unwrap_or_default();

        Ok(GenerateResponse {
            content,
            stop_reason,
            usage,
            model: body.model.clone(),
        })
    }
}

#[async_trait]
impl LLMProvider for OpenAIClient {
    async fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &GenerateConfig,
    ) -> Result<GenerateResponse> {
        let body = self.build_request_body(messages, tools, config);

        let response = self
            .client
            .post(self.api_url())
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(anyhow!("OpenAI API error ({}): {}", status, error_body));
        }

        let api_response: ApiResponse = response.json().await?;
        self.parse_response(&api_response)
    }

    fn supports_vision(&self) -> bool {
        // GPT-4o and GPT-4 Vision support images
        self.model.contains("gpt-4")
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// OpenAI API response structures
#[derive(Debug, Deserialize)]
struct ApiResponse {
    model: String,
    choices: Vec<Choice>,
    usage: Option<ApiUsage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ApiMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ApiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ApiToolCall {
    id: String,
    function: ApiFunction,
}

#[derive(Debug, Deserialize)]
struct ApiFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ApiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request_body() {
        let client = OpenAIClient::new("test-key");
        let messages = vec![Message::user("Hello")];
        let config = GenerateConfig {
            system_prompt: Some("Be helpful".into()),
            ..Default::default()
        };

        let body = client.build_request_body(&messages, &[], &config);

        // System prompt is first message
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "Be helpful");
        assert_eq!(body["messages"][1]["role"], "user");
    }

    #[test]
    fn test_parse_response_text() {
        let client = OpenAIClient::new("test-key");
        let api_resp = ApiResponse {
            model: "gpt-4o".into(),
            choices: vec![Choice {
                message: ApiMessage {
                    content: Some("Hello!".into()),
                    tool_calls: None,
                },
                finish_reason: Some("stop".into()),
            }],
            usage: Some(ApiUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
            }),
        };

        let resp = client.parse_response(&api_resp).unwrap();
        assert_eq!(resp.content.extract_text(), "Hello!");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn test_parse_response_tool_call() {
        let client = OpenAIClient::new("test-key");
        let api_resp = ApiResponse {
            model: "gpt-4o".into(),
            choices: vec![Choice {
                message: ApiMessage {
                    content: None,
                    tool_calls: Some(vec![ApiToolCall {
                        id: "call_123".into(),
                        function: ApiFunction {
                            name: "shell".into(),
                            arguments: r#"{"cmd":"date"}"#.into(),
                        },
                    }]),
                },
                finish_reason: Some("tool_calls".into()),
            }],
            usage: Some(ApiUsage {
                prompt_tokens: 20,
                completion_tokens: 15,
            }),
        };

        let resp = client.parse_response(&api_resp).unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        let calls = resp.content.extract_tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
    }

    #[test]
    fn test_custom_base_url() {
        let client = OpenAIClient::new("key").with_base_url("http://localhost:11434/v1/chat/completions");
        assert_eq!(client.api_url(), "http://localhost:11434/v1/chat/completions");
    }
}
