//! Query SubAgent Tool Executor
//!
//! Queries the status and results of SubAgent tasks via direct call to SubAgentManager.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct QuerySubAgentExecutor;

impl QuerySubAgentExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for QuerySubAgentExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for QuerySubAgentExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let is_list = input.get("list").and_then(|v| v.as_bool()).unwrap_or(false);
        let manager = crate::subagent::manager::global();

        if is_list {
            // List mode
            let parent_session_id = input
                .get("parent_session_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let status_str = input.get("status").and_then(|v| v.as_str());
            let active_only = input
                .get("active_only")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let status = status_str.and_then(|s| match s {
                "pending" => Some(crate::subagent::SubAgentStatus::Pending),
                "running" => Some(crate::subagent::SubAgentStatus::Running),
                "completed" => Some(crate::subagent::SubAgentStatus::Completed),
                "failed" => Some(crate::subagent::SubAgentStatus::Failed),
                "stopped" => Some(crate::subagent::SubAgentStatus::Stopped),
                "timed_out" => Some(crate::subagent::SubAgentStatus::TimedOut),
                _ => None,
            });

            let filter = crate::subagent::SubAgentFilter {
                parent_session_id,
                status,
                active_only,
            };

            let subagents = manager.list(filter).await;

            serde_json::to_string_pretty(&subagents)
                .map_err(|e| format!("Failed to serialize SubAgents: {}", e))
        } else {
            // Single query mode
            let id = input
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter: id (or set list=true)".to_string())?;

            let subagent = manager
                .get(id)
                .await
                .ok_or_else(|| format!("SubAgent '{}' not found", id))?;

            serde_json::to_string_pretty(&subagent)
                .map_err(|e| format!("Failed to serialize SubAgent: {}", e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_query_subagent_missing_id() {
        let executor = QuerySubAgentExecutor::new();
        let input = json!({});
        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Missing required parameter: id"));
    }
}
