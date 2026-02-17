//! Core Plugin trait — defined here in operon-runtime so the FFI bridge can reference it
//! without circular dependencies. Re-exported by operon-plugin-sdk for plugin authors.

use anyhow::Result;
use serde_json::Value;

use crate::hooks::Hook;
use crate::tool::Tool;

/// Plugin trait — the main interface for SilentClaw plugins.
///
/// Plugin authors implement this trait and use `declare_plugin!` to export it.
pub trait Plugin: Send + Sync {
    /// Plugin name (must be unique)
    fn name(&self) -> &str;

    /// Plugin version (semver)
    fn version(&self) -> &str;

    /// API version this plugin was built against
    fn api_version(&self) -> u32;

    /// Initialize plugin with config
    fn init(&mut self, config: Value) -> Result<()>;

    /// Shutdown and cleanup resources
    fn shutdown(&mut self) -> Result<()>;

    /// Tools provided by this plugin
    fn tools(&self) -> Vec<Box<dyn Tool>>;

    /// Hooks provided by this plugin
    fn hooks(&self) -> Vec<Box<dyn Hook>>;
}
