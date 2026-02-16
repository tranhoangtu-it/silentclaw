use crate::{Storage, Tool};
use anyhow::{Context, Result};
use dashmap::DashMap;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

pub struct Runtime {
    tools: Arc<DashMap<String, Arc<dyn Tool>>>,
    storage: Storage,
    dry_run: bool,
    default_timeout: Duration,
    tool_timeouts: DashMap<String, Duration>,
}

impl Runtime {
    /// Create new runtime with dry-run flag and default timeout
    pub fn new(dry_run: bool, default_timeout: Duration) -> Result<Self> {
        Self::with_db("./silentclaw.db", dry_run, default_timeout)
    }

    /// Create new runtime with custom database path
    pub fn with_db(db_path: &str, dry_run: bool, default_timeout: Duration) -> Result<Self> {
        let storage = Storage::open(db_path)?;

        Ok(Self {
            tools: Arc::new(DashMap::new()),
            storage,
            dry_run,
            default_timeout,
            tool_timeouts: DashMap::new(),
        })
    }

    /// Register a tool with optional custom timeout
    pub fn register_tool(&self, name: String, tool: Arc<dyn Tool>) {
        self.tools.insert(name, tool);
    }

    /// Configure timeout for specific tool
    pub fn configure_timeout(&self, tool_name: String, timeout: Duration) {
        self.tool_timeouts.insert(tool_name, timeout);
    }

    /// Get timeout for tool (custom or default)
    pub fn get_timeout(&self, tool_name: &str) -> Duration {
        self.tool_timeouts
            .get(tool_name)
            .map(|t| *t)
            .unwrap_or(self.default_timeout)
    }

    /// Run plan JSON
    pub async fn run_plan(&self, plan: Value) -> Result<()> {
        let steps = plan["steps"]
            .as_array()
            .context("Plan missing 'steps' array")?;

        for (i, step) in steps.iter().enumerate() {
            let tool_name = step["tool"].as_str().context("Step missing 'tool' field")?;

            let input = step["input"].clone();

            if self.dry_run {
                warn!(
                    step = i,
                    tool = tool_name,
                    "DRY-RUN: Skipping tool execution"
                );
                continue;
            }

            let tool = self
                .tools
                .get(tool_name)
                .context(format!("Tool '{}' not registered", tool_name))?;

            let timeout = self.get_timeout(tool_name);

            info!(step = i, tool = tool_name, "Executing tool");

            let result = tokio::time::timeout(timeout, tool.execute(input))
                .await
                .context("Tool execution timeout")?
                .context("Tool execution failed")?;

            info!(step = i, tool = tool_name, "Tool completed");

            // Store result
            self.storage.save_state(&format!("step_{}", i), &result)?;
        }

        Ok(())
    }

    /// Start runtime (placeholder for future async initialization)
    pub async fn start(&self) -> Result<()> {
        info!("Runtime started");
        Ok(())
    }

    /// Stop runtime (placeholder for cleanup)
    pub async fn stop(&self) -> Result<()> {
        info!("Runtime stopped");
        Ok(())
    }
}
