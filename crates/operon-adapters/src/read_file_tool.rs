use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use operon_runtime::{PermissionLevel, Tool, ToolSchemaInfo};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::workspace_guard::WorkspaceGuard;

pub struct ReadFileTool {
    guard: Arc<WorkspaceGuard>,
}

impl ReadFileTool {
    pub fn new(guard: Arc<WorkspaceGuard>) -> Self {
        Self { guard }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    async fn execute(&self, input: Value) -> Result<Value> {
        let path_str = input["path"]
            .as_str()
            .context("Missing required field 'path'")?;
        let offset = input["offset"].as_u64().unwrap_or(0) as usize;
        let limit = input["limit"].as_u64().unwrap_or(0) as usize;

        let path = self.guard.resolve(path_str)?;

        if !path.exists() {
            bail!("File not found: {}", path_str);
        }

        self.guard.check_size(&path).await?;

        // Read once, check binary inline (avoids double read)
        let bytes = tokio::fs::read(&path)
            .await
            .context("Failed to read file")?;
        if bytes.is_empty() {
            return Ok(json!({
                "content": "",
                "total_lines": 0,
                "lines_shown": 0,
                "offset": 0,
            }));
        }
        let check_len = bytes.len().min(8192);
        if bytes[..check_len].contains(&0) {
            bail!("Binary file detected, cannot read: {}", path_str);
        }
        let content = String::from_utf8(bytes).context("File is not valid UTF-8")?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // Apply offset and limit
        let start = offset.min(total_lines);
        let end = if limit > 0 {
            (start + limit).min(total_lines)
        } else {
            total_lines
        };

        // Format with line numbers (cat -n style)
        let numbered: Vec<String> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", start + i + 1, line))
            .collect();

        Ok(json!({
            "content": numbered.join("\n"),
            "total_lines": total_lines,
            "lines_shown": end - start,
            "offset": start,
        }))
    }

    fn name(&self) -> &str {
        "read_file"
    }

    fn schema(&self) -> ToolSchemaInfo {
        ToolSchemaInfo {
            name: "read_file".to_string(),
            description: "Read a file with optional line offset and limit".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to workspace" },
                    "offset": { "type": "integer", "description": "Line offset (0-based)" },
                    "limit": { "type": "integer", "description": "Max lines to read (0 = all)" }
                },
                "required": ["path"]
            }),
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}
