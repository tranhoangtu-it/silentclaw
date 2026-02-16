use anyhow::{Context, Result};
use async_trait::async_trait;
use operon_runtime::Tool;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::process::Command;
use tracing::{info, warn};

pub struct ShellTool {
    dry_run: bool,
    timeout: Duration,
}

impl ShellTool {
    /// Create new shell tool
    pub fn new(dry_run: bool) -> Self {
        Self {
            dry_run,
            timeout: Duration::from_secs(60),
        }
    }

    /// Configure custom timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Execute shell command with timeout
    async fn execute_command(&self, cmd: &str) -> Result<Value> {
        if self.dry_run {
            warn!(cmd, "SANDBOX MODE - command not executed");
            return Ok(json!({
                "exit_code": 0,
                "stdout": "[dry-run]",
                "stderr": ""
            }));
        }

        info!(cmd, "Executing shell command");

        let output =
            tokio::time::timeout(self.timeout, Command::new("sh").arg("-c").arg(cmd).output())
                .await
                .context("Command timeout")?
                .context("Command execution failed")?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok(json!({
            "exit_code": exit_code,
            "stdout": stdout,
            "stderr": stderr
        }))
    }
}

#[async_trait]
impl Tool for ShellTool {
    async fn execute(&self, input: Value) -> Result<Value> {
        let cmd = input["cmd"].as_str().context("Input missing 'cmd' field")?;

        self.execute_command(cmd).await
    }

    fn name(&self) -> &str {
        "shell"
    }
}
