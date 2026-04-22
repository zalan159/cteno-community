//! Tool Registry
//!
//! Central registry for all tools. Manages tool registration and execution.

use super::{ToolConfig, ToolExecutor};
use std::collections::HashMap;
use std::sync::Arc;

/// Tool Registry
pub struct ToolRegistry {
    /// Tool configurations (id -> config)
    configs: HashMap<String, ToolConfig>,
    /// Tool executors (id -> executor)
    executors: HashMap<String, Arc<dyn ToolExecutor>>,
    /// Cached LLM tool schemas (id -> llm::Tool), computed once at registration
    cached_llm_tools: HashMap<String, crate::llm::Tool>,
}

impl ToolRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            configs: HashMap::new(),
            executors: HashMap::new(),
            cached_llm_tools: HashMap::new(),
        }
    }

    /// Register a tool with its config and executor
    pub fn register(&mut self, config: ToolConfig, executor: Arc<dyn ToolExecutor>) {
        let id = config.id.clone();
        let llm_tool = config.to_llm_tool();
        self.configs.insert(id.clone(), config);
        self.executors.insert(id.clone(), executor);
        self.cached_llm_tools.insert(id, llm_tool);
    }

    /// Register a tool with only its config (for tools that execute via /skills endpoint)
    pub fn register_tool(&mut self, config: ToolConfig) {
        let id = config.id.clone();
        let llm_tool = config.to_llm_tool();
        self.configs.insert(id.clone(), config);
        self.cached_llm_tools.insert(id, llm_tool);
    }

    /// Get tool configuration by ID
    pub fn get_config(&self, tool_id: &str) -> Option<&ToolConfig> {
        self.configs.get(tool_id)
    }

    /// Get all tool configurations
    pub fn get_all_configs(&self) -> Vec<&ToolConfig> {
        self.configs.values().collect()
    }

    /// Execute a tool synchronously
    pub async fn execute(&self, tool_id: &str, input: serde_json::Value) -> Result<String, String> {
        let executor = self
            .executors
            .get(tool_id)
            .ok_or_else(|| format!("Tool not found: {}", tool_id))?;

        executor.execute(input).await
    }

    /// Execute a tool in the background
    pub async fn execute_background(
        &self,
        tool_id: &str,
        input: serde_json::Value,
        session_id: Option<String>,
    ) -> Result<String, String> {
        let executor = self
            .executors
            .get(tool_id)
            .ok_or_else(|| format!("Tool not found: {}", tool_id))?;

        if !executor.supports_background() {
            return Err(format!(
                "Tool {} does not support background execution",
                tool_id
            ));
        }

        executor.execute_background(input, session_id).await
    }

    /// Get tools formatted for LLM (with full instructions).
    /// Returns ALL tools (both immediate and deferred).
    pub fn get_tools_for_llm(&self) -> Vec<crate::llm::Tool> {
        self.cached_llm_tools.values().cloned().collect()
    }

    /// Get only immediate (non-deferred) tools for the LLM system prompt.
    /// These tools have their full schema sent to the LLM on every turn.
    pub fn get_immediate_tools_for_llm(&self) -> Vec<crate::llm::Tool> {
        self.configs
            .values()
            .filter(|c| !c.is_deferred())
            .filter_map(|c| self.cached_llm_tools.get(&c.id).cloned())
            .collect()
    }

    /// Get deferred tool names with their descriptions (for listing in system prompt).
    /// Returns (tool_id, description, search_hint) tuples.
    pub fn get_deferred_tool_summaries(&self) -> Vec<(String, String, Option<String>)> {
        self.configs
            .values()
            .filter(|c| c.is_deferred())
            .map(|c| (c.id.clone(), c.description.clone(), c.search_hint.clone()))
            .collect()
    }

    /// Get tool schema by exact name (used by tool_search select: mode).
    pub fn get_tool_schema_by_name(&self, name: &str) -> Option<crate::llm::Tool> {
        self.cached_llm_tools.get(name).cloned()
    }

    /// Search deferred tools by keyword query.
    /// Returns up to `max_results` matching tool schemas, ranked by relevance.
    pub fn search_deferred_tools(&self, query: &str, max_results: usize) -> Vec<crate::llm::Tool> {
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();

        if query_words.is_empty() {
            return vec![];
        }

        let mut scored: Vec<(i32, &super::ToolConfig)> = self
            .configs
            .values()
            .filter(|c| c.is_deferred())
            .map(|c| {
                let mut score = 0i32;
                let name_lower = c.id.to_lowercase();
                let desc_lower = c.description.to_lowercase();
                let hint_lower = c.search_hint.as_deref().unwrap_or("").to_lowercase();

                // Exact name match
                if name_lower == query_lower {
                    score += 100;
                }

                for w in &query_words {
                    // Name part match (split by underscore)
                    let name_parts: Vec<&str> = name_lower.split('_').collect();
                    if name_parts.iter().any(|p| p == w) {
                        score += 10;
                    } else if name_lower.contains(w) {
                        score += 5;
                    }

                    // Search hint match (curated keywords, high signal)
                    if hint_lower.contains(w) {
                        score += 4;
                    }

                    // Description match
                    if desc_lower.contains(w) {
                        score += 2;
                    }
                }

                // MCP tool prefix matching (mcp__{server}__{tool})
                if name_lower.starts_with("mcp__") {
                    let without_prefix = name_lower.trim_start_matches("mcp__");
                    for w in &query_words {
                        if without_prefix.starts_with(w) {
                            score += 12;
                        }
                    }
                }

                (score, c)
            })
            .filter(|(score, _)| *score > 0)
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored
            .into_iter()
            .take(max_results)
            .filter_map(|(_, c)| self.cached_llm_tools.get(&c.id).cloned())
            .collect()
    }

    /// Get tool count
    pub fn count(&self) -> usize {
        self.configs.len()
    }

    /// Unregister a single tool by ID
    pub fn unregister(&mut self, tool_id: &str) -> bool {
        let had_config = self.configs.remove(tool_id).is_some();
        let had_executor = self.executors.remove(tool_id).is_some();
        self.cached_llm_tools.remove(tool_id);
        had_config || had_executor
    }

    /// Unregister all tools whose ID starts with the given prefix
    pub fn unregister_by_prefix(&mut self, prefix: &str) {
        let ids_to_remove: Vec<String> = self
            .configs
            .keys()
            .filter(|id| id.starts_with(prefix))
            .cloned()
            .collect();

        for id in &ids_to_remove {
            self.configs.remove(id);
            self.executors.remove(id);
            self.cached_llm_tools.remove(id);
        }

        if !ids_to_remove.is_empty() {
            log::info!(
                "Unregistered {} tools with prefix '{}'",
                ids_to_remove.len(),
                prefix
            );
        }
    }

    /// Check if a tool exists
    pub fn has_tool(&self, tool_id: &str) -> bool {
        self.configs.contains_key(tool_id)
    }

    /// List all tool IDs
    pub fn list_ids(&self) -> Vec<String> {
        self.configs.keys().cloned().collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{ToolCategory, ToolConfig};
    use async_trait::async_trait;

    struct MockExecutor;

    #[async_trait]
    impl ToolExecutor for MockExecutor {
        async fn execute(&self, _input: serde_json::Value) -> Result<String, String> {
            Ok("mock result".to_string())
        }
    }

    #[tokio::test]
    async fn test_tool_registry() {
        let mut registry = ToolRegistry::new();

        let config = ToolConfig {
            id: "test_tool".to_string(),
            name: "Test Tool".to_string(),
            description: "A test tool".to_string(),
            category: ToolCategory::System,
            input_schema: serde_json::json!({}),
            instructions: "Test instructions".to_string(),
            supports_background: false,
            should_defer: false,
            always_load: false,
            search_hint: None,
            is_read_only: false,
            is_concurrency_safe: false,
        };

        registry.register(config.clone(), Arc::new(MockExecutor));

        assert_eq!(registry.count(), 1);
        assert!(registry.has_tool("test_tool"));
        assert_eq!(registry.get_config("test_tool").unwrap().id, "test_tool");

        let result = registry.execute("test_tool", serde_json::json!({})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "mock result");
    }
}
