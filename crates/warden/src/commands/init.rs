use anyhow::Result;
use std::path::Path;

const DEFAULT_CONFIG: &str = r#"# SilentClaw Configuration
version = 1

[runtime]
dry_run = true
timeout_secs = 60
max_parallel = 4

[tools.shell]
enabled = true
blocklist = ["rm -rf", "mkfs", "dd if="]
allowlist = []

[tools.python]
enabled = true
scripts_dir = "./tools/python_examples"

[tools.timeouts]

[llm]
provider = "anthropic"
model = ""
"#;

/// Initialize a new config file
pub fn run_init(path: &Path) -> Result<()> {
    if path.exists() {
        anyhow::bail!("Config already exists at {:?}", path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, DEFAULT_CONFIG)?;
    println!("Created config at {:?}", path);
    Ok(())
}
