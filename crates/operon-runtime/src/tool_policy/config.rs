//! Configuration for the tool policy pipeline layers.

use serde::{Deserialize, Serialize};

/// Configuration for the 7-layer tool policy pipeline.
/// Each layer can be individually enabled/disabled via TOML config.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolPolicyConfig {
    /// Master switch: if false, no policy layers are evaluated
    #[serde(default)]
    pub enabled: bool,

    /// Layer 2: Permission check
    #[serde(default = "default_true")]
    pub permission_enabled: bool,

    /// Default permission level for callers: "read", "write", "execute", "network", "admin"
    #[serde(default = "default_permission")]
    pub default_permission: String,

    /// Layer 3: Rate limiting
    #[serde(default)]
    pub rate_limit_enabled: bool,

    /// Max tool calls per tool per minute
    #[serde(default = "default_max_calls")]
    pub max_calls_per_minute: u32,

    /// Layer 4: Input validation against tool schemas
    #[serde(default = "default_true")]
    pub input_validation_enabled: bool,

    /// Layer 5: Dry-run guard
    #[serde(default = "default_true")]
    pub dry_run_guard_enabled: bool,

    /// Tools that bypass dry-run guard (always execute even in dry-run mode)
    #[serde(default)]
    pub dry_run_bypass_tools: Vec<String>,

    /// Layer 6: Audit logging
    #[serde(default = "default_true")]
    pub audit_enabled: bool,
}

fn default_true() -> bool {
    true
}

fn default_permission() -> String {
    "read".to_string()
}

fn default_max_calls() -> u32 {
    60
}

impl Default for ToolPolicyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            permission_enabled: default_true(),
            default_permission: default_permission(),
            rate_limit_enabled: false,
            max_calls_per_minute: default_max_calls(),
            input_validation_enabled: default_true(),
            dry_run_guard_enabled: default_true(),
            dry_run_bypass_tools: vec![],
            audit_enabled: default_true(),
        }
    }
}
