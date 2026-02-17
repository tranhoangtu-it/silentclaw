# SilentClaw - Code Standards & Development Guidelines

**Last Updated:** 2026-02-18
**Version:** 2.1.0 (Phase 6 Code Review)
**Audience:** Developers, maintainers, contributors

## Project Overview

**SilentClaw** is a comprehensive Rust agent platform providing:
- **LLM Integration** - Anthropic/OpenAI with failover chains
- **Agent Loop** - Conversation state + tool orchestration
- **Event Hooks** - Extensible event-driven architecture (DashMap registry)
- **Plugin System** - Dynamic tool/hook loading with API versioning
- **Gateway Server** - HTTP/WebSocket API (Axum framework)
- **Config Hot-Reload** - File watcher for live updates
- **Tool Adapters** - Python + Shell execution
- **Structured Logging** - JSON logs via tracing

**Codebase Stats:**
- **Repository:** 5 crates + 1 SDK crate
- **Clippy Warnings:** 0
- **Format Issues:** 0
- **Architecture:** Modular, event-driven, extensible

## Codebase Structure

```
silentclaw/
├── Cargo.toml
├── README.md
│
├── crates/
│   ├── operon-runtime/          # Core engine + new features
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── tool.rs
│   │       ├── runtime.rs
│   │       ├── storage.rs
│   │       ├── agent_module.rs  # (NEW)
│   │       ├── llm/             # (NEW) Provider + clients
│   │       ├── hooks/           # (NEW) Event system
│   │       ├── config/          # (NEW) Hot-reload
│   │       ├── plugin/          # (NEW) Plugin loader
│   │       ├── replay.rs        # (NEW) Fixture support
│   │       └── scheduler.rs     # (NEW) Task scheduling
│   │
│   ├── operon-adapters/         # Tool implementations
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── python_adapter.rs
│   │       └── shell_tool.rs
│   │
│   ├── operon-gateway/          # (NEW) HTTP/WebSocket
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── server.rs
│   │       ├── session_manager.rs
│   │       └── types.rs
│   │
│   ├── operon-plugin-sdk/       # (NEW) Plugin SDK
│   │   └── src/
│   │       └── lib.rs
│   │
│   └── warden/                  # CLI (expanded)
│       └── src/
│           ├── main.rs
│           ├── cli.rs
│           ├── config.rs
│           └── commands/
│               ├── run_plan.rs
│               ├── chat.rs        # (NEW)
│               ├── serve.rs       # (NEW)
│               └── plugin.rs      # (NEW)
│
├── docs/
│   ├── system-architecture.md
│   ├── code-standards.md (this file)
│   ├── codebase-summary.md
│   └── known-limitations.md
│
└── examples/
    ├── plan_hello.json
    └── echo_tool.py
```

## Rust Code Standards

### 1. Module Organization

**Principle:** Single responsibility, clear separation of concerns

**File Naming (snake_case):**
```
src/
├── lib.rs           # Public exports
├── tool.rs          # Tool trait
├── runtime.rs       # Runtime orchestrator
├── storage.rs       # Storage layer
└── commands/
    ├── mod.rs       # Re-exports
    └── run_plan.rs  # Specific command
```

**Module Visibility:**
```rust
// lib.rs - only export public API
pub mod runtime;
pub mod storage;
pub mod tool;

pub use runtime::Runtime;
pub use storage::Storage;
pub use tool::Tool;

// Keep private utilities internal
mod config;  // Not exported
mod utils;   // Not exported
```

**Re-export Pattern:**
```rust
// Inside modules, re-export from submodules
pub use self::runtime::Runtime;
pub use self::runtime::ExecutionResult;

// Users: operon_runtime::Runtime (not operon_runtime::runtime::Runtime)
```

### 2. Error Handling

**Use `anyhow::Result<T>` for all public APIs:**

```rust
use anyhow::{Context, Result};

// ✅ Good: Returns anyhow::Result
pub async fn execute(&self, input: Value) -> Result<Value> {
    let result = some_operation()
        .context("Operation failed")?;
    Ok(result)
}

// ❌ Bad: Custom error type
pub async fn execute(&self, input: Value) -> std::result::Result<Value, MyError> {
    // More verbose, less ergonomic
}
```

**Context Chains:**

```rust
// ✅ Good: Rich error context
File::open("config.toml")
    .context("Failed to read config.toml from {}")?

// ❌ Bad: Generic errors
File::open("config.toml").map_err(|e| anyhow::anyhow!("Error: {}", e))?
```

**Never unwrap in production code:**

```rust
// ✅ Good: Explicit error handling
match value.get("field") {
    Some(v) => process(v),
    None => anyhow::bail!("Missing required field: field")
}

// ❌ Bad: Panic on None
let field = value.get("field").unwrap();  // Only in tests/examples

// ❌ Bad: Hidden panic
let field = value["field"].as_str().unwrap();
```

### 3. Arc Pattern & Builder Safety (NEW - Phase 6)

**Build runtime before Arc wrapping (safer, more flexible):**

```rust
// ✅ Good (Phase 6): Construct fully, then wrap
let mut runtime = Runtime::new(false, default_timeout)?;
runtime.set_policy(policy_pipeline);
runtime.configure_timeout("shell", shell_timeout);
let runtime = Arc::new(runtime);  // Wrap after full construction

// ❌ Bad (Pre-Phase 6): Arc first, then get_mut()
let runtime = Arc::new(Runtime::new(false, default_timeout)?);
Arc::get_mut(&mut runtime).expect("").set_policy(policy_pipeline);  // Fragile
```

**Benefits:**
- No panic on `Arc::get_mut().expect()`
- Clearer intent (construct → wrap → immutable)
- More flexible for refactoring
- Works with shared ownership patterns

**Guidance:** Always build state before Arc wrapping. Use builders for configuration.

### 4. Async Patterns

**Use `#[tokio::main]` for async entry points:**

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let config = load_config()?;
    run_plan(&config).await?;
    Ok(())
}

// NOT: fn main() { tokio::runtime::Runtime::new()... }
```

**Use `#[async_trait]` for trait methods:**

```rust
use async_trait::async_trait;

#[async_trait]
pub trait Tool {
    async fn execute(&self, input: Value) -> Result<Value>;
    fn name(&self) -> &str;
}

#[async_trait]
impl Tool for ShellTool {
    async fn execute(&self, input: Value) -> Result<Value> {
        // Implementation
    }

    fn name(&self) -> &str {
        "shell"
    }
}
```

**Avoid blocking operations in async code:**

```rust
// ✅ Good: Async file operations
let content = tokio::fs::read_to_string(path).await?;

// ❌ Bad: Blocking file I/O in async context
let content = std::fs::read_to_string(path)?;

// ✅ Good: Async sleep
tokio::time::sleep(Duration::from_secs(1)).await;

// ❌ Bad: Blocking sleep
std::thread::sleep(Duration::from_secs(1));
```

### 5. Type Safety

**Use strong types, avoid String for identifiers:**

```rust
// ✅ Good: Newtype for tool names
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolName(String);

impl ToolName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

// ❌ Bad: Raw String everywhere
pub fn register_tool(runtime: &Runtime, name: String, tool: Arc<dyn Tool>) {
    // Unclear what this String represents
}
```

**Use associated types for flexibility:**

```rust
// ✅ Good: Tool output type is flexible
pub trait Tool {
    type Output: Serialize;

    async fn execute(&self, input: Value) -> Result<Self::Output>;
}

// ❌ Bad: Always returns Value
pub trait Tool {
    async fn execute(&self, input: Value) -> Result<Value>;
}
```

### 6. Logging & Observability

**Use `tracing` macros, not `println!`:**

```rust
use tracing::{debug, info, warn, error};

// ✅ Good: Structured logging
debug!(script = script_path, "Python subprocess spawned");
info!(step_id = %step.id, elapsed_ms = %elapsed, "Step completed");
warn!(error = ?err, "Timeout detected");

// ❌ Bad: Unstructured output
println!("Spawned Python subprocess for {}", script_path);
eprintln!("ERROR: {}", err);
```

**Include contextual fields:**

```rust
// ✅ Good: Rich context
info!(
    tool_name = %tool.name(),
    timeout_ms = timeout.as_millis(),
    dry_run = dry_run,
    "Executing tool"
);

// ❌ Bad: No context
info!("Executing tool");
```

**Log at appropriate levels:**

| Level | When | Example |
|-------|------|---------|
| **debug** | Development details, verbose | "Request sent to Python", "Parsed config" |
| **info** | Normal operations | "Plan started", "Step completed" |
| **warn** | Concerning but handled | "Timeout, retrying", "Missing optional config" |
| **error** | Failures that need attention | "Tool crashed", "Invalid plan format" |

### 7. Documentation & Comments

**Public APIs require doc comments:**

```rust
/// Spawns a Python subprocess for tool execution.
///
/// # Arguments
/// * `script_path` - Relative or absolute path to Python script
/// * `timeout` - Per-call timeout duration
///
/// # Returns
/// A new PyAdapter instance ready for method calls.
///
/// # Errors
/// Returns error if Python process fails to spawn.
///
/// # Example
/// ```no_run
/// let adapter = PyAdapter::spawn("./tools/my_tool.py").await?;
/// let result = adapter.call("execute", json!({})).await?;
/// ```
pub async fn spawn(script_path: &str) -> Result<Self> {
    Self::spawn_with_timeout(script_path, Duration::from_secs(60)).await
}
```

**Add comments for non-obvious logic:**

```rust
// ✅ Good: Explains why
if self.state.load(Ordering::SeqCst) != 0 {
    // Prevent tool registration during plan execution to ensure consistent step behavior
    anyhow::bail!("Cannot register tools while runtime is executing");
}

// ❌ Bad: States what code does (obvious)
// Check if state is not idle
if self.state.load(Ordering::SeqCst) != 0 {
    // ...
}
```

**No commented-out code:**

```rust
// ❌ Bad: Dead code clutters codebase
// let old_timeout = self.timeout;
// TODO: remove this old code

// ✅ Good: Use git history if needed
// If needed, check git log --all -p -- runtime.rs
```

### 8. Testing

**Test file naming (suffix `_test.rs` or `_tests.rs`):**

```
src/
├── runtime.rs
├── runtime_tests.rs        # ✅ Good
├── tests/
│   └── integration_tests.rs
```

**Use unit tests for single components, integration tests for workflows:**

```rust
// shell_tool_tests.rs - Unit tests
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_simple_command() {
        let tool = ShellTool::new(false);  // real execution
        let result = tool.execute(json!({"cmd": "echo hello"})).await.unwrap();
        assert_eq!(result["stdout"], "hello\n");
    }

    #[tokio::test]
    async fn test_dry_run_prevents_execution() {
        let tool = ShellTool::new(true);  // dry-run
        let result = tool.execute(json!({"cmd": "rm -rf /"})).await;
        // Should succeed but with warning logs
        assert!(result.is_ok());
    }
}

// tests/integration_test.rs - Full workflow
#[tokio::test]
async fn test_full_plan_execution() {
    let plan = load_plan("examples/plan_hello.json");
    let runtime = Runtime::new(false);
    let result = runtime.run_plan(plan).await;
    assert!(result.is_ok());
}
```

**Test edge cases:**

```rust
#[tokio::test]
async fn test_timeout_enforcement() {
    let tool = ShellTool::new(false).with_timeout(Duration::from_millis(100));
    let result = tool.execute(json!({"cmd": "sleep 10"})).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("timeout"));
}

#[tokio::test]
async fn test_nonzero_exit_code() {
    let tool = ShellTool::new(false);
    let result = tool.execute(json!({"cmd": "exit 42"})).await.unwrap();
    assert_eq!(result["exit_code"], 42);
}

#[tokio::test]
async fn test_stderr_capture() {
    let tool = ShellTool::new(false);
    let result = tool.execute(json!({"cmd": "echo error >&2"})).await.unwrap();
    assert_eq!(result["stderr"], "error\n");
}
```

### 9. Code Formatting

**Use `cargo fmt` for all code:**

```bash
# Before committing
cargo fmt --all

# Check in CI
cargo fmt -- --check
```

**Use `cargo clippy` to catch common mistakes:**

```bash
# Run linter
cargo clippy --all -- -D warnings

# Fix automatically where possible
cargo clippy --fix --allow-dirty
```

### 10. Concurrent Data Structures & Atomics (Phase 6 Enhanced)

**Use AtomicU64 for non-blocking counters (Phase 6):**

```rust
use std::sync::atomic::{AtomicU64, Ordering};

// ✅ Good: Non-blocking counter for IDs (Phase 6)
static CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn next_call_id(name: &str) -> String {
    let n = CALL_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("gemini_{}_{}", name, n)  // Globally unique, no collisions
}

// ❌ Bad: Mutex-protected counter
let counter = Arc::new(Mutex::new(0));
let mut c = counter.lock().unwrap();
*c += 1;  // Blocking!
```

**Benefits:**
- No locks, pure atomic operations
- Safe across concurrent threads
- Fast (CPU cache-friendly)
- Good for high-frequency operations (tool calls)

### 11. Concurrent Data Structures

**Use DashMap for lock-free concurrent access:**

```rust
use dashmap::DashMap;

// ✅ Good: Lock-free concurrent hashmap
let hooks: DashMap<HookEvent, Vec<Arc<dyn Hook>>> = DashMap::new();
hooks.insert(HookEvent::BeforeToolCall, vec![...]);

for entry in hooks.iter() {
    process(entry.key(), entry.value());
}

// ❌ Bad: Mutex contention on hot path
let hooks = Mutex::new(HashMap::new());
let _guard = hooks.lock().unwrap();  // Blocks other threads
```

**Broadcast channels for pub/sub (WebSocket patterns):**

```rust
use tokio::sync::broadcast;

// ✅ Good: Multi-receiver pub/sub
let (tx, _rx) = broadcast::channel(100);

// Multiple clients subscribe
let mut rx1 = tx.subscribe();
let mut rx2 = tx.subscribe();

// Broadcast to all
tx.send(message).unwrap();

// ❌ Bad: Mutex + Vec for multiple receivers
let listeners = Mutex::new(vec![...]);
for listener in listeners.lock().unwrap().iter() {
    listener.send(message);  // Sequential, not broadcast
}
```

### 12. Event-Driven Architecture

**Hook pattern for extensibility:**

```rust
use async_trait::async_trait;

// ✅ Good: Event handler abstraction
#[async_trait]
pub trait Hook: Send + Sync {
    async fn handle(&self, context: HookContext) -> Result<HookResult>;
}

// Plugin registers hook at runtime
pub struct AuditHook;

#[async_trait]
impl Hook for AuditHook {
    async fn handle(&self, context: HookContext) -> Result<HookResult> {
        info!(event = ?context.event, user = context.user_id, "Audit log");
        Ok(HookResult::Continue)
    }
}

// Registry manages hooks (DashMap)
hooks_registry.register(HookEvent::BeforeToolCall, Arc::new(AuditHook))?;

// ❌ Bad: Hard-coded audit logic in every function
async fn execute_tool(name: &str) -> Result<()> {
    info!("Executing tool: {}", name);  // Can't remove without code change
    // ...
}
```

**Hook context should be minimal:**

```rust
// ✅ Good: Structured event data
pub struct HookContext {
    pub event: HookEvent,
    pub user_id: String,
    pub tool_name: String,
    pub metadata: HashMap<String, Value>,  // Extensible
}

// ❌ Bad: Large, tightly-coupled context
pub struct HookContext {
    pub runtime: Arc<Runtime>,  // Circular dependency
    pub session: Arc<Session>,
    pub full_history: Vec<Message>,  // Too much data
}
```

### 13. Plugin System Patterns

**Plugin trait design for static dispatch:**

```rust
// ✅ Good: Trait with dyn object support
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn api_version(&self) -> u32;  // Version checking
    fn tools(&self) -> Vec<Box<dyn Tool>>;
    fn hooks(&self) -> Vec<Box<dyn Hook>>;
}

// Manifest versioning prevents ABI breaks
pub const API_VERSION: u32 = 1;

pub struct MyPlugin;
impl Default for MyPlugin {
    fn default() -> Self { Self }
}

#[async_trait]
impl Tool for MyPluginTool {
    // ...
}

impl Plugin for MyPlugin {
    fn api_version(&self) -> u32 { API_VERSION }
    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![Box::new(MyPluginTool)]
    }
    // ...
}

// Declare entry point macro
declare_plugin!(MyPlugin);
```

**Plugin loader with version safety:**

```rust
// ✅ Good: Check API version before loading
pub async fn load_plugin(&mut self, path: &Path) -> Result<()> {
    let manifest: PluginManifest = load_toml(path)?;

    // Check API version compatibility
    if manifest.api_version != API_VERSION {
        anyhow::bail!(
            "Plugin {} requires API v{}, but runtime is v{}",
            manifest.name, manifest.api_version, API_VERSION
        );
    }

    // Load dynamic library
    let plugin = unsafe { load_library(&path)? };
    self.plugins.insert(manifest.name, plugin);
    Ok(())
}
```

### 14. Config Hot-Reload Pattern

**File watcher for live configuration:**

```rust
use notify::{Watcher, RecursiveMode, watcher};
use std::sync::mpsc;

// ✅ Good: Watch config file for changes
pub struct ConfigManager {
    watcher: RecommendedWatcher,
    config: Arc<RwLock<Config>>,
}

impl ConfigManager {
    pub fn new(path: &Path) -> Result<Self> {
        let (tx, rx) = mpsc::channel();

        let mut watcher = watcher(
            move |res: notify::Result<Event>| {
                if let Ok(Event { kind: EventKind::Modify, .. }) = res {
                    tx.send(ConfigReloadEvent).ok();
                }
            },
            Duration::from_secs(2),
        )?;

        watcher.watch(path, RecursiveMode::NonRecursive)?;

        // Spawn reload task
        tokio::spawn(Self::reload_loop(rx, config.clone()));

        Ok(Self { watcher, config })
    }

    async fn reload_loop(rx: Receiver<ConfigReloadEvent>, config: Arc<RwLock<Config>>) {
        while let Ok(_) = rx.recv() {
            if let Ok(new_config) = Config::load() {
                *config.write().unwrap() = new_config;
                info!("Config reloaded");
            }
        }
    }
}

// ❌ Bad: Require restart for config changes
pub fn load_config(path: &str) -> Config {
    Config::from_file(path)  // Static at startup
}
```

### 15. Dependency Management

**Keep dependencies minimal (with justified new ones):**

```toml
[dependencies]
# Core async + serialization (v1.0)
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = "0.3"

# New in v2.0 (justified)
async_trait = "0.1"         # ✅ Async trait support (used pervasively)
dashmap = "5"               # ✅ Lock-free registry (hot path)
axum = "0.7"                # ✅ Minimal HTTP framework (gateway)
uuid = { version = "1", features = ["v4", "serde"] }  # ✅ Session IDs
chrono = { version = "0.4", features = ["serde"] }    # ✅ Timestamps
reqwest = { version = "0.11", features = ["json"] }   # ✅ LLM API calls
notify = "6"                # ✅ File watcher (config reload)
toml = "0.8"                # ✅ Plugin manifest parsing

# New in v2.1 (Production Hardening - justified)
tower = "0.4"               # ✅ Middleware framework (auth, rate limit)
tower-http = { version = "0.5", features = ["cors", "timeout"] }  # ✅ HTTP utilities

# ❌ Avoid unless critical
fancy_string_lib = "1.0"  # Not justified
```

**Use workspace dependencies for consistency:**

```toml
[workspace.dependencies]
tokio = { version = "1", features = ["full"] }

[dependencies]
tokio.workspace = true  # ✅ Single source of truth
```

**Check for security issues regularly:**

```bash
cargo audit  # Scans for known vulnerabilities
```

## Rust Best Practices

### Ownership & Borrowing

```rust
// ✅ Good: Leverage ownership system
pub fn process_plan(plan: Plan) {
    // Takes ownership, plan moved here
}

// ✅ Good: Borrow when you don't need ownership
pub fn validate_plan(plan: &Plan) -> Result<()> {
    // Just read the plan
}

// ⚠️ Acceptable: Mutable borrow when needed
pub fn update_plan(plan: &mut Plan) {
    plan.status = Status::Running;
}

// ❌ Bad: Unnecessary cloning
pub fn validate_plan(plan: Plan) {
    let plan = plan.clone();  // Not needed
}
```

### Pattern Matching

```rust
// ✅ Good: Exhaustive matching
match result {
    Ok(value) => process(value),
    Err(e) => return Err(e),
}

// ✅ Good: Use if-let for single case
if let Some(timeout) = config.timeout {
    set_timeout(timeout);
}

// ❌ Bad: Pattern matching on single value
match maybe_value {
    Some(v) => process(v),
    None => {},  // Do nothing
}
// Use: if let Some(v) = maybe_value { process(v); }
```

## Configuration Standards

### TOML Configuration (`config.toml`)

**Location:** `~/.silentclaw/config.toml`

**Standard Sections:**

```toml
# Runtime settings
[runtime]
dry_run = true              # Default: safe
timeout_secs = 60           # Default: reasonable
data_dir = "~/.silentclaw"  # Default: home directory

# Shell tool settings
[tools.shell]
enabled = true
timeout = 30

# Python tool settings
[tools.python]
enabled = true
timeout = 120
scripts_dir = "./tools"

# Per-tool timeouts
[tools.timeouts]
shell = 30
python = 120
custom_tool = 300
```

**Validation Requirements:**

```rust
// ✅ Good: Validate at load time
pub fn load_config(path: Option<&str>) -> Result<Config> {
    let path = path.unwrap_or("~/.silentclaw/config.toml");
    let content = std::fs::read_to_string(path)
        .context("Failed to read config")?;
    let config: Config = toml::from_str(&content)
        .context("Invalid TOML syntax")?;
    config.validate()?;  // Check required fields
    Ok(config)
}

// ❌ Bad: Partial validation or late detection
pub fn load_config(path: Option<&str>) -> Config {
    // Returns invalid config that fails later
}
```

## Plan JSON Format

**Schema:**

```json
{
  "version": "1.0",
  "metadata": {
    "name": "Example Plan",
    "description": "Demonstrates tool orchestration"
  },
  "steps": [
    {
      "id": "unique_step_id",
      "tool": "shell",
      "description": "Optional description",
      "input": {
        "cmd": "echo hello"
      },
      "timeout": 30
    }
  ]
}
```

**Validation Rules:**

- `version` must be "1.0"
- `steps` array required, non-empty
- Each step must have `id` and `tool`
- `id` must be unique within plan
- `input` is tool-specific (validated by tool)

## Git Workflow

### Commit Messages

**Format:** Conventional Commits

```
<type>(<scope>): <subject>

<body>

<footer>
```

**Types:**
- **feat:** New feature
- **fix:** Bug fix
- **refactor:** Code reorganization (no functional change)
- **test:** Add or update tests
- **docs:** Documentation only
- **chore:** Build, dependencies, CI/CD

**Examples:**

```
✅ Good:
feat(python-adapter): add stderr reader to prevent deadlock
fix(runtime): atomic tool registration to prevent race condition
test(shell-tool): add edge case for large command output
refactor(cli): consolidate timeout configuration logic

❌ Bad:
Fixed stuff
Update code
Added feature
Changes
```

### PR Requirements

Before merging:

- [ ] Runs `cargo fmt --all`
- [ ] Runs `cargo clippy --all -- -D warnings` (zero warnings)
- [ ] All tests pass: `cargo test --all`
- [ ] Commit messages follow conventional format
- [ ] No commented-out code
- [ ] Public APIs documented
- [ ] Changes reflected in CHANGELOG.md

## Build & Release

### Debug Build

```bash
cargo build
# Generates: target/debug/warden
```

### Release Build

```bash
cargo build --release
# Generates: target/release/warden
# Optimizations: -O2, link-time optimization enabled
```

### Testing

```bash
# Run all tests
cargo test --all

# Run with logging visible
RUST_LOG=debug cargo test --all -- --nocapture

# Run single test
cargo test --all shell_tool_tests
```

### Documentation

```bash
# Generate and open Rust docs
cargo doc --open

# Build without opening
cargo doc --no-deps
```

## Production Hardening Patterns

### 14. Bearer Token Authentication

**Middleware pattern for HTTP API security:**

```rust
use axum::middleware::Next;
use axum::http::{Request, StatusCode};

pub async fn auth_middleware<B>(
    req: Request<B>,
    next: Next,
) -> Result<impl IntoResponse, StatusCode> {
    // Extract Authorization header
    let token = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // Validate token
    if !validate_token(token)? {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}

// Apply to routes
let app = Router::new()
    .route("/sessions", post(create_session))
    .layer(middleware::from_fn(auth_middleware));
```

### 15. Rate Limiting (Token Bucket)

**Distribute fairly using concurrent token bucket:**

```rust
use dashmap::DashMap;

pub struct RateLimiter {
    buckets: DashMap<String, TokenBucket>,
    tokens_per_sec: u32,
}

impl RateLimiter {
    pub fn check_rate_limit(&self, client_id: &str) -> Result<(), StatusCode> {
        let mut entry = self.buckets
            .entry(client_id.to_string())
            .or_insert_with(|| TokenBucket::new(self.tokens_per_sec));

        if entry.consume_token() {
            Ok(())
        } else {
            Err(StatusCode::TOO_MANY_REQUESTS)
        }
    }
}

// Apply to routes
let rate_limiter = Arc::new(RateLimiter::new(100)); // 100 req/sec per client

let app = Router::new()
    .route("/api", post(handler))
    .layer(middleware::from_fn(move |req, next| {
        let client_id = get_client_id(&req)?;
        rate_limiter.check_rate_limit(&client_id)?;
        Ok(next.run(req).await)
    }));
```

### 16. Input Size Validation

**Enforce message size limits in WebSocket:**

```rust
const MAX_MESSAGE_BYTES: usize = 51200; // 50KB

pub async fn handle_websocket(
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(|mut socket| async move {
        while let Some(msg) = socket.recv().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if text.len() > MAX_MESSAGE_BYTES {
                        let _ = socket.send(Message::Close(None)).await;
                        return;
                    }
                    // Process message
                }
                _ => {}
            }
        }
    })
}
```

### 17. Graceful Shutdown with Drain

**Allow in-flight requests to complete:**

```rust
use tokio::sync::broadcast;

pub async fn start_server() -> Result<()> {
    let (shutdown_tx, _) = broadcast::channel(1);
    let server = axum::Server::bind(&addr)
        .serve(app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_tx.subscribe().recv().await;
        });

    // Signal handler
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        shutdown_tx.send(()).ok();  // Trigger shutdown
    });

    // Graceful drain: 10s timeout
    tokio::time::timeout(
        Duration::from_secs(10),
        server
    ).await??;

    Ok(())
}
```

### 18. Config Validation at Load Time

**Catch errors early, not at runtime:**

```rust
pub struct Config {
    pub version: u32,
    pub runtime: RuntimeConfig,
    pub gateway: GatewayConfig,
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        // Check version compatibility
        if self.version != CURRENT_VERSION {
            anyhow::bail!("Config version {} not supported", self.version);
        }

        // Validate required fields
        if self.gateway.bind.is_empty() {
            anyhow::bail!("gateway.bind is required");
        }

        // Validate constraints
        if self.runtime.max_parallel == 0 {
            anyhow::bail!("runtime.max_parallel must be > 0");
        }

        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        config.validate()?;  // Validate immediately
        Ok(config)
    }
}
```

### 19. Environment Variable Overrides

**Allow runtime configuration without file changes:**

```rust
use std::env;

pub fn load_config_with_env(path: &Path) -> Result<Config> {
    let mut config = Config::load(path)?;

    // Allow env var overrides for sensitive values
    if let Ok(timeout) = env::var("SILENTCLAW_TIMEOUT") {
        config.runtime.timeout_secs = timeout.parse()?;
    }

    if let Ok(api_key) = env::var("ANTHROPIC_API_KEY") {
        config.llm.api_key = api_key;
    }

    Ok(config)
}
```

### 20. Per-Hook Timeout & Critical Flags

**Prevent hooks from blocking execution:**

```rust
#[async_trait]
pub trait Hook: Send + Sync {
    async fn handle(&self, context: HookContext) -> Result<HookResult>;
    fn timeout(&self) -> Duration { Duration::from_secs(5) }  // Default
    fn critical(&self) -> bool { false }  // Non-blocking by default
}

pub async fn execute_hooks(hooks: &[Arc<dyn Hook>]) -> Result<()> {
    for hook in hooks {
        let timeout = hook.timeout();
        let critical = hook.critical();

        match tokio::time::timeout(timeout, hook.handle(context)).await {
            Ok(Ok(result)) => { /* continue */ }
            Ok(Err(e)) => {
                if critical {
                    return Err(e);  // Abort on critical failure
                } else {
                    warn!(error = ?e, "Hook failed (non-critical)");
                }
            }
            Err(_) => {
                if critical {
                    anyhow::bail!("Critical hook timeout");
                } else {
                    warn!("Hook timeout (non-critical)");
                }
            }
        }
    }
    Ok(())
}
```

### 21. Vision/Multimodal Content Encoding

**Base64 encode images for LLM APIs:**

```rust
use base64::{engine::general_purpose, Engine as _};
use std::fs;

pub fn encode_image_for_anthropic(image_path: &Path) -> Result<String> {
    // Determine media type
    let extension = image_path.extension()
        .and_then(|s| s.to_str())
        .ok_or(anyhow::anyhow!("Invalid image path"))?;

    let media_type = match extension {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => anyhow::bail!("Unsupported image format: {}", extension),
    };

    // Read and encode
    let image_data = fs::read(image_path)?;
    let base64_image = general_purpose::STANDARD.encode(&image_data);

    Ok(format!("data:{};base64,{}", media_type, base64_image))
}
```

### 22. Cumulative Token Tracking

**Monitor LLM usage without expensive queries:**

```rust
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Usage {
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
}

impl Usage {
    pub fn add_input(&self, tokens: u64) {
        self.input_tokens.fetch_add(tokens, Ordering::Relaxed);
    }

    pub fn add_output(&self, tokens: u64) {
        self.output_tokens.fetch_add(tokens, Ordering::Relaxed);
    }

    pub fn total(&self) -> u64 {
        let input = self.input_tokens.load(Ordering::Relaxed);
        let output = self.output_tokens.load(Ordering::Relaxed);
        input + output
    }
}

// In Session
pub struct Session {
    pub id: String,
    pub usage: Usage,  // Track cumulatively
}

impl Session {
    pub fn warn_if_approaching_limit(&self) {
        let total = self.usage.total();
        let limit = 100_000; // Example limit
        if total > (limit as f64 * 0.8) as u64 {
            warn!(tokens = total, "Approaching token limit");
        }
    }
}
```

## Security Guidelines

### Input Validation

**Always validate user/config input:**

```rust
// ✅ Good: Validate before use
pub fn execute_command(cmd: &str) -> Result<()> {
    validate_command(cmd)?;  // Check for dangerous patterns
    execute_impl(cmd).await
}

fn validate_command(cmd: &str) -> Result<()> {
    if cmd.contains("rm -rf") {
        anyhow::bail!("Dangerous command pattern detected");
    }
    Ok(())
}

// ❌ Bad: No validation
pub async fn execute_command(cmd: &str) -> Result<()> {
    execute_impl(cmd).await
}
```

### Safe Defaults

**Always default to safe mode:**

```toml
# ✅ Good: Safe by default
[runtime]
dry_run = true          # Prevent accidents
allow_tools = false     # Explicit opt-in for execution

# ❌ Bad: Dangerous by default
[runtime]
dry_run = false         # Executes everything
allow_tools = true      # No safety check
```

### Subprocess Isolation

**Keep subprocess communication well-defined:**

```rust
// ✅ Good: Defined protocol
pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
    // JSON-over-stdio with validation
    validate_params(params)?;
    send_request(method, params)?;
    receive_response().await
}

// ❌ Bad: Pass through without validation
pub async fn call(&mut self, raw_json: &str) -> Result<Value> {
    // Accepts any JSON, could be exploited
}
```

## Performance Considerations

### Async Patterns

**Use DashMap for lock-free concurrent access:**

```rust
use dashmap::DashMap;

let tools = DashMap::new();
tools.insert("shell", Arc::new(shell_tool));
// No mutex contention even with concurrent access
```

**Avoid unnecessary allocations:**

```rust
// ✅ Good: Single allocation
let result = compute_expensive_value();

// ❌ Bad: Multiple allocations
for i in 0..100 {
    let result = compute_expensive_value();
    results.push(result);  // Repeated allocations
}

// Better:
let mut results = Vec::with_capacity(100);
for i in 0..100 {
    results.push(compute_expensive_value());
}
```

### Storage Optimization

**Batch writes when possible:**

```rust
// ✅ Good: Single transaction for multiple steps
let mut tx = storage.transaction();
for step in &plan.steps {
    tx.write_result(&step.id, &result)?;
}
tx.commit()?;

// ❌ Bad: Transaction per step
for step in &plan.steps {
    storage.write_result(&step.id, &result)?;
}
```

## Phase 6 Code Review Fixes - Summary

**Key Improvements Applied:**

1. **Arc Pattern Cleanup** - Build runtime fully before Arc wrapping (safer, more idiomatic)
2. **Dry-Run Check Reordering** - Check before policy evaluation to prevent rate-limit inflation
3. **DRY Helpers** - Extract common patterns into helper functions (check_response, parse_permission_level)
4. **Safer Defaults** - Permission level defaults to Read (not Execute)
5. **Atomic Counters** - Use AtomicU64 for thread-safe, lock-free ID generation
6. **Structured Logging** - Add debug/info tracing to critical paths
7. **Type Safety** - ToolResult.name field for Gemini compatibility
8. **Function Signatures** - Simplified to &Runtime (not &Arc<Runtime>)

**Result:** Cleaner code, better patterns, safer defaults, no breaking changes.

---

## Maintenance & Troubleshooting

### Common Issues

| Issue | Cause | Solution |
|-------|-------|----------|
| Compile error: "trait object cannot be... impl Trait" | Trait with async method | Use `#[async_trait]` macro |
| Test timeout | Infinite loop or deadlock | Add `#[tokio::test]` timeout, check for deadlock |
| "Failed to spawn Python process" | Script not found or permissions | Validate script path exists |
| Tool not responding | Subprocess hang or stderr buffer full | Spawn stderr reader task |

### Profiling

**Find performance bottlenecks:**

```bash
# Release build with debugging info
cargo build --release --debug-assertions

# Profile with flamegraph
# (Install: cargo install flamegraph)
cargo flamegraph -- run-plan --file plan.json
```

## References

- **Rust Book:** https://doc.rust-lang.org/book/
- **Tokio Guide:** https://tokio.rs/tokio/tutorial
- **anyhow docs:** https://docs.rs/anyhow/latest/anyhow/
- **tracing guide:** https://tokio.rs/tokio/tutorial/tracing
- **Conventional Commits:** https://www.conventionalcommits.org/

---

**Document Maintainer:** Code Review Team
**Last Review:** 2026-02-16
**Next Review:** Upon major architectural changes or when team size changes
