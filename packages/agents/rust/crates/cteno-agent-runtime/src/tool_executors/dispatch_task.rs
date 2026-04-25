use async_trait::async_trait;
use serde_json::{json, Value};

use crate::task_graph::{self, TaskNodeInput};
use crate::tool::ToolExecutor;

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
        let parent_session_id = input
            .get("__session_id")
            .and_then(|v| v.as_str())
            .or_else(|| input.get("parent_session_id").and_then(|v| v.as_str()))
            .ok_or_else(|| {
                "Missing required parameter: parent_session_id (no __session_id injected)"
                    .to_string()
            })?;

        let tasks = if let Some(tasks_value) = input.get("tasks") {
            serde_json::from_value::<Vec<TaskNodeInput>>(tasks_value.clone())
                .map_err(|e| format!("Invalid tasks DAG: {}", e))?
        } else {
            vec![single_task_input(&input)?]
        };

        let dispatch = task_graph::global()
            .dispatch_graph(parent_session_id, tasks)
            .await?;

        Ok(json!({
            "group_id": dispatch.group_id,
            "total_tasks": dispatch.total_tasks,
            "started_tasks": dispatch.started_tasks,
            "message": format!(
                "Task graph dispatched ({total} task(s)). \
                 \n\nIMPORTANT — END YOUR TURN NOW: \
                 Reply with at most ONE short sentence acknowledging dispatch \
                 (e.g. \"已派发，等通知中...\") and STOP. Do NOT call any other \
                 tool to check progress (run_manager, query_subagent, \
                 list_task_sessions, etc. — those are NOT for dispatched DAG \
                 status). The runtime will autonomously wake you with each \
                 [Task Complete] X handoff as nodes finish; you'll then react \
                 in a fresh turn. Polling here wastes tokens and blocks the \
                 autonomous-turn user-bubbles from rendering in the UI.",
                total = dispatch.total_tasks
            )
        })
        .to_string())
    }
}

fn single_task_input(input: &Value) -> Result<TaskNodeInput, String> {
    let task = input
        .get("task")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required parameter: task or tasks".to_string())?;

    let skill_ids = input
        .get("skill_ids")
        .and_then(|v| v.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(ToString::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(TaskNodeInput {
        id: input
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("task")
            .to_string(),
        task: task.to_string(),
        depends_on: vec![],
        profile_id: input
            .get("__profile_id")
            .and_then(|v| v.as_str())
            .or_else(|| input.get("profile_id").and_then(|v| v.as_str()))
            .map(ToString::to_string),
        skill_ids,
        workdir: input
            .get("workdir")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        agent_type: input
            .get("agent_type")
            .and_then(|v| v.as_str())
            .or_else(|| input.get("agent_id").and_then(|v| v.as_str()))
            .map(ToString::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_task_requires_task() {
        let err = single_task_input(&json!({})).unwrap_err();
        assert!(err.contains("task or tasks"));
    }

    #[test]
    fn single_task_maps_agent_and_profile() {
        let task = single_task_input(&json!({
            "task": "do it",
            "agent_type": "worker",
            "__profile_id": "p1",
            "skill_ids": ["s1", "s2"]
        }))
        .unwrap();

        assert_eq!(task.id, "task");
        assert_eq!(task.agent_type.as_deref(), Some("worker"));
        assert_eq!(task.profile_id.as_deref(), Some("p1"));
        assert_eq!(task.skill_ids, vec!["s1", "s2"]);
    }
}
