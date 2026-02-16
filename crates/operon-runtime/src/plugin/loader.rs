use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::hooks::HookRegistry;
use crate::Runtime;

use super::manifest::{discover_plugins, PluginManifest, PluginType};

/// Current API version plugins must match
pub const CURRENT_API_VERSION: u32 = 1;

/// Loaded plugin info (metadata only - tools/hooks already registered)
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub plugin_dir: std::path::PathBuf,
}

/// Plugin loader: discovers, validates, loads, and registers plugins
pub struct PluginLoader {
    plugins: Arc<RwLock<HashMap<String, LoadedPlugin>>>,
    runtime: Arc<Runtime>,
    hook_registry: Arc<HookRegistry>,
}

impl PluginLoader {
    pub fn new(runtime: Arc<Runtime>, hook_registry: Arc<HookRegistry>) -> Self {
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            runtime,
            hook_registry,
        }
    }

    /// Discover and load all plugins from a directory
    pub async fn load_all(&self, plugin_dir: &Path) -> Result<usize> {
        let discovered = discover_plugins(plugin_dir)?;
        let mut loaded = 0;

        for (manifest, dir) in discovered {
            match self.load_plugin(&manifest, &dir).await {
                Ok(()) => {
                    loaded += 1;
                    info!(plugin = %manifest.name, version = %manifest.version, "Plugin loaded");
                }
                Err(e) => {
                    warn!(plugin = %manifest.name, error = %e, "Failed to load plugin");
                }
            }
        }

        Ok(loaded)
    }

    /// Load a single plugin from manifest
    pub async fn load_plugin(&self, manifest: &PluginManifest, plugin_dir: &Path) -> Result<()> {
        // Validate API version
        if manifest.api_version != CURRENT_API_VERSION {
            return Err(anyhow!(
                "Plugin '{}' API version {} doesn't match runtime version {}",
                manifest.name,
                manifest.api_version,
                CURRENT_API_VERSION
            ));
        }

        // Check for duplicate
        if self.plugins.read().await.contains_key(&manifest.name) {
            return Err(anyhow!("Plugin '{}' already loaded", manifest.name));
        }

        // Validate plugin type
        match manifest.plugin_type {
            PluginType::Native => {
                let entry_path = manifest.resolve_entry_point(plugin_dir);
                if !entry_path.exists() {
                    return Err(anyhow!(
                        "Plugin entry point not found: {:?}",
                        entry_path
                    ));
                }
                // Native loading via libloading deferred until actual .so/.dylib is built
                // For now, we validate manifest and register metadata
                info!(
                    plugin = %manifest.name,
                    entry = ?entry_path,
                    "Native plugin validated (FFI loading requires compiled library)"
                );
            }
        }

        // Store plugin metadata
        self.plugins.write().await.insert(
            manifest.name.clone(),
            LoadedPlugin {
                manifest: manifest.clone(),
                plugin_dir: plugin_dir.to_path_buf(),
            },
        );

        Ok(())
    }

    /// Unload a plugin by name
    pub async fn unload_plugin(&self, name: &str) -> Result<()> {
        self.plugins
            .write()
            .await
            .remove(name)
            .ok_or_else(|| anyhow!("Plugin '{}' not found", name))?;

        info!(plugin = name, "Plugin unloaded");
        Ok(())
    }

    /// List all loaded plugins
    pub async fn list_plugins(&self) -> Vec<(String, String)> {
        self.plugins
            .read()
            .await
            .iter()
            .map(|(name, p)| (name.clone(), p.manifest.version.clone()))
            .collect()
    }

    /// Get reference to runtime (for plugin tool registration)
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    /// Get reference to hook registry (for plugin hook registration)
    pub fn hook_registry(&self) -> &Arc<HookRegistry> {
        &self.hook_registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    fn make_test_runtime() -> Arc<Runtime> {
        Arc::new(
            Runtime::with_db(
                &format!("/tmp/silentclaw-plugin-test-{}.db", uuid::Uuid::new_v4()),
                true,
                Duration::from_secs(30),
            )
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn test_load_plugin_api_version_mismatch() {
        let runtime = make_test_runtime();
        let hook_registry = Arc::new(HookRegistry::new());
        let loader = PluginLoader::new(runtime, hook_registry);

        let manifest = PluginManifest {
            name: "bad-version".into(),
            version: "1.0.0".into(),
            api_version: 999, // wrong version
            author: String::new(),
            description: String::new(),
            plugin_type: PluginType::Native,
            entry_point: "./libtest.so".into(),
            dependencies: vec![],
        };

        let dir = tempfile::tempdir().unwrap();
        let result = loader.load_plugin(&manifest, dir.path()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("API version"));
    }

    #[tokio::test]
    async fn test_load_plugin_duplicate() {
        let runtime = make_test_runtime();
        let hook_registry = Arc::new(HookRegistry::new());
        let loader = PluginLoader::new(runtime, hook_registry);

        let dir = tempfile::tempdir().unwrap();
        // Create a fake .so file so validation passes
        std::fs::write(dir.path().join("libtest.so"), b"fake").unwrap();

        let manifest = PluginManifest {
            name: "test".into(),
            version: "1.0.0".into(),
            api_version: 1,
            author: String::new(),
            description: String::new(),
            plugin_type: PluginType::Native,
            entry_point: "./libtest.so".into(),
            dependencies: vec![],
        };

        loader.load_plugin(&manifest, dir.path()).await.unwrap();
        let result = loader.load_plugin(&manifest, dir.path()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already loaded"));
    }

    #[tokio::test]
    async fn test_list_and_unload() {
        let runtime = make_test_runtime();
        let hook_registry = Arc::new(HookRegistry::new());
        let loader = PluginLoader::new(runtime, hook_registry);

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("libtest.so"), b"fake").unwrap();

        let manifest = PluginManifest {
            name: "test".into(),
            version: "2.0.0".into(),
            api_version: 1,
            author: String::new(),
            description: String::new(),
            plugin_type: PluginType::Native,
            entry_point: "./libtest.so".into(),
            dependencies: vec![],
        };

        loader.load_plugin(&manifest, dir.path()).await.unwrap();

        let list = loader.list_plugins().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "test");
        assert_eq!(list[0].1, "2.0.0");

        loader.unload_plugin("test").await.unwrap();
        assert!(loader.list_plugins().await.is_empty());
    }

    #[tokio::test]
    async fn test_load_all_empty_dir() {
        let runtime = make_test_runtime();
        let hook_registry = Arc::new(HookRegistry::new());
        let loader = PluginLoader::new(runtime, hook_registry);

        let dir = tempfile::tempdir().unwrap();
        let loaded = loader.load_all(dir.path()).await.unwrap();
        assert_eq!(loaded, 0);
    }

    #[tokio::test]
    async fn test_load_all_with_plugin() {
        let runtime = make_test_runtime();
        let hook_registry = Arc::new(HookRegistry::new());
        let loader = PluginLoader::new(runtime, hook_registry);

        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("my-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();

        // Create manifest
        let mut f = std::fs::File::create(plugin_dir.join("plugin.toml")).unwrap();
        write!(
            f,
            r#"
name = "my-plugin"
version = "1.0.0"
api_version = 1
entry_point = "./libmy_plugin.so"
"#
        )
        .unwrap();

        // Create fake entry point
        std::fs::write(plugin_dir.join("libmy_plugin.so"), b"fake").unwrap();

        let loaded = loader.load_all(dir.path()).await.unwrap();
        assert_eq!(loaded, 1);
    }
}
