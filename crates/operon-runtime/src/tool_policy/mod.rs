//! Tool policy pipeline: layered authorization/validation before tool execution.

pub mod config;
pub mod layers;

use crate::tool::PermissionLevel;
use serde_json::Value;

/// Result of a single policy layer evaluation
pub enum PolicyDecision {
    /// Allow the tool call to proceed
    Allow,
    /// Deny the tool call with a reason
    Deny(String),
}

/// Context passed to each policy layer for evaluation
pub struct PolicyContext {
    pub tool_name: String,
    pub input: Value,
    pub caller_permission: PermissionLevel,
    pub dry_run: bool,
    pub session_id: Option<String>,
}

/// Individual policy layer trait.
/// Each layer evaluates a tool call and returns Allow or Deny.
pub trait PolicyLayer: Send + Sync {
    /// Layer name for logging and error messages
    fn name(&self) -> &str;

    /// Evaluate whether the tool call should proceed
    fn evaluate(&self, ctx: &PolicyContext) -> PolicyDecision;

    /// Whether this layer is active (disabled layers are skipped)
    fn enabled(&self) -> bool {
        true
    }
}

/// Pipeline that evaluates policy layers in order.
/// Short-circuits on first Deny.
pub struct ToolPolicyPipeline {
    layers: Vec<Box<dyn PolicyLayer>>,
}

impl ToolPolicyPipeline {
    pub fn new() -> Self {
        Self {
            layers: Vec::new(),
        }
    }

    /// Add a policy layer to the pipeline
    pub fn add_layer(mut self, layer: Box<dyn PolicyLayer>) -> Self {
        self.layers.push(layer);
        self
    }

    /// Evaluate all enabled layers in order.
    /// Returns Ok(()) if all layers Allow, Err with reason on first Deny.
    pub fn evaluate(&self, ctx: &PolicyContext) -> anyhow::Result<()> {
        for layer in &self.layers {
            if !layer.enabled() {
                continue;
            }
            match layer.evaluate(ctx) {
                PolicyDecision::Allow => continue,
                PolicyDecision::Deny(reason) => {
                    tracing::warn!(
                        layer = layer.name(),
                        tool = %ctx.tool_name,
                        reason = %reason,
                        "Tool call denied by policy"
                    );
                    anyhow::bail!("Policy denied by {}: {}", layer.name(), reason);
                }
            }
        }
        Ok(())
    }
}

impl Default for ToolPolicyPipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: always-allow layer
    struct AllowLayer;
    impl PolicyLayer for AllowLayer {
        fn name(&self) -> &str {
            "allow"
        }
        fn evaluate(&self, _ctx: &PolicyContext) -> PolicyDecision {
            PolicyDecision::Allow
        }
    }

    /// Helper: always-deny layer
    struct DenyLayer(String);
    impl PolicyLayer for DenyLayer {
        fn name(&self) -> &str {
            "deny"
        }
        fn evaluate(&self, _ctx: &PolicyContext) -> PolicyDecision {
            PolicyDecision::Deny(self.0.clone())
        }
    }

    /// Helper: disabled layer that would deny
    struct DisabledDenyLayer;
    impl PolicyLayer for DisabledDenyLayer {
        fn name(&self) -> &str {
            "disabled_deny"
        }
        fn evaluate(&self, _ctx: &PolicyContext) -> PolicyDecision {
            PolicyDecision::Deny("should not reach".into())
        }
        fn enabled(&self) -> bool {
            false
        }
    }

    fn test_ctx() -> PolicyContext {
        PolicyContext {
            tool_name: "shell".into(),
            input: serde_json::json!({"cmd": "echo hi"}),
            caller_permission: PermissionLevel::Execute,
            dry_run: false,
            session_id: None,
        }
    }

    #[test]
    fn test_pipeline_all_allow() {
        let pipeline = ToolPolicyPipeline::new()
            .add_layer(Box::new(AllowLayer))
            .add_layer(Box::new(AllowLayer));
        assert!(pipeline.evaluate(&test_ctx()).is_ok());
    }

    #[test]
    fn test_pipeline_deny_stops_execution() {
        let pipeline = ToolPolicyPipeline::new()
            .add_layer(Box::new(AllowLayer))
            .add_layer(Box::new(DenyLayer("blocked".into())))
            .add_layer(Box::new(AllowLayer));
        let err = pipeline.evaluate(&test_ctx()).unwrap_err();
        assert!(err.to_string().contains("blocked"));
    }

    #[test]
    fn test_pipeline_disabled_layer_skipped() {
        let pipeline = ToolPolicyPipeline::new()
            .add_layer(Box::new(AllowLayer))
            .add_layer(Box::new(DisabledDenyLayer))
            .add_layer(Box::new(AllowLayer));
        assert!(pipeline.evaluate(&test_ctx()).is_ok());
    }
}
