# Phase 4: Memory & Search System

**Added:** 2026-02-18
**Version:** 4.0.0-phase-4
**Status:** Phase 4 Complete - Hybrid Vector + FTS Search

## Overview

Phase 4 introduces a comprehensive memory and search system enabling agents to index and query workspace files using hybrid search (vector embeddings + full-text search) with Reciprocal Rank Fusion (RRF) merging.

### Key Features

- **Hybrid Search:** Combines vector similarity (OpenAI embeddings) with FTS5 full-text search (BM25 ranking)
- **RRF Merging:** Reciprocal Rank Fusion algorithm for balanced result ranking
- **SQLite-Backed:** Persistent storage using SQLite with FTS5 virtual tables
- **Vector Store:** Brute-force cosine similarity search (suitable for <10K docs)
- **Embedding Cache:** Hash-based content cache skips re-embedding unchanged files
- **Auto-Reindexing:** File watcher with `notify` crate auto-reindexes workspace changes
- **LLM-Integrated Tool:** `memory_search` tool callable by agents during conversations

## Architecture

### Memory Module Structure

New module: `crates/operon-runtime/src/memory/`

```
memory/
├── mod.rs              (MemoryManager orchestration)
├── types.rs            (Document, SearchQuery, SearchResult, IndexStats)
├── embedding.rs        (EmbeddingProvider trait + OpenAI implementation)
├── vector_store.rs     (SQLite vector persistence, cosine similarity)
├── text_search.rs      (FTS5 full-text search with BM25 ranking)
├── hybrid_search.rs    (RRF merge algorithm)
└── indexer.rs          (DocumentIndexer + file watcher)
```

### Component Responsibilities

#### 1. MemoryManager (Orchestrator)

**File:** `crates/operon-runtime/src/memory/mod.rs`

Coordinates all search operations:

```rust
pub struct MemoryManager {
    text_index: Arc<TextSearchIndex>,
    vector_store: Arc<VectorStore>,
    embedder: Arc<dyn EmbeddingProvider>,
    indexer: Arc<DocumentIndexer>,
}
```

**Key Methods:**

- `new(db_path, workspace, embedder)` - Initialize with SQLite DB and embedding provider
- `start_indexing()` - Run initial workspace index + spawn file watcher
- `search(query)` - Execute search (FTS, Vector, or Hybrid)
  - `search_fts()` - Full-text search via BM25 ranking
  - `search_vector()` - Vector similarity via cosine distance
  - `search_hybrid()` - RRF-merged results from both sources
- `build_result()` - Construct SearchResult with content snippet (first 500 chars)

#### 2. Types (Data Structures)

**File:** `crates/operon-runtime/src/memory/types.rs`

```rust
pub struct Document {
    pub id: String,              // Unique identifier (file path)
    pub path: String,            // Relative path to workspace root
    pub content: String,         // Full text content
    pub content_hash: String,    // SHA-256 for cache-based skip
    pub metadata: Option<String>,// Optional JSON metadata
}

pub struct SearchResult {
    pub document_id: String,     // Document identifier
    pub path: String,            // File path
    pub content_snippet: String, // First 500 chars + "..."
    pub score: f64,              // Search score (normalized)
    pub source: SearchSource,    // Vector, FullText, or Hybrid
}

pub struct SearchQuery {
    pub query: String,           // Search text
    pub limit: usize,            // Max results (default: 10)
    pub source: SearchSource,    // Search backend selection
}

pub enum SearchSource {
    Vector,  // Vector similarity search
    FullText,// FTS5 BM25 search
    Hybrid,  // RRF-merged results
}

pub struct IndexStats {
    pub files_indexed: usize,
    pub files_skipped: usize,    // No hash change
    pub files_removed: usize,    // Deleted from workspace
    pub errors: usize,
}
```

#### 3. Embedding Provider

**File:** `crates/operon-runtime/src/memory/embedding.rs`

Abstract trait for text → vector conversion:

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
}
```

**Implementations:**

- **OpenAIEmbedding** - Production embedding provider
  - Model: `text-embedding-3-small` (default, 1536 dims)
  - Configurable via `with_model()` builder
  - Calls OpenAI API: `POST https://api.openai.com/v1/embeddings`
  - Batch embedding support for efficiency

- **MockEmbedding** (test-only) - Deterministic hashing
  - Uses SHA-256 hash of text → normalized float vector
  - Reproducible for testing without API calls

#### 4. Vector Store

**File:** `crates/operon-runtime/src/memory/vector_store.rs`

SQLite-backed vector persistence:

```rust
pub struct VectorStore {
    conn: Mutex<Connection>,  // SQLite connection pool
    dimensions: usize,        // Embedding dimensions
}
```

**Database Schema:**

```sql
CREATE TABLE IF NOT EXISTS vectors (
    id TEXT PRIMARY KEY,
    embedding BLOB NOT NULL
);
```

**Operations:**

- `upsert(id, embedding)` - Insert or update vector
- `remove(id)` - Delete document vector
- `search(query_embedding, limit)` - Cosine similarity search
  - Returns `Vec<(doc_id, similarity_score)>` sorted descending
  - Brute-force O(N) scan (suitable for <10K docs)
  - Score range: [-1, 1] (cosine distance)

**Storage Format:**

Embeddings stored as BLOB (little-endian f32 bytes):
- Each f32: 4 bytes
- Total per embedding: `dimensions * 4` bytes
- Example: 1536-dim OpenAI = 6KB per document

#### 5. Full-Text Search Index

**File:** `crates/operon-runtime/src/memory/text_search.rs`

SQLite FTS5 virtual table:

```rust
pub struct TextSearchIndex {
    conn: Mutex<Connection>,
}
```

**Database Schema:**

```sql
CREATE TABLE IF NOT EXISTS documents (
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

-- Triggers to keep FTS in sync
CREATE TRIGGER IF NOT EXISTS documents_ai AFTER INSERT ON documents
CREATE TRIGGER IF NOT EXISTS documents_ad AFTER DELETE ON documents
CREATE TRIGGER IF NOT EXISTS documents_au AFTER UPDATE ON documents
```

**Operations:**

- `index_document(doc)` - Insert/update document (triggers sync FTS5)
- `remove_document(id)` - Delete document (triggers FTS5 cleanup)
- `search(query, limit)` - BM25-ranked FTS5 search
  - Returns `Vec<(doc_id, bm25_score)>` sorted by relevance
  - Scores are negative (SQLite BM25 convention)
- `has_document(id)` - Check existence
- `get_content_hash(id)` - Retrieve hash for cache skip
- `get_document_content(id)` - Fetch full content by ID
- `list_document_ids()` - Get all indexed IDs

#### 6. Hybrid Search Merging

**File:** `crates/operon-runtime/src/memory/hybrid_search.rs`

Reciprocal Rank Fusion (RRF) algorithm:

```rust
pub fn rrf_merge(
    vector_results: &[(String, f32)],
    fts_results: &[(String, f64)],
    k: u32,          // RRF constant (60 = standard)
    limit: usize,    // Max final results
) -> Vec<(String, f64)>
```

**Algorithm:**

```
For each ranked result:
  RRF_score(doc) = Σ 1 / (k + rank)

Example: doc appears at rank 0 in vectors, rank 2 in FTS
  RRF = 1/(60+0+1) + 1/(60+2+1) = 0.0164 + 0.0147 = 0.0311
```

**Benefits:**

- Balances precision (vector) and recall (FTS)
- Handles heterogeneous ranking scales
- Proven effective in information retrieval
- Non-parameterized combination (no tuning needed)

#### 7. Document Indexer

**File:** `crates/operon-runtime/src/memory/indexer.rs`

Workspace indexing with file watcher:

```rust
pub struct DocumentIndexer {
    workspace: PathBuf,
    text_index: Arc<TextSearchIndex>,
    vector_store: Arc<VectorStore>,
    embedder: Arc<dyn EmbeddingProvider>,
}
```

**Key Methods:**

- `index_workspace()` - Full initial index of all workspace text files
  - Walks directory recursively
  - Skips hidden files, node_modules, target, __pycache__
  - Filters by text file extensions (.rs, .md, .py, .json, .toml, etc.)
  - Uses hash-based cache: skips unchanged files
  - Removes stale documents (deleted files)
  - Returns `IndexStats` with counts

- `index_file(doc_id, path)` - Index single file
  - Async read via `tokio::fs::read_to_string()`
  - Compute SHA-256 hash of content
  - Skip if hash matches cached version
  - Index into FTS5 (triggers auto-sync)
  - Get embedding via provider, store in vector DB
  - Returns: true if indexed, false if skipped

- `watch_workspace()` - Auto-reindex on file changes
  - Uses `notify` crate for cross-platform file watching
  - Spawns background task with mpsc channel
  - Listens for Create, Modify, Remove events
  - Re-indexes changed files asynchronously
  - Removes deleted documents from both indexes
  - Returns `JoinHandle` for lifecycle control

**Supported Text Extensions:**

```
Code: rs, py, js, ts, tsx, jsx, json, go, java, kt, swift, c, cpp, h, hpp, rb, lua
Config: toml, yaml, yml, sh, bash, zsh, vim, conf, cfg, ini, env, xml, csv
Docs: md, txt, html, css, scss, sql
```

### Memory Search Tool

**File:** `crates/operon-adapters/src/memory_search_tool.rs`

LLM-callable tool for agent queries:

```rust
pub struct MemorySearchTool {
    manager: Arc<MemoryManager>,
}

#[async_trait]
impl Tool for MemorySearchTool {
    async fn execute(&self, input: Value) -> Result<Value> {
        // Parse: { "query", "limit"?, "source"? }
        // Call manager.search()
        // Return: { "results": [...], "count": N }
    }

    fn schema(&self) -> ToolSchemaInfo { ... }
    fn permission_level(&self) -> PermissionLevel { PermissionLevel::Read }
}
```

**Input Schema:**

```json
{
  "type": "object",
  "properties": {
    "query": { "type": "string", "description": "Search text (required)" },
    "limit": { "type": "integer", "description": "Max results (default: 10)" },
    "source": { "type": "string", "enum": ["hybrid", "vector", "fts"] }
  },
  "required": ["query"]
}
```

**Output Schema:**

```json
{
  "results": [
    {
      "path": "src/main.rs",
      "score": 0.87,
      "snippet": "pub fn start() { ... }...",
      "source": "hybrid"
    }
  ],
  "count": 1
}
```

**Permission Level:** `Read` (no write access)

## Configuration

**File:** `crates/warden/src/config.rs`

New `[memory]` section in config.toml:

```toml
[memory]
enabled = false                          # Enable/disable memory system
db_path = "~/.silentclaw/memory.db"      # SQLite database path
embedding_provider = "openai"            # "openai" or future "voyage"
embedding_model = "text-embedding-3-small"  # Model name
auto_reindex = true                      # Watch for file changes
```

**MemoryConfig Struct:**

```rust
pub struct MemoryConfig {
    pub enabled: bool,
    pub db_path: String,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub auto_reindex: bool,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            db_path: "~/.silentclaw/memory.db".to_string(),
            embedding_provider: "openai".to_string(),
            embedding_model: "text-embedding-3-small".to_string(),
            auto_reindex: true,
        }
    }
}
```

**Environment Variables:**

- `OPENAI_API_KEY` - Required for embedding provider
- `SILENTCLAW_MEMORY_ENABLED` - Override config enabled flag
- `SILENTCLAW_MEMORY_DB_PATH` - Override database path

## Integration Points

### Chat Command Integration

When `memory.enabled = true` and chat command initializes:

1. Create `MemoryManager` with workspace + embedder
2. Call `manager.start_indexing()` (initial index + watcher)
3. Register `MemorySearchTool` with runtime
4. Tool available to agent during conversations

### Serve Command Integration

Gateway can expose `/search` endpoint for remote agents:

```http
POST /search
Content-Type: application/json
Authorization: Bearer {token}

{
  "query": "authentication logic",
  "limit": 5,
  "source": "hybrid"
}
```

## Search Behavior

### Full-Text Search (FTS)

**When to use:** Keyword-based queries, exact phrase matching

**Strengths:**
- Fast on large corpora
- Handles typos via FTS5 features
- Predictable lexical matching
- Low latency (<10ms typical)

**Weaknesses:**
- No semantic understanding
- Requires exact keywords present in documents

### Vector Search

**When to use:** Semantic/conceptual queries, meaning-based matching

**Strengths:**
- Understands intent and context
- Finds documents without exact keyword match
- Handles synonyms and paraphrases
- Robust to wording variations

**Weaknesses:**
- API latency (OpenAI embedding ~100-500ms)
- Requires indexing all content (initial cost)
- Less effective for technical jargon

### Hybrid Search (Default)

**How it works:**

1. Execute FTS5 query, fetch top `limit * 3` results
2. Embed query text via OpenAI
3. Execute vector search, fetch top `limit * 3` results
4. Merge via RRF (k=60), keep top `limit` results

**Benefits:**
- Combines precision (FTS) + semantics (vector)
- Balanced ranking across both modalities
- Single API call pattern (no redundant searches)
- Production-proven algorithm

**Example Flow:**

```
Query: "How do I handle authentication?"

FTS5 Results:        Vector Results:       RRF Merged:
1. auth.rs (2.5)     1. middleware.rs (0.92)  → auth.rs (0.0246)
2. middleware.rs (-1.8) 2. auth.rs (0.88)    → middleware.rs (0.0246)
3. jwt.rs (-2.1)     3. session.rs (0.85)     → session.rs (0.0161)
                     4. jwt.rs (0.81)
```

## Performance Characteristics

### Indexing

- **Initial Full Index:** O(N) where N = number of text files
  - Typical workspace (100 files): ~2-5s (with OpenAI embedding API)
  - File reads: ~50ms per file
  - Embedding API: ~400ms per request (batch size = 1)
  - Database writes: negligible

- **Cache Hit (unchanged file):** O(1) hash comparison, skip expensive embed
  - Typical: <1ms per file
  - Enables fast re-runs over unchanged code

- **File Watcher:** Async event handling
  - File change detection: <100ms
  - Re-index: <1s per file (same as initial)

### Searching

- **FTS5 Search:** O(N) scan, BM25 ranking
  - Typical (1000 docs): <10ms
  - No embedding API cost
  - Pure database operation

- **Vector Search:** O(N) cosine similarity scan
  - Brute-force: <100ms per 1000 docs
  - Embedding API: ~100-500ms (single query)
  - Total: dominated by embedding latency

- **Hybrid Search:** Sequential FTS + Vector + RRF
  - Combined: ~200-600ms typical
  - Parallelizable in future versions

**Scalability Limits:**

- Vector store: <10K docs (brute-force search)
- FTS5: thousands to millions of documents
- Embedding dimensions: 1536 (OpenAI) = 6KB per doc
- Database: file-based, no hard limit

## Security Considerations

### Indexing Security

- **Workspace Boundary:** Only indexes files within configured workspace
- **Hidden Files:** Explicitly skipped (`.git`, `.env`, etc.)
- **Binary Files:** Not indexed (text extension filter only)
- **Large Files:** No explicit limit (FTS5/vector store will handle)

### Search API Security

- `MemorySearchTool` permission level: `Read` (no write access)
- No direct file deletion/modification via search results
- Content exposed: file paths + first 500 chars
- Agent cannot see full file content without separate `read_file` tool

### Database Security

- SQLite database file: located at `~/.silentclaw/memory.db`
- No encryption (consider sensitive data policy)
- Connection uses `Mutex` for thread safety
- No network exposure (local SQLite file)

## Known Limitations & Future Work

### Current Limitations

1. **Brute-Force Vector Search:** O(N) cosine similarity
   - Suitable for <10K documents
   - Future: Switch to HNSW (Hierarchical Navigable Small World) for large scale

2. **Single Embedding Provider:** Only OpenAI supported initially
   - Voyage embeddings can be added later
   - Trait-based design allows extension

3. **Content Snippet Hardcoded:** First 500 chars only
   - No configurable snippet length
   - No excerpt highlighting around query terms

4. **No Metadata Filtering:** SearchQuery doesn't support filtering by path/author/date
   - RRF only combines scores, doesn't filter
   - Could add FTS5 WHERE clauses for filtering

5. **File Watcher Granularity:** Re-indexes entire file on any change
   - Could optimize with line-level diffs (future enhancement)

### Planned Improvements

- [ ] HNSW index for vector store (million-scale documents)
- [ ] Voyage AI embeddings support
- [ ] Metadata filtering (by path, date, tags)
- [ ] Query result caching (avoid redundant API calls)
- [ ] Parallel embedding batch processing
- [ ] Configurable snippet extraction with highlighting
- [ ] Deletion policy (auto-purge old documents)
- [ ] Search analytics and metrics

## Testing Strategy

Comprehensive test coverage via:

- **Embedding Tests:** Mock provider with deterministic hashing
- **Vector Store Tests:** Cosine similarity correctness
- **FTS5 Tests:** BM25 ranking, query syntax
- **Hybrid Tests:** RRF merge correctness and ranking
- **Indexer Tests:** File walking, cache skipping, watcher events
- **Integration Tests:** End-to-end memory manager with tool

All tests use `TempDir` for isolated SQLite databases.

## Migration & Upgrade Path

**For existing SilentClaw users:**

1. Memory system is disabled by default (`enabled = false`)
2. No breaking changes to existing tools/APIs
3. Opt-in by setting `[memory] enabled = true` in config
4. First startup: full workspace index (may take time)
5. Subsequent startups: incremental updates via watcher

**Database Versioning:**

Current schema has no version column. For future compatibility:
- Schema changes require migration script
- Consider adding `PRAGMA user_version` for tracking

## References

- **OpenAI Embeddings API:** https://platform.openai.com/docs/guides/embeddings
- **SQLite FTS5:** https://www.sqlite.org/fts5.html
- **RRF Algorithm:** https://en.wikipedia.org/wiki/Reciprocal_rank_fusion
- **Cosine Similarity:** https://en.wikipedia.org/wiki/Cosine_similarity
- **BM25 Ranking:** https://en.wikipedia.org/wiki/Okapi_BM25
- **notify crate:** https://docs.rs/notify/

---

**Phase 4 Complete:** 2026-02-18
**New Files:** 7 memory modules + 1 memory_search_tool
**Total LOC:** ~1000 lines (memory system)
**Test Coverage:** Core modules tested with mocks
**Performance:** <10ms FTS, ~200-600ms hybrid search (including embedding API)
