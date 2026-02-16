use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Hook lifecycle events
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    /// Before tool execution
    ToolCallBefore,
    /// After tool execution
    ToolCallAfter,
    /// Session started
    SessionStart,
    /// Session ended
    SessionEnd,
    /// Config reloaded
    ConfigReload,
}

/// Context passed to hooks on event trigger
#[derive(Debug, Clone)]
pub struct HookContext {
    pub event: HookEvent,
    /// Event-specific data (tool name, input, output, etc.)
    pub data: Value,
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
}

/// Result from hook execution
#[derive(Debug, Clone, Default)]
pub struct HookResult {
    /// Pre-hook can modify request data
    pub modified_data: Option<Value>,
    /// Hook can abort the operation
    pub abort: bool,
}
