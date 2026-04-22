use super::*;
use crate::happy_client::permission::PermissionMode;
use crate::session_message_codec::SessionMessageCodec;
use cteno_host_session_wire::ConnectionType;

fn update_local_permission_mode(
    permission_handler: &PermissionHandler,
    permission_mode: Option<&str>,
) {
    let Some(mode_str) = permission_mode else {
        return;
    };

    if let Some(mode) = PermissionHandler::parse_mode(mode_str) {
        let current = permission_handler.get_mode();
        if current != PermissionMode::BypassPermissions || mode == PermissionMode::BypassPermissions
        {
            permission_handler.set_mode(mode);
        }
    }
}

fn build_local_stream_callback(
    channel: tauri::ipc::Channel<crate::AgentStreamEvent>,
) -> crate::llm::StreamCallback {
    Arc::new(move |delta: serde_json::Value| {
        let ch = channel.clone();
        let delta_type = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match delta_type {
            "stream-start" => match ch.send(crate::AgentStreamEvent::StreamStart) {
                Ok(_) => log::info!("[StreamCallback] sent stream-start"),
                Err(e) => log::error!("[StreamCallback] stream-start failed: {e}"),
            },
            "text-delta" => {
                let text = delta
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let len = text.len();
                match ch.send(crate::AgentStreamEvent::TextDelta { text }) {
                    Ok(_) => log::info!("[StreamCallback] sent text-delta len={len}"),
                    Err(e) => log::error!("[StreamCallback] text-delta failed: {e}"),
                }
            }
            "thinking-delta" => {
                let text = delta
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                match ch.send(crate::AgentStreamEvent::ThinkingDelta { text }) {
                    Ok(_) => log::info!("[StreamCallback] sent thinking-delta"),
                    Err(e) => log::error!("[StreamCallback] thinking-delta failed: {e}"),
                }
            }
            "stream-end" => match ch.send(crate::AgentStreamEvent::StreamEnd) {
                Ok(_) => log::info!("[StreamCallback] sent stream-end"),
                Err(e) => log::error!("[StreamCallback] stream-end failed: {e}"),
            },
            "finished" => match ch.send(crate::AgentStreamEvent::Finished) {
                Ok(_) => log::info!("[StreamCallback] sent finished"),
                Err(e) => log::error!("[StreamCallback] finished failed: {e}"),
            },
            other => {
                log::warn!("[StreamCallback] unknown delta type: {other}");
            }
        }
        Box::pin(async {})
    })
}

impl SessionConnectionHandle {
    /// Inject a decrypted remote-sync user message into the local queue
    /// without echoing it back to Happy Server.
    pub async fn inject_remote_message(
        &self,
        text: String,
        images: Vec<serde_json::Value>,
        permission_mode: Option<String>,
        local_id: Option<String>,
    ) -> Result<(), String> {
        let sid = self.session_id.clone();

        update_local_permission_mode(&self.permission_handler, permission_mode.as_deref());

        let mut agent_msg = if images.is_empty() {
            AgentMessage::user(sid.clone(), text)
        } else {
            AgentMessage::user_with_images(sid.clone(), text, images)
        };
        agent_msg.local_id = local_id;
        self.execution_state
            .queue
            .push(agent_msg)
            .map_err(|e| format!("Failed to queue remote sync message: {}", e))?;

        let started = worker::spawn_background_queue_worker_if_idle(
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
            worker::BackgroundWorkerOptions {
                worker_label: "remote-sync",
                auto_hibernate: true,
                auto_rename_persona: true,
            },
        );

        if !started {
            log::info!(
                "[RemoteSync] Session {} already processing, message queued for existing worker",
                sid
            );
        }

        Ok(())
    }

    /// Inject a user message directly from Tauri IPC (desktop local path).
    /// The local queue + worker runtime is the community-edition execution core.
    /// Remote server sync is treated as a best-effort sidecar for commercial/mobile use.
    pub async fn inject_local_message(
        &self,
        text: String,
        images: Vec<serde_json::Value>,
        permission_mode: Option<String>,
        local_id: Option<String>,
        channel: tauri::ipc::Channel<crate::AgentStreamEvent>,
    ) -> Result<(), String> {
        let sid = self.session_id.clone();
        log::info!(
            "[LocalIPC] Injecting local message for session {}: {} chars",
            sid,
            text.len()
        );

        update_local_permission_mode(&self.permission_handler, permission_mode.as_deref());

        let mut agent_msg = if images.is_empty() {
            AgentMessage::user(sid.clone(), text.clone())
        } else {
            AgentMessage::user_with_images(sid.clone(), text.clone(), images.clone())
        };
        agent_msg.local_id = local_id.clone();
        self.execution_state
            .queue
            .push(agent_msg)
            .map_err(|e| format!("Failed to queue local message: {}", e))?;

        log::info!(
            "[LocalIPC] Message queued for session {}, queue_len={}",
            sid,
            self.execution_state.queue.len(&sid)
        );

        super::sync::spawn_optional_remote_user_sync(
            sid.clone(),
            self.socket.clone(),
            self.message_codec,
            text,
            images,
            local_id,
        );

        let stream_callback = build_local_stream_callback(channel.clone());
        let started = worker::run_worker_loop_if_idle(
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
                worker_label: "local-ipc",
                auto_hibernate: true,
                auto_rename_persona: true,
                execution_mode: worker::WorkerExecutionMode::LocalIpc { stream_callback },
            },
        )
        .await;

        if !started {
            log::info!(
                "[LocalIPC] Worker already running for session {}, message queued for existing worker",
                sid
            );
            let _ = channel.send(crate::AgentStreamEvent::Finished);
            return Ok(());
        }

        let _ = channel.send(crate::AgentStreamEvent::Finished);
        Ok(())
    }
}

impl SessionConnection {
    /// Establish a pure-local session connection for logged-out desktop mode.
    /// No remote transport, heartbeat, or Happy Server sync is required.
    pub(crate) async fn establish_local_connection(
        session_id: String,
        agent_config: SessionAgentConfig,
        session_connections: SessionRegistry,
    ) -> Result<Self, String> {
        let socket = Arc::new(HappySocket::local(ConnectionType::SessionScoped {
            session_id: session_id.clone(),
        }));
        crate::happy_client::local_sink::attach_to_socket(&socket);

        let heartbeat_running = Arc::new(AtomicBool::new(false));
        let consecutive_failures = Arc::new(AtomicU32::new(0));
        let context_tokens = Arc::new(AtomicU32::new(0));
        let compression_threshold = Arc::new(AtomicU32::new(64000));
        let execution_state = ExecutionState::new(Arc::new(AgentMessageQueue::new()));
        // Reuse the process-global session handler so pending permission
        // requests survive disconnect+reconnect races. See
        // `happy_client::permission::get_or_create_handler`.
        let permission_handler =
            crate::happy_client::permission::get_or_create_handler(&session_id, 0);
        let profile_id_ref = agent_config.profile_id.clone();
        let local_origin = Arc::new(AtomicBool::new(true));

        let conn = Self {
            session_id,
            socket,
            message_codec: SessionMessageCodec::plaintext(),
            heartbeat_running,
            execution_state,
            consecutive_failures,
            permission_handler,
            profile_id: profile_id_ref,
            context_tokens,
            compression_threshold,
            agent_config,
            local_origin,
            session_connections,
            // T11 scaffold: executor wiring is opt-in (see connection.rs).
            executor: None,
            session_ref: None,
        };

        conn.start_heartbeat().await;

        Ok(conn)
    }
}
