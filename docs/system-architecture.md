# SilentClaw System Architecture

**Last Updated:** 2026-02-18
**Version:** 5.1.0-phase-6
**Status:** Phase 6 Complete - Code Review Fixes (Patterns, Defaults, DRY)

## Overview

SilentClaw is a comprehensive agent platform combining:
- **LLM Integration** - 3 providers (Anthropic/OpenAI/Gemini) with streaming (SSE) and failover chains
- **Agent Loop** - Conversation state + tool calling + auto-iteration
- **Streaming** - Native SSE parsers with 1MB buffer protection
- **Config Hot-Reload** - File watcher for live updates without restart
- **Event Hooks** - DashMap-based event system for extensibility
- **Plugin System** - Dynamic tool/hook loading with API versioning
- **Gateway** - HTTP/WebSocket API for remote access
- **Async Runtime** - Tokio-based execution engine
- **Tool Security** - 7-layer policy pipeline (authorization, validation, rate-limiting, audit)
- **Memory System** - Hybrid search (vector + FTS5) for workspace indexing

Architecture evolution:

```
v1.0: plan → runtime → tools → result
v2.0: user → agent → LLM → tool → observe → feedback (loop) → response
      + plugins, hooks, gateway, persistence
v2.1: + streaming (SSE) + config hot-reload (Phase 1)
v2.2: + plugin FFI + gateway integration tests (Phase 2)
v2.3: + filesystem tools (read/write/edit/patch) with workspace scoping (Phase 3)
v4.0: + memory system (hybrid search: vector + FTS5, RRF merge) (Phase 4)
v5.0: + Gemini LLM provider + 7-layer tool policy pipeline (Phase 5)
```

## Architecture Layers (10 Layers)

### Layer 1: LLM Provider Integration (Production Hardened + Phase 1 Streaming)

**Purpose:** Unified interface to multiple LLM providers with streaming, failover, and timeout hardening

**Provider Trait:**
```rust
#[async_trait]
pub trait LLMProvider: Send + Sync {
    async fn generate(&self, messages: &[Message], tools: &[ToolSchema], config: &GenerateConfig) -> Result<GenerateResponse>;

    async fn generate_stream(&self, messages: &[Message], tools: &[ToolSchema], config: &GenerateConfig) -> Result<Receiver<StreamChunk>>;

    fn supports_vision(&self) -> bool;
    fn model_name(&self) -> &str;
}
```

**Implementations:**
- **AnthropicClient** - Anthropic API with native SSE streaming
  - Supports `tool_use` block content type
  - Vision/multimodal base64 encoding
  - HTTP timeouts: 120s request, 10s connect
  - Native streaming via `generate_stream()` override

- **OpenAIClient** - OpenAI API (GPT-4/3.5) with vision and streaming
  - Function calling format
  - Vision/multimodal base64 encoding
  - Native streaming via `generate_stream()` override

- **GeminiClient** (NEW - Phase 5) - Google Gemini API with SSE streaming
  - Function declarations format (similar to OpenAI)
  - Vision via inlineData base64 encoding
  - HTTP timeouts: 120s request, 10s connect
  - Auth: API key as query parameter `?key=API_KEY`
  - Models: gemini-2.0-flash (default), gemini-2.5-pro
  - Base URL: `https://generativelanguage.googleapis.com/v1beta`
  - Endpoints: `:generateContent` (non-streaming), `:streamGenerateContent?alt=sse` (streaming)
  - Native streaming via `generate_stream()` override using `parse_gemini_sse()`

- **ProviderChain (Failover)** - Fallback logic with retry-after
  - Configurable list of providers
  - Exponential backoff on failure
  - Retry-After header parsing
  - Seamless provider switching

**Phase 1: Streaming Module** (`streaming.rs`)

New module for SSE parsing:

```rust
pub enum StreamChunk {
    TextDelta(String),
    ToolCallStart { id: String, name: String },
    ToolCallDelta { id: String, input_delta: String },
    Done { stop_reason: StopReason, usage: Usage },
}

pub fn parse_anthropic_sse(data: &str) -> Option<StreamChunk>
pub fn parse_openai_sse(data: &str) -> Vec<StreamChunk>
```

**Anthropic Streaming Events:**
- `content_block_start` (type=tool_use) → ToolCallStart
- `content_block_delta` (type=text_delta) → TextDelta
- `content_block_delta` (type=input_json_delta) → ToolCallDelta
- `message_delta` → Done with stop_reason
- Unknown events → filtered out

**OpenAI Streaming Events:**
- `[DONE]` → Done signal
- `choices[].delta.content` → TextDelta
- `choices[].delta.tool_calls[].id` → ToolCallStart
- `choices[].delta.tool_calls[].function.arguments` → ToolCallDelta
- `choices[].finish_reason` → Done with stop_reason

**Gemini Streaming Events (NEW - Phase 5):**
- `candidates[].content.parts[].text` → TextDelta
- `candidates[].content.parts[].functionCall` → ToolCallStart + ToolCallDelta
- `candidates[].finishReason` → Done with stop_reason mapping:
  - `"STOP"` → `StopReason::EndTurn`
  - `"MAX_TOKENS"` → `StopReason::MaxTokens`
  - `"TOOL_USE"` or contains functionCall → `StopReason::ToolUse`
- `usageMetadata` → Usage (promptTokenCount + candidatesTokenCount)

**Features:**
- Async request/response
- Structured message format with roles
- Tool schema inference from tool registry
- Stop reason tracking (end_turn, tool_use, max_tokens)
- Vision/multimodal content support
- Cumulative usage tracking
- Streaming with 1MB buffer limit (OOM protection)
- Default fallback for non-streaming providers
- 3 provider failover chain (Anthropic → OpenAI → Gemini)

**Tests:** 78 total (22 streaming: 6 Anthropic + 6 OpenAI + 5 Gemini + 5 shared tests, 12 provider fallback, 14 Anthropic, 12 OpenAI, 5 Gemini, 5 failover, 8 others)

### Layer 2: Agent Loop (Production Hardened)

**Purpose:** Conversation agent with LLM + tool orchestration and token tracking

**Agent Struct:**
```rust
pub struct Agent {
    config: AgentConfig,
    runtime: Arc<Runtime>,
    llm: Arc<dyn LLMProvider>,
    session_store: SessionStore,
}
```

**AgentConfig:**
- System prompt
- Max iterations per user turn (safety)
- Temperature, max_tokens
- Tool allowlist (empty = all)
- Model override

**Session Management:**
- Unique session ID (UUID v4)
- Message history (immutable log)
- Timestamps (created_at, updated_at)
- Metadata map for extensibility
- Persistence: JSON files per session
- Cumulative token tracking (in Session)
- Context overflow warning at 80% threshold

**Agent Loop:**
1. User submits message
2. Add to session history
3. Call LLM with tool schemas (uses streaming if available)
4. Parse StreamChunks from streaming response
5. Accumulate tool calls from ToolCallDelta chunks
6. Execute tools via runtime
7. Append results to history
8. Loop until stop reason = end_turn
9. Return final message

### Layer 3: Streaming & Response Accumulation (NEW - Phase 1)

**Purpose:** Parse and accumulate streaming LLM responses into structured tool calls

**Flow:**
```
LLM sends SSE stream
    ↓
receive StreamChunk
    ├─ TextDelta → accumulate into response text
    ├─ ToolCallStart { id, name } → create new tool call context
    ├─ ToolCallDelta { id, input_delta } → append to current tool's input_json
    └─ Done { stop_reason, usage } → finalize response
    ↓
Parse accumulated input_json into structured ToolCall
    ↓
Return GenerateResponse with all tool calls
```

**Buffer Protection:**
- Track accumulated bytes across streaming response
- Max 1MB per response
- Error if limit exceeded
- Prevents runaway generator attacks

**Fallback Path:**
- If provider doesn't support streaming
- Wrap non-streaming `generate()` call as single-shot stream
- Emit text, then tool calls, then Done

### Layer 4: Config Hot-Reload (NEW - Phase 1)

**Purpose:** Live configuration updates without restart

**ConfigManager<C> Struct:**
```rust
pub struct ConfigManager<C: DeserializeOwned + Send + Sync + 'static> {
    config: Arc<RwLock<C>>,
    config_path: PathBuf,
    reload_tx: broadcast::Sender<ConfigReloadEvent>,
}

pub enum ConfigReloadEvent {
    Success,
    Failure(String),
}
```

**File Watching:**
- Uses `notify-debouncer-mini` crate
- Monitors config file for changes
- 500ms debounce to avoid thrashing
- Spawned in blocking task (not blocking async runtime)

**Reload Flow:**
1. User saves config file
2. Debouncer waits 500ms (no other changes)
3. ConfigManager::watch() detects change
4. Load and parse TOML
5. Update Arc<RwLock<C>> atomically
6. Broadcast event to all subscribers
7. Subscribers re-read config on next request

**Integration with Commands:**
- `chat.rs` - watches config, can reload agent settings mid-session
- `serve.rs` - watches config, can reload gateway settings on the fly
- Both use broadcast channel to detect reload events

**Features:**
- Generic over any config type (C: DeserializeOwned)
- Atomic updates via Arc<RwLock<C>>
- Broadcast channel for event notification
- Non-blocking async operations

### Layer 5: Event Hooks (Production Hardened)

**Purpose:** Event-driven extensibility with security permissions and per-hook timeout

**Hook Registry (DashMap):**
```rust
pub struct HookRegistry {
    hooks: DashMap<HookEvent, Vec<Arc<dyn Hook>>>,
}
```

**Hook Types (Events):**
- `BeforeToolCall` - Before tool execution
- `AfterToolCall` - After tool execution
- `BeforeStep` - Before step execution
- `AfterStep` - After step execution
- `MessageReceived` - New user message
- `ResponseGenerated` - LLM response ready
- `SessionCreated` / `SessionClosed`
- Custom plugin hooks

**Hook Interface (Hardened):**
```rust
#[async_trait]
pub trait Hook: Send + Sync {
    async fn handle(&self, context: HookContext) -> Result<HookResult>;
    fn timeout(&self) -> Duration;  // Per-hook timeout
    fn critical(&self) -> bool;     // Abort on failure
}

pub enum PermissionLevel {
    Read,      // View data only
    Write,     // Modify session/config
    Execute,   // Run tools
    Network,   // Make HTTP requests
    Admin,     // Full access
}
```

**Features:**
- Per-hook timeout (replaces global 5s constant)
- Critical hook support (abort on failure)
- Permission level enforcement

Use cases:
- Audit logging
- Metrics collection (prometheus)
- Custom validation/filtering
- Rate limiting
- Caching strategy

### Layer 6: Gemini Provider Streaming Enhancements (Phase 6)

**Tool Call ID Generation (Phase 6 Improvement):**

Added AtomicU64-based counter for globally unique tool call IDs:

```rust
static CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn next_call_id(name: &str) -> String {
    let n = CALL_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("gemini_{}_{}", name, n)
}
```

**Benefits:**
- Prevents tool call ID collisions in streaming responses
- Works across concurrent streaming requests
- Minimal contention (atomic, no locks)
- Unique within runtime session

**Integration with Streaming:**
- `parse_gemini_sse()` calls `gemini::next_call_id()` for each tool call
- Consistent ID format across all Gemini responses
- Enables reliable tool result matching

**Response Validation (Phase 6 DRY):**

Added `check_response()` helper to reduce duplication:

```rust
fn check_response(response: Response) -> Result<String> {
    if response.status().is_success() {
        response.text().await
    } else {
        Err(response.error_for_status()?)
    }
}
```

Applied in both streaming and non-streaming paths.

**Structured Logging (Phase 6):**
- `debug!()` for SSE parsing steps
- `info!()` for API requests and responses
- Fields: model, tool names, stream flag

---

### Layer 7: Plugin System with FFI (Enhanced - Phase 2)

**Purpose:** Dynamic native plugin loading with safe FFI boundary

**Plugin FFI Bridge (NEW - Phase 2):**

Uses `libloading` crate for runtime .so/.dylib loading with panic safety:

```rust
pub struct PluginHandle {
    _library: Library,      // Shared library (.so/.dylib)
    plugin: Box<dyn Plugin>, // Loaded plugin trait object
}

impl PluginHandle {
    pub fn load(path: &Path) -> Result<Self>
    pub fn plugin(&self) -> &dyn Plugin
    pub fn shutdown_and_drop(self) -> ()
}
```

**Double-Boxing Pattern:**
- Plugin author: `Box::new(MyPlugin) → Box::new(Box::new(MyPlugin))`
- Generated by `declare_plugin!` macro
- Serialized as thin `*mut c_void` pointer
- Deserialized: `*mut c_void → Box<Box<dyn Plugin>> → Box<dyn Plugin>`
- Avoids fat pointer FFI boundary issues

**Panic Isolation:**
- `catch_unwind` wraps `_plugin_create()` call
- `catch_unwind` wraps `plugin.shutdown()` call
- Plugin panics logged, not propagated

**Plugin Discovery:**
- Scan plugin directories for `plugin.toml`
- Parse manifest: name, version, api_version
- Load plugin library: `libplugin_name.so` / `.dylib` via `PluginHandle`

**Plugin Manifest (TOML):**
```toml
[plugin]
name = "custom-tools"
version = "0.1.0"
api_version = 1
description = "Custom tools for workflows"

[plugin.config]  # Optional: passed to Plugin::init()
setting1 = "value"
timeout_ms = 5000

[[tools]]
name = "analyzer"
description = "Analyzes data"

[[hooks]]
name = "audit_logger"
```

**Plugin Loader:**
```rust
pub struct PluginLoader {
    plugins: HashMap<String, Arc<dyn Plugin>>,
}

impl PluginLoader {
    pub async fn load(&mut self, path: &Path) -> Result<()>;
    pub async fn unload(&mut self, name: &str) -> Result<()>;
}
```

**Version Checking:**
- Plugin API version must == SDK API_VERSION (u32)
- Prevents ABI incompatibility
- Runtime error if mismatch detected

**Plugin Trait (MOVED - Phase 2):**

Defined in `operon-runtime::plugin::plugin_trait` (no circular deps):

```rust
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn api_version(&self) -> u32;
    fn init(&mut self, config: Value) -> Result<()>;  // config from manifest [plugin.config]
    fn shutdown(&mut self) -> Result<()>;
    fn tools(&self) -> Vec<Box<dyn Tool>>;
    fn hooks(&self) -> Vec<Box<dyn Hook>>;
}
```

Re-exported by `operon-plugin-sdk` for plugin authors.

**FFI Safety Documentation:**
- `PluginHandle` uses double-boxing pattern to avoid fat pointer issues at FFI boundary
- `Library` field dropped AFTER plugin (Rust drop order guarantee)
- `shutdown_and_drop()` wraps `plugin.shutdown()` with `catch_unwind` for panic isolation
- Comprehensive safety comments on all unsafe blocks

### Layer 7: Gateway Server (Production Hardened + Phase 2 Tests)

**Purpose:** HTTP/WebSocket API for remote agent access with full integration test coverage

**Server (Axum):**
```rust
pub async fn start_server(
    host: &str,
    port: u16,
    state: AppState,
) -> Result<()>
```

**Security Features:**
- Bearer token authentication (AuthConfig, auth_middleware)
- Token bucket rate limiter (RateLimiter with DashMap)
- CORS origins configuration (permissive default)
- Input validation: 50KB limit (REST + WebSocket)
- 10s graceful shutdown drain
- 5-min WebSocket idle timeout

**Phase 2 Test Coverage (20 integration tests):**
- **Health & Sessions (8 tests):** Health endpoint, session CRUD, message sending, payload limits
- **Auth & Rate Limiting (8 tests):** Constant-time token comparison, rate limiter bucket algorithm, concurrent limits
- **WebSocket (4 tests):** Upgrade handling, event broadcast, subscription management
- **Test Infrastructure:** TempDir-backed test databases for auto-cleanup, helper utilities for stateful/stateless patterns

**HTTP Routes:**
- `GET /health` - Liveness check
- `POST /sessions` - Create new session (auth required)
- `GET /sessions/{id}` - Get session state (auth required)
- `DELETE /sessions/{id}` - Close session (auth required)

**WebSocket Endpoint:**
- `WS /ws/{id}` - Real-time agent communication
  - 5-min idle timeout
  - Bearer token validation
  - 50KB message size limit
- Upgrade HTTP connection to WebSocket
- Broadcast channels for multi-client sync
- Auto-reconnect support

**Session Manager:**
- Tracks active sessions (HashMap)
- `send_message()` uses remove/insert pattern: removes session (short write lock) → processes LLM call lock-free → re-inserts session (short write lock)
- Persists to JSON on disk
- Cleanup on disconnect (timeout)
- Concurrent session support

**Message Format:**
```json
// Client → Server (user message)
{
  "type": "message",
  "content": "What's the weather?"
}

// Server → Client (agent response)
{
  "type": "response",
  "content": "I'll check the weather for you.",
  "session_id": "uuid",
  "message_id": "uuid"
}
```

**Auth Middleware:**
- Validates `Authorization: Bearer <token>` header via `subtle` crate constant-time comparison
- Prevents timing attack side-channels
- Configurable token validation logic
- Returns 401 Unauthorized on invalid token

**Rate Limiter:**
- Token bucket algorithm per client
- Configurable tokens/second
- Wired into Axum router middleware (now active)
- Returns 429 Too Many Requests when limited

### Layer 8: Tool Policy Pipeline (Phase 6 Enhanced Defaults)

**Purpose:** 7-layer authorization/validation pipeline executed before every tool.execute() call

**ToolPolicyPipeline:**
```rust
pub struct ToolPolicyPipeline {
    layers: Vec<Box<dyn PolicyLayer>>,
}

pub enum PolicyDecision {
    Allow,
    Deny(String),
}

pub struct PolicyContext {
    pub tool_name: String,
    pub input: Value,
    pub caller_permission: PermissionLevel,
    pub dry_run: bool,
    pub session_id: Option<String>,
}
```

**Pipeline Flow:**
```
Agent calls runtime.execute_tool(name, input)
    ↓
ToolPolicyPipeline::evaluate(context)
    ├── Layer 1: ToolExistence     - Is the tool registered?
    ├── Layer 2: PermissionCheck   - Does caller have required permission level?
    ├── Layer 3: RateLimit         - Per-tool call rate within window?
    ├── Layer 4: InputValidation   - Does input match tool's JSON schema?
    ├── Layer 5: DryRunGuard       - Is tool allowed in current execution mode?
    ├── Layer 6: AuditLog          - Log tool call attempt (always Allow)
    └── Layer 7: TimeoutEnforce    - Set per-tool timeout wrapper
    ↓
tool.execute(input)  // Only if all layers Allow
```

**Layer Details:**

1. **ToolExistence** - Validates tool is registered in runtime
   - Deny: "tool not found: {name}"

2. **PermissionCheck** - Hierarchical permission enforcement (Phase 6: safer default)
   - Hierarchy: Read < Write < Execute < Network < Admin
   - Default changed from Execute to Read (safer default, Phase 6)
   - Deny: "insufficient permission for tool {name}"
   - Parameter: `default_permission` passed to layer constructor

3. **RateLimit** - Token bucket per tool
   - DashMap-based concurrent tracking
   - Configurable max_calls_per_minute
   - Deny: "rate limit exceeded for tool {name}"

4. **InputValidation** - Schema validation
   - Checks required fields present
   - Type matching against tool schema
   - Deny: "invalid input for tool {name}: {reason}"

5. **DryRunGuard** - Execution mode safety
   - If dry_run=true, only allows Read permission tools
   - Configurable bypass list (e.g., read_file, memory_search)
   - Deny: "tool {name} blocked in dry-run mode"

6. **AuditLog** - Audit trail (always Allow, side-effect only)
   - Structured logging via tracing::info!
   - Fields: tool_name, session_id, timestamp, permission_level

7. **TimeoutEnforce** - Timeout enforcement (always Allow)
   - Sets per-tool timeout metadata
   - Consumed by runtime during execution

**Configuration (Phase 6: safer defaults):**
```toml
[tool_policy]
enabled = true

[tool_policy.permission]
enabled = true
default_level = "read"  # Phase 6: Changed from "execute" to "read" for safety

[tool_policy.rate_limit]
enabled = false
max_calls_per_minute = 60

[tool_policy.input_validation]
enabled = true

[tool_policy.dry_run_guard]
enabled = true
bypass_tools = ["read_file", "memory_search"]

[tool_policy.audit]
enabled = true
```

**Features:**
- Short-circuit on first Deny (fail-fast)
- Zero overhead when disabled (runtime bool checks)
- Extensible: custom layers via PolicyLayer trait
- DashMap for lock-free concurrent rate limiting
- Per-layer enable/disable configuration
- Clear error messages with layer name + reason

**Tests:** 12 tests (3 pipeline orchestration + 9 individual layer tests)

### Layer 8: Core Runtime Execution Order (Phase 6 Improved)

**Tool Execution Flow (Phase 6 - Reordered for Correctness):**

```
Agent calls runtime.execute_tool(name, input)
    ↓
[Phase 6] Check if dry_run=true FIRST (before policy evaluation)
    ├─ If dry_run: skip execution, don't evaluate policy
    └─ If execute: proceed to policy pipeline
    ↓
ToolPolicyPipeline::evaluate(context) [only if not dry-run]
    ├── Layer 1: ToolExistence
    ├── Layer 2: PermissionCheck (with safe default "read")
    ├── Layer 3: RateLimit
    ├── Layer 4: InputValidation
    ├── Layer 5: DryRunGuard (redundant now, but kept for defense-in-depth)
    ├── Layer 6: AuditLog
    └── Layer 7: TimeoutEnforce
    ↓
tool.execute(input)
```

**Why This Matters (Phase 6):**
- Dry-run skip prevents rate-limit counter inflation
- Tools never penalized for skipped executions
- Policy context reflects actual execution state

---

### Layer 9: Core Runtime (operon-runtime)

**Purpose:** Async Tool trait and orchestration engine with policy integration

**Key Components:**

- **Tool Trait** - Generic async interface for any tool type
  ```rust
  #[async_trait]
  pub trait Tool {
      async fn execute(&self, input: Value) -> Result<Value>;
      fn name(&self) -> &str;
  }
  ```

- **Runtime Struct** - Plan executor with async step orchestration and policy integration
  - Tool registry (DashMap for lock-free concurrency)
  - Per-tool timeout configuration
  - Dry-run flag for safety
  - JSON structured logging via tracing
  - Optional ToolPolicyPipeline (Phase 5) - evaluated before every tool.execute()
  - Policy context includes tool_name, input, permission level, dry_run, session_id

- **Storage Module** - Persistent step result storage
  - Uses redb (actively maintained, pure Rust)
  - Memory-mapped I/O for efficiency
  - Transaction-based result storage

**Design Decisions:**
- **DashMap over Mutex:** Lock-free concurrent hashmap prevents contention
- **async_trait for tool/hook:** Enables async implementations without dyn complexity
- **Broadcast channels:** Pub/sub for gateway multi-client updates
- **File watchers (notify-debouncer-mini):** Live config reloading without restart
- **Atomic types for scheduling:** Lock-free task queue coordination
- **No unsafe blocks:** Type safety throughout
- **Dry-run default:** Configuration enables user choice

### Layer 10: Memory & Search System (Phase 4)

**Purpose:** Workspace file indexing and semantic/full-text search for agent memory

**MemoryManager (Orchestrator):**
```rust
pub struct MemoryManager {
    text_index: Arc<TextSearchIndex>,
    vector_store: Arc<VectorStore>,
    embedder: Arc<dyn EmbeddingProvider>,
    indexer: Arc<DocumentIndexer>,
}
```

**Key Components:**

1. **Vector Store** - SQLite vector embeddings
   - Schema: `vectors` table (id, embedding BLOB)
   - Search: O(N) cosine similarity (brute-force, <10K docs)
   - Storage: f32 arrays as little-endian bytes
   - Scalability: Suitable for workspace-scale datasets

2. **Text Search Index** - SQLite FTS5
   - Schema: `documents` table + `documents_fts` virtual table
   - Triggers: Auto-sync INSERT/UPDATE/DELETE between tables
   - Ranking: Built-in SQLite BM25 function
   - Query: Wildcard matching, phrase queries, negation

3. **Embedding Provider** - Text → Vector conversion
   - Trait: `async fn embed(text)` + `fn dimensions()`
   - Implementation: OpenAI text-embedding-3-small (1536 dims)
   - Mock: SHA-256 deterministic hashing for testing
   - Batch: `embed_batch()` for efficient multi-document processing

4. **Document Indexer** - Workspace file processing
   - Initial index: Walks workspace recursively, filters text extensions
   - File watcher: Async `notify` crate for change detection
   - Cache: SHA-256 hash skips re-embedding unchanged files
   - Auto-reindex: Spawned background task handles file events
   - Cleanup: Removes stale documents when files deleted

5. **Hybrid Search** - RRF merging
   - Algorithm: Reciprocal Rank Fusion (k=60)
   - Formula: RRF_score(doc) = Σ 1/(k + rank)
   - Combines: Vector (cosine) + FTS5 (BM25) rankings
   - Non-parameterized: No tuning required

**Search Modes:**

- **Vector Search:** Semantic/conceptual matching
  - Embedding → cosine similarity against all vectors
  - Handles paraphrases, synonyms, intent matching
  - Latency: ~100-500ms (dominated by OpenAI API)

- **Full-Text Search:** Keyword/phrase matching
  - BM25-ranked FTS5 queries
  - Fast (<10ms), lexical precision
  - Effective for code patterns, exact terms

- **Hybrid Search:** Combined ranking (default)
  - Execute both, merge via RRF
  - Balanced precision + semantics
  - Production-proven algorithm

**Search Results:**

```rust
pub struct SearchResult {
    pub document_id: String,
    pub path: String,
    pub content_snippet: String,  // First 500 chars
    pub score: f64,               // RRF score (normalized)
    pub source: SearchSource,     // Vector, FullText, or Hybrid
}
```

**File Indexing:**

Supported text extensions: .rs, .py, .js, .ts, .json, .toml, .yaml, .md, .html, .sql, .sh, etc.

Excluded: hidden files, node_modules/, target/, __pycache__/, binary files

**Integration with Agent Loop:**

1. Memory enabled in config: `[memory] enabled = true`
2. On startup: MemoryManager initializes, full workspace index
3. File watcher spawned: async re-indexing on changes
4. Tool registered: `memory_search` available to agent
5. Agent queries: calls tool with search text, gets results
6. Results: paths + snippets passed back to agent for context

**Database:**

- Location: `~/.silentclaw/memory.db` (configurable)
- Type: SQLite file-based
- Tables: documents, vectors, documents_fts (virtual)
- No network exposure (local-only)
- No encryption (consider data policy for sensitive projects)

---

### Layer 10: Memory Search Tool (operon-adapters)

**Purpose:** LLM-callable interface for workspace file search (see Layer 10 Memory System above for backend details)

### Layer 11: Tool Adapters (operon-adapters - Phase 6 Enhanced)

**Purpose:** Bridge Rust runtime with external tools and filesystem operations

#### Filesystem Tools (NEW - Phase 3, Code Review Hardened)

**Purpose:** Workspace-scoped file operations with path traversal protection and async I/O

**WorkspaceGuard (Central Security, H1: Now Async)**
```rust
pub struct WorkspaceGuard {
    root: PathBuf,
    max_file_size: u64,
}

impl WorkspaceGuard {
    pub fn resolve(&self, input_path: &str) -> Result<PathBuf>
    pub async fn is_text_file(path: &Path) -> Result<bool>
    pub async fn check_size(&self, path: &Path) -> Result<()>
}
```

Ensures all file operations stay within workspace boundary:
- Canonicalizes paths (resolves symlinks)
- Validates `canonical.starts_with(&self.root)`
- Rejects traversal attempts (e.g., `../../etc/passwd`)
- Binary detection: async read of first 8KB only (M1 optimization: was reading entire file)
- Methods now async via `tokio::fs` (H1: was blocking `std::fs`)

**Diff Parser Module (H2: Extracted, NEW)**
```rust
pub enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

pub struct Hunk {
    pub old_start: usize,
    pub lines: Vec<HunkLine>,
}

pub struct FilePatch {
    pub path: String,
    pub hunks: Vec<Hunk>,
}

pub fn parse_unified_diff(patch: &str) -> Result<Vec<FilePatch>>
```

Extracted unified diff parsing into dedicated module for reusability and single source of truth.

**read_file Tool (M2: Inline binary check):**
- Input: `{ "path": string, "offset": u64?, "limit": u64? }`
- Returns: Content with line numbers (cat -n format)
- 10MB max file size (configurable)
- Read-only permission level
- Single async read, checks binary status inline (was calling `is_text_file()` separately, causing double I/O)

**write_file Tool:**
- Input: `{ "path": string, "content": string }`
- Atomic: writes to temp file, then renames
- Creates parent directories automatically
- Returns: `{ "bytes_written": N, "path": "resolved_path" }`
- Write permission level
- Uses async `tokio::fs` for atomic operations

**edit_file Tool:**
- Input: `{ "path": string, "old_string": string, "new_string": string, "replace_all": bool? }`
- Exact string matching (like Claude Code Edit tool)
- Detects ambiguous matches (multiple occurrences without replace_all=true)
- Returns: `{ "replacements": N, "path": "resolved_path" }`
- Write permission level

**apply_patch Tool (H2: Uses diff_parser module):**
- Input: `{ "patch": string }` (unified diff format)
- Calls `diff_parser::parse_unified_diff()` for unified diff parsing
- Applies hunks with context matching
- Atomic: writes to temp file, then renames
- Returns: `{ "files_modified": N, "hunks_applied": N }`
- Write permission level

**Features:**
- All 4 tools constructed with `WorkspaceGuard` for path validation
- Async I/O prevents starving tokio runtime threads (H1)
- Atomic writes prevent partial file corruption on crash
- Binary file detection prevents corrupting non-text files (only reads 8KB, M1)
- File size limits prevent memory exhaustion
- Comprehensive error messages for path traversal, ambiguous matches, etc.

**Tool Registration (Phase 6: Simplified Signatures)**
Updated helper functions in `operon-adapters/src/lib.rs`:
```rust
pub fn register_shell_tool(
    runtime: &Runtime,  // Phase 6: Changed from &Arc<Runtime>
    dry_run: bool,
    blocklist: Vec<String>,
    allowlist: Vec<String>,
) -> Result<()>

pub fn register_filesystem_tools(
    runtime: &Runtime,  // Phase 6: Changed from &Arc<Runtime>
    workspace: PathBuf,
    max_file_size_mb: u64,
) -> Result<()>
```

**Phase 6 Improvement:**
- Simplified function signatures: `&Runtime` instead of `&Arc<Runtime>`
- No Arc dereferencing needed in callers
- More flexible for future refactoring
- Eliminates duplication in `chat.rs` and `serve.rs` (was ~20 lines each)

#### Python Adapter (PyAdapter)

**Protocol:** JSON-over-stdio

```json
// Request
{"id": 1, "method": "execute", "params": {...}}

// Response
{"id": 1, "result": {...}, "error": null}
```

**Process Model:**
- Spawns Python subprocess
- Communication via stdin/stdout pipes
- Per-request ID tracking for multiplexing
- Configurable timeout per call

**Implementation Details:**
- subprocess spawned with `python3 script_path`
- stdin/stdout piped for JSON communication
- stderr piped but requires external handler (deadlock risk)

#### Shell Tool

**Purpose:** Execute system commands safely

**Execution Model:**
- Invokes `sh -c "command"`
- Captures stdout/stderr
- Returns JSON with exit code

**Safety Features:**
- Dry-run mode logs but doesn't execute
- Explicit `--allow-tools` required for real execution
- Timeout enforcement prevents runaway processes
- Error handling for non-zero exit codes

### Layer 12: CLI Interface (warden - Phase 6 Refactored)

**Purpose:** Entry point for all SilentClaw modes with config validation

**Commands:**
1. **run-plan** - Execute plan JSON with tools
   - `--file <path>` - Plan file
   - `--record <dir>` - Save fixture for replay
   - `--replay <dir>` - Skip tool execution, use recorded results

2. **chat** - Interactive agent conversation (Phase 1: uses streaming, Phase 6: refactored Arc pattern)
   - `--agent <name>` - Agent config
   - `--session <id>` - Resume existing session
   - REPL loop: read user input → agent loop → display response
   - Phase 6: Build Runtime before Arc wrapping (safer builder pattern)

3. **serve** - Gateway HTTP/WebSocket server (Phase 1: with hot-reload)
   - `--host <addr>` - Bind address (default: 127.0.0.1)
   - `--port <num>` - Listen port (default: 8080)
   - Bearer token auth required
   - Rate limiting enabled
   - Graceful shutdown (10s drain)
   - Concurrent session management
   - Config hot-reload wiring

4. **plugin** - Manage plugins
   - `list` - Show installed plugins
   - `load <path>` - Load from directory
   - `unload <name>` - Unload by name

5. **init** - Bootstrap config file
   - Generates default config.toml
   - Includes security defaults (blocklist, version)

**Global Flags:**
- `--execution-mode {auto|dry-run|execute}` - Control tool execution
- `--config <path>` - Config file (default: ~/.silentclaw/config.toml)

**Environment Variables:**
- `SILENTCLAW_TIMEOUT` - Override timeout_secs
- `SILENTCLAW_MAX_PARALLEL` - Override max_parallel
- `SILENTCLAW_DRY_RUN` - Override dry_run
- `ANTHROPIC_API_KEY` - Anthropic API key
- `OPENAI_API_KEY` - OpenAI API key
- `GOOGLE_API_KEY` - Google Gemini API key (Phase 5)

**Configuration (Hardened):**
```toml
version = 1                    # Config version

[runtime]
dry_run = true                 # Safe default
timeout_secs = 60              # Global timeout
max_parallel = 4               # Parallel task limit
data_dir = "~/.silentclaw"     # Default: home directory

[tools.shell]
enabled = true
blocklist = ["rm -rf", "mkfs"] # Dangerous patterns
allowlist = []                 # Empty = all allowed

[tools.python]
enabled = true
scripts_dir = "./tools/python_examples"

[tools.filesystem]
enabled = true
workspace = "."                # Workspace root
max_file_size_mb = 10          # Read limit

[tools.timeouts]
shell = 30
python = 120

[llm]
provider = "anthropic"         # Options: "anthropic", "openai", "gemini" (Phase 5)
model = ""
anthropic_api_key = ""
openai_api_key = ""
gemini_api_key = ""            # NEW - Phase 5

[memory]                       # NEW - Phase 4
enabled = false                # Opt-in system
db_path = "~/.silentclaw/memory.db"
embedding_provider = "openai"  # text-embedding-3-small
embedding_model = "text-embedding-3-small"
auto_reindex = true            # Watch for file changes

[tool_policy]                  # NEW - Phase 5
enabled = true

[tool_policy.permission]
enabled = true
default_level = "execute"      # read, write, execute, network, admin

[tool_policy.rate_limit]
enabled = false
max_calls_per_minute = 60

[tool_policy.input_validation]
enabled = true

[tool_policy.dry_run_guard]
enabled = true
bypass_tools = ["read_file", "memory_search"]

[tool_policy.audit]
enabled = true

[gateway]
bind = "127.0.0.1:8080"
idle_timeout_secs = 300
max_message_bytes = 51200
graceful_shutdown_secs = 10
```

## Execution Flows

### Flow 1: Plan Execution (Original)

```
warden run-plan --file plan.json [--execution-mode execute]
    ↓
Config Loading (TOML)
    ↓
Plan JSON Parsing + Validation
    ↓
Tool Registration (built-in adapters)
    ↓
Runtime::run_plan() [Sequential]
    ├── For each step:
    │   ├── Hook: BeforeStep
    │   ├── Lookup tool by name
    │   ├── Hook: BeforeToolCall
    │   ├── Execute tool.execute(input) [real or dry-run]
    │   ├── Hook: AfterToolCall
    │   ├── Store result in redb
    │   └── Hook: AfterStep
    └── Return aggregated results
    ↓
[Optional] Save fixture (--record) for replay
    ↓
Output (JSON to stdout)
```

### Flow 2: Agent Chat Loop with Streaming (Phase 1)

```
warden chat --agent default [--session <id>]
    ↓
Config Loading + ConfigManager starts watching file
    ↓
Create/Load Session (JSON file)
    ├── Session ID
    ├── Message history
    └── Metadata
    ↓
REPL Loop:
    ├─ Prompt user: "You: "
    ├─ Read user input
    ├─ Add UserMessage to session.messages
    ├─ Build LLM request:
    │  ├── system_prompt
    │  ├── messages (history)
    │  ├── tools (from runtime registry)
    │  └── config (temp, max_tokens)
    ├─ Hook: MessageReceived
    ├─ Call LLM with streaming:
    │  ├── agent.generate_stream(messages, tools, config)
    │  ├── Receive SSE stream
    │  ├── Parse StreamChunk (TextDelta, ToolCall, Done)
    │  ├── Accumulate text + tool calls
    │  └── If stop_reason=ToolUse:
    │      ├── Extract tool calls from accumulated input_json
    │      ├── For each tool call:
    │      │   ├── Hook: BeforeToolCall
    │      │   ├── Execute via runtime.get_tool()
    │      │   ├── Hook: AfterToolCall
    │      │   └── Add ToolResult to messages
    │      └── Loop back (send updated messages + results)
    ├─ Hook: ResponseGenerated
    ├─ Display final message
    ├─ Check ConfigReloadEvent (if config changed, reload)
    ├─ Save session (JSON)
    └─ Repeat or exit
    ↓
Exit on EOF or "exit" command
```

### Flow 3: Gateway Server with Hot-Reload (Phase 1)

```
warden serve --host 127.0.0.1 --port 8080
    ↓
Config Loading + ConfigManager starts watching file
    ↓
Axum HTTP server starts, listening
    ↓
[Background] File watcher monitors config.toml
    ├─ If changed: reload atomically, broadcast event
    └─ Continue serving (no restart)
    ↓
Client: POST /sessions → Create new session
    ↓
Response: { session_id: "uuid", created_at, ... }
    ↓
Client: WS /ws/{session_id}
    ├─ Upgrade to WebSocket
    ├─ Session Manager tracks connection
    ├─ Broadcast channel setup
    └─ Ready for messages
    ↓
Receive WebSocket message:
    ├─ Parse JSON request
    ├─ Add to session.messages
    ├─ Trigger Agent Loop (same as chat mode, with streaming)
    ├─ Broadcast response to all clients
    └─ Save session
    ↓
Client disconnect:
    ├─ Close WebSocket
    ├─ Cleanup broadcast channel
    ├─ Session persisted on disk
    └─ Can reconnect later with session_id
```

## Component Interactions

### Tool Registration (Phase 3 CR: Using Helpers)

```rust
use operon_adapters::{register_shell_tool, register_filesystem_tools};

let runtime = Runtime::new(dry_run);

// Register shell tool (H3: uses helper to avoid duplication)
register_shell_tool(
    &runtime,
    dry_run,
    vec!["rm -rf".to_string(), "mkfs".to_string()],
    vec![]
)?;

// Register filesystem tools (H3: uses helper to avoid duplication)
register_filesystem_tools(
    &runtime,
    PathBuf::from("/workspace"),
    10  // max_file_size_mb
)?;

// Manual registration for custom tools
let py_adapter = PyAdapter::spawn("./tools/my_tool.py").await?;
runtime.register_tool("python", Arc::new(py_adapter))?;
```

### Streaming Response Parsing

```rust
// Receive SSE stream from LLM
let mut rx = agent.generate_stream(messages, tools, config).await?;

let mut text = String::new();
let mut tool_calls = Vec::new();
let mut current_tool = None;

while let Some(chunk) = rx.recv().await {
    match chunk {
        StreamChunk::TextDelta(s) => text.push_str(&s),
        StreamChunk::ToolCallStart { id, name } => {
            current_tool = Some((id, name, String::new()));
        }
        StreamChunk::ToolCallDelta { id, input_delta } => {
            if let Some((_, _, ref mut input)) = current_tool {
                input.push_str(&input_delta);
            }
        }
        StreamChunk::Done { stop_reason, usage } => {
            if let Some((id, name, input)) = current_tool {
                tool_calls.push(ToolCall { id, name, input });
            }
            break;
        }
    }
}
```

### Config Reload Wiring

```rust
// In command (chat or serve)
let config = Config::load(&config_path)?;
let config_manager = ConfigManager::new(config_path, config);
let config_handle = config_manager.config();
let mut reload_rx = config_manager.subscribe_reload();

// Spawn watcher
tokio::spawn({
    let cm = config_manager.clone();
    async move {
        if let Err(e) = cm.watch().await {
            error!("Config watcher failed: {}", e);
        }
    }
});

// In main loop
tokio::select! {
    event = reload_rx.recv() => {
        match event {
            Ok(ConfigReloadEvent::Success) => {
                info!("Config reloaded");
                let latest = config_handle.read().await;
                // Re-read config for next request
            }
            Ok(ConfigReloadEvent::Failure(e)) => {
                warn!("Config reload failed: {}", e);
            }
            Err(_) => {}
        }
    }
    // ... other event handling
}
```

## Technology Stack

| Layer | Component | Technology | Rationale |
|-------|-----------|-----------|-----------|
| **Streaming** | SSE Parsing | serde_json | Type-safe JSON deserialization |
| **Streaming** | Buffer Management | Vec<u8> with size tracking | Simple, efficient |
| **Config** | File Watcher | notify-debouncer-mini | Lightweight, built-in debouncing |
| **Config** | Storage** | Arc<RwLock<C>> | Atomic updates, lock-free reads |
| **LLM** | Provider clients | reqwest, serde_json | HTTP async client for API integration |
| **Agent** | Session mgmt | uuid, chrono | Standard identifiers + timestamps |
| **Server** | HTTP/WebSocket | axum, tokio | Minimal, composable web framework |
| **Core** | Async Runtime | tokio 1.x | Industry standard, multi-threaded |
| **Core** | Serialization | serde + serde_json | Type-safe, zero-copy |
| **Core** | Error Handling | anyhow | Ergonomic context chains |
| **Core** | Logging | tracing + tracing-subscriber | Structured JSON logs |
| **Core** | Storage | redb | Pure Rust, actively maintained |
| **Core** | Concurrency | DashMap | Lock-free hashmap |
| **Core** | Traits | async_trait | Async trait support macro |
| **CLI** | Arg parsing | clap v4 | Modern derives-based parsing |

## Cross-Platform Support

- **Linux:** Full support, tested in CI
- **macOS:** Full support, tested in CI
- **Windows:** Shell tool uses hardcoded "sh" (untested, see Known Limitations)

## Concurrency Model

**Current:** Single-threaded execution with streaming
- Steps execute sequentially
- Streaming SSE parsed on single tokio thread
- Tools can run async internally
- DashMap enables future parallel step execution

**Streaming Concurrency:**
- Multiple clients can stream simultaneously (broadcast channels)
- Each client gets own StreamChunk stream
- Non-blocking broadcast sends

**Future:** Parallel steps with DAG scheduling
- Plan DAG with dependencies
- Independent steps execute concurrently
- Synchronized barrier between sequential phases

## Security Model

**Threat Model:**
- Plan JSON from trusted sources
- Tools (Python scripts) are trusted code
- Runs in trusted local environment
- Streaming responses validated (1MB limit)

**Current Mitigations:**
- ✅ Dry-run enabled by default (prevents accidents)
- ✅ Explicit `--allow-tools` required for real execution
- ✅ Type-safe JSON parsing (no injection via format strings)
- ✅ Process isolation (subprocesses, not eval)
- ✅ Timeout enforcement (resource exhaustion)
- ✅ No unsafe blocks
- ✅ Streaming buffer limit (OOM protection)
- ✅ Bearer token authentication (gateway)

**Known Gaps:**
- ❌ Command injection risk (shell commands not validated)
- ❌ Path traversal (Python script_path not restricted)
- ❌ No audit logging of executions

See Known Limitations section for details.

## Data Flow

### Plan Execution Pipeline

```
Plan JSON
    ↓
Validation
    ↓
Step Processing
    │
    ├─→ Input from Plan
    ├─→ Tool Lookup
    ├─→ Timeout Configuration
    ├─→ Tool Execution (subprocess)
    ├─→ Result Parsing
    ├─→ Storage Write (redb)
    └─→ Output for next step
    ↓
Final Result (JSON)
```

### Streaming Response Pipeline

```
LLM SSE Stream (HTTP chunked encoding)
    ↓
Receive chunk: "data: {...}\n\n"
    ↓
Strip "data: " prefix
    ↓
Parse JSON
    ↓
parse_anthropic_sse() or parse_openai_sse()
    ↓
StreamChunk variant
    ↓
Send to agent (tokio channel)
    ↓
Agent accumulates:
    - Text fragments → final response text
    - Tool deltas → final tool call input_json
    ↓
Return structured GenerateResponse
```

### Config Reload Pipeline

```
Config file on disk (~/config.toml)
    ↓
[User edits]
    ↓
notify watcher detects change
    ↓
Debounce (500ms wait)
    ↓
ConfigManager::reload()
    ↓
Parse TOML
    ↓
Update Arc<RwLock<Config>>
    ↓
Broadcast ConfigReloadEvent::Success
    ↓
Subscribers (chat, serve) receive event
    ↓
Next agent request reads updated config
```

### Python Tool Communication

```
Rust Request
    ↓
JSON Serialization
    ↓
Write to stdin
    ↓
Python Subprocess Read
    ↓
Python Execution
    ↓
JSON Response Write
    ↓
Rust stdout Read
    ↓
JSON Deserialization
    ↓
Result Extraction
```

## Scaling Considerations

### Current Limits
- Single-threaded step execution (sequential)
- Plan size limited by JSON parsing (typically 10-100 MB fine)
- Storage by redb file size (production deployments should monitor)
- Streaming buffer: 1MB per response (OOM protection)

### Optimization Opportunities
1. **Parallel Steps:** Implement DAG scheduling for independent steps
2. **Streaming Plans:** Process large plans in chunks
3. **Tool Pooling:** Reuse Python interpreter instances
4. **Result Caching:** Memoize identical inputs across plan runs
5. **Streaming Metrics:** Track latency per chunk type

## Known Limitations

See `/docs/known-limitations.md` for comprehensive details. Key Phase 1 notes:

1. **Streaming currently sequential** - could parallelize on multi-tool requests
2. **ConfigManager reload is eventually consistent** - updates happen 500ms after file save
3. **File watcher may miss rapid changes** - changes <500ms apart may coalesce

## Performance Profile

### Streaming Overhead
- SSE parsing: <1ms per chunk (JSON deserialization)
- Buffer accumulation: O(1) amortized
- Broadcast send: <100μs per subscriber
- 1MB buffer check: O(1) atomic operation

### Config Reload
- File watch debounce: 500ms
- Reload parse: <10ms (TOML deserialization)
- RwLock read: <1μs (uncontended)
- Broadcast send: <100μs

### OOM Protection
- Max streaming buffer: 1MB per response
- Prevents runaway generator attacks
- Graceful error on limit exceeded

## Extensibility Points

### Adding New Tool Types

```rust
use async_trait::async_trait;
use operon_runtime::Tool;

pub struct MyTool;

#[async_trait]
impl Tool for MyTool {
    async fn execute(&self, input: Value) -> Result<Value> {
        // Implementation
    }

    fn name(&self) -> &str {
        "my_tool"
    }
}

// Register in warden
runtime.register_tool("my_tool".to_string(), Arc::new(MyTool));
```

### Custom Configuration

Extend `config.toml` with custom sections:

```toml
[tools.my_tool]
setting1 = "value"
enabled = true
```

Parse in `config.rs` and pass to tool constructor.

### Custom Hooks

```rust
#[async_trait]
pub struct MyHook;

#[async_trait]
impl Hook for MyHook {
    async fn handle(&self, context: HookContext) -> Result<HookResult> {
        // Custom logic
        Ok(HookResult::Continue)
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(5)
    }

    fn critical(&self) -> bool {
        false
    }
}

// Register in plugin
registry.register(HookEvent::BeforeToolCall, Arc::new(MyHook))?;
```

## Maintenance & Operations

### Health Checks

```bash
# Validate config
warden run-plan --file test_plan.json  # Dry-run by default

# Check Rust compilation
cargo build --release

# Run full test suite (68 tests)
cargo test --all
```

### Monitoring Recommendations

1. **Streaming Latency:** Track time between chunk emissions
2. **Config Reload Frequency:** Monitor how often config changes
3. **Tool Failures:** Count failures by tool type
4. **Resource Usage:** Monitor subprocess spawning rate
5. **Storage Growth:** Track redb database file size

### Troubleshooting

| Issue | Root Cause | Solution |
|-------|-----------|----------|
| Streaming stalls | Network issue or large response | Check 1MB buffer limit |
| Config not reloading | File watcher not triggered | Check file permissions, 500ms debounce |
| Commands execute in dry-run | Dry-run flag not honored | Check config.runtime.dry_run = true |
| Timeout errors on Python | Subprocess hanging on stderr | Spawn stderr reader (High priority fix) |
| Tool not found errors | Missing tool registration | Ensure all steps reference registered tools |
| JSON parse errors | Malformed plan JSON | Validate plan against schema |

## References

- **README:** `/README.md` - Quickstart guide
- **Memory & Search:** `/docs/memory-search-system.md` - Phase 4 detailed documentation
- **Known Limitations:** `/docs/known-limitations.md` - Detailed issue tracking
- **Code Standards:** `/docs/code-standards.md` - Development guidelines
- **Codebase Summary:** `/docs/codebase-summary.md` - Structure overview
- **Anthropic Streaming:** https://docs.anthropic.com/en/api/messages-streaming
- **OpenAI Streaming:** https://platform.openai.com/docs/api-reference/chat/create
- **OpenAI Embeddings:** https://platform.openai.com/docs/guides/embeddings
- **SQLite FTS5:** https://www.sqlite.org/fts5.html
- **RRF Algorithm:** https://en.wikipedia.org/wiki/Reciprocal_rank_fusion
- **SSE Standard:** https://html.spec.whatwg.org/multipage/server-sent-events.html
- **notify-debouncer-mini:** https://docs.rs/notify-debouncer-mini/

---

**Phase 5 Completed:** 2026-02-18
**New Systems:**
- Google Gemini LLM provider (third provider with SSE streaming, tool calling, vision)
- 7-layer tool policy pipeline (authorization, validation, rate-limiting, audit)
**New Modules:** 4 modules (gemini.rs + tool_policy/), ~865 LOC total
**Modified Files:** 6 files (streaming.rs, config.rs, runtime.rs, chat.rs, lib.rs, llm/mod.rs)
**Test Coverage:** 132 tests (22 new: 10 Gemini + 12 tool policy, all passing)
**Code Quality:** 0 clippy warnings, 0 unsafe blocks, trait-based design
**Security:** Tool policy pipeline with permission check, rate limiting, input validation, audit logging
**LLM Providers:** 3 providers (Anthropic, OpenAI, Gemini) with automatic failover
