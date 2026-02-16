use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};

/// Parsed step with dependency info
#[derive(Debug, Clone)]
pub struct ScheduledStep {
    pub index: usize,
    pub id: String,
    pub tool: String,
    pub input: Value,
    pub depends_on: Vec<String>,
}

/// Parse plan steps and extract dependency info.
pub fn parse_steps(plan: &Value) -> Result<Vec<ScheduledStep>> {
    let steps = plan["steps"]
        .as_array()
        .context("Plan missing 'steps' array")?;

    let mut result = Vec::with_capacity(steps.len());

    for (i, step) in steps.iter().enumerate() {
        let id = step["id"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("step_{}", i));

        let tool = step["tool"]
            .as_str()
            .context(format!("Step {} missing 'tool' field", i))?
            .to_string();

        let input = step["input"].clone();

        let depends_on = step["depends_on"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        result.push(ScheduledStep {
            index: i,
            id,
            tool,
            input,
            depends_on,
        });
    }

    Ok(result)
}

/// Compute execution levels via topological sort (Kahn's algorithm).
/// Each inner Vec is a set of step indices that can execute in parallel.
pub fn compute_levels(steps: &[ScheduledStep]) -> Result<Vec<Vec<usize>>> {
    let id_to_idx: HashMap<&str, usize> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();

    // Validate dependencies exist
    for step in steps {
        for dep in &step.depends_on {
            if !id_to_idx.contains_key(dep.as_str()) {
                anyhow::bail!(
                    "Step '{}' depends on '{}' which does not exist",
                    step.id,
                    dep
                );
            }
        }
    }

    // Compute in-degree and adjacency
    let n = steps.len();
    let mut in_degree = vec![0usize; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];

    for (i, step) in steps.iter().enumerate() {
        for dep in &step.depends_on {
            let dep_idx = id_to_idx[dep.as_str()];
            dependents[dep_idx].push(i);
            in_degree[i] += 1;
        }
    }

    // Kahn's algorithm with level tracking
    let mut queue: VecDeque<usize> = VecDeque::new();
    for (i, &deg) in in_degree.iter().enumerate() {
        if deg == 0 {
            queue.push_back(i);
        }
    }

    let mut levels: Vec<Vec<usize>> = Vec::new();
    let mut processed = 0;

    while !queue.is_empty() {
        let level: Vec<usize> = queue.drain(..).collect();
        processed += level.len();

        let mut next_queue = VecDeque::new();
        for &idx in &level {
            for &dep_idx in &dependents[idx] {
                in_degree[dep_idx] -= 1;
                if in_degree[dep_idx] == 0 {
                    next_queue.push_back(dep_idx);
                }
            }
        }

        levels.push(level);
        queue = next_queue;
    }

    if processed != n {
        anyhow::bail!("Cycle detected in step dependencies");
    }

    Ok(levels)
}

/// Check if plan has any dependencies declared
pub fn has_dependencies(steps: &[ScheduledStep]) -> bool {
    steps.iter().any(|s| !s.depends_on.is_empty())
}
