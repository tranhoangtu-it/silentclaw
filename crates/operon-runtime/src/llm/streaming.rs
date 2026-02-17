//! SSE parsing utilities for LLM streaming responses.
//! Handles Anthropic and OpenAI server-sent event formats.

use bytes::Bytes;
use futures::StreamExt;
use serde::Deserialize;

use super::types::{StopReason, StreamChunk, Usage};

/// Max SSE buffer size (1MB) to prevent OOM from malformed streams
const MAX_BUFFER_SIZE: usize = 1_048_576;

/// Drive an SSE byte stream: read chunks, buffer until `\n\n` boundary,
/// decode UTF-8 at event boundaries (not chunk boundaries), parse, and send.
///
/// Uses `Vec<u8>` buffer to avoid corrupting multi-byte UTF-8 chars
/// split across HTTP chunks.
pub async fn drive_sse_stream<S, F>(
    mut byte_stream: S,
    mut parse_event: F,
    tx: tokio::sync::mpsc::Sender<StreamChunk>,
) where
    S: futures::Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
    F: FnMut(&str) -> Vec<StreamChunk>,
{
    let mut buffer: Vec<u8> = Vec::new();

    while let Some(chunk_result) = byte_stream.next().await {
        let bytes = match chunk_result {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("SSE read error: {}", e);
                let _ = tx
                    .send(StreamChunk::Done {
                        stop_reason: StopReason::EndTurn,
                        usage: Usage::default(),
                    })
                    .await;
                return;
            }
        };

        buffer.extend_from_slice(&bytes);

        // Guard against unbounded buffer growth
        if buffer.len() > MAX_BUFFER_SIZE {
            tracing::error!("SSE buffer exceeded {}B limit, aborting", MAX_BUFFER_SIZE);
            let _ = tx
                .send(StreamChunk::Done {
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                })
                .await;
            return;
        }

        // Process complete SSE events delimited by \n\n
        while let Some(pos) = find_double_newline(&buffer) {
            let event_bytes = buffer[..pos].to_vec();
            buffer = buffer[pos + 2..].to_vec();

            // Safe to decode here: SSE events are complete UTF-8 at boundaries
            let event_block = String::from_utf8_lossy(&event_bytes);

            let data = event_block
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .unwrap_or("");

            if data.is_empty() {
                continue;
            }

            let chunks = parse_event(data);
            for chunk in chunks {
                if tx.send(chunk).await.is_err() {
                    return; // receiver dropped
                }
            }
        }
    }
}

/// Find position of b"\n\n" in buffer
fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

// --- Anthropic SSE parsing ---

/// Anthropic SSE event types we care about
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicEvent {
    #[serde(rename = "content_block_start")]
    ContentBlockStart { content_block: AnthropicBlock },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { delta: AnthropicDelta },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: AnthropicMessageDelta,
        usage: Option<AnthropicUsage>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct AnthropicBlock {
    #[serde(rename = "type")]
    block_type: String,
    id: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDelta {
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    output_tokens: Option<u32>,
}

/// Parse an Anthropic SSE event data string into a StreamChunk.
/// Returns None for events we don't need to forward (ping, message_start, etc.)
pub fn parse_anthropic_sse(data: &str) -> Option<StreamChunk> {
    let event: AnthropicEvent = serde_json::from_str(data).ok()?;

    match event {
        AnthropicEvent::ContentBlockStart { content_block } => {
            if content_block.block_type == "tool_use" {
                Some(StreamChunk::ToolCallStart {
                    id: content_block.id.unwrap_or_default(),
                    name: content_block.name.unwrap_or_default(),
                })
            } else {
                None // text block start - no data to emit yet
            }
        }
        AnthropicEvent::ContentBlockDelta { delta } => match delta {
            AnthropicDelta::TextDelta { text } => Some(StreamChunk::TextDelta(text)),
            AnthropicDelta::InputJsonDelta { partial_json } => {
                // Tool call input delta - caller must track current tool_use id
                Some(StreamChunk::ToolCallDelta {
                    id: String::new(), // filled by caller from block tracking
                    input_delta: partial_json,
                })
            }
        },
        AnthropicEvent::MessageDelta { delta, usage } => {
            let stop_reason = match delta.stop_reason.as_deref() {
                Some("tool_use") => StopReason::ToolUse,
                Some("max_tokens") => StopReason::MaxTokens,
                _ => StopReason::EndTurn,
            };
            Some(StreamChunk::Done {
                stop_reason,
                usage: Usage {
                    input_tokens: 0, // only available in message_start
                    output_tokens: usage.and_then(|u| u.output_tokens).unwrap_or(0),
                },
            })
        }
        AnthropicEvent::MessageStop => None, // message_delta already emitted Done
        AnthropicEvent::Unknown => None,
    }
}

// --- OpenAI SSE parsing ---

#[derive(Debug, Deserialize)]
struct OpenAIDelta {
    choices: Option<Vec<OpenAIChoice>>,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    delta: Option<OpenAIMessageDelta>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIMessageDelta {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCallDelta>>,
}

#[allow(dead_code)] // index used by OpenAI for tool call ordering
#[derive(Debug, Deserialize)]
struct OpenAIToolCallDelta {
    index: Option<u32>,
    id: Option<String>,
    function: Option<OpenAIFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAIFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
}

/// Parse an OpenAI SSE data line into StreamChunk(s).
/// Returns empty vec for unparseable data.
/// May return multiple chunks if both text and tool deltas present.
pub fn parse_openai_sse(data: &str) -> Vec<StreamChunk> {
    let trimmed = data.trim();
    if trimmed == "[DONE]" {
        return vec![StreamChunk::Done {
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        }];
    }

    let delta: OpenAIDelta = match serde_json::from_str(trimmed) {
        Ok(d) => d,
        Err(_) => return vec![],
    };

    let mut chunks = Vec::new();

    let Some(choices) = delta.choices else {
        return chunks;
    };

    for choice in &choices {
        // Check finish_reason first
        if let Some(ref reason) = choice.finish_reason {
            let stop_reason = match reason.as_str() {
                "tool_calls" => StopReason::ToolUse,
                "length" => StopReason::MaxTokens,
                _ => StopReason::EndTurn,
            };
            let usage = delta
                .usage
                .as_ref()
                .map(|u| Usage {
                    input_tokens: u.prompt_tokens.unwrap_or(0),
                    output_tokens: u.completion_tokens.unwrap_or(0),
                })
                .unwrap_or_default();
            chunks.push(StreamChunk::Done { stop_reason, usage });
            continue;
        }

        let Some(ref msg_delta) = choice.delta else {
            continue;
        };

        // Text content delta
        if let Some(ref content) = msg_delta.content {
            if !content.is_empty() {
                chunks.push(StreamChunk::TextDelta(content.clone()));
            }
        }

        // Tool call deltas
        if let Some(ref tool_calls) = msg_delta.tool_calls {
            for tc in tool_calls {
                if let Some(ref id) = tc.id {
                    // New tool call start
                    let name = tc
                        .function
                        .as_ref()
                        .and_then(|f| f.name.clone())
                        .unwrap_or_default();
                    chunks.push(StreamChunk::ToolCallStart {
                        id: id.clone(),
                        name,
                    });
                } else if let Some(ref func) = tc.function {
                    // Argument delta for existing tool call
                    if let Some(ref args) = func.arguments {
                        chunks.push(StreamChunk::ToolCallDelta {
                            id: String::new(), // caller tracks by index
                            input_delta: args.clone(),
                        });
                    }
                }
            }
        }
    }

    chunks
}

// --- Gemini SSE parsing ---

#[derive(Debug, Deserialize)]
struct GeminiStreamResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiContent {
    parts: Option<Vec<GeminiPart>>,
}

#[derive(Debug, Deserialize)]
struct GeminiPart {
    text: Option<String>,
    #[serde(rename = "functionCall")]
    function_call: Option<GeminiFunctionCall>,
}

#[derive(Debug, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GeminiUsageMetadata {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<u32>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<u32>,
}

/// Parse a Gemini SSE data string into StreamChunk(s).
/// Gemini streams `candidates[0].content.parts[]` per event.
pub fn parse_gemini_sse(data: &str) -> Vec<StreamChunk> {
    let trimmed = data.trim();
    let resp: GeminiStreamResponse = match serde_json::from_str(trimmed) {
        Ok(r) => r,
        Err(_) => return vec![],
    };

    let mut chunks = Vec::new();

    let Some(candidates) = resp.candidates else {
        return chunks;
    };

    for candidate in &candidates {
        // Parse content parts
        if let Some(ref content) = candidate.content {
            if let Some(ref parts) = content.parts {
                for part in parts {
                    if let Some(ref text) = part.text {
                        if !text.is_empty() {
                            chunks.push(StreamChunk::TextDelta(text.clone()));
                        }
                    }
                    if let Some(ref fc) = part.function_call {
                        let call_id = super::gemini::next_call_id(&fc.name);
                        chunks.push(StreamChunk::ToolCallStart {
                            id: call_id.clone(),
                            name: fc.name.clone(),
                        });
                        if let Some(ref args) = fc.args {
                            let args_str = args.to_string();
                            if args_str != "null" {
                                chunks.push(StreamChunk::ToolCallDelta {
                                    id: call_id,
                                    input_delta: args_str,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Parse finish reason
        if let Some(ref reason) = candidate.finish_reason {
            let stop_reason = match reason.as_str() {
                "STOP" => StopReason::EndTurn,
                "MAX_TOKENS" => StopReason::MaxTokens,
                _ => {
                    // Check if this event contains function calls -> ToolUse
                    let has_fc = candidate
                        .content
                        .as_ref()
                        .and_then(|c| c.parts.as_ref())
                        .map(|parts| parts.iter().any(|p| p.function_call.is_some()))
                        .unwrap_or(false);
                    if has_fc {
                        StopReason::ToolUse
                    } else {
                        StopReason::EndTurn
                    }
                }
            };

            let usage = resp
                .usage_metadata
                .as_ref()
                .map(|u| Usage {
                    input_tokens: u.prompt_token_count.unwrap_or(0),
                    output_tokens: u.candidates_token_count.unwrap_or(0),
                })
                .unwrap_or_default();

            chunks.push(StreamChunk::Done { stop_reason, usage });
        }
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Anthropic tests ---

    #[test]
    fn test_anthropic_text_delta() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let chunk = parse_anthropic_sse(data).unwrap();
        match chunk {
            StreamChunk::TextDelta(text) => assert_eq!(text, "Hello"),
            other => panic!("Expected TextDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_anthropic_tool_call_start() {
        let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_123","name":"shell"}}"#;
        let chunk = parse_anthropic_sse(data).unwrap();
        match chunk {
            StreamChunk::ToolCallStart { id, name } => {
                assert_eq!(id, "toolu_123");
                assert_eq!(name, "shell");
            }
            other => panic!("Expected ToolCallStart, got {:?}", other),
        }
    }

    #[test]
    fn test_anthropic_tool_call_delta() {
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":"}}"#;
        let chunk = parse_anthropic_sse(data).unwrap();
        match chunk {
            StreamChunk::ToolCallDelta { input_delta, .. } => {
                assert_eq!(input_delta, r#"{"cmd":"#);
            }
            other => panic!("Expected ToolCallDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_anthropic_message_delta_done() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#;
        let chunk = parse_anthropic_sse(data).unwrap();
        match chunk {
            StreamChunk::Done { stop_reason, usage } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
                assert_eq!(usage.output_tokens, 42);
            }
            other => panic!("Expected Done, got {:?}", other),
        }
    }

    #[test]
    fn test_anthropic_unknown_event() {
        let data = r#"{"type":"ping"}"#;
        assert!(parse_anthropic_sse(data).is_none());
    }

    // --- OpenAI tests ---

    #[test]
    fn test_openai_text_delta() {
        let data = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"Hi"}}]}"#;
        let chunks = parse_openai_sse(data);
        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            StreamChunk::TextDelta(text) => assert_eq!(text, "Hi"),
            other => panic!("Expected TextDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_openai_done_signal() {
        let chunks = parse_openai_sse("[DONE]");
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], StreamChunk::Done { .. }));
    }

    #[test]
    fn test_openai_tool_call_start() {
        let data = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"shell","arguments":""}}]}}]}"#;
        let chunks = parse_openai_sse(data);
        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            StreamChunk::ToolCallStart { id, name } => {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "shell");
            }
            other => panic!("Expected ToolCallStart, got {:?}", other),
        }
    }

    #[test]
    fn test_openai_tool_call_argument_delta() {
        let data = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"cmd\":"}}]}}]}"#;
        let chunks = parse_openai_sse(data);
        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            StreamChunk::ToolCallDelta { input_delta, .. } => {
                assert_eq!(input_delta, r#"{"cmd":"#);
            }
            other => panic!("Expected ToolCallDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_openai_finish_reason_stop() {
        let data =
            r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;
        let chunks = parse_openai_sse(data);
        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            StreamChunk::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
            }
            other => panic!("Expected Done, got {:?}", other),
        }
    }

    // --- Gemini tests ---

    #[test]
    fn test_gemini_text_delta() {
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"Hello"}],"role":"model"}}]}"#;
        let chunks = parse_gemini_sse(data);
        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            StreamChunk::TextDelta(text) => assert_eq!(text, "Hello"),
            other => panic!("Expected TextDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_gemini_function_call() {
        let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"shell","args":{"cmd":"date"}}}],"role":"model"}}]}"#;
        let chunks = parse_gemini_sse(data);
        assert_eq!(chunks.len(), 2);
        match &chunks[0] {
            StreamChunk::ToolCallStart { name, .. } => assert_eq!(name, "shell"),
            other => panic!("Expected ToolCallStart, got {:?}", other),
        }
        match &chunks[1] {
            StreamChunk::ToolCallDelta { input_delta, .. } => {
                assert!(input_delta.contains("cmd"));
            }
            other => panic!("Expected ToolCallDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_gemini_finish_reason_stop() {
        let data = r#"{"candidates":[{"content":{"parts":[],"role":"model"},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#;
        let chunks = parse_gemini_sse(data);
        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            StreamChunk::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 5);
            }
            other => panic!("Expected Done, got {:?}", other),
        }
    }

    #[test]
    fn test_gemini_finish_reason_max_tokens() {
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"truncat"}],"role":"model"},"finishReason":"MAX_TOKENS"}]}"#;
        let chunks = parse_gemini_sse(data);
        assert!(chunks.len() >= 2);
        assert!(matches!(&chunks[0], StreamChunk::TextDelta(_)));
        match chunks.last().unwrap() {
            StreamChunk::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::MaxTokens);
            }
            other => panic!("Expected Done, got {:?}", other),
        }
    }

    #[test]
    fn test_gemini_empty_candidates() {
        let data = r#"{"candidates":[]}"#;
        let chunks = parse_gemini_sse(data);
        assert!(chunks.is_empty());
    }
}
