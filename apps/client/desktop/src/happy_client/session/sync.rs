use super::*;
use crate::executor_normalizer::ExecutorNormalizer;
use crate::session_message_codec::SessionMessageCodec;
use multi_agent_runtime_core::EventStream;

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

/// Best-effort relay of a user message to the **remote** transport so other
/// clients (mobile, second tab) attached to the same session get it.
///
/// **Caller contract**: caller has *already* surfaced the message in the
/// local UI (e.g. the frontend optimistic-rendered it on send-button click,
/// or the local sink wrote it via [`inject_user_message_into_session`]).
/// This helper only handles outbound replication.
///
/// - **Local mode**: no-op. There is no remote server to relay to, and the
///   community-mode `HappySocket::emit` is a stub that silently drops the
///   payload. Returning `Ok(())` here keeps the call site mode-agnostic.
/// - **Commercial / Socket.IO mode**: emits `SESSION_MESSAGE_RELAY_EVENT`
///   so happy-server fans the message out to other connected clients.
async fn relay_user_message_to_remote(
    socket: &HappySocket,
    session_id: &str,
    text: &str,
    images: &[serde_json::Value],
    sent_from: &str,
    local_id: Option<String>,
) -> Result<(), String> {
    if socket.is_local() {
        let _ = (text, images, sent_from, local_id, session_id);
        return Ok(());
    }

    let payload = json!({
        "sessionId": session_id,
        "message": build_relay_user_message(text, images, sent_from, local_id),
    });
    socket.emit(SESSION_MESSAGE_RELAY_EVENT, payload).await
}

/// Inject a user-role message into a session so it appears as a user-bubble
/// in the persona transcript. Transport-agnostic — picks the correct
/// delivery channel for the active socket mode AND **also** does the
/// remote relay so other clients see it.
///
/// Use this when the message is **synthetic** (no frontend optimistic
/// render exists yet) — e.g. `dispatch_task`'s initial prompt or the
/// autonomous-turn handler's `[Task Complete] X` handoffs.
///
/// For user-typed messages whose bubble was already rendered optimistically
/// at send-button click, use [`relay_user_message_to_remote`] instead — it
/// only does the cross-device sync without re-rendering.
///
/// - **Local mode**: writes the message into the session's
///   `agent_sessions.messages` row via the global [`LocalEventSink`] and
///   emits the `local-session:message-appended` Tauri event so the
///   frontend renders the bubble in real-time.
/// - **Commercial / Socket.IO mode**: emits `SESSION_MESSAGE_RELAY_EVENT`
///   so happy-server takes care of persistence + on_update broadcast.
async fn inject_user_message_into_session(
    socket: &HappySocket,
    session_id: &str,
    text: &str,
    images: &[serde_json::Value],
    sent_from: &str,
    local_id: Option<String>,
) -> Result<(), String> {
    if socket.is_local() {
        // `images` is unused on this path — the local sink appends a plain
        // text user message and image attachments on the local pipeline are
        // wired up at the worker layer.
        let _ = (images, sent_from, local_id);
        return crate::happy_client::local_sink::append_user_message_to_local_session(
            session_id,
            text,
        );
    }

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
        // The frontend already rendered this user-bubble optimistically when
        // the user clicked send, so we only need the remote sync — never
        // a second local render (would cause a duplicate bubble).
        if let Err(e) = relay_user_message_to_remote(
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
    /// Persist a runtime-owned ACP payload that arrived outside the active
    /// turn stream. This is display/transport only; Cteno runtime remains the
    /// owner of SubAgent and DAG progression.
    pub async fn persist_runtime_acp_message(
        &self,
        acp_data: serde_json::Value,
    ) -> Result<(), String> {
        send_acp_message(
            self.socket.as_ref(),
            &self.session_id,
            acp_data,
            &self.message_codec,
        )
        .await
    }

    /// Send a user-role message into this session, triggering agent processing.
    ///
    /// Used by `dispatch_task` to inject the task prompt as if the user sent it.
    pub async fn send_initial_user_message(&self, content: &str) -> Result<(), String> {
        let images: &[serde_json::Value] = &[];
        // Synthetic message — no frontend optimistic render exists, so use
        // the full inject path that both renders the bubble and (in
        // commercial mode) syncs to other clients.
        inject_user_message_into_session(
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

    /// Consume an autonomous-turn event stream coming from the cteno adapter
    /// and render it through a freshly-built `ExecutorNormalizer`.
    ///
    /// Used by the cteno autonomous_turn_handler registered on the executor:
    /// when the cteno-agent self-initiates a turn (e.g. after a SubAgent
    /// handoff), the adapter's dispatcher hands us:
    /// - the **synthetic user-message text** that triggered the turn (if
    ///   any) — typically the concatenated `[Task Complete] X\n\n result`
    ///   blocks for queued subagent handoffs. Persisted as a user message in
    ///   the persona transcript before the turn's assistant frames stream
    ///   in, so the user can see what the agent was reacting to.
    /// - an `EventStream` carrying the turn's assistant-side events.
    ///
    /// Each call spawns a dedicated consumer task that runs until the stream
    /// emits `TurnComplete` (or errors out), feeding every event through a
    /// normalizer keyed by this session. Mirrors the normalizer surface used
    /// by user-driven turns (`run_executor_turn`).
    pub fn spawn_autonomous_turn_consumer(
        &self,
        synthetic_user_message: Option<String>,
        stream: EventStream,
    ) {
        let session_id = self.session_id.clone();
        let socket = self.socket.clone();
        // Keep a separate clone alive past `normalizer` construction
        // (which moves `socket`) so the spawned task can still call the
        // transport-agnostic `relay_user_message_to_session` for the
        // synthetic user-bubble.
        let socket_for_relay = self.socket.clone();
        let message_codec = self.message_codec;
        let permission_handler = self.permission_handler.clone();
        let context_tokens = self.context_tokens.clone();
        let compression_threshold = self.compression_threshold.clone();
        let agent_config = self.agent_config.clone();
        let executor = match self.executor.clone() {
            Some(exec) => exec,
            None => {
                log::warn!(
                    "[Session {}] autonomous turn arrived but no executor wired — dropping",
                    self.session_id
                );
                return;
            }
        };
        let session_ref = match self.session_ref.clone() {
            Some(sr) => sr,
            None => {
                log::warn!(
                    "[Session {}] autonomous turn arrived but no session_ref wired — dropping",
                    self.session_id
                );
                return;
            }
        };

        tokio::spawn(async move {
            use futures_util::StreamExt;

            let task_id = format!("autonomous-{}", uuid::Uuid::new_v4());
            log::info!(
                "[Session {}] autonomous turn consumer started (task_id={}, synthetic_msg={} chars)",
                session_id,
                task_id,
                synthetic_user_message.as_deref().map(str::len).unwrap_or(0)
            );
            let normalizer = ExecutorNormalizer::new(
                session_id.clone(),
                socket,
                message_codec,
                None,
                permission_handler,
                task_id.clone(),
                executor,
                session_ref,
                agent_config.server_url.clone(),
                agent_config.auth_token.clone(),
                agent_config.db_path.clone(),
                Some(context_tokens),
                Some(compression_threshold),
            );

            // Surface the synthetic user-message text BEFORE consuming
            // assistant frames so the user can see what triggered this
            // autonomous turn (e.g. the `[Task Complete] X` handoffs).
            // `inject_user_message_into_session` is transport-agnostic and
            // does the full render: in local mode it routes through the
            // LocalEventSink (SQLite + Tauri event); in commercial mode it
            // emits `SESSION_MESSAGE_RELAY_EVENT` so the server fans the
            // message out. There's no frontend optimistic render for an
            // autonomous turn, so we need the full inject path (vs the
            // remote-only `relay_user_message_to_remote` used for already-
            // rendered user-typed messages).
            if let Some(text) = synthetic_user_message.as_deref() {
                let images: &[serde_json::Value] = &[];
                if let Err(e) = inject_user_message_into_session(
                    socket_for_relay.as_ref(),
                    &session_id,
                    text,
                    images,
                    "autonomous_turn",
                    None,
                )
                .await
                {
                    log::warn!(
                        "[Session {}] autonomous turn synthetic user-message inject failed: {}",
                        session_id,
                        e
                    );
                }
            }

            let mut stream = Box::pin(stream);
            let mut event_count: u32 = 0;
            while let Some(event_result) = stream.next().await {
                let event = match event_result {
                    Ok(event) => event,
                    Err(e) => {
                        log::warn!(
                            "[Session {}] autonomous turn stream error: {}",
                            session_id,
                            e
                        );
                        break;
                    }
                };
                event_count += 1;
                match normalizer.process_event(event).await {
                    Ok(true) => break,
                    Ok(false) => continue,
                    Err(e) => {
                        log::warn!(
                            "[Session {}] autonomous turn normalizer error: {}",
                            session_id,
                            e
                        );
                        break;
                    }
                }
            }
            log::info!(
                "[Session {}] autonomous turn consumer finished after {} events",
                session_id,
                event_count
            );
        });
    }
}
