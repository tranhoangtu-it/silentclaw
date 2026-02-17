use anyhow::Result;
use operon_runtime::{HookRegistry, PluginLoader, Runtime};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

/// Plugin subcommand actions
pub enum PluginAction {
    List,
    Load(PathBuf),
    Unload(String),
}

pub async fn execute(action: PluginAction) -> Result<()> {
    let plugin_dir = dirs_home().join(".silentclaw").join("plugins");
    let runtime = Arc::new(Runtime::new(true, Duration::from_secs(60))?);
    let hook_registry = Arc::new(HookRegistry::new());
    let loader = PluginLoader::new(runtime, hook_registry);

    match action {
        PluginAction::List => {
            // Load all discovered plugins
            let _ = loader.load_all(&plugin_dir).await?;
            let plugins = loader.list_plugins().await;

            if plugins.is_empty() {
                println!("No plugins installed.");
                println!("Plugin directory: {:?}", plugin_dir);
            } else {
                println!("Installed plugins:");
                for (name, version) in plugins {
                    println!("  {} ({})", name, version);
                }
            }
        }
        PluginAction::Load(path) => {
            let manifest = operon_runtime::PluginManifest::load(&path.join("plugin.toml"))?;
            loader.load_plugin(&manifest, &path).await?;
            info!(plugin = %manifest.name, "Plugin loaded successfully");
            println!("Plugin '{}' loaded.", manifest.name);
        }
        PluginAction::Unload(name) => {
            // First load to populate
            let _ = loader.load_all(&plugin_dir).await?;
            loader.unload_plugin(&name).await?;
            println!("Plugin '{}' unloaded.", name);
        }
    }

    Ok(())
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}
