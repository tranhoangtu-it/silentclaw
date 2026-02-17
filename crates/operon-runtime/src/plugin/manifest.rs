use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Plugin type (native dynamic library only for now)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PluginType {
    Native,
}

/// Plugin manifest (parsed from plugin.toml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub api_version: u32,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_plugin_type")]
    pub plugin_type: PluginType,
    /// Path to shared library relative to manifest dir
    pub entry_point: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Optional plugin configuration passed to `Plugin::init()`
    #[serde(default)]
    pub config: serde_json::Value,
}

fn default_plugin_type() -> PluginType {
    PluginType::Native
}

impl PluginManifest {
    /// Load manifest from plugin.toml file
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .context(format!("Failed to read plugin manifest: {:?}", path))?;
        let manifest: Self =
            toml::from_str(&content).context(format!("Failed to parse manifest: {:?}", path))?;
        Ok(manifest)
    }

    /// Resolve entry point path relative to manifest directory
    pub fn resolve_entry_point(&self, manifest_dir: &Path) -> std::path::PathBuf {
        manifest_dir.join(&self.entry_point)
    }
}

/// Discover plugins by walking a directory for plugin.toml files
pub fn discover_plugins(plugin_dir: &Path) -> Result<Vec<(PluginManifest, std::path::PathBuf)>> {
    let mut plugins = Vec::new();

    if !plugin_dir.exists() {
        return Ok(plugins);
    }

    for entry in std::fs::read_dir(plugin_dir)? {
        let entry = entry?;
        let manifest_path = entry.path().join("plugin.toml");
        if manifest_path.exists() {
            match PluginManifest::load(&manifest_path) {
                Ok(manifest) => {
                    plugins.push((manifest, entry.path()));
                }
                Err(e) => {
                    tracing::warn!(path = ?manifest_path, error = %e, "Skipping invalid plugin");
                }
            }
        }
    }

    Ok(plugins)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_parse_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("plugin.toml");
        let mut f = std::fs::File::create(&manifest_path).unwrap();
        write!(
            f,
            r#"
name = "test-plugin"
version = "1.0.0"
api_version = 1
author = "Test"
description = "Test plugin"
plugin_type = "native"
entry_point = "./libtest.dylib"
"#
        )
        .unwrap();

        let manifest = PluginManifest::load(&manifest_path).unwrap();
        assert_eq!(manifest.name, "test-plugin");
        assert_eq!(manifest.api_version, 1);
        assert_eq!(manifest.plugin_type, PluginType::Native);
    }

    #[test]
    fn test_discover_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = discover_plugins(dir.path()).unwrap();
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_discover_nonexistent_dir() {
        let plugins = discover_plugins(Path::new("/nonexistent")).unwrap();
        assert!(plugins.is_empty());
    }
}
