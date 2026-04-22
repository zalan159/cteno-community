//! Tool Search Executor
//!
//! Searches and retrieves deferred tool definitions on demand.
//! Deferred tools are not sent to the LLM in the initial system prompt;
//! the agent uses this meta-tool to discover and load them when needed.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct ToolSearchExecutor;

impl ToolSearchExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ToolSearchExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for ToolSearchExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let max_results = input
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as usize;

        if query.is_empty() {
            return Err("query parameter is required".to_string());
        }

        let registry = crate::hooks::tool_registry_handle()
            .ok_or_else(|| "Tool registry not available".to_string())?;
        let reg = registry.read().await;

        // Check for "select:ToolName,ToolName2" direct selection
        if let Some(names_str) = query.strip_prefix("select:") {
            let requested: Vec<&str> = names_str
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();

            let mut found = Vec::new();
            let mut missing = Vec::new();

            for name in &requested {
                if let Some(tool) = reg.get_tool_schema_by_name(name) {
                    found.push(tool);
                } else {
                    missing.push(name.to_string());
                }
            }

            if found.is_empty() {
                return Ok(json!({
                    "matches": [],
                    "query": query,
                    "message": format!("No tools found: {}", missing.join(", ")),
                    "total_deferred_tools": reg.get_deferred_tool_summaries().len()
                })
                .to_string());
            }

            // Build <functions> block matching the standard tool definition format
            let functions_block = build_functions_block(&found);

            if !missing.is_empty() {
                log::info!(
                    "[ToolSearch] select: found {}, missing: {}",
                    found
                        .iter()
                        .map(|t| t.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    missing.join(", ")
                );
            } else {
                log::info!(
                    "[ToolSearch] select: {}",
                    found
                        .iter()
                        .map(|t| t.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }

            return Ok(functions_block);
        }

        // Keyword search
        let matches = reg.search_deferred_tools(&query, max_results);

        log::info!(
            "[ToolSearch] keyword search for \"{}\", found {} matches",
            query,
            matches.len()
        );

        if matches.is_empty() {
            // Also try exact name match across ALL tools (including non-deferred)
            // in case the model is searching for an already-loaded tool
            if let Some(tool) = reg.get_tool_schema_by_name(&query) {
                log::info!(
                    "[ToolSearch] exact match found in non-deferred tools: {}",
                    tool.name
                );
                let functions_block = build_functions_block(&[tool]);
                return Ok(functions_block);
            }

            return Ok(json!({
                "matches": [],
                "query": query,
                "message": "No matching deferred tools found",
                "total_deferred_tools": reg.get_deferred_tool_summaries().len()
            })
            .to_string());
        }

        let functions_block = build_functions_block(&matches);
        Ok(functions_block)
    }
}

/// Build a <functions> block containing tool definitions in the standard format.
/// This mirrors the format used at the top of the system prompt so the LLM
/// can use the tools immediately after receiving the search result.
fn build_functions_block(tools: &[crate::llm::Tool]) -> String {
    let mut lines = Vec::new();
    lines.push("<functions>".to_string());

    for tool in tools {
        let tool_def = json!({
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        });
        lines.push(format!(
            "<function>{}</function>",
            serde_json::to_string(&tool_def).unwrap_or_default()
        ));
    }

    lines.push("</functions>".to_string());
    lines.join("\n")
}
