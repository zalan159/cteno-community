//! Get Session Output Tool Executor
//!
//! Retrieves the latest messages from a task session.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct GetSessionOutputExecutor {
    db_path: PathBuf,
}

impl GetSessionOutputExecutor {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }
}

#[async_trait]
impl ToolExecutor for GetSessionOutputExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let session_id = input
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: session_id")?;

        let last_n = input.get("last_n").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

        let session_mgr = crate::agent_session::AgentSessionManager::new(self.db_path.clone());

        let session = session_mgr
            .get_session(session_id)?
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        let messages = &session.messages;
        let start = if messages.len() > last_n {
            messages.len() - last_n
        } else {
            0
        };

        let recent: Vec<Value> = messages[start..]
            .iter()
            .map(|m| {
                json!({
                    "role": m.role,
                    "content": if m.content.len() > 2000 {
                        let boundary = m.content.floor_char_boundary(2000);
                        format!("{}...[truncated]", &m.content[..boundary])
                    } else {
                        m.content.clone()
                    },
                    "timestamp": m.timestamp,
                })
            })
            .collect();

        Ok(json!({
            "session_id": session_id,
            "status": format!("{:?}", session.status),
            "total_messages": messages.len(),
            "messages": recent,
        })
        .to_string())
    }
}
