# SilentClaw

Lightweight local LLM-driven action orchestrator — Rust rewrite of OpenClaw focused on speed, reliability, and full system control.

## Overview

SilentClaw maintains OpenClaw's runtime loop semantics:
```
prompt → LLM → planner → tool selection → executor → observe → feedback
```

**Key Features:**
- 🦀 **Rust Performance:** Async runtime with tokio for concurrent tool execution
- 🐍 **Python Parity:** JSON-over-stdio adapter maintains compatibility with existing tools
- 🔒 **Sandbox Default:** Dry-run mode prevents accidental destructive operations
- 📊 **Structured Logs:** JSON logging via tracing-subscriber for observability
- 🎯 **Replay Mode:** Deterministic testing with fixture playback (coming soon)
- ⚙️ **Per-Tool Timeouts:** Configurable timeout for each tool type

## Quickstart

### Prerequisites
- Rust 1.70+ (stable)
- Python 3.8+ (for Python tools)

### Build & Run

```bash
# Clone repository
git clone https://github.com/<GITHUB_USER>/silentclaw.git
cd silentclaw

# Build release binary
cargo build --release

# Run example plan (dry-run mode)
./target/release/warden run-plan --file examples/plan_hello.json

# Run with real execution
./target/release/warden run-plan --file examples/plan_hello.json --allow-tools

# Enable structured logging
RUST_LOG=info ./target/release/warden run-plan --file examples/plan_hello.json
```

## Architecture

```
silentclaw/
├── crates/
│   ├── operon-runtime/    # Core async runtime + Tool trait
│   ├── operon-adapters/   # Python adapter + shell tool
│   └── warden/            # CLI binary
├── examples/              # Demo plan JSON files
└── tools/                 # Python example tools
```

**Components:**
- **operon-runtime:** Async Tool trait (associated types), Runtime orchestrator, redb storage
- **operon-adapters:** Python subprocess adapter (JSON-over-stdio), shell tool
- **warden:** CLI with run-plan command, TOML config, configurable dry-run mode

## Development

### Format Code
```bash
cargo fmt
```

### Lint
```bash
cargo clippy --all -- -D warnings
```

### Run Tests
```bash
cargo test --all
```

### Build Documentation
```bash
cargo doc --open
```

## Configuration

Create `~/.silentclaw/config.toml`:
```toml
[runtime]
dry_run = true          # Safe by default
timeout_secs = 60       # Default timeout

[tools.shell]
enabled = true

[tools.python]
enabled = true
scripts_dir = "./tools/python_examples"

# Per-tool timeout overrides
[tools.timeouts]
shell = 30
python = 120
```

## Migrating from OpenClaw

```bash
# Convert YAML config to TOML (skeleton implementation)
cargo run --bin migrate_openclaw_config -- --input ~/.openclaw/config.yaml --output ~/.silentclaw/config.toml
```

## Technical Decisions

Based on validation session, SilentClaw implements:
- **Tool Trait:** Associated types for zero-cost abstraction (performance-first)
- **Storage:** redb instead of sled (actively maintained, pure Rust)
- **Timeouts:** Per-tool configuration (flexibility over hardcoded defaults)
- **Dry-run:** Config-based default (user choice via config.toml)
- **Platform:** Full cross-platform support (Linux, macOS, Windows)

## License

MIT OR Apache-2.0
