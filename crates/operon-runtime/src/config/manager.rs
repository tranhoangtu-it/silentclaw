use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use serde::de::DeserializeOwned;
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info};

/// Config reload event
#[derive(Debug, Clone)]
pub enum ConfigReloadEvent {
    Success,
    Failure(String),
}

/// Generic config manager with file watching and hot-reload
pub struct ConfigManager<C: DeserializeOwned + Send + Sync + 'static> {
    config: Arc<RwLock<C>>,
    config_path: PathBuf,
    reload_tx: broadcast::Sender<ConfigReloadEvent>,
}

impl<C: DeserializeOwned + Send + Sync + 'static> ConfigManager<C> {
    pub fn new(path: PathBuf, initial_config: C) -> Self {
        let (reload_tx, _) = broadcast::channel(10);
        Self {
            config: Arc::new(RwLock::new(initial_config)),
            config_path: path,
            reload_tx,
        }
    }

    /// Get shared reference to current config
    pub fn config(&self) -> Arc<RwLock<C>> {
        self.config.clone()
    }

    /// Subscribe to reload events
    pub fn subscribe_reload(&self) -> broadcast::Receiver<ConfigReloadEvent> {
        self.reload_tx.subscribe()
    }

    /// Start watching config file for changes (blocking, run in spawned task)
    pub async fn watch(&self) -> Result<()> {
        let config = self.config.clone();
        let config_path = self.config_path.clone();
        let reload_tx = self.reload_tx.clone();

        // Use std channel for notify (it's not async)
        let (tx, rx) = std::sync::mpsc::channel();

        let mut debouncer = new_debouncer(Duration::from_millis(500), tx)
            .context("Failed to create file watcher")?;

        debouncer
            .watcher()
            .watch(
                config_path.parent().unwrap_or(&config_path),
                notify::RecursiveMode::NonRecursive,
            )
            .context("Failed to watch config directory")?;

        info!(path = ?config_path, "Watching config file for changes");

        // Process events in blocking thread
        tokio::task::spawn_blocking(move || {
            // Keep debouncer alive
            let _debouncer = debouncer;

            for result in rx {
                match result {
                    Ok(events) => {
                        let relevant = events.iter().any(|e| {
                            e.kind == DebouncedEventKind::Any && e.path == config_path
                        });
                        if !relevant {
                            continue;
                        }

                        info!("Config file changed, reloading...");

                        match std::fs::read_to_string(&config_path) {
                            Ok(content) => match toml::from_str::<C>(&content) {
                                Ok(new_config) => {
                                    // Block on async write
                                    let config = config.clone();
                                    let rt = tokio::runtime::Handle::current();
                                    rt.block_on(async {
                                        *config.write().await = new_config;
                                    });
                                    info!("Config reloaded successfully");
                                    let _ = reload_tx.send(ConfigReloadEvent::Success);
                                }
                                Err(e) => {
                                    error!("Config parse failed: {}. Preserving old config.", e);
                                    let _ = reload_tx.send(ConfigReloadEvent::Failure(e.to_string()));
                                }
                            },
                            Err(e) => {
                                error!("Failed to read config: {}. Preserving old config.", e);
                                let _ = reload_tx.send(ConfigReloadEvent::Failure(e.to_string()));
                            }
                        }
                    }
                    Err(e) => {
                        error!("File watcher error: {:?}", e);
                    }
                }
            }
        });

        Ok(())
    }
}
