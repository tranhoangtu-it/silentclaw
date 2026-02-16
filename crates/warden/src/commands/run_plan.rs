use crate::config::Config;
use anyhow::{Context, Result};
use operon_adapters::ShellTool;
use operon_runtime::Runtime;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

pub async fn execute(plan_file: PathBuf, allow_tools: bool, config: &Config) -> Result<()> {
    info!(?plan_file, allow_tools, "Running plan");

    // Read plan JSON
    let plan_content = std::fs::read_to_string(&plan_file)
        .context(format!("Failed to read plan file: {:?}", plan_file))?;

    let plan: serde_json::Value =
        serde_json::from_str(&plan_content).context("Failed to parse plan JSON")?;

    // Determine dry-run mode (CLI flag overrides config)
    let dry_run = if allow_tools {
        false
    } else {
        config.runtime.dry_run
    };

    // Create runtime
    let default_timeout = Duration::from_secs(config.runtime.timeout_secs);
    let runtime = Runtime::new(dry_run, default_timeout)?;

    // Register shell tool if enabled
    if config.tools.shell.enabled {
        let shell_timeout = config
            .tools
            .timeouts
            .get("shell")
            .copied()
            .unwrap_or(config.runtime.timeout_secs);

        let shell_tool = ShellTool::new(dry_run).with_timeout(Duration::from_secs(shell_timeout));

        runtime.register_tool("shell".to_string(), Arc::new(shell_tool));

        // Configure per-tool timeout if specified
        if let Some(&timeout_secs) = config.tools.timeouts.get("shell") {
            runtime.configure_timeout("shell".to_string(), Duration::from_secs(timeout_secs));
        }

        info!("Registered shell tool");
    }

    // Register Python tools if enabled (placeholder - would auto-discover scripts)
    if config.tools.python.enabled {
        info!(
            scripts_dir = config.tools.python.scripts_dir,
            "Python tools enabled (not yet implemented)"
        );
    }

    // Start runtime
    runtime.start().await?;

    // Run plan
    runtime.run_plan(plan).await?;

    // Stop runtime
    runtime.stop().await?;

    info!("Plan execution completed");

    Ok(())
}
