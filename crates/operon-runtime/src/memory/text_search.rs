use crate::memory::types::Document;
use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

/// Full-text search index backed by SQLite FTS5.
pub struct TextSearchIndex {
    conn: Mutex<Connection>,
}

impl TextSearchIndex {
    /// Open or create the SQLite database with FTS5 tables.
    pub fn new(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path).context("Failed to open memory database")?;

        // Enable WAL mode for better concurrent read/write performance
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("Failed to enable WAL mode")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS documents (
                id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                content TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                metadata TEXT
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
                content, path,
                content='documents', content_rowid='rowid'
            );

            -- Triggers to keep FTS in sync with documents table
            CREATE TRIGGER IF NOT EXISTS documents_ai AFTER INSERT ON documents BEGIN
                INSERT INTO documents_fts(rowid, content, path)
                VALUES (new.rowid, new.content, new.path);
            END;

            CREATE TRIGGER IF NOT EXISTS documents_ad AFTER DELETE ON documents BEGIN
                INSERT INTO documents_fts(documents_fts, rowid, content, path)
                VALUES ('delete', old.rowid, old.content, old.path);
            END;

            CREATE TRIGGER IF NOT EXISTS documents_au AFTER UPDATE ON documents BEGIN
                INSERT INTO documents_fts(documents_fts, rowid, content, path)
                VALUES ('delete', old.rowid, old.content, old.path);
                INSERT INTO documents_fts(rowid, content, path)
                VALUES (new.rowid, new.content, new.path);
            END;",
        )
        .context("Failed to initialize FTS5 tables")?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Index a document (upsert into documents + FTS5 via triggers).
    pub fn index_document(&self, doc: &Document) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow!("DB lock poisoned: {}", e))?;
        conn.execute(
            "INSERT INTO documents (id, path, content, content_hash, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                path = excluded.path,
                content = excluded.content,
                content_hash = excluded.content_hash,
                updated_at = datetime('now'),
                metadata = excluded.metadata",
            params![doc.id, doc.path, doc.content, doc.content_hash, doc.metadata],
        )
        .context("Failed to index document")?;
        Ok(())
    }

    /// Remove a document from both tables.
    pub fn remove_document(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow!("DB lock poisoned: {}", e))?;
        conn.execute("DELETE FROM documents WHERE id = ?1", params![id])
            .context("Failed to remove document")?;
        Ok(())
    }

    /// BM25-ranked full-text search. Returns (doc_id, bm25_score) pairs.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<(String, f64)>> {
        let conn = self.conn.lock().map_err(|e| anyhow!("DB lock poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT d.id, bm25(documents_fts) AS score
             FROM documents_fts f
             JOIN documents d ON d.rowid = f.rowid
             WHERE documents_fts MATCH ?1
             ORDER BY score
             LIMIT ?2",
        )?;

        let results = stmt
            .query_map(params![query, limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to collect FTS results")?;

        Ok(results)
    }

    /// Check if a document exists by id.
    pub fn has_document(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| anyhow!("DB lock poisoned: {}", e))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM documents WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Get the content hash for a document (for cache-based skip).
    pub fn get_content_hash(&self, id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow!("DB lock poisoned: {}", e))?;
        let mut stmt = conn.prepare("SELECT content_hash FROM documents WHERE id = ?1")?;
        let result = stmt
            .query_row(params![id], |row| row.get::<_, String>(0))
            .ok();
        Ok(result)
    }

    /// Get document content by id.
    pub fn get_document_content(&self, id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow!("DB lock poisoned: {}", e))?;
        let mut stmt = conn.prepare("SELECT content FROM documents WHERE id = ?1")?;
        let result = stmt
            .query_row(params![id], |row| row.get::<_, String>(0))
            .ok();
        Ok(result)
    }

    /// List all document IDs in the index.
    pub fn list_document_ids(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow!("DB lock poisoned: {}", e))?;
        let mut stmt = conn.prepare("SELECT id FROM documents")?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }
}
