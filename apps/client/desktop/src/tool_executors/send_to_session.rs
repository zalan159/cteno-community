//! Send to Session Tool Executor
//!
//! Sends a message to a running task session, resuming its local connection if
//! it was previously released while idle.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct SendToSessionExecutor;

impl SendToSessionExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SendToSessionExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for SendToSessionExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let session_id = input
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: session_id")?;

        let message = input
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: message")?;

        let spawn_config = crate::local_services::spawn_config()?;
        crate::session_delivery::deliver_message_to_session(
            &spawn_config,
            session_id,
            message,
            "SendToSession",
        )
        .await?;

        log::info!(
            "[SendToSession] Message delivered to session {} ({} chars)",
            session_id,
            message.len()
        );
        Ok(json!({
            "session_id": session_id,
            "message": "Message sent to session. The worker agent will process it."
        })
        .to_string())
    }
}
