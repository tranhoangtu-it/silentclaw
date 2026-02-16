use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub runtime: RuntimeConfig,
    pub tools: ToolsConfig,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,

    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ToolsConfig {
    #[serde(default)]
    pub shell: ShellConfig,

    #[serde(default)]
    pub python: PythonConfig,

    #[serde(default)]
    pub timeouts: HashMap<String, u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ShellConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PythonConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    #[serde(default = "default_scripts_dir")]
    pub scripts_dir: String,
}

fn default_dry_run() -> bool {
    true
}

fn default_timeout() -> u64 {
    60
}

fn default_enabled() -> bool {
    true
}

fn default_scripts_dir() -> String {
    "./tools/python_examples".to_string()
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
        }
    }
}

impl Default for PythonConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            scripts_dir: default_scripts_dir(),
        }
    }
}

/// Load config from file or use defaults
pub fn load_config(path: Option<&Path>) -> Result<Config> {
    if let Some(path) = path {
        let content =
            fs::read_to_string(path).context(format!("Failed to read config file: {:?}", path))?;

        let config: Config = toml::from_str(&content).context("Failed to parse TOML config")?;

        Ok(config)
    } else {
        // Use default config
        Ok(Config {
            runtime: RuntimeConfig {
                dry_run: default_dry_run(),
                timeout_secs: default_timeout(),
            },
            tools: ToolsConfig {
                shell: ShellConfig::default(),
                python: PythonConfig::default(),
                timeouts: HashMap::new(),
            },
        })
    }
}
