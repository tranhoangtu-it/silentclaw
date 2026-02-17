use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use operon_runtime::{PermissionLevel, Tool, ToolSchemaInfo};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::Arc;

use crate::diff_parser::{apply_hunk, parse_unified_diff};
use crate::workspace_guard::WorkspaceGuard;

pub struct ApplyPatchTool {
    guard: Arc<WorkspaceGuard>,
}

impl ApplyPatchTool {
    pub fn new(guard: Arc<WorkspaceGuard>) -> Self {
        Self { guard }
    }
}

#[async_trait]
impl Tool for ApplyPatchTool {
    async fn execute(&self, input: Value) -> Result<Value> {
        let patch = input["patch"]
            .as_str()
            .context("Missing required field 'patch'")?;

        let file_patches = parse_unified_diff(patch)?;
        let mut files_modified = 0;
        let mut hunks_applied = 0;

        for fp in &file_patches {
            let path = self.guard.resolve(&fp.path)?;
            if !path.exists() {
                bail!("Patch target not found: {}", fp.path);
            }

            let content = tokio::fs::read_to_string(&path)
                .await
                .context(format!("Failed to read: {}", fp.path))?;
            let mut lines: Vec<String> = content.lines().map(String::from).collect();

            // Apply hunks in reverse order to preserve line numbers
            let mut sorted_hunks = fp.hunks.clone();
            sorted_hunks.sort_by(|a, b| b.old_start.cmp(&a.old_start));

            for hunk in &sorted_hunks {
                lines = apply_hunk(&lines, hunk)?;
                hunks_applied += 1;
            }

            // Atomic write
            let new_content = lines.join("\n") + if content.ends_with('\n') { "\n" } else { "" };
            let parent = path.parent().unwrap_or(self.guard.root());
            let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
            tmp.write_all(new_content.as_bytes())?;
            tmp.flush()?;
            tmp.persist(&path)?;

            files_modified += 1;
        }

        Ok(json!({
            "files_modified": files_modified,
            "hunks_applied": hunks_applied,
        }))
    }

    fn name(&self) -> &str {
        "apply_patch"
    }

    fn schema(&self) -> ToolSchemaInfo {
        ToolSchemaInfo {
            name: "apply_patch".to_string(),
            description: "Apply a unified diff patch to workspace files".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "patch": { "type": "string", "description": "Unified diff format patch" }
                },
                "required": ["patch"]
            }),
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }
}
