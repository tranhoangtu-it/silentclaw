use anyhow::{bail, Context, Result};

/// A single line in a hunk: context, removal, or addition
#[derive(Clone)]
pub enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Clone)]
pub struct Hunk {
    pub old_start: usize, // 0-based line index
    pub lines: Vec<HunkLine>,
}

pub struct FilePatch {
    pub path: String,
    pub hunks: Vec<Hunk>,
}

pub fn parse_unified_diff(patch: &str) -> Result<Vec<FilePatch>> {
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

pub fn apply_hunk(lines: &[String], hunk: &Hunk) -> Result<Vec<String>> {
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
