//! List Task Sessions Tool Executor
//!
//! Lists all task sessions dispatched by a persona.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct ListTaskSessionsExecutor;

impl ListTaskSessionsExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ListTaskSessionsExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for ListTaskSessionsExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let persona_id = crate::agent_owner::extract_owner_id(&input)
            .ok_or("This tool requires an agent owner context")?;

        let persona_manager = crate::local_services::persona_manager()?;
        let tasks = persona_manager.list_active_tasks(persona_id)?;

        let task_list: Vec<Value> = tasks
            .iter()
            .map(|t| {
                json!({
                    "session_id": t.session_id,
                    "task_description": t.task_description,
                    "created_at": t.created_at,
                })
            })
            .collect();

        Ok(json!({
            "tasks": task_list,
            "count": task_list.len(),
        })
        .to_string())
    }
}
