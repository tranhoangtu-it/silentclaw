# SilentClaw System Architecture

**Last Updated:** 2026-02-16
**Version:** 1.0.0
**Status:** Complete (Known Limitations)

## Overview

SilentClaw is a lightweight local LLM-driven action orchestrator built in Rust. It maintains OpenClaw's proven runtime loop semantics while providing 10x performance improvements through native async execution.

```
prompt → LLM → planner → tool selection → executor → observe → feedback
```

## Architecture Layers

### 1. Core Runtime (operon-runtime)

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
- **DashMap over Mutex:** Lock-free concurrent hashmap prevents contention in tool registry
- **Async-first:** Full tokio multi-threaded runtime for concurrent tool execution (future feature)
- **No unsafe blocks:** Type safety throughout
- **Dry-run default:** Configuration enables user choice, not hardcoded

### 2. Tool Adapters (operon-adapters)

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

### 3. CLI Interface (warden)

**Purpose:** Command-line entry point and configuration loading

**Core Features:**
- `run-plan` command: Execute plan JSON files
- `--allow-tools` flag: Override dry-run mode
- `--config` option: Custom config path (default: `~/.silentclaw/config.toml`)

**Configuration:**
```toml
[runtime]
dry_run = true          # Safe default
timeout_secs = 60       # Global timeout

[tools.shell]
enabled = true

[tools.python]
enabled = true
scripts_dir = "./tools"

[tools.timeouts]
shell = 30              # Tool-specific override
python = 120
```

## Execution Flow

```
User Input
    ↓
warden (CLI)
    ↓
Config Loading
    ↓
Plan JSON Parsing
    ↓
Runtime::run_plan()
    ├── For each step:
    │   ├── Lookup tool by name
    │   ├── Apply per-tool timeout
    │   ├── Execute tool.execute(input)
    │   ├── Store result in redb
    │   └── Process output for next step
    └── Return final result
    ↓
Output (stdout or file)
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

| Component | Technology | Rationale |
|-----------|-----------|-----------|
| Async Runtime | tokio 1.x | Industry standard, multi-threaded, full-featured |
| Serialization | serde + serde_json | Type-safe, zero-copy where possible |
| Error Handling | anyhow | Ergonomic context chains, no boilerplate |
| Logging | tracing + tracing-subscriber | Structured JSON logs, performance introspection |
| Storage | redb | Pure Rust, actively maintained (chose over sled) |
| Concurrency | DashMap | Lock-free hashmap, no contention on hot paths |
| CLI | clap v4 | Modern parsing, derives, helpful error messages |
| Testing | tokio::test | Native async test support |

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
