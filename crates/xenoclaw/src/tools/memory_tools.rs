//! Memory Tools — Tool trait implementations wrapping `memory_store::KnowledgeStore`.
//!
//! Provides:
//! - `memory_search` — search stored knowledge entries by query
//! - `memory_store` — store a new knowledge entry with title, content, and tags

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::debug;

use agent_core::tool_registry::Tool;
use memory_store::KnowledgeStore;

// =============================================================================
// MemorySearchTool
// =============================================================================

/// Tool that searches the knowledge store for relevant entries.
pub struct MemorySearchTool {
    store: Arc<KnowledgeStore>,
}

impl MemorySearchTool {
    /// Create a new MemorySearchTool wrapping the given KnowledgeStore.
    pub fn new(store: Arc<KnowledgeStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search the agent's knowledge store for relevant entries. \
         Returns matching entries ranked by relevance."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to find relevant knowledge entries"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10)",
                    "default": 10
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter 'query'".to_string())?;

        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as u32;

        debug!(query = %query, limit = limit, "Executing memory_search tool");

        let results = self
            .store
            .search_knowledge(query, limit)
            .await
            .map_err(|e| e.to_string())?;

        let entries: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({
                    "id": r.id.0.to_string(),
                    "title": r.title,
                    "content": r.content,
                    "tags": r.tags,
                    "relevance_score": r.relevance_score
                })
            })
            .collect();

        let response = json!({
            "results": entries,
            "count": entries.len()
        });

        Ok(response.to_string())
    }
}

// =============================================================================
// MemoryStoreTool
// =============================================================================

/// Tool that stores a new knowledge entry in the agent's memory.
pub struct MemoryStoreTool {
    store: Arc<KnowledgeStore>,
}

impl MemoryStoreTool {
    /// Create a new MemoryStoreTool wrapping the given KnowledgeStore.
    pub fn new(store: Arc<KnowledgeStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for MemoryStoreTool {
    fn name(&self) -> &str {
        "memory_store"
    }

    fn description(&self) -> &str {
        "Store a new knowledge entry in the agent's persistent memory. \
         Entries can be retrieved later via memory_search."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "A short title for the knowledge entry"
                },
                "content": {
                    "type": "string",
                    "description": "The content to store (max 10,000 characters)"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Tags for categorizing the entry (optional)"
                },
                "metadata": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": "Additional key-value metadata (optional)"
                }
            },
            "required": ["title", "content"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let title = arguments
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter 'title'".to_string())?;

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter 'content'".to_string())?;

        let tags: Vec<String> = arguments
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let metadata: HashMap<String, String> = arguments
            .get("metadata")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        debug!(title = %title, tags = ?tags, "Executing memory_store tool");

        let id = self
            .store
            .store_knowledge(title, content, &tags, &metadata)
            .await
            .map_err(|e| e.to_string())?;

        let response = json!({
            "id": id.0.to_string(),
            "title": title,
            "stored": true
        });

        Ok(response.to_string())
    }

    fn is_destructive(&self) -> bool {
        true
    }
}
