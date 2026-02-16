use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "warden")]
#[command(about = "SilentClaw - Rust agent runtime", long_about = None)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Override dry-run mode from config
    #[arg(long, default_value = "false")]
    pub allow_tools: bool,

    /// Path to config file
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Replay mode (not yet implemented)
    #[arg(long)]
    pub replay: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run a plan from JSON file
    RunPlan {
        /// Path to plan JSON file
        #[arg(long)]
        file: PathBuf,
    },
}
