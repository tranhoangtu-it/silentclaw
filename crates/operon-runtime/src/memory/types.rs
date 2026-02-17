use serde::{Deserialize, Serialize};

/// A document stored in the memory index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// Unique identifier (typically derived from file path)
    pub id: String,
    /// File path relative to workspace root
    pub path: String,
    /// Full text content
    pub content: String,
    /// SHA-256 hash of content for cache-based skip
    pub content_hash: String,
    /// Optional JSON metadata
    pub metadata: Option<String>,
}

/// A single search result returned from memory queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub document_id: String,
    pub path: String,
    pub content_snippet: String,
    pub score: f64,
    pub source: SearchSource,
}

/// Search query parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub source: SearchSource,
}

fn default_limit() -> usize {
    10
}

/// Which search backend(s) to use.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SearchSource {
    Vector,
    #[serde(rename = "fts")]
    FullText,
    #[default]
    Hybrid,
}

/// Statistics returned after an indexing operation.
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    pub files_indexed: usize,
    pub files_skipped: usize,
    pub files_removed: usize,
    pub errors: usize,
}
