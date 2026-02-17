pub mod agent_module;
pub mod config;
pub mod hooks;
pub mod llm;
pub mod memory;
pub mod plugin;
pub mod replay;
pub mod runtime;
pub mod scheduler;
pub mod storage;
pub mod tool;
pub mod tool_policy;

pub use agent_module::{Agent, AgentConfig, Session, SessionStore};
pub use config::{ConfigManager, ConfigReloadEvent};
pub use hooks::{Hook, HookContext, HookEvent, HookRegistry, HookResult};
pub use llm::{
    AnthropicClient, Content, GenerateConfig, GenerateResponse, GeminiClient, LLMProvider, Message,
    OpenAIClient, ProviderChain, Role, StopReason, ToolCall, ToolResult, ToolSchema, Usage,
};
pub use plugin::{Plugin, PluginHandle, PluginLoader, PluginManifest, PluginType};
pub use replay::{Fixture, StepRecord};
pub use runtime::{ExecutionContext, Runtime};
pub use storage::Storage;
pub use tool::{PermissionLevel, Tool, ToolSchemaInfo};
pub use tool_policy::{PolicyContext, PolicyDecision, PolicyLayer, ToolPolicyPipeline};

/// Initialize structured JSON logging
pub fn init_logging() {
    use tracing_subscriber::{fmt, EnvFilter};

    fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
}
