//! Tool System
//!
//! Tools are atomic, always-available execution units with full_disclosure: always.
//! They differ from Skills which are complex functions with optional disclosure.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod registry;

/// Tool category
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::upper_case_acronyms)]
pub enum ToolCategory {
    /// System tools (zsh, file, macos)
    System,
    /// MCP tools (mcp/*)
    MCP,
    /// SubAgent tools (start_subagent, query_subagent, etc.)
    SubAgent,
    /// Persona tools (dispatch_task, list_task_sessions, etc.)
    Persona,
}

/// Tool configuration (loaded from TOOL.md or dynamically registered)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    /// Unique tool ID
    pub id: String,
    /// Display name
    pub name: String,
    /// Short description (1-2 sentences)
    pub description: String,
    /// Tool category
    pub category: ToolCategory,
    /// JSON Schema for input parameters
    pub input_schema: serde_json::Value,
    /// Full instructions (always injected into System Prompt)
    pub instructions: String,
    /// Whether this tool supports background execution
    #[serde(default)]
    pub supports_background: bool,
    /// Whether this tool should be deferred (not sent to LLM until requested via tool_search).
    /// Defaults to false. MCP tools are always deferred unless `always_load` is true.
    #[serde(default)]
    pub should_defer: bool,
    /// Force this tool to always be loaded immediately, even if it would normally be deferred.
    /// Overrides `should_defer` and MCP auto-defer.
    #[serde(default)]
    pub always_load: bool,
    /// 3-10 word keyword hint for ToolSearch fuzzy matching.
    /// Example: "search files by content regex pattern"
    #[serde(default)]
    pub search_hint: Option<String>,
    /// Whether this tool only reads state (no filesystem/network mutation)
    #[serde(default)]
    pub is_read_only: bool,
    /// Whether this tool is safe to run concurrently with other tools
    #[serde(default)]
    pub is_concurrency_safe: bool,
}

/// Tool executor trait
///
/// All tools must implement this trait to be registered in the ToolRegistry.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute the tool synchronously (returns result immediately)
    async fn execute(&self, input: serde_json::Value) -> Result<String, String>;

    /// Check if this tool supports background execution
    fn supports_background(&self) -> bool {
        false
    }

    /// Execute the tool in the background (optional)
    ///
    /// Returns a task ID that can be used to query progress.
    /// Default implementation returns an error.
    async fn execute_background(
        &self,
        _input: serde_json::Value,
        _session_id: Option<String>,
    ) -> Result<String, String> {
        Err("This tool does not support background execution".to_string())
    }
}

/// Convert ToolConfig to LLM Tool format
impl ToolConfig {
    pub fn to_llm_tool(&self) -> crate::llm::Tool {
        crate::llm::Tool {
            name: self.id.clone(),
            description: format!("{}\n\n{}", self.description, self.instructions),
            input_schema: self.input_schema.clone(),
        }
    }

    /// Whether this tool should be deferred (loaded on-demand via tool_search).
    ///
    /// A tool is deferred if:
    /// - It has `should_defer: true` in its config, OR
    /// - It is an MCP tool (category == MCP)
    ///
    /// A tool is NEVER deferred if:
    /// - It has `always_load: true` (overrides everything), OR
    /// - Its id is "tool_search" (the search tool itself must always be available)
    pub fn is_deferred(&self) -> bool {
        // always_load overrides everything
        if self.always_load {
            return false;
        }
        // tool_search itself is never deferred
        if self.id == "tool_search" {
            return false;
        }
        // MCP tools are always deferred (unless always_load was set above)
        if self.category == ToolCategory::MCP {
            return true;
        }
        // Otherwise, respect the should_defer flag
        self.should_defer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_config_serialization() {
        let tool = ToolConfig {
            id: "test_tool".to_string(),
            name: "Test Tool".to_string(),
            description: "A test tool".to_string(),
            category: ToolCategory::System,
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "param1": { "type": "string" }
                }
            }),
            instructions: "Full instructions here".to_string(),
            supports_background: false,
            should_defer: false,
            always_load: false,
            search_hint: None,
            is_read_only: false,
            is_concurrency_safe: false,
        };

        let json = serde_json::to_string(&tool).unwrap();
        let deserialized: ToolConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(tool.id, deserialized.id);
        assert_eq!(tool.category, deserialized.category);
    }
}
