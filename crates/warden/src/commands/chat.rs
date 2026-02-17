use crate::cli::ExecutionMode;
use crate::config::Config;
use anyhow::{anyhow, Result};
use operon_adapters::{register_filesystem_tools, register_shell_tool, MemorySearchTool};
use operon_runtime::{
    Agent, AgentConfig, AnthropicClient, ConfigManager, ConfigReloadEvent, GeminiClient,
    LLMProvider, OpenAIClient, PermissionLevel, ProviderChain, Runtime, SessionStore,
    ToolPolicyPipeline,
};
use operon_runtime::tool_policy::layers::{
    AuditLogLayer, DryRunGuardLayer, InputValidationLayer, PermissionCheckLayer, RateLimitLayer,
    TimeoutEnforceLayer, ToolExistenceLayer,
};
use std::collections::HashMap;
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

    // Create runtime and register tools (build fully before Arc wrapping)
    let default_timeout = Duration::from_secs(config.runtime.timeout_secs);
    let mut runtime = Runtime::new(dry_run, default_timeout)?;

    if config.tools.shell.enabled {
        register_shell_tool(
            &runtime,
            dry_run,
            config.tools.shell.blocklist.clone(),
            config.tools.shell.allowlist.clone(),
        )?;
    }

    if config.tools.filesystem.enabled {
        register_filesystem_tools(
            &runtime,
            PathBuf::from(&config.tools.filesystem.workspace),
            config.tools.filesystem.max_file_size_mb,
        )?;
    }

    // Initialize memory search if enabled
    if config.memory.enabled {
        let db_path = shellexpand::tilde(&config.memory.db_path).to_string();
        let db_path = PathBuf::from(&db_path);
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let embedding_key = std::env::var("OPENAI_API_KEY")
            .or_else(|_| std::env::var("EMBEDDING_API_KEY"))
            .unwrap_or_default();

        if !embedding_key.is_empty() {
            let embedder = Arc::new(
                operon_runtime::memory::embedding::OpenAIEmbedding::new(&embedding_key),
            );
            let workspace = PathBuf::from(&config.tools.filesystem.workspace);
            let manager = Arc::new(
                operon_runtime::memory::MemoryManager::new(&db_path, workspace, embedder)?,
            );

            if config.memory.auto_reindex {
                let watcher_handle = manager.start_indexing().await?;
                // Keep watcher alive for the duration of the process
                tokio::spawn(async move { let _ = watcher_handle.await; });
            }

            runtime.register_tool(
                "memory_search".into(),
                Arc::new(MemorySearchTool::new(manager)),
            )?;
            info!("Memory search enabled");
        } else {
            tracing::warn!("Memory enabled but no embedding API key found (OPENAI_API_KEY)");
        }
    }

    // Build tool policy pipeline if enabled (before Arc wrapping)
    if config.tool_policy.enabled {
        let tool_names = runtime.tool_names();
        let mut pipeline = ToolPolicyPipeline::new()
            .add_layer(Box::new(ToolExistenceLayer::new(tool_names)));

        if config.tool_policy.permission_enabled {
            let default_perm = parse_permission_level(&config.tool_policy.default_permission);
            pipeline = pipeline.add_layer(Box::new(PermissionCheckLayer::new(
                HashMap::new(),
                default_perm,
            )));
        }

        if config.tool_policy.rate_limit_enabled {
            pipeline = pipeline.add_layer(Box::new(RateLimitLayer::new(
                config.tool_policy.max_calls_per_minute,
            )));
        }

        if config.tool_policy.input_validation_enabled {
            pipeline = pipeline.add_layer(Box::new(InputValidationLayer::new(
                HashMap::new(), // TODO: populate from runtime tool schemas
            )));
        }

        if config.tool_policy.dry_run_guard_enabled {
            pipeline = pipeline.add_layer(Box::new(DryRunGuardLayer::new(
                config.tool_policy.dry_run_bypass_tools.clone(),
            )));
        }

        if config.tool_policy.audit_enabled {
            pipeline = pipeline.add_layer(Box::new(AuditLogLayer::new()));
        }

        pipeline = pipeline.add_layer(Box::new(TimeoutEnforceLayer::new()));

        runtime.set_policy(pipeline);
        info!("Tool policy pipeline enabled");
    }

    // All setup done — now wrap in Arc
    let runtime = Arc::new(runtime);

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

    let gemini_key = if config.llm.gemini_api_key.is_empty() {
        std::env::var("GOOGLE_API_KEY").ok()
    } else {
        Some(config.llm.gemini_api_key.clone())
    };

    let mut providers: Vec<Arc<dyn LLMProvider>> = Vec::new();

    // Helper: push Gemini as fallback provider
    let push_gemini_fallback = |providers: &mut Vec<Arc<dyn LLMProvider>>,
                                 key: &Option<String>| {
        if let Some(key) = key {
            providers.push(Arc::new(GeminiClient::new(key)));
        }
    };

    // Primary provider first based on config
    match config.llm.provider.as_str() {
        "gemini" => {
            if let Some(key) = &gemini_key {
                let mut client = GeminiClient::new(key);
                if !config.llm.model.is_empty() {
                    client = client.with_model(&config.llm.model);
                }
                providers.push(Arc::new(client));
            }
            // Fallbacks: anthropic, then openai
            if let Some(key) = &anthropic_key {
                providers.push(Arc::new(AnthropicClient::new(key)));
            }
            if let Some(key) = &openai_key {
                providers.push(Arc::new(OpenAIClient::new(key)));
            }
        }
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
            push_gemini_fallback(&mut providers, &gemini_key);
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
            push_gemini_fallback(&mut providers, &gemini_key);
        }
    }

    if providers.is_empty() {
        return Err(anyhow!(
            "No LLM provider configured. Set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GOOGLE_API_KEY environment variable."
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

/// Parse permission level string from config to enum (defaults to Read for safety)
fn parse_permission_level(s: &str) -> PermissionLevel {
    match s.to_lowercase().as_str() {
        "read" => PermissionLevel::Read,
        "write" => PermissionLevel::Write,
        "execute" => PermissionLevel::Execute,
        "network" => PermissionLevel::Network,
        "admin" => PermissionLevel::Admin,
        _ => PermissionLevel::Read,
    }
}
