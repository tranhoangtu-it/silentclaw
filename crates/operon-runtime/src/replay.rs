use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    pub plan_id: String,
    pub recorded_at: String,
    pub steps: Vec<StepRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    pub index: usize,
    pub tool: String,
    pub input: Value,
    pub output: Value,
    pub duration_ms: u64,
}

impl Fixture {
    pub fn new(plan_id: String) -> Self {
        Self {
            plan_id,
            recorded_at: timestamp_now(),
            steps: Vec::new(),
        }
    }

    /// Save fixture to JSON file
    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir).context("Failed to create fixture directory")?;
        let path = dir.join("fixture.json");
        let content = serde_json::to_string_pretty(self).context("Failed to serialize fixture")?;
        std::fs::write(&path, content).context(format!("Failed to write fixture: {:?}", path))?;
        Ok(())
    }

    /// Load fixture from JSON file
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("fixture.json");
        let content = std::fs::read_to_string(&path)
            .context(format!("Failed to read fixture: {:?}", path))?;
        let fixture: Self =
            serde_json::from_str(&content).context("Failed to parse fixture JSON")?;
        Ok(fixture)
    }
}

/// Simple Unix-epoch timestamp without chrono dependency
pub(crate) fn timestamp_now() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s", duration.as_secs())
}
