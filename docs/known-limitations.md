# SilentClaw - Known Limitations

**Document Date:** 2026-02-16
**Last Review:** Code Review Session 2026-02-16
**Status:** Active Tracking

This document comprehensively tracks all known issues, their severity, impact, and recommended mitigations.

---

## Critical Issues (P0 - Blocking)

### 1. PyAdapter Tool Trait Incompatibility

**Status:** BLOCKING - Python tools non-operational

**Severity:** 🔴 Critical

**Location:** `/crates/operon-adapters/src/python_adapter.rs` lines 106-113

**Problem:**

PyAdapter implements the Tool trait but `execute(&self)` always returns an error:

```rust
#[async_trait]
impl Tool for PyAdapter {
    async fn execute(&self, _input: Value) -> Result<Value> {
        Err(anyhow::anyhow!(
            "PyAdapter requires mutable reference. Use call() method instead of execute()"
        ))
    }
}
```

**Root Cause:**

The Tool trait requires `&self` (immutable reference), but PyAdapter's `call()` method needs `&mut self` to increment the request_id counter (line 46). This violates Liskov Substitution Principle - the tool cannot fulfill its contract through the standard interface.

**Current Impact:**

- ❌ Runtime.run_plan() cannot execute Python tools
- ❌ Example plan_hello.json step 2 (python tool) will always fail
- ❌ Python tool registration succeeds but execution fails silently
- ✅ Tests pass because test code uses `call()` directly, bypassing Tool trait

**Affected Versions:** All v1.0.x (since implementation)

**Recommended Fix:**

**Option 1: Use AtomicU64 (Recommended)**

Convert request_id to atomic for interior mutability:

```rust
use std::sync::atomic::{AtomicU64, Ordering};

pub struct PyAdapter {
    child: Child,
    script_path: String,
    timeout: Duration,
    request_id: AtomicU64,  // Changed from u64
}

// In call():
let id = self.request_id.fetch_add(1, Ordering::SeqCst);

// Now execute() can work:
#[async_trait]
impl Tool for PyAdapter {
    async fn execute(&self, input: Value) -> Result<Value> {
        let method = input["method"].as_str().context("Missing 'method' field")?;
        let params = input.get("params").cloned().unwrap_or(json!({}));
        self.call(method, params).await
    }
}
```

**Option 2: Arc<Mutex<Child>> (Complex)**

Wrap subprocess in Arc<Mutex<>> for shared mutable access. More complex but enables future parallel execution.

**Workaround:** None - Python tools are non-functional

**Priority:** P0 - Must fix before any production deployment with Python tools

**Tracking:** [BLOCKER]

---

## High Priority Issues (P1)

### 2. Python Subprocess Stderr Deadlock Risk

**Status:** UNFIXED - Risk present

**Severity:** 🔴 High (Reliability)

**Location:** `/crates/operon-adapters/src/python_adapter.rs` lines 28-30

**Problem:**

PyAdapter pipes stderr but never reads from it. POSIX pipes have limited buffer size (typically 64KB). If a Python script writes more than this to stderr, the pipe fills and the Python process blocks:

```rust
let child = Command::new("python3")
    .arg(script_path)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())   // ← PIPED but NEVER READ
    .spawn()?;
```

**Deadlock Scenario:**

1. Python script writes 100KB to stderr (exceeds pipe buffer)
2. Python process blocks waiting for buffer space
3. Rust process blocks waiting for stdout response
4. **DEADLOCK** - both processes waiting forever until timeout

**Current Impact:**

- ⚠️ Risk increases with verbose Python tools (logging, debugging output)
- ⚠️ Timeout (60s default) eventually recovers but loses execution
- ✅ Simple test tools with minimal stderr don't trigger (all tests pass)
- ❌ Production tools with logging or errors will deadlock

**Affected Versions:** All v1.0.x

**Recommended Fix:**

**Option 1: Spawn Stderr Reader Task (Recommended)**

```rust
pub async fn spawn_with_timeout(script_path: &str, timeout: Duration) -> Result<Self> {
    let mut child = Command::new("python3")
        .arg(script_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn Python process")?;

    // Spawn background task to drain stderr
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 { break; }  // EOF
                warn!("Python stderr: {}", line.trim());
                line.clear();
            }
        });
    }

    Ok(Self {
        child,
        script_path: script_path.to_string(),
        timeout,
        request_id: 0,
    })
}
```

**Option 2: Inherit Stderr (Simpler)**

```rust
.stderr(Stdio::inherit())  // Pass through to parent stderr
```

Simpler but loses error capture capability.

**Workaround:** Monitor subprocess for hanging, increase timeout

**Priority:** P1 - Critical for production reliability with complex Python tools

**Tracking:** [RELIABILITY]

---

### 3. Shell Command Injection Risk

**Status:** UNFIXED - Mitigated by dry-run

**Severity:** 🔴 High (Security)

**Location:** `/crates/operon-adapters/src/shell_tool.rs` line 43

**Problem:**

ShellTool executes arbitrary shell commands with no validation:

```rust
Command::new("sh").arg("-c").arg(cmd).output()
```

**Attack Vector:**

```json
{
  "tool": "shell",
  "input": {
    "cmd": "echo hello; rm -rf /critical/data; echo done"
  }
}
```

An attacker or malicious plan could chain commands and cause data loss.

**Current Mitigations:**

- ✅ Dry-run mode enabled by default (config.rs line 49)
- ✅ Explicit `--allow-tools` required to enable execution
- ✅ Warning logs in dry-run mode (shell_tool.rs line 32)
- ✅ Timeout prevents runaway processes

**Gaps:**

- ❌ No input validation or sanitization
- ❌ No command allowlist/blocklist
- ❌ User can disable safety via `--allow-tools` flag
- ❌ No audit logging of executed commands

**Threat Model:**

**Safe Scenario:**
- Plan from trusted internal source
- Reviewed before execution
- `--allow-tools` used consciously

**Unsafe Scenario:**
- Plan from untrusted source (URL, user input)
- Automatic execution without review
- Security expectation for command isolation

**Current Impact:**

- ✅ Safe by default (dry-run blocks execution)
- ⚠️ User can disable safety, full system access
- ❌ No defense if user runs with `--allow-tools`

**Affected Versions:** All v1.0.x

**Recommended Fixes:**

**Priority 1: Add Command Validation**

```rust
fn validate_command(cmd: &str) -> Result<()> {
    // Blocklist dangerous patterns
    let dangerous = [
        "rm -rf",      // Recursive delete
        "dd if=",      // Raw disk access
        "mkfs",        // Format filesystem
        ":(){ :|:& };:", // Fork bomb
        "> /dev/sda",  // Direct disk write
    ];

    for pattern in dangerous {
        if cmd.contains(pattern) {
            anyhow::bail!("Dangerous command pattern detected: {}", pattern);
        }
    }

    Ok(())
}

// In execute():
validate_command(&cmd)?;
```

**Priority 2: Optional Allowlist Mode**

```rust
// Allowlist mode (opt-in via env var)
if let Ok(allowlist) = env::var("SHELL_ALLOWLIST") {
    let allowed = allowlist.split(',').collect::<Vec<_>>();
    let cmd_name = cmd.split_whitespace().next().unwrap_or("");
    if !allowed.contains(&cmd_name) {
        anyhow::bail!("Command '{}' not in allowlist", cmd_name);
    }
}
```

**Priority 3: Structured Command API (Future)**

```rust
pub enum SafeCommand {
    Echo { message: String },
    Cat { file: PathBuf },
    Mkdir { path: PathBuf },
    // etc - limit to safe operations
}
```

Replace freeform shell with enum-based dispatch.

**Workaround:** Use dry-run mode only, carefully review `--allow-tools` usage

**Priority:** P1 - Add before production deployment in any network-accessible context

**Tracking:** [SECURITY]

---

### 4. Timeout Configuration Duplication

**Status:** UNFIXED - Dead code present

**Severity:** 🟡 High (Maintainability)

**Location:** `/crates/warden/src/commands/run_plan.rs` lines 33-46 and `/crates/operon-runtime/src/runtime.rs` line 79

**Problem:**

Timeout configured twice in the system:

```rust
// Location 1: Tool construction (DEAD CODE)
let shell_timeout = config.tools.timeouts.get("shell")
    .copied()
    .unwrap_or(config.runtime.timeout_secs);
let shell_tool = ShellTool::new(dry_run)
    .with_timeout(Duration::from_secs(shell_timeout));  // ← Set here

// Location 2: Runtime registration (ACTIVE)
runtime.configure_timeout("shell".to_string(),
                         Duration::from_secs(shell_timeout));  // ← Takes precedence
```

**Current Behavior:**

Runtime's `configure_timeout()` overwrites tool-level timeout setting. The tool's `with_timeout()` method becomes dead code.

**Current Impact:**

- ⚠️ Confusion about which timeout is active
- ⚠️ Tool-level timeout field never used
- ✅ Functional but unclear precedence
- ❌ Maintenance burden (future developers may rely on dead path)

**Affected Versions:** All v1.0.x

**Recommended Fix (Choose One):**

**Option 1: Remove Tool-Level Timeout (Recommended)**

```rust
let shell_tool = ShellTool::new(dry_run);  // No with_timeout()
runtime.register_tool("shell".to_string(), Arc::new(shell_tool));

// Only configure at runtime level
if let Some(&timeout_secs) = config.tools.timeouts.get("shell") {
    runtime.configure_timeout("shell".to_string(),
                             Duration::from_secs(timeout_secs));
}
```

**Option 2: Remove Runtime Timeout Override**

```rust
let shell_tool = ShellTool::new(dry_run)
    .with_timeout(Duration::from_secs(shell_timeout));
runtime.register_tool("shell".to_string(), Arc::new(shell_tool));

// Don't call runtime.configure_timeout()
```

**Option 3: Document Precedence**

Add to README:

> "Timeout precedence: Runtime.configure_timeout() > Tool.with_timeout(). Always set timeouts via runtime.configure_timeout() in run_plan context."

**Workaround:** Understand that runtime-level timeout takes precedence

**Priority:** P1 - Cleanup needed before production (prevents confusion)

**Tracking:** [MAINTAINABILITY]

---

## Medium Priority Issues (P2)

### 5. PyAdapter Timeout Race Condition

**Status:** UNFIXED - Potential data corruption

**Severity:** 🟡 High (Data Integrity)

**Location:** `/crates/operon-adapters/src/python_adapter.rs` lines 60-78

**Problem:**

Write and read operations have separate timeouts, causing protocol desynchronization:

```rust
// Write with independent timeout
tokio::time::timeout(self.timeout, stdin.write_all(request_line.as_bytes()))
    .await
    .context("Timeout writing to Python stdin")?
    .context("Failed to write request")?;

// Read with independent timeout
tokio::time::timeout(self.timeout, reader.read_line(&mut response_line))
    .await
    .context("Timeout reading from Python stdout")?
```

**Edge Case Scenario:**

1. 50% of JSON request written to stdin
2. Write timeout fires (network or slow subprocess)
3. Context error returned
4. Python subprocess blocks, reading incomplete JSON
5. Next request-response cycle starts but protocol out of sync
6. Both sides now misaligned (Rust expects response to request A, Python sending error for incomplete A)

**Current Impact:**

- ⚠️ Timeout defaults to 60s (unlikely to hit mid-transfer for typical data)
- ⚠️ Risk increases with large inputs or slow subprocess
- ❌ Data corruption risk if timeout fires at wrong moment
- ✅ Single-threaded execution limits concurrency (lower risk now)

**Affected Versions:** All v1.0.x

**Recommended Fix:**

Atomic request-response with single timeout:

```rust
pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
    self.request_id += 1;
    let id = self.request_id;

    let request = json!({
        "id": id,
        "method": method,
        "params": params
    });

    let request_line = serde_json::to_string(&request)? + "\n";

    // Single timeout for entire request-response cycle
    tokio::time::timeout(self.timeout, async {
        // Write request
        let stdin = self.child.stdin.as_mut()
            .context("Failed to get stdin")?;
        stdin.write_all(request_line.as_bytes()).await?;
        stdin.flush().await?;

        debug!(id, method, "Sent request to Python");

        // Read response
        let stdout = self.child.stdout.as_mut()
            .context("Failed to get stdout")?;

        let mut reader = BufReader::new(stdout);
        let mut response_line = String::new();

        reader.read_line(&mut response_line).await?;

        let response: Value = serde_json::from_str(&response_line)
            .context("Failed to parse JSON response")?;

        debug!(id, "Received response from Python");

        // Check for error
        if let Some(error) = response.get("error") {
            if !error.is_null() {
                return Err(anyhow::anyhow!("Python error: {}", error));
            }
        }

        response
            .get("result")
            .cloned()
            .context("Response missing 'result' field")
    })
    .await
    .context("Request-response cycle timeout")??
}
```

**Workaround:** Use reasonable timeout values, monitor for desynchronization

**Priority:** P2 - Fix before high-throughput usage

**Tracking:** [DATA-INTEGRITY]

---

### 6. Python Script Path Validation Missing

**Status:** UNFIXED - Late error detection

**Severity:** 🟡 High (DX)

**Location:** `/crates/operon-adapters/src/python_adapter.rs` lines 25-32

**Problem:**

spawn() succeeds even if script doesn't exist. Failure detected on first call():

```rust
pub async fn spawn_with_timeout(script_path: &str, timeout: Duration) -> Result<Self> {
    let child = Command::new("python3")
        .arg(script_path)  // ← No validation
        .spawn()           // ← May fail here, but error is unclear
        .context("Failed to spawn Python process")?;
    // ...
}
```

**Current Behavior:**

```
$ warden run-plan --file plan.json  # References non-existent tool.py
✓ Plan loaded
✓ Tool registered
✗ Step 1 failed: Failed to spawn Python process
```

Error happens during execution, not registration.

**Current Impact:**

- ⚠️ Error message unclear ("Failed to spawn" - could be permission, missing python3, etc.)
- ⚠️ Delays feedback until runtime
- ✅ Documented in test suite (test explicitly validates this behavior)

**Affected Versions:** All v1.0.x

**Recommended Fix:**

Validate before spawn:

```rust
use std::path::Path;

pub async fn spawn_with_timeout(script_path: &str, timeout: Duration) -> Result<Self> {
    // Validate script exists
    if !Path::new(script_path).exists() {
        anyhow::bail!("Python script not found: {}", script_path);
    }

    // Ensure it's a file, not directory
    if !Path::new(script_path).is_file() {
        anyhow::bail!("Script path is not a file: {}", script_path);
    }

    // Check if executable (optional)
    #[cfg(unix)]
    if !std::os::unix::fs::MetadataExt::permissions(
        &std::fs::metadata(script_path)?,
    ).mode() & 0o111 != 0 {
        warn!("Script not executable: {}", script_path);
    }

    let child = Command::new("python3")
        .arg(script_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn Python process")?;

    Ok(Self {
        child,
        script_path: script_path.to_string(),
        timeout,
        request_id: 0,
    })
}
```

**Workaround:** Check script paths before plan execution

**Priority:** P2 - Improves error messages significantly

**Tracking:** [UX]

---

### 7. Dry-Run Bypass Confusion (CLI Logic)

**Status:** UNFIXED - UX issue

**Severity:** 🟠 Medium (UX Risk)

**Location:** `/crates/warden/src/commands/run_plan.rs` lines 20-25

**Problem:**

CLI flag `--allow-tools` defaults to false but unconditionally overrides config when true:

```rust
let dry_run = if allow_tools {
    false  // --allow-tools=true means dry_run=false
} else {
    config.runtime.dry_run  // Else use config setting
};
```

Double-negative logic can confuse users:

| Config | Flag | Result |
|--------|------|--------|
| dry_run=true | (not set) | dry_run=true ✅ |
| dry_run=true | --allow-tools | dry_run=false ❌ |
| dry_run=false | (not set) | dry_run=false ✅ |
| dry_run=false | --allow-tools | dry_run=false ✅ |

**Confusion Scenario:**

```bash
# User sets config: dry_run = true (for safety)
$ warden run-plan --file dangerous.json --allow-tools
# User expects: "Will run in dry-run mode"
# Actual: "Executes real commands"
```

**Current Impact:**

- ⚠️ Double-negative logic requires mental parsing
- ⚠️ User safety expectation violated (sets config=safe, but flag disables it)
- ✅ Functional but counterintuitive

**Affected Versions:** All v1.0.x

**Recommended Fix:**

Three-state execution mode:

```rust
#[derive(ValueEnum, Clone, Debug)]
pub enum ExecutionMode {
    /// Use config.runtime.dry_run setting (default)
    Auto,
    /// Force dry-run regardless of config
    DryRun,
    /// Force real execution regardless of config
    AllowTools,
}

#[derive(Parser)]
pub struct Cli {
    #[arg(long, default_value = "auto", value_enum)]
    pub execution_mode: ExecutionMode,
    // ...
}

// In run_plan command:
let dry_run = match cli.execution_mode {
    ExecutionMode::Auto => config.runtime.dry_run,
    ExecutionMode::DryRun => true,
    ExecutionMode::AllowTools => false,
};
```

**Usage Examples:**

```bash
warden run-plan --file plan.json
# Uses config setting

warden run-plan --file plan.json --execution-mode dry-run
# Forces dry-run regardless of config

warden run-plan --file plan.json --execution-mode allow-tools
# Forces real execution (explicit user intent)
```

**Workaround:** Understand current behavior, verify config before running with --allow-tools

**Priority:** P2 - UX improvement, current behavior is functional

**Tracking:** [UX]

---

## Low Priority Issues (P3-P4)

### 8. PyAdapter Zombie Process Risk

**Status:** UNFIXED - Minor cleanup issue

**Severity:** 🟠 Medium (Process Management)

**Location:** `/crates/operon-adapters/src/python_adapter.rs` lines 120-127

**Problem:**

Drop implementation sends SIGKILL but doesn't wait:

```rust
impl Drop for PyAdapter {
    fn drop(&mut self) {
        if let Err(e) = self.child.start_kill() {
            warn!(error = ?e, "Failed to kill Python subprocess");
        }
        // Missing: self.child.wait() - but can't await in Drop
    }
}
```

Creates zombie process until parent exits (can't await in Drop).

**Impact:**

- ⚠️ Zombie processes accumulate in very long-running applications
- ⚠️ Process table pollution (minor system resource issue)
- ✅ Clean exit but not perfect cleanup
- ✅ Low impact in CLI context (single process, short-lived)

**Affected Versions:** All v1.0.x

**Recommended Fix (Future):**

Add explicit async shutdown method:

```rust
impl PyAdapter {
    pub async fn shutdown(&mut self) -> Result<()> {
        self.child.kill().await?;
        let _ = self.child.wait().await;
        Ok(())
    }
}
```

Call explicitly before dropping in plan execution cleanup.

**Priority:** P3 - Minor in short-lived CLI context, important for long-running services

**Tracking:** [PROCESS-MANAGEMENT]

---

### 9. Runtime Tool Registry Race Condition

**Status:** UNFIXED - Theoretical risk (single-threaded context)

**Severity:** 🟠 Medium (Correctness)

**Location:** `/crates/operon-runtime/src/runtime.rs` lines 37-39

**Problem:**

Tools can be registered/unregistered during plan execution:

```rust
pub fn register_tool(&self, name: String, tool: Arc<dyn Tool>) {
    self.tools.insert(name, tool);  // No state check
}
```

**Race Scenario (Theoretical):**

Thread A calls `run_plan()`, Thread B calls `register_tool()` between steps. Step B expects different tool.

**Current Risk:**

- 🟢 **LOW** - warden CLI is single-threaded
- 🟢 All tools registered before run_plan() starts
- 🟠 **RISK** if library users create multi-threaded scenarios

**Affected Versions:** All v1.0.x

**Recommended Fix (For Library Users):**

Add runtime state machine:

```rust
pub struct Runtime {
    tools: Arc<DashMap<String, Arc<dyn Tool>>>,
    state: AtomicU8,  // 0=idle, 1=running
    // ...
}

pub fn register_tool(&self, name: String, tool: Arc<dyn Tool>) -> Result<()> {
    if self.state.load(Ordering::SeqCst) != 0 {
        anyhow::bail!("Cannot register tools while runtime is executing");
    }
    self.tools.insert(name, tool);
    Ok(())
}

pub async fn run_plan(&self, plan: Value) -> Result<()> {
    self.state.store(1, Ordering::SeqCst);
    // ... execute ...
    self.state.store(0, Ordering::SeqCst);
    Ok(())
}
```

**Priority:** P3 - Document single-threaded assumption or add guard

**Tracking:** [CONCURRENCY]

---

### 10. Error Context Loss in Timeout Handling

**Status:** UNFIXED - DX issue

**Severity:** 🟠 Medium (Developer Experience)

**Location:** `/crates/operon-runtime/src/runtime.rs` lines 83-86

**Problem:**

Nested .context() calls make timeout errors indistinguishable from execution errors:

```rust
let result = tokio::time::timeout(timeout, tool.execute(input))
    .await
    .context("Tool execution timeout")?      // ← Timeout error
    .context("Tool execution failed")?;      // ← Execution error
```

Error message shows "Tool execution failed" even on timeout, obscuring root cause.

**Impact:**

- ⚠️ Debugging difficulty
- ⚠️ Operator confusion about failure mode
- ✅ Functional but poor observability

**Affected Versions:** All v1.0.x

**Recommended Fix:**

Match on timeout explicitly:

```rust
let result = match tokio::time::timeout(timeout, tool.execute(input)).await {
    Err(_elapsed) => {
        anyhow::bail!("Tool '{}' timed out after {:?}", tool_name, timeout)
    }
    Ok(Err(e)) => {
        return Err(e).context(format!("Tool '{}' execution failed", tool_name))
    }
    Ok(Ok(result)) => result,
};
```

**Priority:** P3 - Debugging improvement

**Tracking:** [OBSERVABILITY]

---

## Issue Summary Table

| ID | Issue | Severity | Status | Priority | P0 Blocker |
|----|-------|----------|--------|----------|-----------|
| 1 | PyAdapter Tool trait incompatibility | 🔴 Critical | BLOCKING | P0 | ✅ YES |
| 2 | Python stderr deadlock | 🔴 High | UNFIXED | P1 | ❌ |
| 3 | Shell command injection | 🔴 High | Mitigated | P1 | ❌ |
| 4 | Timeout duplication | 🟡 High | UNFIXED | P1 | ❌ |
| 5 | Timeout race condition | 🟡 High | UNFIXED | P2 | ❌ |
| 6 | Script path validation | 🟡 High | UNFIXED | P2 | ❌ |
| 7 | Dry-run bypass confusion | 🟠 Medium | UNFIXED | P2 | ❌ |
| 8 | Zombie process risk | 🟠 Medium | UNFIXED | P3 | ❌ |
| 9 | Tool registry race | 🟠 Medium | UNFIXED | P3 | ❌ |
| 10 | Error context loss | 🟠 Medium | UNFIXED | P3 | ❌ |

---

## Deployment Guidance

### Safe for Immediate Use

✅ **Development & Testing:**
- Local development environments
- CI/CD testing (shell tool only)
- Design validation
- Documentation examples

✅ **Shell Tool Only:**
- Disable Python tools in config
- Use with trusted plans only
- Monitor for issues

### Requires Fixes Before Production

❌ **Cannot Deploy Yet:**

1. Fix **#1 (PyAdapter trait)** - Python tools completely non-functional
2. Fix **#2 (stderr deadlock)** - Reliability risk
3. Fix **#3 (command injection)** - Add validation layer
4. Add **Integration Test** - Validate full plan execution

### Safe Production (Shell Only)

```toml
[tools.python]
enabled = false  # Disable Python adapter

[tools.shell]
enabled = true
```

### Full Production (After Fixes)

When all P0/P1 issues resolved:

1. ✅ Fix PyAdapter (use AtomicU64)
2. ✅ Spawn stderr reader
3. ✅ Add command validation
4. ✅ Add integration test for full plan
5. ✅ Run complete test suite
6. ✅ Security review of shell validation

---

## Reporting New Issues

When reporting new issues:

1. **Title:** Clear description (e.g., "Shell tool hangs on large output")
2. **Severity:** Critical/High/Medium/Low
3. **Location:** File path and line numbers
4. **Reproduction:** Minimal example to trigger
5. **Current Behavior:** What happens
6. **Expected Behavior:** What should happen
7. **Impact:** Who/what is affected
8. **Workaround:** Temporary solution if available

---

## References

- **System Architecture:** `/docs/system-architecture.md`
- **Code Standards:** `/docs/code-standards.md`
- **Code Review Report:** `/plans/reports/code-reviewer-260216-0902-silentclaw-rust-rewrite.md`
- **README:** `/README.md`
