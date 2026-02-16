# SilentClaw - Code Standards & Development Guidelines

**Last Updated:** 2026-02-16
**Version:** 1.0.0
**Audience:** Developers, maintainers, contributors

## Project Overview

**SilentClaw** is a Rust-based action orchestrator providing:
- Async runtime with Tool trait abstraction
- Python subprocess adapter (JSON-over-stdio protocol)
- Shell command executor with safety defaults
- CLI interface for plan execution
- Structured JSON logging

**Codebase Stats:**
- **Total LOC:** 633 (Rust)
- **Clippy Warnings:** 0
- **Format Issues:** 0
- **Test Coverage:** 11 tests, 100% passing
- **Repository:** Monorepo workspace with 3 crates

## Codebase Structure

```
silentclaw/
├── Cargo.toml              # Workspace definition
├── Cargo.lock              # Dependency lock
├── README.md               # User guide
│
├── crates/
│   ├── operon-runtime/     # Core async runtime
│   │   └── src/
│   │       ├── lib.rs      # Public API (Tool, Runtime, Storage)
│   │       ├── tool.rs     # Tool trait definition
│   │       ├── runtime.rs  # Plan executor engine
│   │       └── storage.rs  # redb persistence layer
│   │
│   ├── operon-adapters/    # Tool implementations
│   │   └── src/
│   │       ├── lib.rs      # Public API
│   │       ├── python_adapter.rs  # JSON-over-stdio for Python
│   │       └── shell_tool.rs      # sh -c executor
│   │
│   └── warden/             # CLI binary
│       └── src/
│           ├── main.rs     # Entry point
│           ├── cli.rs      # Clap arguments
│           ├── config.rs   # TOML config loading
│           └── commands/
│               └── run_plan.rs    # Plan execution
│
├── examples/
│   ├── plan_hello.json     # Demo plan
│   └── echo_tool.py        # Python tool example
│
├── tools/
│   └── (User-provided Python tools)
│
├── tests/
│   ├── shell_tool_tests.rs
│   ├── runtime_tests.rs
│   └── cli_integration_tests.rs
│
└── docs/
    ├── system-architecture.md
    ├── known-limitations.md
    ├── code-standards.md (this file)
    └── codebase-summary.md
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

### 3. Async Patterns

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

### 4. Type Safety

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

### 5. Logging & Observability

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

### 6. Documentation & Comments

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

### 7. Testing

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

### 8. Code Formatting

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

### 9. Dependency Management

**Keep dependencies minimal:**

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }  # ✅ Essential for async
serde = { version = "1", features = ["derive"] }  # ✅ Essential for JSON
anyhow = "1"                                       # ✅ Essential for errors
tracing = "0.1"                                    # ✅ Essential for logging

# NOT: Random dependencies for convenience
fancy_string_lib = "1.0"  # ❌ Avoid unless critical
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
