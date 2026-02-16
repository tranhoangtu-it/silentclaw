use std::sync::Arc;

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde_json::Value;
use tracing::warn;

use super::events::{HookContext, HookEvent};
use super::hook::Hook;

/// Registry for hooks, organized by event type
pub struct HookRegistry {
    hooks: DashMap<HookEvent, Vec<Arc<dyn Hook>>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self {
            hooks: DashMap::new(),
        }
    }

    /// Register a hook for its declared events
    pub fn register(&self, hook: Arc<dyn Hook>) {
        for event in hook.events() {
            self.hooks
                .entry(event.clone())
                .or_default()
                .push(hook.clone());
        }
    }

    /// Trigger all hooks for an event, return (possibly modified) data
    /// Hooks execute sequentially; non-critical errors are isolated (logged, not propagated)
    pub async fn trigger(&self, ctx: HookContext) -> Result<Value> {
        let hooks = self
            .hooks
            .get(&ctx.event)
            .map(|h| h.clone())
            .unwrap_or_default();

        if hooks.is_empty() {
            return Ok(ctx.data.clone());
        }

        let mut data = ctx.data.clone();

        for hook in &hooks {
            let hook_ctx = HookContext {
                data: data.clone(),
                ..ctx.clone()
            };

            let timeout = hook.timeout();

            match tokio::time::timeout(timeout, hook.on_event(&hook_ctx)).await {
                Ok(Ok(result)) if result.abort => {
                    return Err(anyhow!("Hook '{}' aborted operation", hook.name()));
                }
                Ok(Ok(result)) => {
                    if let Some(modified) = result.modified_data {
                        data = modified;
                    }
                }
                Ok(Err(e)) => {
                    warn!(hook = hook.name(), error = %e, "Hook failed");
                    if hook.critical() {
                        return Err(e.context(format!("Critical hook '{}' failed", hook.name())));
                    }
                }
                Err(_) => {
                    warn!(hook = hook.name(), timeout_ms = timeout.as_millis(), "Hook timed out");
                    if hook.critical() {
                        return Err(anyhow!("Critical hook '{}' timed out", hook.name()));
                    }
                }
            }
        }

        Ok(data)
    }

    /// Check if any hooks are registered for an event
    pub fn has_hooks(&self, event: &HookEvent) -> bool {
        self.hooks.get(event).map(|h| !h.is_empty()).unwrap_or(false)
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::events::HookResult;
    use async_trait::async_trait;
    use serde_json::json;

    struct LoggingHook;

    #[async_trait]
    impl Hook for LoggingHook {
        fn name(&self) -> &str {
            "logging"
        }
        fn events(&self) -> &[HookEvent] {
            &[HookEvent::ToolCallBefore, HookEvent::ToolCallAfter]
        }
        async fn on_event(&self, _ctx: &HookContext) -> Result<HookResult> {
            Ok(HookResult::default())
        }
    }

    struct ModifyHook;

    #[async_trait]
    impl Hook for ModifyHook {
        fn name(&self) -> &str {
            "modify"
        }
        fn events(&self) -> &[HookEvent] {
            &[HookEvent::ToolCallBefore]
        }
        async fn on_event(&self, _ctx: &HookContext) -> Result<HookResult> {
            Ok(HookResult {
                modified_data: Some(json!({"modified": true})),
                abort: false,
            })
        }
    }

    struct AbortHook;

    #[async_trait]
    impl Hook for AbortHook {
        fn name(&self) -> &str {
            "abort"
        }
        fn events(&self) -> &[HookEvent] {
            &[HookEvent::ToolCallBefore]
        }
        async fn on_event(&self, _ctx: &HookContext) -> Result<HookResult> {
            Ok(HookResult {
                modified_data: None,
                abort: true,
            })
        }
    }

    struct FailHook;

    #[async_trait]
    impl Hook for FailHook {
        fn name(&self) -> &str {
            "fail"
        }
        fn events(&self) -> &[HookEvent] {
            &[HookEvent::ToolCallBefore]
        }
        async fn on_event(&self, _ctx: &HookContext) -> Result<HookResult> {
            Err(anyhow!("Hook error"))
        }
    }

    fn make_ctx(event: HookEvent) -> HookContext {
        HookContext {
            event,
            data: json!({"tool": "shell"}),
            agent_id: None,
            session_id: None,
        }
    }

    #[tokio::test]
    async fn test_hook_registration_and_trigger() {
        let registry = HookRegistry::new();
        registry.register(Arc::new(LoggingHook));

        assert!(registry.has_hooks(&HookEvent::ToolCallBefore));
        assert!(!registry.has_hooks(&HookEvent::SessionStart));

        let result = registry
            .trigger(make_ctx(HookEvent::ToolCallBefore))
            .await
            .unwrap();
        assert_eq!(result["tool"], "shell");
    }

    #[tokio::test]
    async fn test_hook_modifies_data() {
        let registry = HookRegistry::new();
        registry.register(Arc::new(ModifyHook));

        let result = registry
            .trigger(make_ctx(HookEvent::ToolCallBefore))
            .await
            .unwrap();
        assert_eq!(result["modified"], true);
    }

    #[tokio::test]
    async fn test_hook_abort() {
        let registry = HookRegistry::new();
        registry.register(Arc::new(AbortHook));

        let result = registry
            .trigger(make_ctx(HookEvent::ToolCallBefore))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("aborted"));
    }

    #[tokio::test]
    async fn test_hook_error_isolation() {
        let registry = HookRegistry::new();
        // FailHook errors but shouldn't break the chain
        registry.register(Arc::new(FailHook));
        registry.register(Arc::new(ModifyHook));

        let result = registry
            .trigger(make_ctx(HookEvent::ToolCallBefore))
            .await
            .unwrap();
        // ModifyHook still ran after FailHook
        assert_eq!(result["modified"], true);
    }

    #[tokio::test]
    async fn test_no_hooks_returns_original_data() {
        let registry = HookRegistry::new();
        let result = registry
            .trigger(make_ctx(HookEvent::SessionStart))
            .await
            .unwrap();
        assert_eq!(result["tool"], "shell");
    }
}
