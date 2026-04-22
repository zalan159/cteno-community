//! Close Task Session Tool Executor
//!
//! Closes a completed task session: removes any active local connection and
//! updates persona tracking.

use crate::auth_store_boot::load_persisted_machine_auth;
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct CloseTaskSessionExecutor {
    db_path: PathBuf,
}

impl CloseTaskSessionExecutor {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }
}

#[async_trait]
impl ToolExecutor for CloseTaskSessionExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let session_id = input
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: session_id")?;

        // Close the local DB session
        let session_mgr = crate::agent_session::AgentSessionManager::new(self.db_path.clone());
        session_mgr.close_session(session_id)?;

        // Remove from persona's active tasks tracking
        let persona_manager = crate::local_services::persona_manager()?;
        persona_manager.on_task_complete(session_id).await;

        // Kill the session's Socket.IO connection and delete from server
        if let Ok(spawn_config) = crate::local_services::spawn_config() {
            let conn_opt = spawn_config.session_connections.remove(session_id).await;
            if let Some(conn) = conn_opt {
                conn.kill().await;
                log::info!(
                    "[CloseTaskSession] Session {} killed and removed from active connections",
                    session_id
                );
            }

            let app_data_dir = self
                .db_path
                .parent()
                .and_then(|db_dir| db_dir.parent())
                .unwrap_or_else(|| std::path::Path::new("."));
            if let Some((auth_token, _, _, _)) = load_persisted_machine_auth(app_data_dir)? {
                let url = format!(
                    "{}/v1/sessions/{}",
                    crate::resolved_happy_server_url(),
                    session_id
                );
                match reqwest::Client::new()
                    .delete(&url)
                    .header("Authorization", format!("Bearer {}", auth_token))
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        log::info!(
                            "[CloseTaskSession] Deleted session {} from server",
                            session_id
                        );
                    }
                    Ok(resp) => {
                        log::warn!(
                            "[CloseTaskSession] Server delete session {} failed: {}",
                            session_id,
                            resp.status()
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "[CloseTaskSession] Server delete session {} error: {}",
                            session_id,
                            e
                        );
                    }
                }
            }
        }

        Ok(json!({
            "session_id": session_id,
            "message": "Task session closed and deleted."
        })
        .to_string())
    }
}
