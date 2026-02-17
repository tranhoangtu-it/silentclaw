pub mod embedding;
pub mod hybrid_search;
pub mod indexer;
pub mod text_search;
pub mod types;
pub mod vector_store;

use crate::memory::embedding::EmbeddingProvider;
use crate::memory::hybrid_search::rrf_merge;
use crate::memory::indexer::DocumentIndexer;
use crate::memory::text_search::TextSearchIndex;
use crate::memory::types::{SearchQuery, SearchResult, SearchSource};
use crate::memory::vector_store::VectorStore;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Orchestrates text search, vector search, and hybrid search.
pub struct MemoryManager {
    text_index: Arc<TextSearchIndex>,
    vector_store: Arc<VectorStore>,
    embedder: Arc<dyn EmbeddingProvider>,
    indexer: Arc<DocumentIndexer>,
}

impl MemoryManager {
    pub fn new(
        db_path: &Path,
        workspace: PathBuf,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self> {
        let dims = embedder.dimensions();
        let text_index = Arc::new(TextSearchIndex::new(db_path)?);
        let vector_store = Arc::new(VectorStore::new(db_path, dims)?);
        let indexer = Arc::new(DocumentIndexer::new(
            workspace,
            text_index.clone(),
            vector_store.clone(),
            embedder.clone(),
        ));

        Ok(Self {
            text_index,
            vector_store,
            embedder,
            indexer,
        })
    }

    /// Run initial workspace indexing and start file watcher.
    pub async fn start_indexing(&self) -> Result<tokio::task::JoinHandle<()>> {
        // Initial full index
        self.indexer.index_workspace().await?;
        // Start watching for changes
        self.indexer.clone().watch_workspace()
    }

    /// Search memory using the specified source (vector, FTS, or hybrid).
    pub async fn search(&self, query: SearchQuery) -> Result<Vec<SearchResult>> {
        match query.source {
            SearchSource::FullText => self.search_fts(&query.query, query.limit),
            SearchSource::Vector => self.search_vector(&query.query, query.limit).await,
            SearchSource::Hybrid => self.search_hybrid(&query.query, query.limit).await,
        }
    }

    fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let results = self.text_index.search(query, limit)?;
        results
            .into_iter()
            .map(|(id, score)| self.build_result(&id, score, SearchSource::FullText))
            .collect()
    }

    async fn search_vector(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let query_emb = self.embedder.embed(query).await?;
        let results = self.vector_store.search(&query_emb, limit)?;
        results
            .into_iter()
            .map(|(id, score)| self.build_result(&id, score as f64, SearchSource::Vector))
            .collect()
    }

    async fn search_hybrid(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Fetch more results from each source for better RRF merging
        let fetch_limit = limit * 3;

        let fts_results = self.text_index.search(query, fetch_limit)?;
        let query_emb = self.embedder.embed(query).await?;
        let vector_results = self.vector_store.search(&query_emb, fetch_limit)?;

        let merged = rrf_merge(&vector_results, &fts_results, 60, limit);

        merged
            .into_iter()
            .map(|(id, score)| self.build_result(&id, score, SearchSource::Hybrid))
            .collect()
    }

    fn build_result(&self, id: &str, score: f64, source: SearchSource) -> Result<SearchResult> {
        let content = self
            .text_index
            .get_document_content(id)?
            .unwrap_or_default();

        // Snippet: first 500 chars (safe for multi-byte UTF-8)
        let snippet = if content.chars().count() > 500 {
            content.chars().take(500).collect::<String>() + "..."
        } else {
            content
        };

        Ok(SearchResult {
            document_id: id.to_string(),
            path: id.to_string(),
            content_snippet: snippet,
            score,
            source,
        })
    }
}
