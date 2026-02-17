use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{info, warn};

use crate::llm::provider::LLMProvider;
use crate::llm::types::*;
use crate::Runtime;

// ============================================================================
// AgentConfig
// ============================================================================

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent display name
    pub name: String,
    /// System prompt for LLM
    pub system_prompt: String,
    /// Max loop iterations per user turn (prevent infinite loops)
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    /// LLM temperature
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Max tokens for LLM response
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Tool names to expose to LLM (empty = all registered)
    #[serde(default)]
    pub tools: Vec<String>,
    /// LLM model override (empty = use provider default)
    #[serde(default)]
    pub model: String,
}

fn default_max_iterations() -> usize {
    10
}

fn default_temperature() -> f32 {
    0.7
}

fn default_max_tokens() -> u32 {
    4096
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            system_prompt: "You are a helpful assistant with access to tools.".to_string(),
            max_iterations: default_max_iterations(),
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            tools: Vec::new(),
            model: String::new(),
        }
    }
}

// ============================================================================
// Session
// ============================================================================

/// Conversation session with message history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub agent_name: String,
    pub messages: Vec<Message>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Cumulative token usage across all LLM calls in this session
    #[serde(default)]
    pub cumulative_usage: Usage,
}

impl Session {
    pub fn new(agent_name: &str) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        Self {
            id,
            agent_name: agent_name.to_string(),
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
            metadata: HashMap::new(),
            cumulative_usage: Usage::default(),
        }
    }

    /// Create session with specific ID (for loading from store)
    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    /// Add a message to conversation history
    pub fn add_message(&mut self, msg: Message) {
        self.messages.push(msg);
        self.updated_at = Utc::now();
    }

    /// Add tool results as user messages (Anthropic/OpenAI expect this)
    pub fn add_tool_results(&mut self, results: Vec<ToolResult>) {
        for result in results {
            self.add_message(Message {
                role: Role::User,
                content: Content::ToolResult(result),
            });
        }
    }

    /// Count of messages (for context tracking)
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

// ============================================================================
// SessionStore
// ============================================================================

/// Persistent session store (JSON files)
pub struct SessionStore {
    base_path: PathBuf,
}

impl SessionStore {
    pub fn new(base_path: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&base_path)
            .context(format!("Failed to create session dir: {:?}", base_path))?;
        Ok(Self { base_path })
    }

    /// Save session to JSON file
    pub async fn save(&self, session: &Session) -> Result<()> {
        let path = self.base_path.join(format!("{}.json", session.id));
        let json = serde_json::to_string_pretty(session)?;
        tokio::fs::write(&path, json)
            .await
            .context(format!("Failed to save session: {:?}", path))?;
        Ok(())
    }

    /// Load session from JSON file
    pub async fn load(&self, session_id: &str) -> Result<Session> {
        let path = self.base_path.join(format!("{}.json", session_id));
        let json = tokio::fs::read_to_string(&path)
            .await
            .context(format!("Failed to load session: {:?}", path))?;
        let session: Session = serde_json::from_str(&json)?;
        Ok(session)
    }

    /// List all session IDs
    pub fn list_sessions(&self) -> Result<Vec<String>> {
        let mut sessions = Vec::new();
        for entry in std::fs::read_dir(&self.base_path)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with(".json") {
                    sessions.push(name.trim_end_matches(".json").to_string());
                }
            }
        }
        Ok(sessions)
    }
}

// ============================================================================
// Agent
// ============================================================================

/// Autonomous agent: prompt → LLM → tool calls → execute → observe → repeat
pub struct Agent {
    pub config: AgentConfig,
    provider: Arc<dyn LLMProvider>,
    runtime: Arc<Runtime>,
    pub session: Session,
}

impl Agent {
    pub fn new(config: AgentConfig, provider: Arc<dyn LLMProvider>, runtime: Arc<Runtime>) -> Self {
        let session = Session::new(&config.name);
        Self {
            config,
            provider,
            runtime,
            session,
        }
    }

    /// Resume agent with existing session
    pub fn with_session(mut self, session: Session) -> Self {
        self.session = session;
        self
    }

    /// Process user message through agent loop
    /// Returns final assistant text response
    pub async fn process_message(&mut self, user_msg: &str) -> Result<String> {
        self.session.add_message(Message::user(user_msg));

        let mut iteration = 0;
        loop {
            let gen_config = GenerateConfig {
                model: self.config.model.clone(),
                max_tokens: self.config.max_tokens,
                temperature: self.config.temperature,
                system_prompt: Some(self.config.system_prompt.clone()),
            };

            let tools = self.available_tool_schemas();

            let response = self
                .provider
                .generate(&self.session.messages, &tools, &gen_config)
                .await?;

            // Track cumulative usage
            self.session.cumulative_usage += response.usage.clone();

            let total_tokens = self.session.cumulative_usage.total();
            info!(
                model = %response.model,
                stop_reason = ?response.stop_reason,
                input_tokens = response.usage.input_tokens,
                output_tokens = response.usage.output_tokens,
                cumulative_tokens = total_tokens,
                "LLM response received"
            );

            // Warn when approaching context limit (80%)
            if total_tokens > (self.config.max_tokens * 8 / 10) {
                warn!(
                    total_tokens,
                    max = self.config.max_tokens,
                    "Context approaching limit (80%)"
                );
            }

            // Add assistant response to history
            self.session
                .add_message(Message::assistant(response.content.clone()));

            match response.stop_reason {
                StopReason::EndTurn => {
                    return Ok(response.content.extract_text());
                }
                StopReason::ToolUse => {
                    let results = self.execute_tool_calls(&response.content).await?;
                    self.session.add_tool_results(results);
                }
                StopReason::MaxTokens => {
                    // Try to return partial text instead of hard error
                    let text = response.content.extract_text();
                    if !text.is_empty() {
                        warn!("Context limit reached, returning partial response");
                        return Ok(text);
                    }
                    return Err(anyhow!("Context window exceeded (max_tokens reached)"));
                }
            }

            iteration += 1;
            if iteration >= self.config.max_iterations {
                warn!(
                    max = self.config.max_iterations,
                    "Max iterations reached, stopping agent loop"
                );
                return Err(anyhow!(
                    "Max iterations ({}) reached",
                    self.config.max_iterations
                ));
            }
        }
    }

    /// Execute tool calls from LLM response
    async fn execute_tool_calls(&self, content: &Content) -> Result<Vec<ToolResult>> {
        let tool_calls = content.extract_tool_calls();
        let mut results = Vec::new();

        for call in tool_calls {
            info!(tool = %call.name, id = %call.id, "Executing tool call");

            let output = match self
                .runtime
                .execute_tool(&call.name, call.input.clone())
                .await
            {
                Ok(value) => ToolResult {
                    tool_use_id: call.id.clone(),
                    name: call.name.clone(),
                    output: value.to_string(),
                    is_error: false,
                },
                Err(e) => {
                    warn!(tool = %call.name, error = %e, "Tool execution failed");
                    ToolResult {
                        tool_use_id: call.id.clone(),
                        name: call.name.clone(),
                        output: format!("Error: {}", e),
                        is_error: true,
                    }
                }
            };

            results.push(output);
        }

        Ok(results)
    }

    /// Build tool schemas from registered runtime tools
    fn available_tool_schemas(&self) -> Vec<ToolSchema> {
        let tool_names = if self.config.tools.is_empty() {
            self.runtime.tool_names()
        } else {
            self.config.tools.clone()
        };

        tool_names
            .iter()
            .map(|name| ToolSchema {
                name: name.clone(),
                description: format!("Execute the {} tool", name),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "input": {
                            "type": "string",
                            "description": "Input for the tool"
                        }
                    }
                }),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock LLM that returns predefined responses
    struct MockLLM {
        responses: Vec<GenerateResponse>,
        call_count: AtomicUsize,
    }

    impl MockLLM {
        fn new(responses: Vec<GenerateResponse>) -> Self {
            Self {
                responses,
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl LLMProvider for MockLLM {
        async fn generate(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &GenerateConfig,
        ) -> Result<GenerateResponse> {
            let idx = self.call_count.fetch_add(1, Ordering::Relaxed);
            self.responses
                .get(idx)
                .cloned()
                .ok_or_else(|| anyhow!("No more mock responses"))
        }

        fn supports_vision(&self) -> bool {
            false
        }

        fn model_name(&self) -> &str {
            "mock"
        }
    }

    fn make_runtime() -> (Arc<Runtime>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let runtime = Arc::new(
            Runtime::with_db(
                db_path.to_str().unwrap(),
                true,
                std::time::Duration::from_secs(30),
            )
            .unwrap(),
        );
        (runtime, dir)
    }

    #[tokio::test]
    async fn test_simple_text_response() {
        let llm = Arc::new(MockLLM::new(vec![GenerateResponse {
            content: Content::Text {
                text: "Hello there!".into(),
            },
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            model: "mock".into(),
        }]));

        let (runtime, _dir) = make_runtime();
        let mut agent = Agent::new(AgentConfig::default(), llm, runtime);
        let result = agent.process_message("Hi").await.unwrap();
        assert_eq!(result, "Hello there!");
        assert_eq!(agent.session.message_count(), 2); // user + assistant
    }

    #[tokio::test]
    async fn test_tool_call_then_response() {
        let llm = Arc::new(MockLLM::new(vec![
            // First: LLM wants to call a tool
            GenerateResponse {
                content: Content::ToolCall(ToolCall {
                    id: "tc_1".into(),
                    name: "shell".into(),
                    input: serde_json::json!({"cmd": "date"}),
                }),
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                model: "mock".into(),
            },
            // Second: LLM gives final response after seeing tool result
            GenerateResponse {
                content: Content::Text {
                    text: "The date is today.".into(),
                },
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
                model: "mock".into(),
            },
        ]));

        let (runtime, _dir) = make_runtime();
        // dry_run=true so tool returns dry-run output
        let mut agent = Agent::new(AgentConfig::default(), llm, runtime);
        let result = agent.process_message("What's the date?").await.unwrap();
        assert_eq!(result, "The date is today.");
        // user + assistant(tool_call) + tool_result + assistant(text)
        assert_eq!(agent.session.message_count(), 4);
    }

    #[tokio::test]
    async fn test_max_iterations_limit() {
        // LLM always wants to call tools, never ends
        let responses: Vec<GenerateResponse> = (0..15)
            .map(|i| GenerateResponse {
                content: Content::ToolCall(ToolCall {
                    id: format!("tc_{}", i),
                    name: "shell".into(),
                    input: serde_json::json!({}),
                }),
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                model: "mock".into(),
            })
            .collect();

        let llm = Arc::new(MockLLM::new(responses));
        let config = AgentConfig {
            max_iterations: 3,
            ..AgentConfig::default()
        };

        let (runtime, _dir) = make_runtime();
        let mut agent = Agent::new(config, llm, runtime);
        let result = agent.process_message("loop forever").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Max iterations"));
    }
}
