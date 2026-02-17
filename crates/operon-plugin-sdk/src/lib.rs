//! SilentClaw Plugin SDK
//!
//! Re-exports runtime traits needed for plugin development.
//! Plugin authors implement the `Plugin` trait and export a creation function.

pub use anyhow::Result;
pub use async_trait::async_trait;
pub use serde_json::Value;

// Re-export core traits from runtime
pub use operon_runtime::hooks::{Hook, HookContext, HookEvent, HookResult};
pub use operon_runtime::plugin::Plugin;
pub use operon_runtime::tool::Tool;

/// Current plugin API version. Plugins must match this to load.
pub const API_VERSION: u32 = 1;

/// Macro for plugin entry point. Use in plugin crate:
/// ```ignore
/// use operon_plugin_sdk::*;
///
/// struct MyPlugin;
/// impl Plugin for MyPlugin { ... }
///
/// declare_plugin!(MyPlugin);
/// ```
///
/// ## ABI Contract
///
/// Generates two `extern "C"` symbols using double-boxing for FFI-safe thin pointers:
/// - `_plugin_create() -> *mut c_void` — allocates `Box<Box<dyn Plugin>>`, returns thin pointer
/// - `_plugin_destroy(ptr: *mut c_void)` — reconstructs and drops the double-boxed plugin
///
/// **Constraint:** Plugin and host must be compiled with the same Rust compiler version
/// (same vtable layout). This is guaranteed within a Cargo workspace build.
#[macro_export]
macro_rules! declare_plugin {
    ($plugin_type:ty) => {
        #[no_mangle]
        pub extern "C" fn _plugin_create() -> *mut std::ffi::c_void {
            let plugin: Box<dyn $crate::Plugin> = Box::new(<$plugin_type>::default());
            Box::into_raw(Box::new(plugin)) as *mut std::ffi::c_void
        }

        /// # Safety
        ///
        /// `ptr` must be a pointer previously returned by `_plugin_create` from this
        /// exact plugin build. Calling with any other pointer is undefined behavior.
        ///
        /// ## When to use
        /// - **Same-workspace build:** Host drops plugin via Rust `Drop` (no need to call this)
        /// - **Separately-compiled plugin:** Host MUST call `_plugin_destroy` to ensure
        ///   deallocation uses the plugin's allocator, not the host's
        ///
        /// Currently SilentClaw only supports same-workspace plugins, so `_plugin_destroy`
        /// is generated but not called. It exists for future cross-build support.
        #[no_mangle]
        pub unsafe extern "C" fn _plugin_destroy(ptr: *mut std::ffi::c_void) {
            if !ptr.is_null() {
                drop(Box::from_raw(ptr as *mut Box<dyn $crate::Plugin>));
            }
        }
    };
}
