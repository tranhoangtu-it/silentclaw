use serde::{Deserialize, Serialize};

/// Create session request
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub agent_id: Option<String>,
}

/// Session info response
#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub session_id: String,
    pub agent_name: String,
    pub created_at: String,
    pub message_count: usize,
}

/// Send message request
#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
}

/// Message response
#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub content: String,
    pub session_id: String,
}

/// WebSocket client message
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    SendMessage { content: String },
    Cancel,
}

/// WebSocket server event
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    AgentResponse { content: String },
    ToolCall { name: String, input: serde_json::Value },
    ToolResult { name: String, output: String },
    Error { message: String },
}

/// API error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}
