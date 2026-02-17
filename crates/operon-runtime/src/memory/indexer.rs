use crate::memory::embedding::EmbeddingProvider;
use crate::memory::text_search::TextSearchIndex;
use crate::memory::types::{Document, IndexStats};
use crate::memory::vector_store::VectorStore;
use anyhow::{Context, Result};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Indexes workspace files into text search and vector stores.
pub struct DocumentIndexer {
    workspace: PathBuf,
    text_index: Arc<TextSearchIndex>,
    vector_store: Arc<VectorStore>,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl DocumentIndexer {
    pub fn new(
        workspace: PathBuf,
        text_index: Arc<TextSearchIndex>,
        vector_store: Arc<VectorStore>,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self {
            workspace,
            text_index,
            vector_store,
            embedder,
        }
    }

    /// Index all text files in the workspace. Skips unchanged files (hash match).
    pub async fn index_workspace(&self) -> Result<IndexStats> {
        let mut stats = IndexStats::default();
        let mut seen_ids = HashSet::new();

        let files = collect_text_files(&self.workspace)?;
        info!(count = files.len(), "Indexing workspace files");

        for path in &files {
            let rel_path = match safe_rel_path(path, &self.workspace) {
                Some(r) => r,
                None => {
                    warn!(path = %path.display(), "Skipping path outside workspace");
                    stats.errors += 1;
                    continue;
                }
            };
            let doc_id = rel_path.clone();
            seen_ids.insert(doc_id.clone());

            match self.index_file(&doc_id, path).await {
                Ok(true) => stats.files_indexed += 1,
                Ok(false) => stats.files_skipped += 1,
                Err(e) => {
                    warn!(path = %rel_path, error = %e, "Failed to index file");
                    stats.errors += 1;
                }
            }
        }

        // Remove stale documents (files deleted from workspace)
        if let Ok(existing_ids) = self.text_index.list_document_ids() {
            for id in existing_ids {
                if !seen_ids.contains(&id) {
                    let _ = self.text_index.remove_document(&id);
                    let _ = self.vector_store.remove(&id);
                    stats.files_removed += 1;
                    debug!(id = %id, "Removed stale document");
                }
            }
        }

        info!(?stats, "Workspace indexing complete");
        Ok(stats)
    }

    /// Index a single file. Returns true if indexed, false if skipped (unchanged).
    async fn index_file(&self, doc_id: &str, path: &Path) -> Result<bool> {
        // Skip files larger than 10MB to avoid OOM and embedding API limits
        const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;
        let metadata = tokio::fs::metadata(path).await.context("Failed to read metadata")?;
        if metadata.len() > MAX_FILE_SIZE {
            warn!(path = %path.display(), size = metadata.len(), "Skipping large file");
            return Ok(false);
        }

        let bytes = tokio::fs::read(path).await.context("Failed to read file")?;

        // Skip binary files (null byte heuristic)
        let check_len = bytes.len().min(8192);
        if bytes[..check_len].contains(&0) {
            return Ok(false);
        }

        let content = String::from_utf8(bytes).context("File is not valid UTF-8")?;
        let hash = compute_hash(&content);

        // Skip if content unchanged
        if let Ok(Some(existing_hash)) = self.text_index.get_content_hash(doc_id) {
            if existing_hash == hash {
                return Ok(false);
            }
        }

        let rel_path = safe_rel_path(path, &self.workspace)
            .unwrap_or_else(|| doc_id.to_string());

        // Index into FTS
        let doc = Document {
            id: doc_id.to_string(),
            path: rel_path,
            content: content.clone(),
            content_hash: hash,
            metadata: None,
        };
        self.text_index.index_document(&doc)?;

        // Get embedding and store vector
        match self.embedder.embed(&content).await {
            Ok(embedding) => {
                self.vector_store.upsert(doc_id, &embedding)?;
            }
            Err(e) => {
                warn!(doc_id = %doc_id, error = %e, "Embedding failed, FTS-only index");
            }
        }

        Ok(true)
    }

    /// Watch workspace for file changes and auto-reindex.
    /// Spawns a background task. Returns a handle to stop watching.
    pub fn watch_workspace(self: Arc<Self>) -> Result<tokio::task::JoinHandle<()>> {
        let (tx, mut rx) = mpsc::channel::<PathBuf>(256);

        let workspace = self.workspace.clone();
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                        for path in event.paths {
                            let _ = tx.blocking_send(path);
                        }
                    }
                    _ => {}
                }
            }
        })
        .context("Failed to create file watcher")?;

        watcher
            .watch(&workspace, RecursiveMode::Recursive)
            .context("Failed to watch workspace")?;

        let handle = tokio::spawn(async move {
            let _watcher = watcher; // keep watcher alive
            while let Some(path) = rx.recv().await {
                if !is_text_path(&path) {
                    continue;
                }
                let rel_path = match safe_rel_path(&path, &self.workspace) {
                    Some(r) => r,
                    None => continue,
                };

                if path.exists() {
                    if let Err(e) = self.index_file(&rel_path, &path).await {
                        warn!(path = %rel_path, error = %e, "Re-index failed");
                    } else {
                        debug!(path = %rel_path, "Re-indexed file");
                    }
                } else {
                    // File deleted
                    let _ = self.text_index.remove_document(&rel_path);
                    let _ = self.vector_store.remove(&rel_path);
                    debug!(path = %rel_path, "Removed deleted file from index");
                }
            }
        });

        Ok(handle)
    }
}

/// Validate and produce a safe relative path, rejecting traversal attacks.
fn safe_rel_path(path: &Path, workspace: &Path) -> Option<String> {
    let rel = path.strip_prefix(workspace).ok()?;
    let rel_str = rel.to_str()?;
    if rel_str.contains("..") {
        return None;
    }
    Some(rel_str.to_string())
}

/// Collect all text files from a directory (non-hidden, common extensions).
fn collect_text_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut visited = HashSet::new();
    collect_recursive(dir, &mut files, &mut visited)?;
    Ok(files)
}

fn collect_recursive(dir: &Path, out: &mut Vec<PathBuf>, visited: &mut HashSet<PathBuf>) -> Result<()> {
    // Symlink loop protection
    if let Ok(canonical) = dir.canonicalize() {
        if !visited.insert(canonical) {
            return Ok(());
        }
    }

    let entries = std::fs::read_dir(dir).context(format!("Failed to read dir: {:?}", dir))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files/dirs and common non-text directories
        if name.starts_with('.') || name == "node_modules" || name == "target" || name == "__pycache__"
        {
            continue;
        }

        if path.is_dir() {
            collect_recursive(&path, out, visited)?;
        } else if is_text_path(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Simple heuristic: check file extension for known text types.
fn is_text_path(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    matches!(
        ext,
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "json" | "toml" | "yaml" | "yml"
            | "md" | "txt" | "html" | "css" | "scss" | "sql" | "sh" | "bash" | "zsh"
            | "go" | "java" | "kt" | "swift" | "c" | "cpp" | "h" | "hpp" | "rb"
            | "lua" | "vim" | "conf" | "cfg" | "ini" | "env" | "xml" | "csv"
    )
}

fn compute_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}
