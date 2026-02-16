use crate::cli::ExecutionMode;
use crate::commands::chat::build_provider;
use crate::config::Config;
use anyhow::Result;
use operon_adapters::ShellTool;
use operon_gateway::{start_server, AppState, AuthConfig, RateLimiter, SessionManager};
use operon_runtime::Runtime;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

pub async fn execute(
    host: String,
    port: u16,
    execution_mode: ExecutionMode,
    config: &Config,
) -> Result<()> {
    info!(host = %host, port, "Starting gateway server");

    let provider = build_provider(config)?;

    let dry_run = match execution_mode {
        ExecutionMode::Auto => config.runtime.dry_run,
        ExecutionMode::DryRun => true,
        ExecutionMode::Execute => false,
    };

    let default_timeout = Duration::from_secs(config.runtime.timeout_secs);
    let runtime = Arc::new(Runtime::new(dry_run, default_timeout)?);

    if config.tools.shell.enabled {
        let shell_tool = ShellTool::new(dry_run).with_validation(
            config.tools.shell.blocklist.clone(),
            config.tools.shell.allowlist.clone(),
        );
        runtime.register_tool("shell".to_string(), Arc::new(shell_tool))?;
    }

    let session_manager = Arc::new(SessionManager::new(provider, runtime));

    let state = AppState {
        session_manager,
        auth_config: Arc::new(AuthConfig::new(None)),
        rate_limiter: Arc::new(RateLimiter::new(120)),
        allowed_origins: vec![],
    };

    start_server(state, &host, port).await?;

    Ok(())
}
