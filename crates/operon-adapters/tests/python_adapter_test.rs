use operon_adapters::PyAdapter;
use serde_json::json;

#[tokio::test]
#[ignore] // Requires echo_tool.py to be available
async fn test_python_adapter_roundtrip() {
    let adapter = PyAdapter::spawn("tools/python_examples/echo_tool.py")
        .await
        .unwrap();

    let response = adapter
        .call("echo", json!({"message": "test"}))
        .await
        .unwrap();

    assert_eq!(response, "Echo: test");
}

#[tokio::test]
#[ignore] // Requires Python script
async fn test_python_adapter_timeout() {
    let _script = r#"
import time
import sys
while True:
    time.sleep(1)
"#;
    // Would need to create temp file and spawn
    // Placeholder for timeout test
}

// Phase 2: Path validation — nonexistent script fails fast
#[tokio::test]
async fn test_python_adapter_spawn_nonexistent() {
    let result = PyAdapter::spawn("nonexistent_script.py").await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not found"),
        "Expected 'not found' error, got: {}",
        err_msg
    );
}

// Phase 2: Path validation — directory path rejected
#[tokio::test]
async fn test_python_adapter_spawn_directory_rejected() {
    let result = PyAdapter::spawn(".").await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("not a file"));
}
