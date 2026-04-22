//! Wait Tool Executor
//!
//! Synchronously blocks the agent loop for up to N seconds,
//! returning early if a new message arrives in the session queue.
//! This lets the agent "yield" after dispatching tasks instead of
//! spinning in thought or stopping entirely.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};

/// Poll interval for queue check (seconds).
const POLL_INTERVAL_SECS: u64 = 2;

pub struct WaitExecutor;

impl WaitExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WaitExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for WaitExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let seconds = input.get("seconds").and_then(|v| v.as_u64()).unwrap_or(30);

        let reason = input
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("waiting for task completion");

        let session_id = input
            .get("__session_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        log::info!(
            "[Wait] Waiting up to {}s for session {:?}: {}",
            seconds,
            session_id,
            reason
        );

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(seconds);

        let mut elapsed = 0u64;

        loop {
            // Check if deadline reached
            if tokio::time::Instant::now() >= deadline {
                return Ok(json!({
                    "status": "timeout",
                    "waited_seconds": seconds,
                    "message": format!(
                        "Waited {}s, no new messages arrived. \
                         Report your progress to the user via prompt_user, then stop output. \
                         You will receive a [Task Complete] message when the task finishes.",
                        seconds
                    )
                })
                .to_string());
            }

            // Poll the session queue for new messages via the SpawnConfig hook.
            // The host impl wraps `happy_client::manager::SpawnSessionConfig`
            // and returns the pending queued message's content (if any).
            if let Some(ref sid) = session_id {
                if let Some(provider) = crate::hooks::spawn_config() {
                    if let Some(content) = provider.peek_session_message(sid).await {
                        log::info!(
                            "[Wait] Message arrived after {}s: len={}",
                            elapsed,
                            content.len()
                        );
                        return Ok(json!({
                            "status": "message_arrived",
                            "waited_seconds": elapsed,
                            "message": format!(
                                "A new message arrived after {}s. \
                                 It will be delivered to you automatically. \
                                 Continue processing.",
                                elapsed
                            )
                        })
                        .to_string());
                    }
                }
            }

            // Sleep for poll interval
            tokio::time::sleep(tokio::time::Duration::from_secs(POLL_INTERVAL_SECS)).await;
            elapsed += POLL_INTERVAL_SECS;
        }
    }
}
