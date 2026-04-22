//! Stop SubAgent Tool Executor
//!
//! Stops a running SubAgent task via direct call to SubAgentManager.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct StopSubAgentExecutor;

impl StopSubAgentExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StopSubAgentExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for StopSubAgentExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let id = input
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: id".to_string())?;

        let manager = crate::subagent::manager::global();
        manager.stop(id).await?;

        Ok(json!({
            "success": true,
            "message": format!("SubAgent '{}' stopped successfully", id)
        })
        .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stop_subagent_missing_id() {
        let executor = StopSubAgentExecutor::new();
        let input = json!({});
        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Missing required parameter: id"));
    }
}
