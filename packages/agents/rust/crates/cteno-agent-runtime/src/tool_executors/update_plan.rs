//! Update Plan Tool Executor
//!
//! Validates and acknowledges plan updates. The plan itself is carried
//! in the tool-call input and rendered by the frontend (TodoView);
//! the executor only needs to validate structure and return confirmation.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::Value;

pub struct UpdatePlanExecutor;

impl UpdatePlanExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UpdatePlanExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for UpdatePlanExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let todos = input
            .get("todos")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "Missing required parameter: todos (must be an array)".to_string())?;

        if todos.is_empty() {
            return Err("todos array must not be empty".to_string());
        }

        for (i, item) in todos.iter().enumerate() {
            item.get("content")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("todos[{}]: missing or empty 'content'", i))?;

            let status = item
                .get("status")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("todos[{}]: missing 'status'", i))?;

            match status {
                "pending" | "in_progress" | "completed" => {}
                other => {
                    return Err(format!(
                        "todos[{}]: invalid status '{}' (expected pending|in_progress|completed)",
                        i, other
                    ));
                }
            }
        }

        Ok("Plan updated".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_valid_plan() {
        let executor = UpdatePlanExecutor::new();
        let input = json!({
            "todos": [
                {"content": "Step one", "status": "completed"},
                {"content": "Step two", "status": "in_progress"},
                {"content": "Step three", "status": "pending"}
            ]
        });
        let result = executor.execute(input).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Plan updated");
    }

    #[tokio::test]
    async fn test_missing_todos() {
        let executor = UpdatePlanExecutor::new();
        let input = json!({});
        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("todos"));
    }

    #[tokio::test]
    async fn test_empty_todos() {
        let executor = UpdatePlanExecutor::new();
        let input = json!({"todos": []});
        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must not be empty"));
    }

    #[tokio::test]
    async fn test_invalid_status() {
        let executor = UpdatePlanExecutor::new();
        let input = json!({
            "todos": [{"content": "Do thing", "status": "unknown"}]
        });
        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid status"));
    }
}
