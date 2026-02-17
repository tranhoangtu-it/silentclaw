use crate::cli::ExecutionMode;
use crate::config::Config;
use anyhow::{anyhow, Result};
use operon_adapters::{
    ApplyPatchTool, EditFileTool, ReadFileTool, ShellTool, WorkspaceGuard, WriteFileTool,
};
use operon_runtime::{
    Agent, AgentConfig, AnthropicClient, ConfigManager, ConfigReloadEvent, LLMProvider,
    OpenAIClient, ProviderChain, Runtime, SessionStore,
};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

/// Execute chat command with optional config file path for hot-reload
pub async fn execute(
    agent_name: String,
    session_id: Option<String>,
    execution_mode: ExecutionMode,
    config: &Config,
    config_path: Option<PathBuf>,
) -> Result<()> {
    info!(agent = %agent_name, "Starting chat session");

    // Build LLM provider from config
    let provider = build_provider(config)?;

    // Resolve dry-run
    let dry_run = match execution_mode {
        ExecutionMode::Auto => config.runtime.dry_run,
        ExecutionMode::DryRun => true,
        ExecutionMode::Execute => false,
    };

    // Create runtime and register tools
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
        let ws_root = PathBuf::from(&config.tools.filesystem.workspace);
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

    // Build agent config
    let agent_config = AgentConfig {
        name: agent_name.clone(),
        model: config.llm.model.clone(),
        ..AgentConfig::default()
    };

    // Create or resume agent
    let session_store = SessionStore::new(dirs_home().join(".silentclaw").join("sessions"))?;

    let mut agent = if let Some(ref sid) = session_id {
        let session = session_store.load(sid).await?;
        info!(
            session_id = sid,
            messages = session.message_count(),
            "Resumed session"
        );
        Agent::new(agent_config, provider, runtime).with_session(session)
    } else {
        Agent::new(agent_config, provider, runtime)
    };

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

    println!("SilentClaw Agent [{}] - Type 'exit' to quit", agent_name);
    println!("Session: {}", agent.session.id);
    println!("---");

    // Interactive REPL
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!("> ");
        stdout.flush()?;

        let mut input = String::new();
        stdin.lock().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        if input == "exit" || input == "quit" {
            // Save session before exit
            session_store.save(&agent.session).await?;
            println!("Session saved: {}", agent.session.id);
            break;
        }

        match agent.process_message(input).await {
            Ok(response) => {
                println!("\nAssistant: {}\n", response);
            }
            Err(e) => {
                eprintln!("\nError: {}\n", e);
            }
        }
    }

    Ok(())
}

/// Build LLM provider from config (supports env vars as fallback)
pub fn build_provider(config: &Config) -> Result<Arc<dyn LLMProvider>> {
    let anthropic_key = if config.llm.anthropic_api_key.is_empty() {
        std::env::var("ANTHROPIC_API_KEY").ok()
    } else {
        Some(config.llm.anthropic_api_key.clone())
    };

    let openai_key = if config.llm.openai_api_key.is_empty() {
        std::env::var("OPENAI_API_KEY").ok()
    } else {
        Some(config.llm.openai_api_key.clone())
    };

    let mut providers: Vec<Arc<dyn LLMProvider>> = Vec::new();

    // Primary provider first based on config
    match config.llm.provider.as_str() {
        "openai" => {
            if let Some(key) = &openai_key {
                let mut client = OpenAIClient::new(key);
                if !config.llm.model.is_empty() {
                    client = client.with_model(&config.llm.model);
                }
                providers.push(Arc::new(client));
            }
            if let Some(key) = &anthropic_key {
                providers.push(Arc::new(AnthropicClient::new(key)));
            }
        }
        _ => {
            // Default: anthropic first
            if let Some(key) = &anthropic_key {
                let mut client = AnthropicClient::new(key);
                if !config.llm.model.is_empty() {
                    client = client.with_model(&config.llm.model);
                }
                providers.push(Arc::new(client));
            }
            if let Some(key) = &openai_key {
                providers.push(Arc::new(OpenAIClient::new(key)));
            }
        }
    }

    if providers.is_empty() {
        return Err(anyhow!(
            "No LLM provider configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY environment variable."
        ));
    }

    if providers.len() == 1 {
        Ok(providers.into_iter().next().unwrap())
    } else {
        Ok(Arc::new(ProviderChain::new(providers)))
    }
}

/// Get home directory
fn dirs_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}
