use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use operon_runtime::{PermissionLevel, Tool, ToolSchemaInfo};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::Arc;

use crate::workspace_guard::WorkspaceGuard;

pub struct EditFileTool {
    guard: Arc<WorkspaceGuard>,
}

impl EditFileTool {
    pub fn new(guard: Arc<WorkspaceGuard>) -> Self {
        Self { guard }
    }
}

#[async_trait]
impl Tool for EditFileTool {
    async fn execute(&self, input: Value) -> Result<Value> {
        let path_str = input["path"]
            .as_str()
            .context("Missing required field 'path'")?;
        let old_string = input["old_string"]
            .as_str()
            .context("Missing required field 'old_string'")?;
        let new_string = input["new_string"]
            .as_str()
            .context("Missing required field 'new_string'")?;
        let replace_all = input["replace_all"].as_bool().unwrap_or(false);

        let path = self.guard.resolve(path_str)?;

        if !path.exists() {
            bail!("File not found: {}", path_str);
        }

        self.guard.check_size(&path).await?;

        let content = tokio::fs::read_to_string(&path)
            .await
            .context("Failed to read file")?;
        let match_count = content.matches(old_string).count();

        if match_count == 0 {
            bail!("old_string not found in file: {}", path_str);
        }

        if match_count > 1 && !replace_all {
            bail!(
                "old_string found {} times in file (not unique). Use replace_all=true to replace all.",
                match_count
            );
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        // Atomic write
        let parent = path.parent().unwrap_or(self.guard.root());
        let mut tmp = tempfile::NamedTempFile::new_in(parent)
            .context("Failed to create temp file for atomic write")?;
        tmp.write_all(new_content.as_bytes())?;
        tmp.flush()?;
        tmp.persist(&path)
            .context(format!("Failed to persist edited file: {:?}", path))?;

        Ok(json!({
            "replacements": if replace_all { match_count } else { 1 },
            "path": path_str,
        }))
    }

    fn name(&self) -> &str {
        "edit_file"
    }

    fn schema(&self) -> ToolSchemaInfo {
        ToolSchemaInfo {
            name: "edit_file".to_string(),
            description: "Find and replace exact string in a file".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to workspace" },
                    "old_string": { "type": "string", "description": "Exact string to find" },
                    "new_string": { "type": "string", "description": "Replacement string" },
                    "replace_all": { "type": "boolean", "description": "Replace all occurrences (default: false)" }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }
}
