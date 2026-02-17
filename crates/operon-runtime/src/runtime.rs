use crate::replay::{self, Fixture, StepRecord};
use crate::scheduler::{self, ScheduledStep};
use crate::tool::PermissionLevel;
use crate::tool_policy::{PolicyContext, ToolPolicyPipeline};
use crate::{Storage, Tool};
use anyhow::{Context, Result};
use dashmap::DashMap;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{info, warn};

const STATE_IDLE: u8 = 0;
const STATE_RUNNING: u8 = 1;

/// Controls how the runtime handles tool execution
#[derive(Debug, Clone)]
pub enum ExecutionContext {
    /// Normal execution (default)
    Normal,
    /// Record tool outputs to fixture directory
    Record(PathBuf),
    /// Replay from fixture directory (skip real tools)
    Replay(PathBuf),
}

pub struct Runtime {
    tools: Arc<DashMap<String, Arc<dyn Tool>>>,
    storage: Storage,
    dry_run: bool,
    default_timeout: Duration,
    tool_timeouts: DashMap<String, Duration>,
    state: AtomicU8,
    execution_context: ExecutionContext,
    max_parallel: usize,
    /// Optional policy pipeline evaluated before every tool execution
    policy: Option<ToolPolicyPipeline>,
}

impl Runtime {
    /// Create new runtime with dry-run flag and default timeout
    pub fn new(dry_run: bool, default_timeout: Duration) -> Result<Self> {
        Self::with_db("./silentclaw.db", dry_run, default_timeout)
    }

    /// Create new runtime with custom database path
    pub fn with_db(db_path: &str, dry_run: bool, default_timeout: Duration) -> Result<Self> {
        let storage = Storage::open(db_path)?;

        Ok(Self {
            tools: Arc::new(DashMap::new()),
            storage,
            dry_run,
            default_timeout,
            tool_timeouts: DashMap::new(),
            state: AtomicU8::new(STATE_IDLE),
            execution_context: ExecutionContext::Normal,
            max_parallel: 4,
            policy: None,
        })
    }

    /// Set execution context (record/replay)
    pub fn with_execution_context(mut self, ctx: ExecutionContext) -> Self {
        self.execution_context = ctx;
        self
    }

    /// Set max parallel concurrency
    pub fn with_max_parallel(mut self, max: usize) -> Self {
        self.max_parallel = max.max(1);
        self
    }

    /// Set tool policy pipeline (builder pattern)
    pub fn with_policy(mut self, pipeline: ToolPolicyPipeline) -> Self {
        self.policy = Some(pipeline);
        self
    }

    /// Set tool policy pipeline (mutable reference, for use after Arc creation)
    pub fn set_policy(&mut self, pipeline: ToolPolicyPipeline) {
        self.policy = Some(pipeline);
    }

    /// Register a tool. Fails if runtime is currently executing a plan.
    pub fn register_tool(&self, name: String, tool: Arc<dyn Tool>) -> Result<()> {
        if self.state.load(Ordering::SeqCst) != STATE_IDLE {
            anyhow::bail!("Cannot register tools while runtime is executing a plan");
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    /// Configure timeout for specific tool
    pub fn configure_timeout(&self, tool_name: String, timeout: Duration) {
        self.tool_timeouts.insert(tool_name, timeout);
    }

    /// Get timeout for tool (custom or default)
    pub fn get_timeout(&self, tool_name: &str) -> Duration {
        self.tool_timeouts
            .get(tool_name)
            .map(|t| *t)
            .unwrap_or(self.default_timeout)
    }

    /// Run plan JSON with state machine guard
    pub async fn run_plan(&self, plan: Value) -> Result<()> {
        // Transition Idle → Running (CAS prevents concurrent runs)
        if self
            .state
            .compare_exchange(
                STATE_IDLE,
                STATE_RUNNING,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_err()
        {
            anyhow::bail!("Runtime is already executing a plan");
        }

        let result = self.run_plan_inner(plan).await;

        // Transition Running → Idle (always, even on error)
        self.state.store(STATE_IDLE, Ordering::SeqCst);

        result
    }

    /// Core plan execution: routes to sequential or parallel based on dependencies
    async fn run_plan_inner(&self, plan: Value) -> Result<()> {
        let steps = scheduler::parse_steps(&plan)?;
        let plan_id = plan["id"].as_str().unwrap_or("unknown").to_string();

        // If no dependencies declared, fall back to sequential for backward compat
        if !scheduler::has_dependencies(&steps) {
            return self.run_sequential(&steps, &plan_id).await;
        }

        // Compute execution levels (DAG)
        let levels = scheduler::compute_levels(&steps)?;
        info!(levels = levels.len(), "Executing plan with DAG scheduling");

        let semaphore = Arc::new(Semaphore::new(self.max_parallel));
        let mut recordings: Vec<StepRecord> = Vec::new();

        // Load replay fixture if needed
        let replay_fixture = match &self.execution_context {
            ExecutionContext::Replay(dir) => Some(Fixture::load(dir)?),
            _ => None,
        };

        for (level_idx, level) in levels.iter().enumerate() {
            info!(level = level_idx, steps = level.len(), "Executing level");

            if self.dry_run {
                for &step_idx in level {
                    let step = &steps[step_idx];
                    warn!(step = step.index, tool = %step.tool, "DRY-RUN: Skipping");
                }
                continue;
            }

            // Replay: return recorded outputs
            if let Some(ref fixture) = replay_fixture {
                for &step_idx in level {
                    let step = &steps[step_idx];
                    let record = fixture
                        .steps
                        .iter()
                        .find(|r| r.index == step.index)
                        .context(format!("No fixture for step {}", step.index))?;
                    info!(step = step.index, tool = %step.tool, "REPLAY");
                    self.storage.save_state(&step.id, &record.output)?;
                }
                continue;
            }

            // Execute level in parallel via JoinSet
            let mut join_set = JoinSet::new();

            for &step_idx in level {
                let step = steps[step_idx].clone();
                let tools = self.tools.clone();
                let sem = semaphore.clone();
                let timeout = self.get_timeout(&step.tool);

                join_set.spawn(async move {
                    let _permit = sem
                        .acquire()
                        .await
                        .map_err(|e| anyhow::anyhow!("Semaphore closed: {}", e))?;

                    let tool = tools
                        .get(&step.tool)
                        .context(format!("Tool '{}' not registered", step.tool))?;

                    let start = std::time::Instant::now();

                    let result =
                        match tokio::time::timeout(timeout, tool.execute(step.input.clone())).await
                        {
                            Err(_) => anyhow::bail!(
                                "Tool '{}' timed out after {:.1}s (step '{}')",
                                step.tool,
                                timeout.as_secs_f64(),
                                step.id
                            ),
                            Ok(Err(e)) => {
                                return Err(e).context(format!(
                                    "Tool '{}' failed (step '{}')",
                                    step.tool, step.id
                                ))
                            }
                            Ok(Ok(r)) => r,
                        };

                    let duration_ms = start.elapsed().as_millis() as u64;
                    Ok((step, result, duration_ms))
                });
            }

            // Collect results, fail fast on first error (abort remaining on failure)
            while let Some(task_result) = join_set.join_next().await {
                let joined = task_result.context("Task panicked");
                let (step, result, duration_ms) = match joined.and_then(|r| r) {
                    Ok(v) => v,
                    Err(e) => {
                        join_set.abort_all();
                        return Err(e).context("Step execution failed");
                    }
                };

                info!(step = step.index, tool = %step.tool, duration_ms, "Step completed");
                self.storage.save_state(&step.id, &result)?;

                if matches!(self.execution_context, ExecutionContext::Record(_)) {
                    recordings.push(StepRecord {
                        index: step.index,
                        tool: step.tool.clone(),
                        input: step.input.clone(),
                        output: result,
                        duration_ms,
                    });
                }
            }
        }

        // Save recordings
        if let ExecutionContext::Record(ref dir) = self.execution_context {
            recordings.sort_by_key(|r| r.index);
            let fixture = Fixture {
                plan_id,
                recorded_at: replay::timestamp_now(),
                steps: recordings,
            };
            fixture.save(dir)?;
            info!(dir = ?dir, "Fixture recorded");
        }

        Ok(())
    }

    /// Sequential execution for plans without dependencies (backward compat)
    async fn run_sequential(&self, steps: &[ScheduledStep], plan_id: &str) -> Result<()> {
        let mut recordings: Vec<StepRecord> = Vec::new();

        let replay_fixture = match &self.execution_context {
            ExecutionContext::Replay(dir) => Some(Fixture::load(dir)?),
            _ => None,
        };

        for step in steps {
            if self.dry_run {
                warn!(step = step.index, tool = %step.tool, "DRY-RUN: Skipping tool execution");
                continue;
            }

            // Replay mode
            if let Some(ref fixture) = replay_fixture {
                if let Some(record) = fixture.steps.iter().find(|r| r.index == step.index) {
                    info!(step = step.index, tool = %step.tool, "REPLAY");
                    self.storage.save_state(&step.id, &record.output)?;
                    continue;
                }
            }

            let tool = self
                .tools
                .get(&step.tool)
                .context(format!("Tool '{}' not registered", step.tool))?;

            let timeout = self.get_timeout(&step.tool);
            info!(step = step.index, tool = %step.tool, timeout_ms = timeout.as_millis(), "Executing tool");

            let start = std::time::Instant::now();

            let result = match tokio::time::timeout(timeout, tool.execute(step.input.clone())).await
            {
                Err(_elapsed) => {
                    anyhow::bail!(
                        "Tool '{}' timed out after {:.1}s (step '{}')",
                        step.tool,
                        timeout.as_secs_f64(),
                        step.id
                    );
                }
                Ok(Err(e)) => {
                    return Err(e).context(format!(
                        "Tool '{}' execution failed (step '{}')",
                        step.tool, step.id
                    ));
                }
                Ok(Ok(result)) => result,
            };

            let duration_ms = start.elapsed().as_millis() as u64;
            info!(step = step.index, tool = %step.tool, duration_ms, "Tool completed");
            self.storage.save_state(&step.id, &result)?;

            if matches!(self.execution_context, ExecutionContext::Record(_)) {
                recordings.push(StepRecord {
                    index: step.index,
                    tool: step.tool.clone(),
                    input: step.input.clone(),
                    output: result,
                    duration_ms,
                });
            }
        }

        // Save recordings
        if let ExecutionContext::Record(ref dir) = self.execution_context {
            let fixture = Fixture {
                plan_id: plan_id.to_string(),
                recorded_at: replay::timestamp_now(),
                steps: recordings,
            };
            fixture.save(dir)?;
            info!(dir = ?dir, "Fixture recorded");
        }

        Ok(())
    }

    /// Execute a single tool by name (used by Agent loop)
    pub async fn execute_tool(&self, tool_name: &str, input: Value) -> Result<Value> {
        // Policy pipeline evaluation (if configured)
        if let Some(ref policy) = self.policy {
            let ctx = PolicyContext {
                tool_name: tool_name.to_string(),
                input: input.clone(),
                caller_permission: PermissionLevel::Execute,
                dry_run: self.dry_run,
                session_id: None,
            };
            policy.evaluate(&ctx)?;
        }

        let tool = self
            .tools
            .get(tool_name)
            .ok_or_else(|| anyhow::anyhow!("Tool '{}' not registered", tool_name))?;

        if self.dry_run {
            warn!(tool = tool_name, "DRY-RUN: Skipping tool execution");
            return Ok(serde_json::json!({
                "dry_run": true,
                "tool": tool_name,
                "message": "Skipped in dry-run mode"
            }));
        }

        let timeout = self.get_timeout(tool_name);
        let result = match tokio::time::timeout(timeout, tool.execute(input)).await {
            Err(_) => anyhow::bail!(
                "Tool '{}' timed out after {:.1}s",
                tool_name,
                timeout.as_secs_f64()
            ),
            Ok(Err(e)) => return Err(e).context(format!("Tool '{}' execution failed", tool_name)),
            Ok(Ok(r)) => r,
        };

        Ok(result)
    }

    /// Get list of registered tool names
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|r| r.key().clone()).collect()
    }

    /// Start runtime
    pub async fn start(&self) -> Result<()> {
        info!("Runtime started");
        Ok(())
    }

    /// Stop runtime
    pub async fn stop(&self) -> Result<()> {
        info!("Runtime stopped");
        Ok(())
    }
}
