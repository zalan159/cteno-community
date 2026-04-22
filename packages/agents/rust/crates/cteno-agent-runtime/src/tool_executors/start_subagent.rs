//! Start SubAgent Tool Executor
//!
//! Starts a background SubAgent task via direct call to SubAgentManager.
//!
//! Agent/profile/auth-token resolution is delegated to
//! `SubagentBootstrapProvider` so the runtime doesn't need to pull in
//! SpawnSessionConfig / ProfileStore / machine_auth.json layout.

use crate::hooks;
use crate::subagent::{self, CleanupPolicy};
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct StartSubAgentExecutor;

impl StartSubAgentExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StartSubAgentExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for StartSubAgentExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let agent_id = input
            .get("agent_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: agent_id".to_string())?;

        let task = input
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: task".to_string())?;

        // Use auto-injected __session_id (set by agent executor), fall back to explicit param
        let parent_session_id = input
            .get("__session_id")
            .and_then(|v| v.as_str())
            .or_else(|| input.get("parent_session_id").and_then(|v| v.as_str()))
            .ok_or_else(|| {
                "Missing required parameter: parent_session_id (no __session_id injected)"
                    .to_string()
            })?;

        let label = input.get("label").and_then(|v| v.as_str());

        let cleanup = match input.get("cleanup").and_then(|v| v.as_str()) {
            Some("delete") => CleanupPolicy::Delete,
            _ => CleanupPolicy::Keep,
        };

        // Use auto-injected __profile_id from session context when available.
        let profile_id = input
            .get("__profile_id")
            .and_then(|v| v.as_str())
            .or_else(|| input.get("profile_id").and_then(|v| v.as_str()));

        // Delegate heavy lifting (profile + proxy-auth-token + agent lookup) to
        // the host via SubagentBootstrapProvider.  Community builds that skip
        // the hook surface fail loudly here instead of panicking.
        let bootstrap = hooks::subagent_bootstrap().ok_or_else(|| {
            "SubagentBootstrapProvider not installed — start_subagent requires host bootstrap"
                .to_string()
        })?;

        let (agent_config, exec_ctx) = bootstrap
            .build_subagent_context(agent_id, parent_session_id, profile_id)
            .await?;

        let manager = subagent::manager::global();
        let subagent_id = manager
            .spawn(
                parent_session_id.to_string(),
                agent_id.to_string(),
                task.to_string(),
                label.map(|s| s.to_string()),
                cleanup,
                agent_config,
                exec_ctx,
            )
            .await?;

        let label_text = label.unwrap_or("Background Task");
        let message = format!(
            "SubAgent '{}' started in background (ID: {}). I'll notify you when it completes.",
            label_text, subagent_id
        );

        Ok(json!({
            "id": subagent_id,
            "message": message
        })
        .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_start_subagent_missing_agent_id() {
        let executor = StartSubAgentExecutor::new();
        let input = json!({
            "task": "Test task",
            "parent_session_id": "test-session"
        });
        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Missing required parameter: agent_id"));
    }

    #[tokio::test]
    async fn test_start_subagent_missing_task() {
        let executor = StartSubAgentExecutor::new();
        let input = json!({
            "agent_id": "worker",
            "parent_session_id": "test-session"
        });
        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Missing required parameter: task"));
    }
}
