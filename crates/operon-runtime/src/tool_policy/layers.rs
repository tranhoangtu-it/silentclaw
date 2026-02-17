//! 7-layer policy implementations for tool execution authorization.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Instant;

use crate::tool::PermissionLevel;

use super::{PolicyContext, PolicyDecision, PolicyLayer};

// ============================================================================
// Layer 1: Tool Existence Check
// ============================================================================

/// Verifies that the requested tool is registered in the runtime.
pub struct ToolExistenceLayer {
    registered_tools: HashSet<String>,
    is_enabled: bool,
}

impl ToolExistenceLayer {
    pub fn new(tool_names: Vec<String>) -> Self {
        Self {
            registered_tools: tool_names.into_iter().collect(),
            is_enabled: true,
        }
    }
}

impl PolicyLayer for ToolExistenceLayer {
    fn name(&self) -> &str {
        "tool_existence"
    }

    fn evaluate(&self, ctx: &PolicyContext) -> PolicyDecision {
        if self.registered_tools.contains(&ctx.tool_name) {
            PolicyDecision::Allow
        } else {
            PolicyDecision::Deny(format!("tool not found: {}", ctx.tool_name))
        }
    }

    fn enabled(&self) -> bool {
        self.is_enabled
    }
}

// ============================================================================
// Layer 2: Permission Check
// ============================================================================

/// Compares caller's permission level against tool's required level.
/// Hierarchy: Read < Write < Execute < Network < Admin
pub struct PermissionCheckLayer {
    tool_permissions: HashMap<String, PermissionLevel>,
    /// Default permission for tools not in tool_permissions map (least-privilege: Read)
    default_permission: PermissionLevel,
    is_enabled: bool,
}

impl PermissionCheckLayer {
    pub fn new(
        tool_permissions: HashMap<String, PermissionLevel>,
        default_permission: PermissionLevel,
    ) -> Self {
        Self {
            tool_permissions,
            default_permission,
            is_enabled: true,
        }
    }
}

fn permission_rank(level: &PermissionLevel) -> u8 {
    match level {
        PermissionLevel::Read => 0,
        PermissionLevel::Write => 1,
        PermissionLevel::Execute => 2,
        PermissionLevel::Network => 3,
        PermissionLevel::Admin => 4,
    }
}

impl PolicyLayer for PermissionCheckLayer {
    fn name(&self) -> &str {
        "permission_check"
    }

    fn evaluate(&self, ctx: &PolicyContext) -> PolicyDecision {
        let required = self
            .tool_permissions
            .get(&ctx.tool_name)
            .unwrap_or(&self.default_permission);

        if permission_rank(&ctx.caller_permission) >= permission_rank(required) {
            PolicyDecision::Allow
        } else {
            PolicyDecision::Deny(format!(
                "insufficient permission for tool '{}': caller={:?}, required={:?}",
                ctx.tool_name, ctx.caller_permission, required
            ))
        }
    }

    fn enabled(&self) -> bool {
        self.is_enabled
    }
}

// ============================================================================
// Layer 3: Rate Limit
// ============================================================================

/// Per-tool call rate limiting using a simple sliding window.
pub struct RateLimitLayer {
    /// (window_start, call_count) per tool
    buckets: Mutex<HashMap<String, (Instant, u32)>>,
    max_calls_per_minute: u32,
    is_enabled: bool,
}

impl RateLimitLayer {
    pub fn new(max_calls_per_minute: u32) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            max_calls_per_minute,
            is_enabled: true,
        }
    }
}

impl PolicyLayer for RateLimitLayer {
    fn name(&self) -> &str {
        "rate_limit"
    }

    fn evaluate(&self, ctx: &PolicyContext) -> PolicyDecision {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let window = std::time::Duration::from_secs(60);

        let entry = buckets
            .entry(ctx.tool_name.clone())
            .or_insert((now, 0));

        // Reset window if expired
        if now.duration_since(entry.0) >= window {
            *entry = (now, 0);
        }

        if entry.1 >= self.max_calls_per_minute {
            PolicyDecision::Deny(format!(
                "rate limit exceeded for tool '{}': {}/{} calls/min",
                ctx.tool_name, entry.1, self.max_calls_per_minute
            ))
        } else {
            entry.1 += 1;
            PolicyDecision::Allow
        }
    }

    fn enabled(&self) -> bool {
        self.is_enabled
    }
}

// ============================================================================
// Layer 4: Input Validation
// ============================================================================

/// Basic input validation: checks required fields are present.
/// Uses tool schemas to verify input shape (not full JSON Schema).
pub struct InputValidationLayer {
    /// tool_name -> schema with required fields
    tool_schemas: HashMap<String, serde_json::Value>,
    is_enabled: bool,
}

impl InputValidationLayer {
    pub fn new(tool_schemas: HashMap<String, serde_json::Value>) -> Self {
        Self {
            tool_schemas,
            is_enabled: true,
        }
    }
}

impl PolicyLayer for InputValidationLayer {
    fn name(&self) -> &str {
        "input_validation"
    }

    fn evaluate(&self, ctx: &PolicyContext) -> PolicyDecision {
        let Some(schema) = self.tool_schemas.get(&ctx.tool_name) else {
            return PolicyDecision::Allow; // No schema → skip validation
        };

        // Check required fields
        if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
            for field in required {
                if let Some(field_name) = field.as_str() {
                    if ctx.input.get(field_name).is_none() {
                        return PolicyDecision::Deny(format!(
                            "missing required field '{}' for tool '{}'",
                            field_name, ctx.tool_name
                        ));
                    }
                }
            }
        }

        PolicyDecision::Allow
    }

    fn enabled(&self) -> bool {
        self.is_enabled
    }
}

// ============================================================================
// Layer 5: Dry-Run Guard
// ============================================================================

/// Blocks non-read tools when dry_run mode is active.
/// Configurable bypass list for safe tools (e.g., read_file, memory_search).
pub struct DryRunGuardLayer {
    bypass_tools: HashSet<String>,
    is_enabled: bool,
}

impl DryRunGuardLayer {
    pub fn new(bypass_tools: Vec<String>) -> Self {
        Self {
            bypass_tools: bypass_tools.into_iter().collect(),
            is_enabled: true,
        }
    }
}

impl PolicyLayer for DryRunGuardLayer {
    fn name(&self) -> &str {
        "dry_run_guard"
    }

    fn evaluate(&self, ctx: &PolicyContext) -> PolicyDecision {
        if !ctx.dry_run {
            return PolicyDecision::Allow;
        }

        // Allow read-level tools and bypass tools
        if ctx.caller_permission == PermissionLevel::Read
            || self.bypass_tools.contains(&ctx.tool_name)
        {
            PolicyDecision::Allow
        } else {
            PolicyDecision::Deny(format!(
                "tool '{}' blocked in dry-run mode",
                ctx.tool_name
            ))
        }
    }

    fn enabled(&self) -> bool {
        self.is_enabled
    }
}

// ============================================================================
// Layer 6: Audit Log
// ============================================================================

/// Logs every tool call attempt. Always returns Allow (side-effect only).
pub struct AuditLogLayer {
    is_enabled: bool,
}

impl Default for AuditLogLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditLogLayer {
    pub fn new() -> Self {
        Self { is_enabled: true }
    }
}

impl PolicyLayer for AuditLogLayer {
    fn name(&self) -> &str {
        "audit_log"
    }

    fn evaluate(&self, ctx: &PolicyContext) -> PolicyDecision {
        tracing::info!(
            tool = %ctx.tool_name,
            permission = ?ctx.caller_permission,
            dry_run = ctx.dry_run,
            session_id = ?ctx.session_id,
            "Tool call audit"
        );
        PolicyDecision::Allow
    }

    fn enabled(&self) -> bool {
        self.is_enabled
    }
}

// ============================================================================
// Layer 7: Timeout Enforcement (metadata only)
// ============================================================================

/// Sets per-tool timeout metadata. Always returns Allow.
/// The actual timeout enforcement happens in Runtime::execute_tool().
pub struct TimeoutEnforceLayer {
    is_enabled: bool,
}

impl Default for TimeoutEnforceLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeoutEnforceLayer {
    pub fn new() -> Self {
        Self { is_enabled: true }
    }
}

impl PolicyLayer for TimeoutEnforceLayer {
    fn name(&self) -> &str {
        "timeout_enforce"
    }

    fn evaluate(&self, _ctx: &PolicyContext) -> PolicyDecision {
        // Timeout is enforced by Runtime, this layer is a pass-through marker
        PolicyDecision::Allow
    }

    fn enabled(&self) -> bool {
        self.is_enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx_with(tool: &str, perm: PermissionLevel, dry_run: bool) -> PolicyContext {
        PolicyContext {
            tool_name: tool.into(),
            input: json!({"cmd": "echo hi"}),
            caller_permission: perm,
            dry_run,
            session_id: None,
        }
    }

    // --- Permission Check ---

    #[test]
    fn test_permission_check_allow() {
        let mut perms = HashMap::new();
        perms.insert("shell".into(), PermissionLevel::Execute);
        let layer = PermissionCheckLayer::new(perms, PermissionLevel::Read);
        let ctx = ctx_with("shell", PermissionLevel::Execute, false);
        assert!(matches!(layer.evaluate(&ctx), PolicyDecision::Allow));
    }

    #[test]
    fn test_permission_check_deny() {
        let mut perms = HashMap::new();
        perms.insert("shell".into(), PermissionLevel::Admin);
        let layer = PermissionCheckLayer::new(perms, PermissionLevel::Read);
        let ctx = ctx_with("shell", PermissionLevel::Read, false);
        assert!(matches!(layer.evaluate(&ctx), PolicyDecision::Deny(_)));
    }

    #[test]
    fn test_permission_check_unknown_tool_defaults_to_read() {
        let layer = PermissionCheckLayer::new(HashMap::new(), PermissionLevel::Read);
        // Unknown tool with Read caller → allowed (Read >= Read)
        let ctx = ctx_with("unknown_tool", PermissionLevel::Read, false);
        assert!(matches!(layer.evaluate(&ctx), PolicyDecision::Allow));
        // Unknown tool with Execute caller → also allowed (Execute >= Read)
        let ctx2 = ctx_with("unknown_tool", PermissionLevel::Execute, false);
        assert!(matches!(layer.evaluate(&ctx2), PolicyDecision::Allow));
    }

    // --- Rate Limit ---

    #[test]
    fn test_rate_limit_within_limit() {
        let layer = RateLimitLayer::new(5);
        let ctx = ctx_with("shell", PermissionLevel::Execute, false);
        for _ in 0..5 {
            assert!(matches!(layer.evaluate(&ctx), PolicyDecision::Allow));
        }
    }

    #[test]
    fn test_rate_limit_exceeded() {
        let layer = RateLimitLayer::new(2);
        let ctx = ctx_with("shell", PermissionLevel::Execute, false);
        assert!(matches!(layer.evaluate(&ctx), PolicyDecision::Allow));
        assert!(matches!(layer.evaluate(&ctx), PolicyDecision::Allow));
        assert!(matches!(layer.evaluate(&ctx), PolicyDecision::Deny(_)));
    }

    // --- Input Validation ---

    #[test]
    fn test_input_validation_valid() {
        let mut schemas = HashMap::new();
        schemas.insert(
            "shell".into(),
            json!({"required": ["cmd"], "properties": {"cmd": {"type": "string"}}}),
        );
        let layer = InputValidationLayer::new(schemas);
        let ctx = ctx_with("shell", PermissionLevel::Execute, false);
        assert!(matches!(layer.evaluate(&ctx), PolicyDecision::Allow));
    }

    #[test]
    fn test_input_validation_missing_field() {
        let mut schemas = HashMap::new();
        schemas.insert(
            "shell".into(),
            json!({"required": ["missing_field"]}),
        );
        let layer = InputValidationLayer::new(schemas);
        let ctx = ctx_with("shell", PermissionLevel::Execute, false);
        assert!(matches!(layer.evaluate(&ctx), PolicyDecision::Deny(_)));
    }

    // --- Dry-Run Guard ---

    #[test]
    fn test_dry_run_guard_blocks_write() {
        let layer = DryRunGuardLayer::new(vec![]);
        let ctx = ctx_with("shell", PermissionLevel::Execute, true);
        assert!(matches!(layer.evaluate(&ctx), PolicyDecision::Deny(_)));
    }

    #[test]
    fn test_dry_run_guard_allows_read() {
        let layer = DryRunGuardLayer::new(vec![]);
        let ctx = ctx_with("read_file", PermissionLevel::Read, true);
        assert!(matches!(layer.evaluate(&ctx), PolicyDecision::Allow));
    }

    #[test]
    fn test_dry_run_guard_bypass() {
        let layer = DryRunGuardLayer::new(vec!["memory_search".into()]);
        let ctx = ctx_with("memory_search", PermissionLevel::Execute, true);
        assert!(matches!(layer.evaluate(&ctx), PolicyDecision::Allow));
    }
}
