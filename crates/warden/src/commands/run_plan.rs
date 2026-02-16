use crate::cli::ExecutionMode;
use crate::config::Config;
use anyhow::{Context, Result};
use operon_adapters::ShellTool;
use operon_runtime::{ExecutionContext, Runtime};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

pub async fn execute(
    plan_file: PathBuf,
    execution_mode: ExecutionMode,
    config: &Config,
    record: Option<PathBuf>,
    replay: Option<PathBuf>,
) -> Result<()> {
    info!(?plan_file, ?execution_mode, "Running plan");

    // Read plan JSON
    let plan_content = std::fs::read_to_string(&plan_file)
        .context(format!("Failed to read plan file: {:?}", plan_file))?;

    let plan: serde_json::Value =
        serde_json::from_str(&plan_content).context("Failed to parse plan JSON")?;

    // Resolve dry-run from execution mode
    let dry_run = match execution_mode {
        ExecutionMode::Auto => config.runtime.dry_run,
        ExecutionMode::DryRun => true,
        ExecutionMode::Execute => false,
    };

    // Resolve execution context (record/replay)
    let execution_context = if let Some(dir) = replay {
        ExecutionContext::Replay(dir)
    } else if let Some(dir) = record {
        ExecutionContext::Record(dir)
    } else {
        ExecutionContext::Normal
    };

    // Create runtime (single timeout source)
    let default_timeout = Duration::from_secs(config.runtime.timeout_secs);
    let runtime = Runtime::new(dry_run, default_timeout)?
        .with_execution_context(execution_context)
        .with_max_parallel(config.runtime.max_parallel);

    // Register shell tool if enabled
    if config.tools.shell.enabled {
        let shell_tool = ShellTool::new(dry_run).with_validation(
            config.tools.shell.blocklist.clone(),
            config.tools.shell.allowlist.clone(),
        );

        runtime.register_tool("shell".to_string(), Arc::new(shell_tool))?;

        // Configure per-tool timeout (single location)
        if let Some(&timeout_secs) = config.tools.timeouts.get("shell") {
            runtime.configure_timeout("shell".to_string(), Duration::from_secs(timeout_secs));
        }

        info!("Registered shell tool");
    }

    // Register Python tools if enabled (auto-discovery)
    if config.tools.python.enabled {
        info!(
            scripts_dir = config.tools.python.scripts_dir,
            "Python tools enabled"
        );
        // Auto-discovery deferred to when scripts_dir actually exists
        // Tools are registered individually when discovered
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
