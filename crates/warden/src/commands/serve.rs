use crate::cli::ExecutionMode;
use crate::commands::chat::build_provider;
use crate::config::Config;
use anyhow::Result;
use operon_adapters::{
    ApplyPatchTool, EditFileTool, ReadFileTool, ShellTool, WorkspaceGuard, WriteFileTool,
};
use operon_gateway::{start_server, AppState, AuthConfig, RateLimiter, SessionManager};
use operon_runtime::{ConfigManager, ConfigReloadEvent, Runtime};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

/// Execute serve command with optional config file path for hot-reload
pub async fn execute(
    host: String,
    port: u16,
    execution_mode: ExecutionMode,
    config: &Config,
    config_path: Option<PathBuf>,
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

    if config.tools.filesystem.enabled {
        let ws_root = std::path::PathBuf::from(&config.tools.filesystem.workspace);
        let guard = Arc::new(WorkspaceGuard::new(
            ws_root,
            config.tools.filesystem.max_file_size_mb,
        )?);
        runtime.register_tool(
            "read_file".into(),
            Arc::new(ReadFileTool::new(guard.clone())),
        )?;
        runtime.register_tool(
            "write_file".into(),
            Arc::new(WriteFileTool::new(guard.clone())),
        )?;
        runtime.register_tool(
            "edit_file".into(),
            Arc::new(EditFileTool::new(guard.clone())),
        )?;
        runtime.register_tool("apply_patch".into(), Arc::new(ApplyPatchTool::new(guard)))?;
    }

    // Start config hot-reload watcher if config path is provided
    if let Some(ref path) = config_path {
        let config_manager = ConfigManager::<Config>::new(path.clone(), Config::default_config());
        let mut reload_rx = config_manager.subscribe_reload();

        // Spawn watcher
        let watcher_handle = tokio::spawn({
            let cm = config_manager;
            async move {
                if let Err(e) = cm.watch().await {
                    tracing::error!("Config watcher failed: {}", e);
                }
            }
        });

        // Spawn reload listener
        tokio::spawn(async move {
            while let Ok(event) = reload_rx.recv().await {
                match event {
                    ConfigReloadEvent::Success => {
                        info!("Config file reloaded successfully (note: runtime provider swap not yet implemented)");
                    }
                    ConfigReloadEvent::Failure(err) => {
                        tracing::warn!("Config reload failed: {}. Old config preserved.", err);
                    }
                }
            }
            drop(watcher_handle);
        });
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
