//! Tests for filesystem tools: workspace guard, read, write, edit, apply_patch.

use operon_adapters::{ApplyPatchTool, EditFileTool, ReadFileTool, WorkspaceGuard, WriteFileTool};
use operon_runtime::Tool;
use serde_json::json;
use std::sync::Arc;

fn make_guard(dir: &std::path::Path) -> Arc<WorkspaceGuard> {
    Arc::new(WorkspaceGuard::new(dir.to_path_buf(), 10).unwrap())
}

// ── WorkspaceGuard ──────────────────────────────────────────────────────

#[test]
fn test_workspace_resolve_valid_path() {
    let dir = tempfile::tempdir().unwrap();
    let canonical_root = dir.path().canonicalize().unwrap();
    std::fs::write(dir.path().join("hello.txt"), "hi").unwrap();
    let guard = make_guard(dir.path());
    let resolved = guard.resolve("hello.txt").unwrap();
    assert!(resolved.starts_with(&canonical_root));
}

#[test]
fn test_workspace_reject_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let guard = make_guard(dir.path());
    let result = guard.resolve("../../etc/passwd");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("traversal"));
}

#[test]
fn test_workspace_resolve_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let canonical_root = dir.path().canonicalize().unwrap();
    let guard = make_guard(dir.path());
    let resolved = guard.resolve("new_file.txt").unwrap();
    assert!(resolved.starts_with(&canonical_root));
}

#[tokio::test]
async fn test_binary_file_detection() {
    let dir = tempfile::tempdir().unwrap();
    let text_path = dir.path().join("text.txt");
    std::fs::write(&text_path, "hello world").unwrap();
    assert!(WorkspaceGuard::is_text_file(&text_path).await.unwrap());

    let bin_path = dir.path().join("binary.bin");
    std::fs::write(&bin_path, b"hello\x00world").unwrap();
    assert!(!WorkspaceGuard::is_text_file(&bin_path).await.unwrap());
}

// ── ReadFileTool ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_read_file_basic() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("test.txt"), "line1\nline2\nline3\n").unwrap();

    let tool = ReadFileTool::new(make_guard(dir.path()));
    let result = tool.execute(json!({"path": "test.txt"})).await.unwrap();
    assert_eq!(result["total_lines"], 3);
    assert_eq!(result["lines_shown"], 3);
}

#[tokio::test]
async fn test_read_file_with_offset_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("test.txt"), "a\nb\nc\nd\ne\n").unwrap();

    let tool = ReadFileTool::new(make_guard(dir.path()));
    let result = tool
        .execute(json!({"path": "test.txt", "offset": 1, "limit": 2}))
        .await
        .unwrap();
    assert_eq!(result["lines_shown"], 2);
    assert_eq!(result["offset"], 1);
    let content = result["content"].as_str().unwrap();
    assert!(content.contains("b"));
    assert!(content.contains("c"));
    assert!(!content.contains("\ta\n"));
}

#[tokio::test]
async fn test_read_file_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let tool = ReadFileTool::new(make_guard(dir.path()));
    let result = tool.execute(json!({"path": "nope.txt"})).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[tokio::test]
async fn test_read_file_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let tool = ReadFileTool::new(make_guard(dir.path()));
    let result = tool.execute(json!({"path": "../../etc/passwd"})).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_read_binary_rejected() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("bin.dat"), b"data\x00here").unwrap();

    let tool = ReadFileTool::new(make_guard(dir.path()));
    let result = tool.execute(json!({"path": "bin.dat"})).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Binary"));
}

// ── WriteFileTool ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_write_file_new() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteFileTool::new(make_guard(dir.path()));

    let result = tool
        .execute(json!({"path": "out.txt", "content": "hello"}))
        .await
        .unwrap();
    assert_eq!(result["bytes_written"], 5);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        "hello"
    );
}

#[tokio::test]
async fn test_write_file_creates_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteFileTool::new(make_guard(dir.path()));

    tool.execute(json!({"path": "sub/dir/file.txt", "content": "nested"}))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("sub/dir/file.txt")).unwrap(),
        "nested"
    );
}

#[tokio::test]
async fn test_write_file_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("old.txt"), "old content").unwrap();

    let tool = WriteFileTool::new(make_guard(dir.path()));
    tool.execute(json!({"path": "old.txt", "content": "new content"}))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("old.txt")).unwrap(),
        "new content"
    );
}

#[tokio::test]
async fn test_write_file_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteFileTool::new(make_guard(dir.path()));
    let result = tool
        .execute(json!({"path": "../../evil.txt", "content": "hacked"}))
        .await;
    assert!(result.is_err());
}

// ── EditFileTool ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_edit_single_replace() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("code.rs"), "fn hello() {}\nfn world() {}\n").unwrap();

    let tool = EditFileTool::new(make_guard(dir.path()));
    let result = tool
        .execute(json!({
            "path": "code.rs",
            "old_string": "fn hello() {}",
            "new_string": "fn greeting() {}"
        }))
        .await
        .unwrap();
    assert_eq!(result["replacements"], 1);
    let content = std::fs::read_to_string(dir.path().join("code.rs")).unwrap();
    assert!(content.contains("fn greeting() {}"));
    assert!(content.contains("fn world() {}"));
}

#[tokio::test]
async fn test_edit_replace_all() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("dup.txt"), "foo bar foo baz foo\n").unwrap();

    let tool = EditFileTool::new(make_guard(dir.path()));
    let result = tool
        .execute(json!({
            "path": "dup.txt",
            "old_string": "foo",
            "new_string": "qux",
            "replace_all": true
        }))
        .await
        .unwrap();
    assert_eq!(result["replacements"], 3);
    let content = std::fs::read_to_string(dir.path().join("dup.txt")).unwrap();
    assert!(!content.contains("foo"));
}

#[tokio::test]
async fn test_edit_not_found() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "hello").unwrap();

    let tool = EditFileTool::new(make_guard(dir.path()));
    let result = tool
        .execute(json!({
            "path": "f.txt",
            "old_string": "nonexistent",
            "new_string": "x"
        }))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[tokio::test]
async fn test_edit_ambiguous_match() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "aaa bbb aaa").unwrap();

    let tool = EditFileTool::new(make_guard(dir.path()));
    let result = tool
        .execute(json!({
            "path": "f.txt",
            "old_string": "aaa",
            "new_string": "ccc"
        }))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not unique"));
}

// ── ApplyPatchTool ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_apply_patch_simple() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file.txt"), "line1\nline2\nline3\n").unwrap();

    let patch = "\
--- a/file.txt
+++ b/file.txt
@@ -1,3 +1,3 @@
 line1
-line2
+line2_modified
 line3
";

    let tool = ApplyPatchTool::new(make_guard(dir.path()));
    let result = tool.execute(json!({"patch": patch})).await.unwrap();
    assert_eq!(result["files_modified"], 1);
    assert_eq!(result["hunks_applied"], 1);

    let content = std::fs::read_to_string(dir.path().join("file.txt")).unwrap();
    assert!(content.contains("line2_modified"));
    assert!(!content.contains("\nline2\n"));
}

#[tokio::test]
async fn test_apply_patch_file_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let patch = "\
--- a/missing.txt
+++ b/missing.txt
@@ -1,1 +1,1 @@
-old
+new
";
    let tool = ApplyPatchTool::new(make_guard(dir.path()));
    let result = tool.execute(json!({"patch": patch})).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[tokio::test]
async fn test_apply_patch_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let tool = ApplyPatchTool::new(make_guard(dir.path()));
    let result = tool
        .execute(json!({"patch": "this is not a valid diff"}))
        .await;
    assert!(result.is_err());
}
