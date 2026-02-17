pub mod ffi_bridge;
pub mod loader;
pub mod manifest;
pub mod plugin_trait;

pub use ffi_bridge::PluginHandle;
pub use loader::PluginLoader;
pub use manifest::{discover_plugins, PluginManifest, PluginType};
pub use plugin_trait::Plugin;
