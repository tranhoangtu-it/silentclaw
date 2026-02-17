pub mod apply_patch_tool;
pub mod diff_parser;
pub mod edit_file_tool;
pub mod memory_search_tool;
pub mod python_adapter;
pub mod read_file_tool;
pub mod shell_tool;
pub mod workspace_guard;
pub mod write_file_tool;

pub use apply_patch_tool::ApplyPatchTool;
pub use edit_file_tool::EditFileTool;
pub use memory_search_tool::MemorySearchTool;
pub use python_adapter::PyAdapter;
pub use read_file_tool::ReadFileTool;
pub use shell_tool::ShellTool;
pub use workspace_guard::WorkspaceGuard;
pub use write_file_tool::WriteFileTool;

use anyhow::Result;
use operon_runtime::Runtime;
use std::path::PathBuf;
use std::sync::Arc;

/// Register shell tool on the runtime if enabled.
pub fn register_shell_tool(
    runtime: &Arc<Runtime>,
    dry_run: bool,
    blocklist: Vec<String>,
    allowlist: Vec<String>,
) -> Result<()> {
    let shell_tool = ShellTool::new(dry_run).with_validation(blocklist, allowlist);
    runtime.register_tool("shell".to_string(), Arc::new(shell_tool))
}

/// Register all filesystem tools (read, write, edit, patch) on the runtime.
pub fn register_filesystem_tools(
    runtime: &Arc<Runtime>,
    workspace: PathBuf,
    max_file_size_mb: u64,
) -> Result<()> {
    let guard = Arc::new(WorkspaceGuard::new(workspace, max_file_size_mb)?);
    runtime.register_tool("read_file".into(), Arc::new(ReadFileTool::new(guard.clone())))?;
    runtime.register_tool("write_file".into(), Arc::new(WriteFileTool::new(guard.clone())))?;
    runtime.register_tool("edit_file".into(), Arc::new(EditFileTool::new(guard.clone())))?;
    runtime.register_tool("apply_patch".into(), Arc::new(ApplyPatchTool::new(guard)))?;
    Ok(())
}
