use operon_adapters::ShellTool;
use operon_runtime::Tool;
use serde_json::json;

#[tokio::test]
async fn test_shell_tool_execute_echo() {
    let tool = ShellTool::new(false); // allow-tools mode
    let input = json!({"cmd": "echo hello"});
    let result = tool.execute(input).await.unwrap();

    assert_eq!(result["exit_code"], 0);
    assert!(result["stdout"].as_str().unwrap().contains("hello"));
}

#[tokio::test]
async fn test_shell_tool_timeout() {
    let tool = ShellTool::new(false).with_timeout(std::time::Duration::from_secs(2));
    let input = json!({"cmd": "sleep 10"});

    let result = tool.execute(input).await;
    assert!(result.is_err()); // Should timeout
}

#[tokio::test]
async fn test_shell_tool_dry_run() {
    let tool = ShellTool::new(true); // dry-run mode
    let input = json!({"cmd": "rm -rf /"});
    let result = tool.execute(input).await.unwrap();

    assert_eq!(result["stdout"], "[dry-run]");
    assert_eq!(result["exit_code"], 0);
}

#[tokio::test]
async fn test_shell_tool_stderr_capture() {
    let tool = ShellTool::new(false);
    let input = json!({"cmd": "echo error >&2"});
    let result = tool.execute(input).await.unwrap();

    assert_eq!(result["exit_code"], 0);
    assert!(result["stderr"].as_str().unwrap().contains("error"));
}

#[tokio::test]
async fn test_shell_tool_nonzero_exit() {
    let tool = ShellTool::new(false);
    let input = json!({"cmd": "exit 42"});
    let result = tool.execute(input).await.unwrap();

    assert_eq!(result["exit_code"], 42);
}
