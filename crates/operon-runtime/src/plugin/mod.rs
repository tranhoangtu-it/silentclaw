pub mod loader;
pub mod manifest;

pub use loader::PluginLoader;
pub use manifest::{discover_plugins, PluginManifest, PluginType};
