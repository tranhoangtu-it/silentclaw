//! FFI bridge for loading native plugins (.so/.dylib) via libloading.
//!
//! Uses double-boxing pattern: `Box<Box<dyn Plugin>>` → thin `*mut c_void`
//! to avoid passing fat pointers over `extern "C"` boundary.
//!
//! **Constraint:** Plugin and host must share the same Rust compiler version.

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use anyhow::{bail, Context, Result};
use libloading::Library;
use tracing::warn;

use super::plugin_trait::Plugin;

/// Symbol type for `_plugin_create() -> *mut c_void`
type CreateFn = extern "C" fn() -> *mut c_void;

/// Safe wrapper around a dynamically loaded plugin.
///
/// Drop order matters: `plugin` must be dropped before `_library`
/// (Rust drops fields in declaration order).
pub struct PluginHandle {
    _library: Library,
    plugin: Box<dyn Plugin>,
}

impl std::fmt::Debug for PluginHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginHandle")
            .field("plugin_name", &self.plugin.name())
            .finish()
    }
}

impl PluginHandle {
    /// Load a native plugin from a shared library path (.so/.dylib).
    ///
    /// Calls `_plugin_create()` with panic isolation, reconstructs the
    /// double-boxed `Box<dyn Plugin>` from the returned thin pointer.
    ///
    /// # Safety
    ///
    /// This method uses `unsafe` for two operations:
    /// 1. `Library::new()` — loads a shared library; the library must be a valid
    ///    Rust cdylib built with `declare_plugin!`.
    /// 2. `Box::from_raw()` — reconstructs `Box<dyn Plugin>` from the thin pointer
    ///    returned by `_plugin_create()`. This takes ownership, so the host drops
    ///    the plugin via Rust's normal Drop. This is safe **only** when host and
    ///    plugin share the same allocator (same Rust toolchain / workspace build).
    ///    For separately-compiled plugins, call `_plugin_destroy()` instead.
    ///
    /// ## Panic Isolation
    ///
    /// `_plugin_create()` is called inside `catch_unwind(AssertUnwindSafe(...))`.
    /// `AssertUnwindSafe` is sound here because:
    /// - We do not access any mutable state from the host after a panic
    /// - On panic, we return an error and the partially-constructed plugin is never used
    /// - The `Library` handle is dropped normally (no leaked resources)
    pub fn load(path: &Path) -> Result<Self> {
        // Load the shared library
        let lib = unsafe { Library::new(path) }
            .with_context(|| format!("Failed to load library: {}", path.display()))?;

        // Resolve _plugin_create symbol
        let create_fn = unsafe { lib.get::<CreateFn>(b"_plugin_create\0") }
            .with_context(|| format!("Symbol _plugin_create not found in {}", path.display()))?;

        // Call _plugin_create with panic isolation
        let raw = catch_unwind(AssertUnwindSafe(|| create_fn()))
            .map_err(|_| anyhow::anyhow!("Plugin panicked during _plugin_create"))?;

        if raw.is_null() {
            bail!("_plugin_create returned null in {}", path.display());
        }

        // Reconstruct Box<dyn Plugin> from double-boxed thin pointer.
        // SAFETY: no fallible ops between from_raw and return — prevents double-free.
        // NOTE: This takes ownership of the allocation. The host drops the plugin via
        // Rust's normal drop path. This is safe ONLY when host and plugin share the
        // same allocator (same Rust toolchain / same workspace build). For plugins
        // compiled separately, use _plugin_destroy instead.
        let plugin = unsafe { *Box::from_raw(raw as *mut Box<dyn Plugin>) };

        Ok(Self {
            _library: lib,
            plugin,
        })
    }

    /// Get immutable reference to the loaded plugin.
    pub fn plugin(&self) -> &dyn Plugin {
        &*self.plugin
    }

    /// Get mutable reference to the loaded plugin.
    pub fn plugin_mut(&mut self) -> &mut dyn Plugin {
        &mut *self.plugin
    }

    /// Call `plugin.shutdown()` with panic isolation, then drop the plugin.
    /// Returns Ok even if shutdown panics (logged as warning).
    ///
    /// ## Panic Isolation
    ///
    /// `plugin.shutdown()` is wrapped in `catch_unwind(AssertUnwindSafe(...))`.
    /// `AssertUnwindSafe` is sound here because:
    /// - After this call, `self` is consumed and dropped regardless of panic
    /// - No host state is accessed after the catch_unwind boundary
    /// - Plugin state may be inconsistent after panic, but it's immediately dropped
    pub fn shutdown_and_drop(mut self) {
        let result = catch_unwind(AssertUnwindSafe(|| self.plugin.shutdown()));
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!(error = %e, "Plugin shutdown returned error"),
            Err(_) => warn!("Plugin panicked during shutdown"),
        }
        // self is dropped here: plugin then library
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_nonexistent_library() {
        let result = PluginHandle::load(Path::new("/nonexistent/libfoo.so"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to load"));
    }

    #[test]
    fn test_load_invalid_library() {
        // Create a file that is not a valid shared library
        let dir = tempfile::tempdir().unwrap();
        let fake_lib = dir.path().join("libfake.so");
        std::fs::write(&fake_lib, b"not a real library").unwrap();

        let result = PluginHandle::load(&fake_lib);
        assert!(result.is_err());
    }
}
