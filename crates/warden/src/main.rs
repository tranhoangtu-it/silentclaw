mod cli;
mod commands;
mod config;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    operon_runtime::init_logging();

    // Parse CLI args
    let cli = Cli::parse();

    // Load config
    let config = config::load_config(cli.config.as_deref())?;

    // Dispatch to command
    match cli.command {
        Commands::RunPlan { file } => {
            commands::run_plan::execute(file, cli.allow_tools, &config).await?;
        }
    }

    Ok(())
}
