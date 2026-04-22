//! Dispatch Task Tool Executor
//!
//! Dispatches a single task or a task graph (DAG) to worker sessions via PersonaManager.

use crate::task_graph::TaskNodeInput;
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct DispatchTaskExecutor;

impl DispatchTaskExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DispatchTaskExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for DispatchTaskExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        // Get owner_id from the injected session context
        let persona_id = crate::agent_owner::extract_owner_id(&input)
            .ok_or("This tool requires an agent owner context")?;

        let persona_manager = crate::local_services::persona_manager()?;

        // Check if this is a task graph (tasks array) or a single task
        if let Some(tasks_val) = input.get("tasks") {
            // === Task Graph mode ===
            let tasks: Vec<TaskNodeInput> = serde_json::from_value(tasks_val.clone())
                .map_err(|e| format!("Invalid tasks array: {}", e))?;

            let group_id = persona_manager.dispatch_task_graph(persona_id, &tasks)?;

            let root_count = tasks.iter().filter(|t| t.depends_on.is_empty()).count();
            Ok(json!({
                "group_id": group_id,
                "total_tasks": tasks.len(),
                "root_tasks_started": root_count,
                "message": format!(
                    "Task graph dispatched (group {}). {} tasks total, {} root tasks started immediately. \
                     Results will be pushed back as [Task Complete] messages. \
                     Final summary as [Task Group Complete] when all done.",
                    group_id, tasks.len(), root_count
                )
            })
            .to_string())
        } else {
            // === Single task mode (original behavior) ===
            let task = input
                .get("task")
                .and_then(|v| v.as_str())
                .ok_or("Missing required parameter: task (or tasks array for task graph)")?;

            let workdir = input.get("workdir").and_then(|v| v.as_str());
            let profile_id = input.get("profile_id").and_then(|v| v.as_str());
            let agent_type = input.get("agent_type").and_then(|v| v.as_str());
            let agent_flavor = input.get("agent_flavor").and_then(|v| v.as_str());
            let label = input.get("label").and_then(|v| v.as_str());
            let skill_ids: Option<Vec<String>> = input
                .get("skill_ids")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                });

            let session_id = persona_manager
                .dispatch_task_async(
                    persona_id,
                    task,
                    workdir,
                    profile_id,
                    skill_ids.as_deref(),
                    agent_type,
                    agent_flavor,
                    label,
                    None,
                )
                .await?;

            Ok(json!({
                "session_id": session_id,
                "message": format!("Task dispatched to session {}. Results will be pushed back as [Task Complete] message.", session_id)
            })
            .to_string())
        }
    }
}
