use anyhow::{bail, Context, Result};
use std::path::{Component, Path, PathBuf};
use tokio::io::AsyncReadExt;

/// Workspace-scoped path resolver — prevents path traversal attacks.
/// All file operations must resolve paths through this guard.
pub struct WorkspaceGuard {
    root: PathBuf,
    max_file_size: u64,
}

impl WorkspaceGuard {
    pub fn new(root: PathBuf, max_file_size_mb: u64) -> Result<Self> {
        let root = root
            .canonicalize()
            .context(format!("Workspace root not found: {:?}", root))?;
        Ok(Self {
            root,
            max_file_size: max_file_size_mb * 1024 * 1024,
        })
    }

    /// Resolve a user-provided path relative to workspace root.
    /// Rejects paths that escape the workspace via `..` or symlinks.
    pub fn resolve(&self, input_path: &str) -> Result<PathBuf> {
        let joined = self.root.join(input_path);

        // For existing paths, canonicalize resolves symlinks
        let resolved = if joined.exists() {
            joined.canonicalize()?
        } else {
            // Normalize `..` and `.` components without requiring path to exist
            normalize_path(&joined)
        };

        if !resolved.starts_with(&self.root) {
            bail!(
                "Path traversal denied: {:?} is outside workspace {:?}",
                input_path,
                self.root
            );
        }
        Ok(resolved)
    }

    /// Check if file is a text file (no null bytes in first 8KB).
    /// Only reads up to 8KB instead of the entire file.
    pub async fn is_text_file(path: &Path) -> Result<bool> {
        let mut file =
            tokio::fs::File::open(path)
                .await
                .context("Failed to open file for binary check")?;
        let mut buf = vec![0u8; 8192];
        let n = file
            .read(&mut buf)
            .await
            .context("Failed to read file for binary check")?;
        Ok(!buf[..n].contains(&0))
    }

    /// Check file size against limit
    pub async fn check_size(&self, path: &Path) -> Result<()> {
        let meta = tokio::fs::metadata(path)
            .await
            .context("Failed to read file metadata")?;
        if meta.len() > self.max_file_size {
            bail!(
                "File too large: {} bytes (max {} MB)",
                meta.len(),
                self.max_file_size / (1024 * 1024)
            );
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Normalize a path by resolving `.` and `..` components without filesystem access.
fn normalize_path(path: &Path) -> PathBuf {
    let mut parts: Vec<Component> = Vec::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                // Only pop normal components, never pop root/prefix
                if matches!(parts.last(), Some(Component::Normal(_))) {
                    parts.pop();
                }
            }
            Component::CurDir => {}
            other => parts.push(other),
        }
    }
    parts.iter().collect()
}
