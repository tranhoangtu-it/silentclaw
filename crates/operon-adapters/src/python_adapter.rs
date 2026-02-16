use anyhow::{Context, Result};
use async_trait::async_trait;
use operon_runtime::Tool;
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tracing::{debug, warn};

pub struct PyAdapter {
    child: Child,
    script_path: String,
    timeout: Duration,
    request_id: u64,
}

impl PyAdapter {
    /// Spawn Python script subprocess
    pub async fn spawn(script_path: &str) -> Result<Self> {
        Self::spawn_with_timeout(script_path, Duration::from_secs(60)).await
    }

    /// Spawn with custom timeout
    pub async fn spawn_with_timeout(script_path: &str, timeout: Duration) -> Result<Self> {
        let child = Command::new("python3")
            .arg(script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn Python process")?;

        debug!(script = script_path, "Python subprocess spawned");

        Ok(Self {
            child,
            script_path: script_path.to_string(),
            timeout,
            request_id: 0,
        })
    }

    /// Call Python method with params
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.request_id += 1;
        let id = self.request_id;

        let request = json!({
            "id": id,
            "method": method,
            "params": params
        });

        // Write request
        let stdin = self.child.stdin.as_mut().context("Failed to get stdin")?;

        let request_line = serde_json::to_string(&request)? + "\n";

        tokio::time::timeout(self.timeout, stdin.write_all(request_line.as_bytes()))
            .await
            .context("Timeout writing to Python stdin")?
            .context("Failed to write request")?;

        stdin.flush().await?;

        debug!(id, method, "Sent request to Python");

        // Read response
        let stdout = self.child.stdout.as_mut().context("Failed to get stdout")?;

        let mut reader = BufReader::new(stdout);
        let mut response_line = String::new();

        tokio::time::timeout(self.timeout, reader.read_line(&mut response_line))
            .await
            .context("Timeout reading from Python stdout")?
            .context("Failed to read response")?;

        let response: Value =
            serde_json::from_str(&response_line).context("Failed to parse JSON response")?;

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
    }

    /// Configure timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait]
impl Tool for PyAdapter {
    async fn execute(&self, _input: Value) -> Result<Value> {
        // Note: Tool trait takes &self, but we need &mut for call()
        // For now, return error directing user to use call() directly
        Err(anyhow::anyhow!(
            "PyAdapter requires mutable reference. Use call() method instead of execute()"
        ))
    }

    fn name(&self) -> &str {
        &self.script_path
    }
}

impl Drop for PyAdapter {
    fn drop(&mut self) {
        // Kill child process on drop
        if let Err(e) = self.child.start_kill() {
            warn!(error = ?e, "Failed to kill Python subprocess");
        }
    }
}
