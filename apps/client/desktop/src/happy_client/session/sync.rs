use super::*;
use crate::session_message_codec::SessionMessageCodec;

const SESSION_MESSAGE_RELAY_EVENT: &str = "session-message-relay";

fn build_remote_user_content(text: &str, images: &[serde_json::Value]) -> serde_json::Value {
    if images.is_empty() {
        json!({ "type": "text", "text": text })
    } else {
        let mut blocks = images.to_vec();
        blocks.push(json!({ "type": "text", "text": text }));
        serde_json::Value::Array(blocks)
    }
}

fn build_relay_user_message(
    text: &str,
    images: &[serde_json::Value],
    sent_from: &str,
    local_id: Option<String>,
) -> serde_json::Value {
    json!({
        "role": "user",
        "content": build_remote_user_content(text, images),
        "meta": {
            "sentFrom": sent_from
        },
        "localId": local_id,
    })
}

async fn relay_user_message_to_session(
    socket: &HappySocket,
    session_id: &str,
    text: &str,
    images: &[serde_json::Value],
    sent_from: &str,
    local_id: Option<String>,
) -> Result<(), String> {
    let payload = json!({
        "sessionId": session_id,
        "message": build_relay_user_message(text, images, sent_from, local_id),
    });

    socket.emit(SESSION_MESSAGE_RELAY_EVENT, payload).await
}

pub(super) fn spawn_optional_remote_user_sync(
    session_id: String,
    socket: Arc<HappySocket>,
    _message_codec: SessionMessageCodec,
    text: String,
    images: Vec<serde_json::Value>,
    local_id: Option<String>,
) {
    tokio::spawn(async move {
        if let Err(e) = relay_user_message_to_session(
            socket.as_ref(),
            &session_id,
            &text,
            &images,
            "mac",
            local_id,
        )
        .await
        {
            log::warn!("[LocalIPC] Failed to relay user message to server: {}", e);
        } else {
            log::info!(
                "[LocalIPC] User message relayed to server for session {}",
                session_id
            );
        }
    });
}

impl SessionConnectionHandle {
    /// Send a user-role message into this session, triggering agent processing.
    ///
    /// Used by `dispatch_task` to inject the task prompt as if the user sent it.
    pub async fn send_initial_user_message(&self, content: &str) -> Result<(), String> {
        let images: &[serde_json::Value] = &[];
        relay_user_message_to_session(
            self.socket.as_ref(),
            &self.session_id,
            content,
            images,
            "cli",
            None,
        )
        .await?;

        // Also push directly to the local agent queue and start the worker.
        // Socket.IO broadcast doesn't echo back to the sender, so the on_update
        // handler will never receive this message. We must inject it locally.
        let sid = self.session_id.clone();
        if let Err(e) = self
            .execution_state
            .queue
            .push(AgentMessage::user(sid.clone(), content.to_string()))
        {
            log::error!(
                "[Session {}] Failed to push initial message to queue: {}",
                self.session_id,
                e
            );
            return Err(format!("Failed to queue initial message: {}", e));
        }

        let started = worker::spawn_worker_loop_if_idle(
            worker::BackgroundWorkerState {
                session_id: sid.clone(),
                execution_state: self.execution_state.clone(),
                config: self.agent_config.clone(),
                socket_for_response: self.socket.clone(),
                message_codec: self.message_codec,
                perm_handler: self.permission_handler.clone(),
                context_tokens: self.context_tokens.clone(),
                compression_threshold: self.compression_threshold.clone(),
                executor: self.executor.clone(),
                session_ref: self.session_ref.clone(),
            },
            worker::WorkerLoopOptions {
                worker_label: "initial-message",
                auto_hibernate: true,
                auto_rename_persona: false,
                execution_mode: worker::WorkerExecutionMode::Background,
            },
        );

        if !started {
            log::info!(
                "[Session {}] Initial message queued for existing worker",
                self.session_id
            );
        }

        log::info!(
            "[Session {}] Sent initial user message ({} chars) and started agent worker",
            self.session_id,
            content.len()
        );

        Ok(())
    }
}
