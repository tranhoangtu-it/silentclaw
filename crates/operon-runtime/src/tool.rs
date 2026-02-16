use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

/// Permission level for tool execution
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PermissionLevel {
    Read,
    Write,
    Execute,
    Network,
    Admin,
}

/// Tool JSON schema for LLM function calling
#[derive(Debug, Clone)]
pub struct ToolSchemaInfo {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Async Tool trait
/// Note: Uses async_trait for trait object compatibility with DashMap storage
#[async_trait]
pub trait Tool: Send + Sync {
    /// Execute tool with input, returns result
    async fn execute(&self, input: Value) -> Result<Value>;

    /// Tool name for registration
    fn name(&self) -> &str;

    /// Return JSON schema for LLM tool calling (default: generic object)
    fn schema(&self) -> ToolSchemaInfo {
        ToolSchemaInfo {
            name: self.name().to_string(),
            description: format!("Execute the {} tool", self.name()),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string", "description": "Input for the tool" }
                }
            }),
        }
    }

    /// Permission level required (default: Execute)
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }
}
