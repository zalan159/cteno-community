//! Delete Scheduled Task Tool Executor
//!
//! Deletes a scheduled task via direct call to the TaskScheduler.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct DeleteScheduledTaskExecutor;

impl DeleteScheduledTaskExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DeleteScheduledTaskExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for DeleteScheduledTaskExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let task_id = input
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: task_id")?;

        let scheduler = crate::local_services::scheduler()
            .map_err(|e| format!("Scheduler not available: {}", e))?;
        let deleted = scheduler.delete_task(task_id)?;

        if !deleted {
            return Err(format!("Task '{}' not found", task_id));
        }

        Ok(json!({
            "success": true,
            "message": format!("Scheduled task '{}' deleted.", task_id)
        })
        .to_string())
    }
}
