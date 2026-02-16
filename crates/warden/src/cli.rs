use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum PluginCommands {
    /// List installed plugins
    List,
    /// Load a plugin from directory
    Load {
        /// Path to plugin directory (containing plugin.toml)
        path: PathBuf,
    },
    /// Unload a plugin by name
    Unload {
        /// Plugin name
        name: String,
    },
}

#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum ExecutionMode {
    /// Use config.runtime.dry_run setting (default)
    Auto,
    /// Force dry-run: no tools execute
    DryRun,
    /// Force real execution: tools run regardless of config
    Execute,
}

#[derive(Parser)]
#[command(name = "warden")]
#[command(about = "SilentClaw - Rust agent runtime", long_about = None)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Execution mode: auto (use config), dry-run (force safe), execute (force real)
    #[arg(long, default_value = "auto", value_enum)]
    pub execution_mode: ExecutionMode,

    /// [DEPRECATED] Alias for --execution-mode execute
    #[arg(long, default_value = "false", hide = true)]
    pub allow_tools: bool,

    /// Path to config file
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Record tool outputs to fixture directory for replay testing
    #[arg(long, conflicts_with = "replay")]
    pub record: Option<PathBuf>,

    /// Replay from fixture directory (skip real tool execution)
    #[arg(long, conflicts_with = "record")]
    pub replay: Option<PathBuf>,
}

impl Cli {
    /// Resolve effective execution mode (--allow-tools overrides if set)
    pub fn effective_execution_mode(&self) -> ExecutionMode {
        if self.allow_tools {
            ExecutionMode::Execute
        } else {
            self.execution_mode.clone()
        }
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new config file
    Init {
        /// Path for new config file
        #[arg(default_value = "silentclaw.toml")]
        path: PathBuf,
    },
    /// Run a plan from JSON file
    RunPlan {
        /// Path to plan JSON file
        #[arg(long)]
        file: PathBuf,
    },
    /// Interactive chat with an agent
    Chat {
        /// Agent name (uses default config if not specified)
        #[arg(long, default_value = "default")]
        agent: String,
        /// Resume existing session by ID
        #[arg(long)]
        session: Option<String>,
    },
    /// Manage plugins
    Plugin {
        #[command(subcommand)]
        action: PluginCommands,
    },
    /// Start the HTTP/WebSocket gateway server
    Serve {
        /// Host to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to listen on
        #[arg(long, default_value = "8080")]
        port: u16,
    },
}
