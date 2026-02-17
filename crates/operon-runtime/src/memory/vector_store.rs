use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;
use tracing::warn;

/// Vector storage with brute-force cosine similarity search.
/// Uses SQLite to persist embeddings as BLOBs.
/// Sufficient for workspace-scale datasets (<10K docs). Swap to HNSW if needed.
pub struct VectorStore {
    conn: Mutex<Connection>,
    dimensions: usize,
}

impl VectorStore {
    pub fn new(db_path: &Path, dimensions: usize) -> Result<Self> {
        let conn = Connection::open(db_path).context("Failed to open vector database")?;

        conn.pragma_update(None, "journal_mode", "WAL")
            .context("Failed to enable WAL mode")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS vectors (
                id TEXT PRIMARY KEY,
                embedding BLOB NOT NULL
            );",
        )
        .context("Failed to initialize vector table")?;

        Ok(Self {
            conn: Mutex::new(conn),
            dimensions,
        })
    }

    /// Insert or update an embedding for a document.
    pub fn upsert(&self, id: &str, embedding: &[f32]) -> Result<()> {
        let bytes = embedding_to_bytes(embedding);
        let conn = self.conn.lock().map_err(|e| anyhow!("DB lock poisoned: {}", e))?;
        conn.execute(
            "INSERT INTO vectors (id, embedding) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET embedding = excluded.embedding",
            params![id, bytes],
        )
        .context("Failed to upsert vector")?;
        Ok(())
    }

    /// Remove an embedding by document id.
    pub fn remove(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow!("DB lock poisoned: {}", e))?;
        conn.execute("DELETE FROM vectors WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Cosine similarity search. Returns (doc_id, similarity_score) sorted descending.
    pub fn search(&self, query_embedding: &[f32], limit: usize) -> Result<Vec<(String, f32)>> {
        let conn = self.conn.lock().map_err(|e| anyhow!("DB lock poisoned: {}", e))?;
        let mut stmt = conn.prepare("SELECT id, embedding FROM vectors")?;

        let mut scored: Vec<(String, f32)> = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((id, blob))
            })?
            .filter_map(|r| r.ok())
            .filter_map(|(id, blob)| {
                match bytes_to_embedding(&blob, self.dimensions) {
                    Ok(emb) => Some((id, cosine_similarity(query_embedding, &emb))),
                    Err(e) => {
                        warn!(doc_id = %id, error = %e, "Skipping corrupted embedding");
                        None
                    }
                }
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        Ok(scored)
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !norm_a.is_finite() || !norm_b.is_finite() || norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    let sim = dot / (norm_a * norm_b);
    if sim.is_finite() { sim } else { 0.0 }
}

fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn bytes_to_embedding(bytes: &[u8], dimensions: usize) -> Result<Vec<f32>> {
    let expected_len = dimensions * 4;
    if bytes.len() != expected_len {
        anyhow::bail!(
            "Dimension mismatch: expected {} bytes ({} dims), got {} bytes",
            expected_len, dimensions, bytes.len()
        );
    }
    Ok((0..dimensions)
        .map(|i| {
            let start = i * 4;
            f32::from_le_bytes([bytes[start], bytes[start + 1], bytes[start + 2], bytes[start + 3]])
        })
        .collect())
}
