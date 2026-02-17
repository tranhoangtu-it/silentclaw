use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use operon_runtime::{PermissionLevel, Tool, ToolSchemaInfo};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::Arc;

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

            let content =
                std::fs::read_to_string(&path).context(format!("Failed to read: {}", fp.path))?;
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

/// A single line in a hunk: context, removal, or addition
#[derive(Clone)]
enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Clone)]
struct Hunk {
    old_start: usize, // 0-based line index
    lines: Vec<HunkLine>,
}

struct FilePatch {
    path: String,
    hunks: Vec<Hunk>,
}

fn parse_unified_diff(patch: &str) -> Result<Vec<FilePatch>> {
    let mut file_patches = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_hunks: Vec<Hunk> = Vec::new();
    let mut current_hunk: Option<Hunk> = None;

    for line in patch.lines() {
        if line.starts_with("+++ b/") || line.starts_with("+++ ") {
            if let Some(h) = current_hunk.take() {
                current_hunks.push(h);
            }
            if let Some(path) = current_path.take() {
                if !current_hunks.is_empty() {
                    file_patches.push(FilePatch {
                        path,
                        hunks: std::mem::take(&mut current_hunks),
                    });
                }
            }
            let path = line
                .strip_prefix("+++ b/")
                .or_else(|| line.strip_prefix("+++ "))
                .unwrap_or("")
                .to_string();
            current_path = Some(path);
        } else if line.starts_with("--- ") {
            continue;
        } else if line.starts_with("@@ ") {
            if let Some(h) = current_hunk.take() {
                current_hunks.push(h);
            }
            let old_start = parse_hunk_header(line)?;
            current_hunk = Some(Hunk {
                old_start,
                lines: Vec::new(),
            });
        } else if let Some(ref mut hunk) = current_hunk {
            if let Some(removed) = line.strip_prefix('-') {
                hunk.lines.push(HunkLine::Remove(removed.to_string()));
            } else if let Some(added) = line.strip_prefix('+') {
                hunk.lines.push(HunkLine::Add(added.to_string()));
            } else if let Some(ctx) = line.strip_prefix(' ') {
                hunk.lines.push(HunkLine::Context(ctx.to_string()));
            }
        }
    }

    if let Some(h) = current_hunk {
        current_hunks.push(h);
    }
    if let Some(path) = current_path {
        if !current_hunks.is_empty() {
            file_patches.push(FilePatch {
                path,
                hunks: current_hunks,
            });
        }
    }

    if file_patches.is_empty() {
        bail!("No valid patches found in diff");
    }
    Ok(file_patches)
}

/// Parse `@@ -start,count +start,count @@` → old_start (1-based → 0-based)
fn parse_hunk_header(line: &str) -> Result<usize> {
    let part = line
        .split("@@")
        .nth(1)
        .context("Invalid hunk header")?
        .trim();
    let old_part = part.split(' ').next().context("Invalid hunk range")?;
    let start_str = old_part
        .strip_prefix('-')
        .unwrap_or(old_part)
        .split(',')
        .next()
        .context("Invalid hunk start")?;
    let start: usize = start_str.parse().context("Invalid hunk line number")?;
    Ok(start.saturating_sub(1)) // 1-based → 0-based
}

fn apply_hunk(lines: &[String], hunk: &Hunk) -> Result<Vec<String>> {
    let mut result = Vec::with_capacity(lines.len());
    let start = hunk.old_start;

    // Copy lines before hunk
    result.extend_from_slice(&lines[..start.min(lines.len())]);

    // Walk hunk lines, consuming old lines and emitting new lines
    let mut pos = start;
    for hl in &hunk.lines {
        match hl {
            HunkLine::Context(ctx) => {
                if pos < lines.len() && lines[pos] != *ctx {
                    bail!(
                        "Context mismatch at line {}: expected {:?}, found {:?}",
                        pos + 1,
                        ctx,
                        lines[pos]
                    );
                }
                result.push(ctx.clone());
                pos += 1;
            }
            HunkLine::Remove(rem) => {
                if pos < lines.len() && lines[pos] != *rem {
                    bail!(
                        "Hunk mismatch at line {}: expected {:?}, found {:?}",
                        pos + 1,
                        rem,
                        lines[pos]
                    );
                }
                pos += 1; // skip removed line
            }
            HunkLine::Add(add) => {
                result.push(add.clone()); // don't advance pos
            }
        }
    }

    // Copy remaining lines after hunk
    if pos < lines.len() {
        result.extend_from_slice(&lines[pos..]);
    }

    Ok(result)
}
