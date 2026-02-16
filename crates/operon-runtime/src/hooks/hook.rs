use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;

use super::events::{HookContext, HookEvent, HookResult};

/// Hook trait for intercepting runtime events
#[async_trait]
pub trait Hook: Send + Sync {
    /// Hook name for logging
    fn name(&self) -> &str;

    /// Events this hook subscribes to
    fn events(&self) -> &[HookEvent];

    /// Handle event, return result (can modify data or abort)
    async fn on_event(&self, ctx: &HookContext) -> Result<HookResult>;

    /// Custom timeout for this hook (default: 5s)
    fn timeout(&self) -> Duration {
        Duration::from_secs(5)
    }

    /// Whether this hook is critical (failure aborts operation)
    fn critical(&self) -> bool {
        false
    }
}
