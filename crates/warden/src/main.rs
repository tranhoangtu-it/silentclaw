mod cli;
mod commands;
mod config;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands, PluginCommands};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    operon_runtime::init_logging();

    // Parse CLI args
    let cli = Cli::parse();

    // Handle init command early (doesn't need config)
    if let Commands::Init { path } = &cli.command {
        return commands::init::run_init(path);
    }

    // Load config
    let config = config::load_config(cli.config.as_deref())?;

    // Resolve execution mode (--allow-tools backward compat)
    let execution_mode = cli.effective_execution_mode();

    // Dispatch to command
    match cli.command {
        Commands::Init { .. } => {
            // Already handled above
            unreachable!()
        }
        Commands::RunPlan { file } => {
            commands::run_plan::execute(file, execution_mode, &config, cli.record, cli.replay)
                .await?;
        }
        Commands::Chat { agent, session } => {
            commands::chat::execute(agent, session, execution_mode, &config).await?;
        }
        Commands::Plugin { action } => {
            let plugin_action = match action {
                PluginCommands::List => commands::plugin::PluginAction::List,
                PluginCommands::Load { path } => commands::plugin::PluginAction::Load(path),
                PluginCommands::Unload { name } => commands::plugin::PluginAction::Unload(name),
            };
            commands::plugin::execute(plugin_action).await?;
        }
        Commands::Serve { host, port } => {
            commands::serve::execute(host, port, execution_mode, &config).await?;
        }
    }

    Ok(())
}
