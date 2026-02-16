# SilentClaw System Architecture

**Last Updated:** 2026-02-17
**Version:** 2.0.0
**Status:** Upgraded - LLM, Agent, Plugin, Gateway

## Overview

SilentClaw is a comprehensive agent platform combining:
- **LLM Integration** - Anthropic/OpenAI with failover chains
- **Agent Loop** - Conversation state + tool calling + auto-iteration
- **Event Hooks** - DashMap-based event system for extensibility
- **Plugin System** - Dynamic tool/hook loading with API versioning
- **Gateway** - HTTP/WebSocket API for remote access
- **Async Runtime** - Tokio-based execution engine
- **Config Hot-Reload** - File watcher for live configuration updates

Architecture evolution:

```
v1.0: plan → runtime → tools → result
v2.0: user → agent → LLM → tool → observe → feedback (loop) → response
      + plugins, hooks, gateway, persistence
```

## Architecture Layers (5 Layers)

### Layer 1: LLM Provider Integration (Production Hardened)

**Purpose:** Unified interface to multiple LLM providers with failover and timeout hardening

**Provider Trait:**
```rust
pub trait LLMProvider: Send + Sync {
    async fn generate(&self, config: GenerateConfig) -> Result<GenerateResponse>;
}
```

**Implementations:**
- **AnthropicClient** - Uses Anthropic API for tool calling + vision
  - Supports `tool_use` block content type
  - Vision/multimodal base64 encoding
  - HTTP timeouts: 120s request, 10s connect (ClientBuilder)
  - Extracts ToolCall from response
  - API key via `ANTHROPIC_API_KEY` env var

- **OpenAIClient** - Uses OpenAI API (GPT-4/3.5) with vision
  - Function calling format
  - Vision/multimodal base64 encoding (OpenAI format)
  - Message streaming support
  - HTTP timeouts: 120s request, 10s connect
  - API key via `OPENAI_API_KEY` env var

- **ProviderChain (Failover)** - Fallback logic with retry-after
  - Configurable list of providers
  - Exponential backoff on failure
  - Retry-After header parsing
  - Context-aware error logging
  - Seamless provider switching

**Features:**
- Async request/response
- Structured message format with roles
- Tool schema inference from tool registry
- Stop reason tracking (end_turn, tool_use, etc.)
- Vision/multimodal content support
- Cumulative usage tracking (AddAssign impl, total() method)
- ModelInfo struct for model capabilities catalog

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
3. Call LLM with tool schemas
4. Parse tool calls from response
5. Execute tools via runtime
6. Append results to history
7. Loop until stop reason = end_turn
8. Return final message

### Layer 3: Event Hooks (Production Hardened)

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

### Layer 4: Plugin System (NEW)

**Purpose:** Dynamic tool and hook loading with version safety

**Plugin Discovery:**
- Scan plugin directories for `plugin.toml`
- Parse manifest: name, version, api_version
- Load plugin library: `libplugin_name.so` / `.dll`

**Plugin Manifest (TOML):**
```toml
[plugin]
name = "custom-tools"
version = "0.1.0"
api_version = 1  # Must match SDK API_VERSION
description = "Custom tools for workflows"

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

**Plugin Trait:**
```rust
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn api_version(&self) -> u32;
    fn init(&mut self, config: Value) -> Result<()>;
    fn shutdown(&mut self) -> Result<()>;
    fn tools(&self) -> Vec<Box<dyn Tool>>;
    fn hooks(&self) -> Vec<Box<dyn Hook>>;
}
```

### Layer 5: Gateway Server (Production Hardened)

**Purpose:** HTTP/WebSocket API for remote agent access with security hardening

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
- Validates `Authorization: Bearer <token>` header
- Configurable token validation logic
- Returns 401 Unauthorized on invalid token

**Rate Limiter:**
- Token bucket algorithm per client
- Configurable tokens/second
- Returns 429 Too Many Requests when limited

### Layer 6: Core Runtime (operon-runtime)

**Purpose:** Async Tool trait and orchestration engine

**Key Components:**

- **Tool Trait** - Generic async interface for any tool type
  ```rust
  #[async_trait]
  pub trait Tool {
      async fn execute(&self, input: Value) -> Result<Value>;
      fn name(&self) -> &str;
  }
  ```

- **Runtime Struct** - Plan executor with async step orchestration
  - Tool registry (DashMap for lock-free concurrency)
  - Per-tool timeout configuration
  - Dry-run flag for safety
  - JSON structured logging via tracing

- **Storage Module** - Persistent step result storage
  - Uses redb (actively maintained, pure Rust)
  - Memory-mapped I/O for efficiency
  - Transaction-based result storage

**Design Decisions:**
- **DashMap over Mutex:** Lock-free concurrent hashmap prevents contention
- **async_trait for tool/hook:** Enables async implementations without dyn complexity
- **Broadcast channels:** Pub/sub for gateway multi-client updates
- **File watchers (notify):** Live config reloading without restart
- **Atomic types for scheduling:** Lock-free task queue coordination
- **No unsafe blocks:** Type safety throughout (only in test mocks)
- **Dry-run default:** Configuration enables user choice

### Layer 7: Tool Adapters (operon-adapters)

**Purpose:** Bridge Rust runtime with external tools

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
- Per-request ID tracking for multiplexing (future)
- Configurable timeout per call

**Implementation Details:**
- subprocess spawned with `python3 script_path`
- stdin/stdout piped for JSON communication
- stderr piped but requires external handler (deadlock risk - see Known Limitations)

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

### Layer 8: CLI Interface (warden - Hardened)

**Purpose:** Entry point for all SilentClaw modes with config validation

**Commands:**
1. **run-plan** - Execute plan JSON with tools
   - `--file <path>` - Plan file
   - `--record <dir>` - Save fixture for replay
   - `--replay <dir>` - Skip tool execution, use recorded results

2. **chat** - Interactive agent conversation
   - `--agent <name>` - Agent config
   - `--session <id>` - Resume existing session
   - REPL loop: read user input → agent loop → display response

3. **serve** - Gateway HTTP/WebSocket server (Hardened)
   - `--host <addr>` - Bind address (default: 127.0.0.1)
   - `--port <num>` - Listen port (default: 8080)
   - Bearer token auth required
   - Rate limiting enabled
   - Graceful shutdown (10s drain)
   - Concurrent session management

4. **plugin** - Manage plugins
   - `list` - Show installed plugins
   - `load <path>` - Load from directory
   - `unload <name>` - Unload by name

5. **init** - Bootstrap config file (NEW)
   - Generates default config.toml
   - Includes security defaults (blocklist, version)

**Global Flags:**
- `--execution-mode {auto|dry-run|execute}` - Control tool execution
- `--config <path>` - Config file (default: ~/.silentclaw/config.toml)

**Environment Variables (NEW):**
- `SILENTCLAW_TIMEOUT` - Override timeout_secs
- `SILENTCLAW_MAX_PARALLEL` - Override max_parallel
- `SILENTCLAW_DRY_RUN` - Override dry_run
- `ANTHROPIC_API_KEY` - Anthropic API key
- `OPENAI_API_KEY` - OpenAI API key

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

[tools.timeouts]
shell = 30                     # Tool-specific override
python = 120

[llm]
provider = "anthropic"
model = ""                     # Model specification

[gateway]
bind = "127.0.0.1:8080"       # Server address
idle_timeout_secs = 300        # 5-min WebSocket timeout
max_message_bytes = 51200      # 50KB limit
graceful_shutdown_secs = 10    # Drain period
```

**Validation (NEW):**
- Config version checking
- Semantic validation (validate() method)
- Required fields enforced at load time
- Environment variable overrides supported

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

### Flow 2: Agent Chat Loop (NEW)

```
warden chat --agent default [--session <id>]
    ↓
Config Loading
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
    ├─ Call LLM (with failover chain)
    │  ├── If ToolUse in response:
    │  │   ├── Extract tool name + params
    │  │   ├── Hook: BeforeToolCall
    │  │   ├── Execute via runtime.get_tool()
    │  │   ├── Hook: AfterToolCall
    │  │   └── Add ToolResult to messages
    │  └── Repeat until stop_reason != ToolUse
    ├─ Hook: ResponseGenerated
    ├─ Display final message
    ├─ Save session (JSON)
    └─ Repeat or exit
    ↓
Exit on EOF or "exit" command
```

### Flow 3: Gateway Server (NEW)

```
warden serve --host 127.0.0.1 --port 8080
    ↓
Axum HTTP server starts, listening
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
    ├─ Trigger Agent Loop (same as chat mode)
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

### Tool Registration

```rust
let runtime = Runtime::new(dry_run);

// Register adapters
let shell_tool = ShellTool::new(dry_run);
runtime.register_tool("shell", Arc::new(shell_tool));

let py_adapter = PyAdapter::spawn("./tools/my_tool.py").await?;
runtime.register_tool("python", Arc::new(py_adapter));
```

### Plan Execution

```json
{
  "version": "1.0",
  "steps": [
    {
      "id": "step1",
      "tool": "shell",
      "input": {"cmd": "echo 'Hello'"}
    },
    {
      "id": "step2",
      "tool": "python",
      "input": {"method": "process", "params": {"data": "..."}}
    }
  ]
}
```

## Technology Stack

| Layer | Component | Technology | Rationale |
|-------|-----------|-----------|-----------|
| **LLM** | Provider clients | reqwest, serde_json | HTTP async client for API integration |
| **Agent** | Session mgmt | uuid, chrono | Standard identifiers + timestamps |
| **Server** | HTTP/WebSocket | axum, tokio | Minimal, composable web framework |
| **Config** | File monitoring | notify | Efficient file system watcher |
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

**Current:** Single-threaded execution
- Steps execute sequentially
- Tools can run async internally (via PyAdapter/subprocess)
- DashMap enables future parallel step execution

**Future:** Parallel steps with DAG scheduling
- Plan DAG with dependencies
- Independent steps execute concurrently
- Synchronized barrier between sequential phases

## Security Model

**Threat Model:**
- Plan JSON from trusted sources
- Tools (Python scripts) are trusted code
- Runs in trusted local environment

**Current Mitigations:**
- ✅ Dry-run enabled by default (prevents accidents)
- ✅ Explicit `--allow-tools` required for real execution
- ✅ Type-safe JSON parsing (no injection via format strings)
- ✅ Process isolation (subprocesses, not eval)
- ✅ Timeout enforcement (resource exhaustion)
- ✅ No unsafe blocks

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

### Optimization Opportunities
1. **Parallel Steps:** Implement DAG scheduling for independent steps
2. **Streaming Plans:** Process large plans in chunks
3. **Tool Pooling:** Reuse Python interpreter instances
4. **Result Caching:** Memoize identical inputs across plan runs

## Known Limitations

See `/docs/known-limitations.md` for comprehensive details. Key issues:

1. **PyAdapter Tool Trait Incompatibility (CRITICAL)**
   - `execute(&self)` requires immutable reference
   - PyAdapter needs `&mut self` for request_id counter
   - Result: Python tools fail when called via Tool trait
   - Workaround: Use `call()` method directly (internal only)

2. **Shell Command Injection (HIGH)**
   - No validation on shell commands
   - Mitigated by dry-run default + `--allow-tools` explicit flag
   - Recommendation: Add command validation layer before production

3. **Python Subprocess Stderr Deadlock (HIGH)**
   - stderr piped but never read
   - Large stderr output (>64KB) blocks Python process
   - Risk: Plans with verbose Python tools may deadlock
   - Fix: Spawn background stderr reader task

4. **Timeout Configuration Duplication (HIGH)**
   - Timeout configurable in two places (tool + runtime)
   - Current behavior: Runtime timeout takes precedence
   - Result: Tool-level timeout field is dead code
   - Recommendation: Unify configuration

5. **Dry-Run Bypass Confusion (MEDIUM)**
   - `--allow-tools` defaults to false
   - When true, unconditionally overrides config dry_run
   - Risk: Users expecting config-driven safety can bypass with flag
   - Recommendation: Use three-state execution mode

See `/docs/known-limitations.md` for complete list and mitigations.

## Performance Profile

### Benchmarks (Local)

- **Plan Parsing:** <1ms (serde JSON)
- **Tool Registration:** <1μs (DashMap insert)
- **Shell Execution:** ~10-500ms (depends on command)
- **Python Tool Execution:** ~50-200ms (subprocess overhead + execution)
- **Storage Write:** ~1-5ms (redb transaction)

### Bottlenecks

1. **Sequential Steps:** Cannot parallelize independent steps (current design)
2. **Subprocess Communication:** JSON serialization + pipe overhead vs. native calls
3. **Storage I/O:** Per-step write to disk (optimization: batch writes)

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

## Maintenance & Operations

### Health Checks

```bash
# Validate config
warden run-plan --file test_plan.json  # Dry-run by default

# Check Rust compilation
cargo build --release

# Run full test suite
cargo test --all
```

### Monitoring Recommendations

1. **Plan Execution Time:** Track per-step duration
2. **Tool Failures:** Count failures by tool type
3. **Resource Usage:** Monitor subprocess spawning rate
4. **Storage Growth:** Track redb database file size

### Troubleshooting

| Issue | Root Cause | Solution |
|-------|-----------|----------|
| Plan steps not executing | PyAdapter broken (CRITICAL) | Use shell tool only or fix PyAdapter |
| Commands execute in dry-run | Dry-run flag not honored | Check config.runtime.dry_run = true |
| Timeout errors on Python | Subprocess hanging on stderr | Spawn stderr reader (High priority fix) |
| Tool not found errors | Missing tool registration | Ensure all steps reference registered tools |
| JSON parse errors | Malformed plan JSON | Validate plan against schema |

## References

- **README:** `/README.md` - Quickstart guide
- **Known Limitations:** `/docs/known-limitations.md` - Detailed issue tracking
- **Code Standards:** `/docs/code-standards.md` - Development guidelines
- **Codebase Summary:** `/docs/codebase-summary.md` - Structure overview
