use anyhow::Result;
use async_trait::async_trait;
use operon_runtime::{ExecutionContext, Fixture, Runtime, Tool};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
    let runtime = Runtime::with_db(&db_path, false, Duration::from_secs(60)).unwrap();
    let tool = Arc::new(MockTool::new("mock"));

    runtime
        .register_tool("mock".to_string(), tool.clone())
        .unwrap();

    let plan = json!({
        "id": "test-001",
        "steps": [
            {"tool": "mock", "input": {"data": "test"}}
        ]
    });

    let result = runtime.run_plan(plan).await;
    assert!(result.is_ok());

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn test_runtime_dry_run_mode() {
    let db_path = get_test_db_path();
    let runtime = Runtime::with_db(&db_path, true, Duration::from_secs(60)).unwrap();
    let tool = Arc::new(MockTool::new("mock"));

    runtime.register_tool("mock".to_string(), tool).unwrap();

    let plan = json!({
        "id": "test-002",
        "steps": [
            {"tool": "mock", "input": {"cmd": "dangerous"}}
        ]
    });

    let result = runtime.run_plan(plan).await;
    assert!(result.is_ok());

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn test_runtime_missing_tool() {
    let db_path = get_test_db_path();
    let runtime = Runtime::with_db(&db_path, false, Duration::from_secs(60)).unwrap();

    let plan = json!({
        "id": "test-003",
        "steps": [
            {"tool": "nonexistent", "input": {}}
        ]
    });

    let result = runtime.run_plan(plan).await;
    assert!(result.is_err());

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn test_runtime_per_tool_timeout() {
    let db_path = get_test_db_path();
    let runtime = Runtime::with_db(&db_path, false, Duration::from_secs(60)).unwrap();

    runtime.configure_timeout("slow_tool".to_string(), Duration::from_secs(120));

    let timeout = runtime.get_timeout("slow_tool");
    assert_eq!(timeout.as_secs(), 120);

    let default_timeout = runtime.get_timeout("other_tool");
    assert_eq!(default_timeout.as_secs(), 60);

    let _ = std::fs::remove_file(&db_path);
}

// Phase 5: State machine — register_tool returns Result
#[tokio::test]
async fn test_runtime_register_tool_returns_result() {
    let db_path = get_test_db_path();
    let runtime = Runtime::with_db(&db_path, false, Duration::from_secs(60)).unwrap();
    let tool = Arc::new(MockTool::new("test"));

    let result = runtime.register_tool("test".to_string(), tool);
    assert!(result.is_ok());

    let _ = std::fs::remove_file(&db_path);
}

// Phase 6: Record and replay
#[tokio::test]
async fn test_runtime_record_and_replay() {
    let db_path = get_test_db_path();
    let fixture_dir = std::env::temp_dir().join(format!(
        "silentclaw-fixture-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));

    // 1. Record execution
    let runtime = Runtime::with_db(&db_path, false, Duration::from_secs(60))
        .unwrap()
        .with_execution_context(ExecutionContext::Record(fixture_dir.clone()));

    let tool = Arc::new(MockTool::new("mock"));
    runtime.register_tool("mock".to_string(), tool).unwrap();

    let plan = json!({
        "id": "test-record",
        "steps": [
            {"tool": "mock", "input": {"data": "hello"}}
        ]
    });

    runtime.run_plan(plan.clone()).await.unwrap();

    // Verify fixture file exists
    assert!(fixture_dir.join("fixture.json").exists());

    // 2. Replay execution (no tools needed)
    let db_path2 = get_test_db_path();
    let runtime2 = Runtime::with_db(&db_path2, false, Duration::from_secs(60))
        .unwrap()
        .with_execution_context(ExecutionContext::Replay(fixture_dir.clone()));

    let result = runtime2.run_plan(plan).await;
    assert!(result.is_ok());

    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(&db_path2);
    let _ = std::fs::remove_dir_all(&fixture_dir);
}

// Phase 6: Replay step count mismatch
#[tokio::test]
async fn test_runtime_replay_step_count_mismatch() {
    let fixture_dir = std::env::temp_dir().join(format!(
        "silentclaw-fixture-mismatch-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));

    // Write fixture with 0 steps
    let fixture = Fixture::new("test".to_string());
    fixture.save(&fixture_dir).unwrap();

    let db_path = get_test_db_path();
    let runtime = Runtime::with_db(&db_path, false, Duration::from_secs(60))
        .unwrap()
        .with_execution_context(ExecutionContext::Replay(fixture_dir.clone()));

    // Plan has 1 step, fixture has 0 — replay should still work via find()
    // (sequential replay uses find() which returns None, then falls through to tool lookup)
    // This actually requires the tool to be registered for non-replay steps
    let plan = json!({
        "id": "test",
        "steps": [{"tool": "mock", "input": {}}]
    });

    let result = runtime.run_plan(plan).await;
    // Missing tool since fixture has no matching step and tool isn't registered
    assert!(result.is_err());

    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_dir_all(&fixture_dir);
}

// Phase 7: Parallel independent steps
#[tokio::test]
async fn test_parallel_independent_steps() {
    let db_path = get_test_db_path();
    let runtime = Runtime::with_db(&db_path, false, Duration::from_secs(60))
        .unwrap()
        .with_max_parallel(4);

    let tool = Arc::new(MockTool::new("mock"));
    runtime.register_tool("mock".to_string(), tool).unwrap();

    let plan = json!({
        "id": "test-parallel",
        "steps": [
            {"id": "a", "tool": "mock", "input": {"n": 1}, "depends_on": []},
            {"id": "b", "tool": "mock", "input": {"n": 2}, "depends_on": []},
            {"id": "c", "tool": "mock", "input": {"n": 3}, "depends_on": ["a", "b"]}
        ]
    });

    let result = runtime.run_plan(plan).await;
    assert!(result.is_ok());

    let _ = std::fs::remove_file(&db_path);
}

// Phase 7: Cycle detection
#[tokio::test]
async fn test_cycle_detection() {
    let db_path = get_test_db_path();
    let runtime = Runtime::with_db(&db_path, false, Duration::from_secs(60)).unwrap();

    let tool = Arc::new(MockTool::new("mock"));
    runtime.register_tool("mock".to_string(), tool).unwrap();

    let plan = json!({
        "id": "test-cycle",
        "steps": [
            {"id": "a", "tool": "mock", "input": {}, "depends_on": ["b"]},
            {"id": "b", "tool": "mock", "input": {}, "depends_on": ["a"]}
        ]
    });

    let result = runtime.run_plan(plan).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Cycle"));

    let _ = std::fs::remove_file(&db_path);
}

// Phase 7: Missing dependency
#[tokio::test]
async fn test_missing_dependency() {
    let db_path = get_test_db_path();
    let runtime = Runtime::with_db(&db_path, false, Duration::from_secs(60)).unwrap();

    let plan = json!({
        "id": "test-missing-dep",
        "steps": [
            {"id": "a", "tool": "mock", "input": {}, "depends_on": ["nonexistent"]}
        ]
    });

    let result = runtime.run_plan(plan).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("does not exist"));

    let _ = std::fs::remove_file(&db_path);
}

// Phase 7: Sequential backward compat (no depends_on)
#[tokio::test]
async fn test_sequential_backward_compat() {
    let db_path = get_test_db_path();
    let runtime = Runtime::with_db(&db_path, false, Duration::from_secs(60)).unwrap();

    let tool = Arc::new(MockTool::new("mock"));
    runtime.register_tool("mock".to_string(), tool).unwrap();

    let plan = json!({
        "id": "test-compat",
        "steps": [
            {"tool": "mock", "input": {"data": "1"}},
            {"tool": "mock", "input": {"data": "2"}}
        ]
    });

    let result = runtime.run_plan(plan).await;
    assert!(result.is_ok());

    let _ = std::fs::remove_file(&db_path);
}
