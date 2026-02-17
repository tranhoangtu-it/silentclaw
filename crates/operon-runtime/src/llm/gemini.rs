use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::{Client, ClientBuilder};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

use super::provider::LLMProvider;
use super::streaming::{drive_sse_stream, parse_gemini_sse};
use super::types::*;

const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const DEFAULT_MODEL: &str = "gemini-2.0-flash";

/// Google Gemini API client
pub struct GeminiClient {
    client: Client,
    api_key: String,
    model: String,
    base_url: Option<String>,
}

impl GeminiClient {
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

    /// Redact API key from error body to prevent leaking in logs
    fn redact_key(body: &str, key: &str) -> String {
        if key.len() > 4 {
            body.replace(key, &format!("{}...", &key[..4]))
        } else {
            body.to_string()
        }
    }

    /// Build API URL for generate or stream endpoint.
    /// NOTE: Gemini API requires the key as a query parameter (Google's design).
    /// Do not log URLs containing the API key.
    fn api_url(&self, stream: bool) -> String {
        let base = self
            .base_url
            .as_deref()
            .unwrap_or(GEMINI_BASE_URL);
        if stream {
            format!(
                "{}/models/{}:streamGenerateContent?alt=sse&key={}",
                base, self.model, self.api_key
            )
        } else {
            format!(
                "{}/models/{}:generateContent?key={}",
                base, self.model, self.api_key
            )
        }
    }

    /// Build Gemini API request body from messages and tools
    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &GenerateConfig,
    ) -> Value {
        let contents: Vec<Value> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| self.message_to_api(m))
            .collect();

        let mut body = json!({ "contents": contents });

        // Generation config
        body["generationConfig"] = json!({
            "temperature": config.temperature,
            "maxOutputTokens": config.max_tokens,
        });

        // System instruction (Gemini uses systemInstruction field)
        if let Some(ref sys) = config.system_prompt {
            body["systemInstruction"] = json!({
                "parts": [{"text": sys}]
            });
        }

        // Tools (function declarations)
        if !tools.is_empty() {
            let declarations: Vec<Value> = tools.iter().map(|t| self.tool_to_api(t)).collect();
            body["tools"] = json!([{
                "functionDeclarations": declarations
            }]);
        }

        body
    }

    fn message_to_api(&self, msg: &Message) -> Value {
        let role = match msg.role {
            Role::User => "user",
            Role::Assistant => "model",
            Role::System => "user",
        };

        let parts = match &msg.content {
            Content::Text { text } => json!([{"text": text}]),
            Content::ToolCall(tc) => {
                json!([{
                    "functionCall": {
                        "name": tc.name,
                        "args": tc.input,
                    }
                }])
            }
            Content::ToolResult(tr) => {
                json!([{
                    "functionResponse": {
                        "name": tr.tool_use_id,
                        "response": {"result": tr.output}
                    }
                }])
            }
            Content::Mixed { parts } => {
                let api_parts: Vec<Value> = parts
                    .iter()
                    .map(|p| match p {
                        Content::Text { text } => json!({"text": text}),
                        Content::ToolCall(tc) => json!({
                            "functionCall": {
                                "name": tc.name,
                                "args": tc.input,
                            }
                        }),
                        _ => json!({"text": ""}),
                    })
                    .collect();
                json!(api_parts)
            }
            Content::Image { data, mime } => {
                use base64::Engine;
                let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                json!([{
                    "inlineData": {
                        "mimeType": mime,
                        "data": encoded,
                    }
                }])
            }
        };

        json!({"role": role, "parts": parts})
    }

    fn tool_to_api(&self, tool: &ToolSchema) -> Value {
        json!({
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        })
    }

    fn parse_response(&self, body: &GeminiApiResponse) -> Result<GenerateResponse> {
        let candidate = body
            .candidates
            .first()
            .ok_or_else(|| anyhow!("Gemini: no candidates in response"))?;

        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        if let Some(ref content) = candidate.content {
            if let Some(ref parts) = content.parts {
                for part in parts {
                    if let Some(ref text) = part.text {
                        text_parts.push(Content::Text { text: text.clone() });
                    }
                    if let Some(ref fc) = part.function_call {
                        tool_calls.push(Content::ToolCall(ToolCall {
                            id: format!("gemini_{}", fc.name),
                            name: fc.name.clone(),
                            input: fc.args.clone().unwrap_or(Value::Null),
                        }));
                    }
                }
            }
        }

        let mut all_parts = text_parts;
        all_parts.extend(tool_calls);

        let content = if all_parts.len() == 1 {
            all_parts.into_iter().next().unwrap()
        } else if all_parts.is_empty() {
            Content::Text {
                text: String::new(),
            }
        } else {
            Content::Mixed { parts: all_parts }
        };

        let stop_reason = match candidate.finish_reason.as_deref() {
            Some("MAX_TOKENS") => StopReason::MaxTokens,
            Some("STOP") => {
                // Check if response contains function calls
                if content.extract_tool_calls().is_empty() {
                    StopReason::EndTurn
                } else {
                    StopReason::ToolUse
                }
            }
            _ => StopReason::EndTurn,
        };

        let usage = body
            .usage_metadata
            .as_ref()
            .map(|u| Usage {
                input_tokens: u.prompt_token_count.unwrap_or(0),
                output_tokens: u.candidates_token_count.unwrap_or(0),
            })
            .unwrap_or_default();

        Ok(GenerateResponse {
            content,
            stop_reason,
            usage,
            model: self.model.clone(),
        })
    }
}

#[async_trait]
impl LLMProvider for GeminiClient {
    async fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &GenerateConfig,
    ) -> Result<GenerateResponse> {
        let body = self.build_request_body(messages, tools, config);
        let url = self.api_url(false);

        let response = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Gemini API error ({}): {}", status, Self::redact_key(&error_body, &self.api_key)));
        }

        let api_response: GeminiApiResponse = response.json().await?;
        self.parse_response(&api_response)
    }

    async fn generate_stream(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &GenerateConfig,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>> {
        let body = self.build_request_body(messages, tools, config);
        let url = self.api_url(true);

        let response = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Gemini API error ({}): {}", status, Self::redact_key(&error_body, &self.api_key)));
        }

        let (tx, rx) = tokio::sync::mpsc::channel(32);

        tokio::spawn({
            let byte_stream = response.bytes_stream();
            async move {
                drive_sse_stream(byte_stream, parse_gemini_sse, tx).await;
            }
        });

        Ok(rx)
    }

    fn supports_vision(&self) -> bool {
        true
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

// --- Gemini API response types (non-streaming) ---

#[derive(Debug, Deserialize)]
struct GeminiApiResponse {
    candidates: Vec<GeminiApiCandidate>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiApiUsage>,
}

#[derive(Debug, Deserialize)]
struct GeminiApiCandidate {
    content: Option<GeminiApiContent>,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiApiContent {
    parts: Option<Vec<GeminiApiPart>>,
}

#[derive(Debug, Deserialize)]
struct GeminiApiPart {
    text: Option<String>,
    #[serde(rename = "functionCall")]
    function_call: Option<GeminiApiFunctionCall>,
}

#[derive(Debug, Deserialize)]
struct GeminiApiFunctionCall {
    name: String,
    args: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct GeminiApiUsage {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<u32>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request_body() {
        let client = GeminiClient::new("test-key");
        let messages = vec![Message::user("Hello")];
        let config = GenerateConfig {
            system_prompt: Some("You are helpful".into()),
            ..Default::default()
        };

        let body = client.build_request_body(&messages, &[], &config);

        assert!(body["contents"].is_array());
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "You are helpful");
        // f32 -> f64 precision: 0.7f32 becomes ~0.6999999...
        let temp = body["generationConfig"]["temperature"].as_f64().unwrap();
        assert!((temp - 0.7).abs() < 0.001);
    }

    #[test]
    fn test_build_request_body_with_tools() {
        let client = GeminiClient::new("test-key");
        let messages = vec![Message::user("Run date")];
        let tools = vec![ToolSchema {
            name: "shell".into(),
            description: "Execute shell command".into(),
            input_schema: json!({"type": "object", "properties": {"cmd": {"type": "string"}}}),
        }];
        let config = GenerateConfig::default();

        let body = client.build_request_body(&messages, &tools, &config);

        let declarations = &body["tools"][0]["functionDeclarations"];
        assert!(declarations.is_array());
        assert_eq!(declarations[0]["name"], "shell");
    }

    #[test]
    fn test_parse_response_text() {
        let client = GeminiClient::new("test-key");
        let api_resp = GeminiApiResponse {
            candidates: vec![GeminiApiCandidate {
                content: Some(GeminiApiContent {
                    parts: Some(vec![GeminiApiPart {
                        text: Some("Hello!".into()),
                        function_call: None,
                    }]),
                }),
                finish_reason: Some("STOP".into()),
            }],
            usage_metadata: Some(GeminiApiUsage {
                prompt_token_count: Some(10),
                candidates_token_count: Some(5),
            }),
        };

        let resp = client.parse_response(&api_resp).unwrap();
        assert_eq!(resp.content.extract_text(), "Hello!");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(resp.usage.input_tokens, 10);
    }

    #[test]
    fn test_parse_response_function_call() {
        let client = GeminiClient::new("test-key");
        let api_resp = GeminiApiResponse {
            candidates: vec![GeminiApiCandidate {
                content: Some(GeminiApiContent {
                    parts: Some(vec![GeminiApiPart {
                        text: None,
                        function_call: Some(GeminiApiFunctionCall {
                            name: "shell".into(),
                            args: Some(json!({"cmd": "date"})),
                        }),
                    }]),
                }),
                finish_reason: Some("STOP".into()),
            }],
            usage_metadata: None,
        };

        let resp = client.parse_response(&api_resp).unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        let calls = resp.content.extract_tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
    }

    #[test]
    fn test_parse_response_mixed() {
        let client = GeminiClient::new("test-key");
        let api_resp = GeminiApiResponse {
            candidates: vec![GeminiApiCandidate {
                content: Some(GeminiApiContent {
                    parts: Some(vec![
                        GeminiApiPart {
                            text: Some("Let me check.".into()),
                            function_call: None,
                        },
                        GeminiApiPart {
                            text: None,
                            function_call: Some(GeminiApiFunctionCall {
                                name: "shell".into(),
                                args: Some(json!({"cmd": "date"})),
                            }),
                        },
                    ]),
                }),
                finish_reason: Some("STOP".into()),
            }],
            usage_metadata: None,
        };

        let resp = client.parse_response(&api_resp).unwrap();
        assert_eq!(resp.content.extract_text(), "Let me check.");
        assert_eq!(resp.content.extract_tool_calls().len(), 1);
    }
}
