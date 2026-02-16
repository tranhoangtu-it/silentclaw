//! SilentClaw Plugin SDK
//!
//! Re-exports runtime traits needed for plugin development.
//! Plugin authors implement the `Plugin` trait and export a creation function.

pub use anyhow::Result;
pub use async_trait::async_trait;
pub use serde_json::Value;

// Re-export core traits from runtime
pub use operon_runtime::hooks::{Hook, HookContext, HookEvent, HookResult};
pub use operon_runtime::tool::Tool;

/// Current plugin API version. Plugins must match this to load.
pub const API_VERSION: u32 = 1;

/// Plugin trait - the main interface for SilentClaw plugins
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

/// Macro for plugin entry point. Use in plugin crate:
/// ```ignore
/// use operon_plugin_sdk::*;
///
/// struct MyPlugin;
/// impl Plugin for MyPlugin { ... }
///
/// declare_plugin!(MyPlugin);
/// ```
#[macro_export]
macro_rules! declare_plugin {
    ($plugin_type:ty) => {
        #[no_mangle]
        pub extern "C" fn _plugin_create() -> *mut dyn $crate::Plugin {
            Box::into_raw(Box::new(<$plugin_type>::default()))
        }
    };
}
