use anyhow::Result;
use async_trait::async_trait;
use operon_runtime::{Runtime, Tool};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

fn get_test_db_path() -> String {
    let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("./silentclaw-test-{}.db", id)
}

// Mock tool for testing
struct MockTool {
    name: String,
}

impl MockTool {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Tool for MockTool {
    async fn execute(&self, input: Value) -> Result<Value> {
        Ok(json!({
            "tool": self.name,
            "input": input
        }))
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[tokio::test]
async fn test_runtime_register_and_execute() {
    let db_path = get_test_db_path();
    let runtime = Runtime::with_db(&db_path, false, std::time::Duration::from_secs(60)).unwrap();
    let tool = Arc::new(MockTool::new("mock"));

    runtime.register_tool("mock".to_string(), tool.clone());

    let plan = json!({
        "id": "test-001",
        "steps": [
            {"tool": "mock", "input": {"data": "test"}}
        ]
    });

    let result = runtime.run_plan(plan).await;
    assert!(result.is_ok());

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn test_runtime_dry_run_mode() {
    let db_path = get_test_db_path();
    let runtime = Runtime::with_db(&db_path, true, std::time::Duration::from_secs(60)).unwrap();
    let tool = Arc::new(MockTool::new("mock"));

    runtime.register_tool("mock".to_string(), tool);

    let plan = json!({
        "id": "test-002",
        "steps": [
            {"tool": "mock", "input": {"cmd": "dangerous"}}
        ]
    });

    // In dry-run mode, plan should succeed without executing
    let result = runtime.run_plan(plan).await;
    assert!(result.is_ok());

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn test_runtime_missing_tool() {
    let db_path = get_test_db_path();
    let runtime = Runtime::with_db(&db_path, false, std::time::Duration::from_secs(60)).unwrap();

    let plan = json!({
        "id": "test-003",
        "steps": [
            {"tool": "nonexistent", "input": {}}
        ]
    });

    let result = runtime.run_plan(plan).await;
    assert!(result.is_err());

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn test_runtime_per_tool_timeout() {
    let db_path = get_test_db_path();
    let runtime = Runtime::with_db(&db_path, false, std::time::Duration::from_secs(60)).unwrap();

    // Configure custom timeout for specific tool
    runtime.configure_timeout("slow_tool".to_string(), std::time::Duration::from_secs(120));

    let timeout = runtime.get_timeout("slow_tool");
    assert_eq!(timeout.as_secs(), 120);

    let default_timeout = runtime.get_timeout("other_tool");
    assert_eq!(default_timeout.as_secs(), 60);

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}
