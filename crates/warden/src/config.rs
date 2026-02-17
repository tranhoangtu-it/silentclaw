use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    /// Config schema version
    #[serde(default = "default_config_version")]
    pub version: u32,
    pub runtime: RuntimeConfig,
    pub tools: ToolsConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub tool_policy: operon_runtime::tool_policy::config::ToolPolicyConfig,
}

fn default_config_version() -> u32 {
    1
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LlmConfig {
    /// Anthropic API key (or set ANTHROPIC_API_KEY env)
    #[serde(default)]
    pub anthropic_api_key: String,
    /// OpenAI API key (or set OPENAI_API_KEY env)
    #[serde(default)]
    pub openai_api_key: String,
    /// Google Gemini API key (or set GOOGLE_API_KEY env)
    #[serde(default)]
    pub gemini_api_key: String,
    /// Default provider: "anthropic", "openai", or "gemini"
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Default model (empty = provider default)
    #[serde(default)]
    pub model: String,
}

fn default_provider() -> String {
    "anthropic".to_string()
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            anthropic_api_key: String::new(),
            openai_api_key: String::new(),
            gemini_api_key: String::new(),
            provider: default_provider(),
            model: String::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,

    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    #[serde(default = "default_max_parallel")]
    pub max_parallel: usize,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ToolsConfig {
    #[serde(default)]
    pub shell: ShellConfig,

    #[serde(default)]
    pub python: PythonConfig,

    #[serde(default)]
    pub filesystem: FilesystemConfig,

    #[serde(default)]
    pub timeouts: HashMap<String, u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FilesystemConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Workspace root (relative to CWD or absolute)
    #[serde(default = "default_workspace")]
    pub workspace: String,

    /// Max file size in MB for read operations
    #[serde(default = "default_max_file_size_mb")]
    pub max_file_size_mb: u64,
}

fn default_workspace() -> String {
    ".".to_string()
}

fn default_max_file_size_mb() -> u64 {
    10
}

impl Default for FilesystemConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            workspace: default_workspace(),
            max_file_size_mb: default_max_file_size_mb(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ShellConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Command patterns to block (substring match)
    #[serde(default)]
    pub blocklist: Vec<String>,

    /// If non-empty, only allow commands starting with these executables
    #[serde(default)]
    pub allowlist: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PythonConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    #[serde(default = "default_scripts_dir")]
    pub scripts_dir: String,
}

fn default_dry_run() -> bool {
    true
}

fn default_timeout() -> u64 {
    60
}

fn default_max_parallel() -> usize {
    4
}

fn default_enabled() -> bool {
    true
}

fn default_scripts_dir() -> String {
    "./tools/python_examples".to_string()
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            blocklist: Vec::new(),
            allowlist: Vec::new(),
        }
    }
}

impl Default for PythonConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            scripts_dir: default_scripts_dir(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MemoryConfig {
    #[serde(default)]
    pub enabled: bool,

    /// Path to SQLite database for memory index
    #[serde(default = "default_memory_db_path")]
    pub db_path: String,

    /// Embedding provider: "openai" or "voyage"
    #[serde(default = "default_embedding_provider")]
    pub embedding_provider: String,

    /// Embedding model name
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,

    /// Auto-reindex on file changes
    #[serde(default = "default_auto_reindex")]
    pub auto_reindex: bool,
}

fn default_memory_db_path() -> String {
    "~/.silentclaw/memory.db".to_string()
}

fn default_embedding_provider() -> String {
    "openai".to_string()
}

fn default_embedding_model() -> String {
    "text-embedding-3-small".to_string()
}

fn default_auto_reindex() -> bool {
    true
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            db_path: default_memory_db_path(),
            embedding_provider: default_embedding_provider(),
            embedding_model: default_embedding_model(),
            auto_reindex: default_auto_reindex(),
        }
    }
}

impl Config {
    /// Create a default config (used as initial value for ConfigManager)
    pub fn default_config() -> Self {
        Self {
            version: default_config_version(),
            runtime: RuntimeConfig {
                dry_run: default_dry_run(),
                timeout_secs: default_timeout(),
                max_parallel: default_max_parallel(),
            },
            tools: ToolsConfig {
                shell: ShellConfig::default(),
                python: PythonConfig::default(),
                filesystem: FilesystemConfig::default(),
                timeouts: HashMap::new(),
            },
            llm: LlmConfig::default(),
            memory: MemoryConfig::default(),
            tool_policy: operon_runtime::tool_policy::config::ToolPolicyConfig::default(),
        }
    }

    /// Validate configuration values
    pub fn validate(&self) -> Result<()> {
        if self.runtime.timeout_secs == 0 {
            anyhow::bail!("runtime.timeout_secs must be > 0");
        }
        if self.runtime.timeout_secs > 3600 {
            tracing::warn!(
                "runtime.timeout_secs is very high ({}s)",
                self.runtime.timeout_secs
            );
        }
        if self.runtime.max_parallel == 0 || self.runtime.max_parallel > 100 {
            anyhow::bail!("runtime.max_parallel must be between 1-100");
        }
        Ok(())
    }

    /// Apply environment variable overrides
    pub fn apply_env_overrides(&mut self) {
        // Runtime overrides
        if let Ok(val) = std::env::var("SILENTCLAW_TIMEOUT") {
            if let Ok(secs) = val.parse::<u64>() {
                self.runtime.timeout_secs = secs;
            }
        }
        if let Ok(val) = std::env::var("SILENTCLAW_MAX_PARALLEL") {
            if let Ok(n) = val.parse::<usize>() {
                self.runtime.max_parallel = n;
            }
        }
        if let Ok(val) = std::env::var("SILENTCLAW_DRY_RUN") {
            if let Ok(b) = val.parse::<bool>() {
                self.runtime.dry_run = b;
            }
        }
        // LLM key overrides
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            if self.llm.anthropic_api_key.is_empty() {
                self.llm.anthropic_api_key = key;
            }
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            if self.llm.openai_api_key.is_empty() {
                self.llm.openai_api_key = key;
            }
        }
        if let Ok(key) = std::env::var("GOOGLE_API_KEY") {
            if self.llm.gemini_api_key.is_empty() {
                self.llm.gemini_api_key = key;
            }
        }
    }
}

/// Load config from file or use defaults
pub fn load_config(path: Option<&Path>) -> Result<Config> {
    let mut config = if let Some(path) = path {
        let content =
            fs::read_to_string(path).context(format!("Failed to read config file: {:?}", path))?;

        toml::from_str(&content).context("Failed to parse TOML config")?
    } else {
        Config::default_config()
    };

    // Apply environment variable overrides
    config.apply_env_overrides();

    // Validate config
    config.validate()?;

    Ok(config)
}
