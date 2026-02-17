use anyhow::{Context, Result};
use async_trait::async_trait;
use operon_runtime::{PermissionLevel, Tool, ToolSchemaInfo};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::Arc;

use crate::workspace_guard::WorkspaceGuard;

pub struct WriteFileTool {
    guard: Arc<WorkspaceGuard>,
}

impl WriteFileTool {
    pub fn new(guard: Arc<WorkspaceGuard>) -> Self {
        Self { guard }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    async fn execute(&self, input: Value) -> Result<Value> {
        let path_str = input["path"]
            .as_str()
            .context("Missing required field 'path'")?;
        let content = input["content"]
            .as_str()
            .context("Missing required field 'content'")?;

        let path = self.guard.resolve(path_str)?;

        // Create parent directories
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context(format!("Failed to create directories: {:?}", parent))?;
        }

        // Atomic write: temp file + rename
        let parent = path.parent().unwrap_or(self.guard.root());
        let mut tmp = tempfile::NamedTempFile::new_in(parent)
            .context("Failed to create temp file for atomic write")?;

        tmp.write_all(content.as_bytes())
            .context("Failed to write to temp file")?;
        tmp.flush()?;

        tmp.persist(&path)
            .context(format!("Failed to persist file: {:?}", path))?;

        Ok(json!({
            "bytes_written": content.len(),
            "path": path_str,
        }))
    }

    fn name(&self) -> &str {
        "write_file"
    }

    fn schema(&self) -> ToolSchemaInfo {
        ToolSchemaInfo {
            name: "write_file".to_string(),
            description: "Write content to a file atomically".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to workspace" },
                    "content": { "type": "string", "description": "Content to write" }
                },
                "required": ["path", "content"]
            }),
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }
}
