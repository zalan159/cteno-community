//! List Scheduled Tasks Tool Executor
//!
//! Lists scheduled tasks via direct call to the TaskScheduler.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::Value;

pub struct ListScheduledTasksExecutor;

impl ListScheduledTasksExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ListScheduledTasksExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for ListScheduledTasksExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let enabled_only = input
            .get("enabled_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let scheduler = crate::local_services::scheduler()
            .map_err(|e| format!("Scheduler not available: {}", e))?;
        let tasks = scheduler.list_tasks(enabled_only)?;

        Ok(serde_json::to_string(&tasks)
            .map_err(|e| format!("Failed to serialize tasks: {}", e))?)
    }
}
