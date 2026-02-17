use anyhow::{Context, Result};
use async_trait::async_trait;
use operon_runtime::memory::types::{SearchQuery, SearchSource};
use operon_runtime::memory::MemoryManager;
use operon_runtime::{PermissionLevel, Tool, ToolSchemaInfo};
use serde_json::{json, Value};
use std::sync::Arc;

/// LLM-callable tool for searching agent memory (workspace files).
pub struct MemorySearchTool {
    manager: Arc<MemoryManager>,
}

impl MemorySearchTool {
    pub fn new(manager: Arc<MemoryManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    async fn execute(&self, input: Value) -> Result<Value> {
        let query_str = input["query"]
            .as_str()
            .context("Missing required field 'query'")?;
        let limit = input["limit"].as_u64().unwrap_or(10) as usize;
        let source = match input["source"].as_str().unwrap_or("hybrid") {
            "vector" => SearchSource::Vector,
            "fts" => SearchSource::FullText,
            _ => SearchSource::Hybrid,
        };

        let query = SearchQuery {
            query: query_str.to_string(),
            limit,
            source,
        };

        let results = self.manager.search(query).await?;

        let items: Vec<Value> = results
            .into_iter()
            .map(|r| {
                json!({
                    "path": r.path,
                    "score": r.score,
                    "snippet": r.content_snippet,
                    "source": r.source,
                })
            })
            .collect();

        Ok(json!({
            "results": items,
            "count": items.len(),
        }))
    }

    fn name(&self) -> &str {
        "memory_search"
    }

    fn schema(&self) -> ToolSchemaInfo {
        ToolSchemaInfo {
            name: "memory_search".to_string(),
            description: "Search workspace files using hybrid vector + full-text search".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query text" },
                    "limit": { "type": "integer", "description": "Max results (default: 10)" },
                    "source": {
                        "type": "string",
                        "enum": ["hybrid", "vector", "fts"],
                        "description": "Search mode (default: hybrid)"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}
