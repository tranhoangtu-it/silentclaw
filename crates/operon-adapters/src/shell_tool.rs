use anyhow::{Context, Result};
use async_trait::async_trait;
use operon_runtime::{PermissionLevel, Tool, ToolSchemaInfo};
use serde_json::{json, Value};
use tokio::process::Command;
use tracing::{info, warn};

/// Default dangerous patterns blocked regardless of config
const BUILTIN_BLOCKLIST: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    ":(){ :|:& };:",
    "mkfs",
    "> /dev/sd",
    "> /dev/nvme",
    "dd if=",
    "chmod -R 777 /",
    "chown -R",
    "eval ",
    "base64 ",
    "${ifs}",
];

/// Shell meta-characters that allow chaining commands
const SHELL_OPERATORS: &[&str] = &[";", "&&", "||", "|", "`", "$("];

pub struct ShellTool {
    dry_run: bool,
    blocklist: Vec<String>,
    allowlist: Vec<String>,
}

impl ShellTool {
    /// Create new shell tool
    pub fn new(dry_run: bool) -> Self {
        Self {
            dry_run,
            blocklist: Vec::new(),
            allowlist: Vec::new(),
        }
    }

    /// Configure command validation lists
    pub fn with_validation(mut self, blocklist: Vec<String>, allowlist: Vec<String>) -> Self {
        self.blocklist = blocklist;
        self.allowlist = allowlist;
        self
    }

    /// Execute shell command (no internal timeout — runtime manages timeout)
    async fn execute_command(&self, cmd: &str) -> Result<Value> {
        // Validate command before any execution
        validate_command(cmd, &self.blocklist, &self.allowlist)?;

        if self.dry_run {
            warn!(cmd, "SANDBOX MODE - command not executed");
            return Ok(json!({
                "exit_code": 0,
                "stdout": "[dry-run]",
                "stderr": ""
            }));
        }

        // Audit log: record exact command being executed
        info!(cmd, "Executing shell command");

        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .await
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

    fn schema(&self) -> ToolSchemaInfo {
        ToolSchemaInfo {
            name: "shell".to_string(),
            description: "Execute a shell command safely".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "cmd": {
                        "type": "string",
                        "description": "Shell command to execute"
                    }
                },
                "required": ["cmd"]
            }),
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }
}

/// Validate command against blocklist and optional allowlist.
fn validate_command(cmd: &str, blocklist: &[String], allowlist: &[String]) -> Result<()> {
    let cmd_lower = cmd.to_lowercase();

    // Check built-in blocklist
    for pattern in BUILTIN_BLOCKLIST {
        if cmd_lower.contains(pattern) {
            anyhow::bail!("Command blocked (dangerous pattern '{}'): {}", pattern, cmd);
        }
    }

    // Check user-configured blocklist
    for pattern in blocklist {
        if cmd_lower.contains(&pattern.to_lowercase()) {
            anyhow::bail!("Command blocked (config blocklist '{}'): {}", pattern, cmd);
        }
    }

    // Check allowlist (if configured)
    if !allowlist.is_empty() {
        let cmd_executable = cmd.split_whitespace().next().unwrap_or("");
        if !allowlist.iter().any(|a| a == cmd_executable) {
            anyhow::bail!(
                "Command '{}' not in allowlist. Allowed: {:?}",
                cmd_executable,
                allowlist
            );
        }
        // Block shell operators that could chain unauthorized commands
        for op in SHELL_OPERATORS {
            if cmd.contains(op) {
                anyhow::bail!(
                    "Command contains shell operator '{}' which is not allowed in allowlist mode",
                    op
                );
            }
        }
    }

    Ok(())
}
