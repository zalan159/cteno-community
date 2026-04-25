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
        let todos = ["todos", "newTodos", "items", "plan"]
            .iter()
            .find_map(|key| input.get(*key).and_then(|v| v.as_array()))
            .ok_or_else(|| {
                "Missing required parameter: todos/newTodos/items/plan (must be an array)"
                    .to_string()
            })?;

        if todos.is_empty() {
            return Err("todos array must not be empty".to_string());
        }

        for (i, item) in todos.iter().enumerate() {
            item.get("content")
                .or_else(|| item.get("task"))
                .or_else(|| item.get("title"))
                .or_else(|| item.get("step"))
                .or_else(|| item.get("text"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    format!("todos[{}]: missing or empty 'content'/'task'/'title'", i)
                })?;

            let status = item
                .get("status")
                .and_then(|v| v.as_str())
                .or_else(|| {
                    item.get("done").and_then(|v| v.as_bool()).map(|done| {
                        if done {
                            "completed"
                        } else {
                            "pending"
                        }
                    })
                })
                .ok_or_else(|| format!("todos[{}]: missing 'status'/'done'", i))?;

            match status.trim().to_ascii_lowercase().as_str() {
                "pending" | "queued" | "todo" => {}
                "in_progress" | "in-progress" | "inprogress" | "running" | "active" => {}
                "completed" | "complete" | "done" | "success" | "succeeded" => {}
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
    async fn test_task_done_shape() {
        let executor = UpdatePlanExecutor::new();
        let input = json!({
            "todos": [
                {"task": "Step one", "done": true},
                {"task": "Step two", "done": false}
            ]
        });
        let result = executor.execute(input).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Plan updated");
    }

    #[tokio::test]
    async fn test_title_shape() {
        let executor = UpdatePlanExecutor::new();
        let input = json!({
            "todos": [
                {"title": "Step one", "status": "in_progress"},
                {"title": "Step two", "status": "pending"}
            ]
        });
        let result = executor.execute(input).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Plan updated");
    }

    #[tokio::test]
    async fn test_plan_alias_shape() {
        let executor = UpdatePlanExecutor::new();
        let input = json!({
            "plan": [
                {"step": "Step one", "status": "running"},
                {"step": "Step two", "status": "queued"}
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
