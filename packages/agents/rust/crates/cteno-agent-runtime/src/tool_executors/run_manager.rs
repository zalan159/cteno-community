//! Run Manager Executor
//!
//! Agent-facing tool for managing background runs started by tools (e.g., shell with background=true).
use crate::runs::RunManager;
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct RunManagerExecutor {
    run_manager: Arc<RunManager>,
}

impl RunManagerExecutor {
    pub fn new(run_manager: Arc<RunManager>) -> Self {
        Self { run_manager }
    }

    fn caller_session_id(input: &Value) -> Result<String, String> {
        input
            .get("__session_id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "Missing __session_id (internal)".to_string())
    }
}

#[async_trait]
impl ToolExecutor for RunManagerExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let session_id = Self::caller_session_id(&input)?;
        let op = input
            .get("op")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .ok_or_else(|| "Missing 'op'".to_string())?;

        match op {
            "list" => {
                let runs = self.run_manager.list_runs(Some(&session_id)).await;
                Ok(serde_json::to_string_pretty(&runs).unwrap_or_else(|_| "[]".to_string()))
            }
            "get" => {
                let run_id = input
                    .get("run_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim())
                    .ok_or_else(|| "Missing 'run_id'".to_string())?;
                let run = self
                    .run_manager
                    .get_run(run_id)
                    .await
                    .ok_or_else(|| "Run not found".to_string())?;
                if run.session_id != session_id {
                    return Err("Run not found".to_string());
                }
                Ok(serde_json::to_string_pretty(&run).unwrap_or_default())
            }
            "logs" => {
                let run_id = input
                    .get("run_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim())
                    .ok_or_else(|| "Missing 'run_id'".to_string())?;
                let run = self
                    .run_manager
                    .get_run(run_id)
                    .await
                    .ok_or_else(|| "Run not found".to_string())?;
                if run.session_id != session_id {
                    return Err("Run not found".to_string());
                }
                let max_bytes = input
                    .get("max_bytes")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
                    .unwrap_or(8000)
                    .clamp(256, 256_000);
                let content = self.run_manager.tail_log(run_id, max_bytes).await?;
                Ok(content)
            }
            "stop" => {
                let run_id = input
                    .get("run_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim())
                    .ok_or_else(|| "Missing 'run_id'".to_string())?;
                let run = self
                    .run_manager
                    .get_run(run_id)
                    .await
                    .ok_or_else(|| "Run not found".to_string())?;
                if run.session_id != session_id {
                    return Err("Run not found".to_string());
                }
                let reason = input
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("stopped by agent");
                let killed = self.run_manager.kill_run(run_id, reason).await;
                Ok(format!("stop: {} (killed={})", run_id, killed))
            }
            _ => Err(format!("Unknown op: {}", op)),
        }
    }
}
