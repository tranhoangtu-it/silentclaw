use anyhow::{Context, Result};
use async_trait::async_trait;
use operon_runtime::{PermissionLevel, Tool, ToolSchemaInfo};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

pub struct PyAdapter {
    /// Mutex-protected stdin+stdout for atomic request-response
    io: Mutex<(ChildStdin, BufReader<ChildStdout>)>,
    script_path: String,
    request_id: AtomicU64,
    /// Handle for kill on drop
    child_handle: std::sync::Mutex<Option<Child>>,
    /// Background stderr reader task
    stderr_handle: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for PyAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyAdapter")
            .field("script_path", &self.script_path)
            .field("request_id", &self.request_id)
            .finish_non_exhaustive()
    }
}

impl PyAdapter {
    /// Spawn Python script subprocess
    pub async fn spawn(script_path: &str) -> Result<Self> {
        // Validate script path exists and is a file
        let path = Path::new(script_path);
        if !path.exists() {
            anyhow::bail!("Python script not found: {}", script_path);
        }
        if !path.is_file() {
            anyhow::bail!("Python script path is not a file: {}", script_path);
        }

        let mut child = Command::new("python3")
            .arg(script_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("Failed to spawn Python process")?;

        let stdin = child.stdin.take().context("Failed to get stdin")?;
        let stdout = child.stdout.take().context("Failed to get stdout")?;
        let reader = BufReader::new(stdout);

        // Spawn background stderr reader to prevent deadlock
        let stderr_handle = if let Some(stderr) = child.stderr.take() {
            let script = script_path.to_string();
            Some(tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) => break, // EOF - subprocess exited
                        Ok(_) => {
                            warn!(
                                script = %script,
                                stderr = %line.trim(),
                                "Python stderr"
                            );
                        }
                        Err(e) => {
                            warn!(error = ?e, "Failed to read Python stderr");
                            break;
                        }
                    }
                }
            }))
        } else {
            None
        };

        debug!(script = script_path, "Python subprocess spawned");

        Ok(Self {
            io: Mutex::new((stdin, reader)),
            script_path: script_path.to_string(),
            request_id: AtomicU64::new(0),
            child_handle: std::sync::Mutex::new(Some(child)),
            stderr_handle,
        })
    }

    /// Spawn with custom timeout (deprecated — timeout now managed by Runtime)
    #[deprecated(note = "Timeout now managed by Runtime. Use spawn() instead.")]
    pub async fn spawn_with_timeout(script_path: &str, _timeout: Duration) -> Result<Self> {
        Self::spawn(script_path).await
    }

    /// Call Python method with params (takes &self, thread-safe)
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst) + 1;

        let request = json!({
            "id": id,
            "method": method,
            "params": params
        });

        let request_line = serde_json::to_string(&request)? + "\n";

        // Lock io pair for atomic request-response
        let mut io = self.io.lock().await;
        let (ref mut stdin, ref mut reader) = *io;

        // Write request
        stdin
            .write_all(request_line.as_bytes())
            .await
            .context("Failed to write request")?;
        stdin.flush().await?;

        debug!(id, method, "Sent request to Python");

        // Read response
        let mut response_line = String::new();
        reader
            .read_line(&mut response_line)
            .await
            .context("Failed to read response")?;

        let response: Value =
            serde_json::from_str(&response_line).context("Failed to parse JSON response")?;

        debug!(id, "Received response from Python");

        // Validate response ID matches request
        if let Some(resp_id) = response.get("id").and_then(|v| v.as_u64()) {
            if resp_id != id {
                anyhow::bail!(
                    "Response ID mismatch: expected {}, got {} (method: {})",
                    id,
                    resp_id,
                    method
                );
            }
        }

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

    /// Gracefully shut down the Python subprocess
    pub async fn shutdown(&mut self) -> Result<()> {
        // Cancel stderr reader
        if let Some(handle) = self.stderr_handle.take() {
            handle.abort();
        }

        // Kill and wait for child — take out of mutex before awaiting
        let child = self
            .child_handle
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        if let Some(mut child) = child {
            child
                .kill()
                .await
                .context("Failed to kill Python subprocess")?;
            child
                .wait()
                .await
                .context("Failed to wait for Python subprocess")?;
            debug!(script = %self.script_path, "Python subprocess shut down cleanly");
        }

        Ok(())
    }
}

#[async_trait]
impl Tool for PyAdapter {
    async fn execute(&self, input: Value) -> Result<Value> {
        let method = input["method"]
            .as_str()
            .context("Input missing 'method' field")?;
        let params = input.get("params").cloned().unwrap_or(json!({}));
        self.call(method, params).await
    }

    fn name(&self) -> &str {
        &self.script_path
    }

    fn schema(&self) -> ToolSchemaInfo {
        ToolSchemaInfo {
            name: self.script_path.clone(),
            description: format!("Execute Python script: {}", self.script_path),
            parameters: json!({
                "type": "object",
                "properties": {
                    "method": {
                        "type": "string",
                        "description": "Python method to call"
                    },
                    "params": {
                        "type": "object",
                        "description": "Parameters to pass to the method"
                    }
                },
                "required": ["method"]
            }),
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }
}

impl Drop for PyAdapter {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child_handle.lock() {
            if let Some(ref mut child) = *guard {
                if let Err(e) = child.start_kill() {
                    warn!(error = ?e, "Failed to kill Python subprocess");
                }
            }
        }
    }
}

/// Scan directory for .py files, spawn PyAdapter for each
pub async fn discover_python_tools(scripts_dir: &str) -> Result<Vec<(String, PyAdapter)>> {
    let dir = Path::new(scripts_dir);
    if !dir.exists() || !dir.is_dir() {
        warn!(
            dir = scripts_dir,
            "Python scripts directory not found, skipping auto-discovery"
        );
        return Ok(Vec::new());
    }

    let mut tools = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "py") {
            let tool_name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .context("Invalid script filename")?
                .to_string();
            let adapter = PyAdapter::spawn(path.to_str().context("Invalid path encoding")?).await?;
            info!(tool = %tool_name, path = ?path, "Auto-discovered Python tool");
            tools.push((tool_name, adapter));
        }
    }

    Ok(tools)
}
