use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::hooks::HookRegistry;
use crate::Runtime;

use super::ffi_bridge::PluginHandle;
use super::manifest::{discover_plugins, PluginManifest, PluginType};

/// Current API version plugins must match
pub const CURRENT_API_VERSION: u32 = 1;

/// Loaded plugin: manifest metadata + optional FFI handle for native plugins
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub plugin_dir: std::path::PathBuf,
    /// FFI handle — present when the .so/.dylib was successfully loaded
    pub handle: Option<PluginHandle>,
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

    /// Load a single plugin from manifest.
    ///
    /// For native plugins: loads .so/.dylib via FFI, calls init(), registers tools+hooks.
    /// If the entry point is not a valid shared library, falls back to metadata-only mode.
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

        let mut ffi_handle: Option<PluginHandle> = None;

        // Validate and load plugin
        match manifest.plugin_type {
            PluginType::Native => {
                let entry_path = manifest.resolve_entry_point(plugin_dir);
                if !entry_path.exists() {
                    return Err(anyhow!("Plugin entry point not found: {:?}", entry_path));
                }

                // Attempt FFI loading
                match PluginHandle::load(&entry_path) {
                    Ok(mut handle) => {
                        // Init with panic isolation.
                        // AssertUnwindSafe is sound: on panic, we return Err and never
                        // use the plugin handle. The handle is dropped, cleaning up resources.
                        let config = manifest.config.clone();
                        let init_result =
                            catch_unwind(AssertUnwindSafe(|| handle.plugin_mut().init(config)));

                        match init_result {
                            Ok(Ok(())) => {
                                // Register tools
                                for tool in handle.plugin().tools() {
                                    let name = tool.name().to_string();
                                    if let Err(e) =
                                        self.runtime.register_tool(name.clone(), Arc::from(tool))
                                    {
                                        warn!(tool = %name, error = %e, "Failed to register plugin tool");
                                    }
                                }

                                // Register hooks
                                for hook in handle.plugin().hooks() {
                                    self.hook_registry.register(Arc::from(hook));
                                }

                                info!(
                                    plugin = %manifest.name,
                                    entry = ?entry_path,
                                    "Native plugin loaded via FFI"
                                );
                                ffi_handle = Some(handle);
                            }
                            Ok(Err(e)) => {
                                warn!(plugin = %manifest.name, error = %e, "Plugin init failed");
                                return Err(anyhow!(
                                    "Plugin '{}' init failed: {}",
                                    manifest.name,
                                    e
                                ));
                            }
                            Err(_) => {
                                warn!(plugin = %manifest.name, "Plugin panicked during init");
                                return Err(anyhow!(
                                    "Plugin '{}' panicked during init",
                                    manifest.name
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        // Not a valid .so/.dylib — register metadata only
                        info!(
                            plugin = %manifest.name,
                            entry = ?entry_path,
                            error = %e,
                            "Native plugin validated (FFI loading unavailable)"
                        );
                    }
                }
            }
        }

        // Store plugin
        self.plugins.write().await.insert(
            manifest.name.clone(),
            LoadedPlugin {
                manifest: manifest.clone(),
                plugin_dir: plugin_dir.to_path_buf(),
                handle: ffi_handle,
            },
        );

        Ok(())
    }

    /// Unload a plugin by name. Calls shutdown on FFI-loaded plugins.
    pub async fn unload_plugin(&self, name: &str) -> Result<()> {
        let loaded = self
            .plugins
            .write()
            .await
            .remove(name)
            .ok_or_else(|| anyhow!("Plugin '{}' not found", name))?;

        // Shutdown FFI handle if present
        if let Some(handle) = loaded.handle {
            handle.shutdown_and_drop();
        }

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

    fn make_test_runtime() -> (Arc<Runtime>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let runtime = Arc::new(
            Runtime::with_db(db_path.to_str().unwrap(), true, Duration::from_secs(30)).unwrap(),
        );
        (runtime, dir)
    }

    #[tokio::test]
    async fn test_load_plugin_api_version_mismatch() {
        let (runtime, _dir) = make_test_runtime();
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
            config: serde_json::Value::Null,
        };

        let dir = tempfile::tempdir().unwrap();
        let result = loader.load_plugin(&manifest, dir.path()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("API version"));
    }

    #[tokio::test]
    async fn test_load_plugin_duplicate() {
        let (runtime, _dir) = make_test_runtime();
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
            config: serde_json::Value::Null,
        };

        loader.load_plugin(&manifest, dir.path()).await.unwrap();
        let result = loader.load_plugin(&manifest, dir.path()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already loaded"));
    }

    #[tokio::test]
    async fn test_list_and_unload() {
        let (runtime, _dir) = make_test_runtime();
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
            config: serde_json::Value::Null,
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
        let (runtime, _dir) = make_test_runtime();
        let hook_registry = Arc::new(HookRegistry::new());
        let loader = PluginLoader::new(runtime, hook_registry);

        let dir = tempfile::tempdir().unwrap();
        let loaded = loader.load_all(dir.path()).await.unwrap();
        assert_eq!(loaded, 0);
    }

    #[tokio::test]
    async fn test_load_all_with_plugin() {
        let (runtime, _dir) = make_test_runtime();
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
