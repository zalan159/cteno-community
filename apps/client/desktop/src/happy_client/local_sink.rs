//! `LocalEventSink` implementation for the desktop app.
//!
//! Attached to the per-session `HappySocket::local(...)` instances at
//! construction time (see `attach_to_socket`). Remote sockets
//! (`HappySocket::connect`) **do not** get the sink installed — their
//! broadcasts go straight to Happy Server and the frontend there receives
//! them over Socket.IO.
//!
//! For local-only sessions the codec is always `Plaintext`, so the
//! `encrypted_message` / `encrypted_state` arguments delivered to the sink
//! are in fact raw UTF-8 JSON. No decryption is needed here.
//!
//! Responsibilities:
//!   * ACP persisted messages → append to `agent_sessions.messages`
//!   * ACP transient messages → Tauri event only (not persisted)
//!   * agent-state updates → persist snapshot + Tauri event
//!   * metadata updates      → Tauri event
//!
//! The frontend's `sync.ts` subscribes to `local-session:*` Tauri events and
//! forwards them into the storage layer / invalidates the messages sync.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::happy_client::socket::{HappySocket, LocalEventSink, LocalEventSinkArc};
use cteno_agent_runtime::agent_session::{AgentSessionManager, SessionMessage};
use serde_json::json;
use tauri::{AppHandle, Emitter};

/// Process-global sink. Installed once at app startup via
/// [`install_global_sink`] and attached to every **local** `HappySocket`
/// constructed afterwards via [`attach_to_socket`]. Constructions that run
/// before the global is set silently skip attachment.
static GLOBAL_SINK: OnceLock<Arc<DesktopLocalSink>> = OnceLock::new();

pub fn install_global_sink(sink: Arc<DesktopLocalSink>) {
    if GLOBAL_SINK.set(sink).is_err() {
        log::warn!("[LocalSink] global sink already installed; ignoring");
    }
}

/// Public accessor to the installed global sink. Returns `None` if no
/// sink has been registered yet (e.g. very early boot or non-Tauri tests).
/// Used by the cteno executor registry to register the same sink as the
/// `SessionEventSink` for adapter-side subagent lifecycle events.
pub fn global() -> Option<Arc<DesktopLocalSink>> {
    GLOBAL_SINK.get().cloned()
}

/// Call at every `HappySocket::local(...)` construction site. No-op for
/// sockets created via `HappySocket::connect` (remote mode) — callers just
/// skip this helper there.
pub fn attach_to_socket(socket: &HappySocket) {
    if let Some(sink) = GLOBAL_SINK.get() {
        let arc: LocalEventSinkArc = sink.clone();
        socket.install_local_sink(arc);
    } else {
        log::debug!("[LocalSink] attach_to_socket called before global sink install");
    }
}

pub struct DesktopLocalSink {
    db_path: PathBuf,
    app_handle: AppHandle,
    /// Per-session write mutex used to serialise concurrent
    /// read-modify-write `update_messages` cycles. Without this, races
    /// between assistant-frame appends (`on_message`) and synthetic
    /// user-bubble appends (`append_user_message_to_local_session`) drop
    /// messages: each writer reads the current `messages` array, pushes
    /// its own entry, and overwrites the column — whoever flushes last
    /// wins and clobbers the other. Observed concretely in DAG runs where
    /// multiple `[Task Complete] X` user-bubbles fired close together
    /// alongside autonomous-turn assistant frames; only the largest batch
    /// survived.
    ///
    /// We keep the mutex inside an `Arc` so we can clone the lock handle
    /// out from under the registry mutex before doing any I/O — long
    /// SQLite work mustn't pin the registry. Std `Mutex` is fine here:
    /// contention is bounded by per-session traffic and the critical
    /// section is a single read-modify-write, no async awaits inside.
    session_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl DesktopLocalSink {
    pub fn new(db_path: PathBuf, app_handle: AppHandle) -> Self {
        Self {
            db_path,
            app_handle,
            session_locks: Mutex::new(HashMap::new()),
        }
    }

    fn emit_tauri(&self, event: &str, payload: serde_json::Value) {
        if let Err(e) = self.app_handle.emit(event, payload) {
            log::warn!("[LocalSink] failed to emit '{event}': {e}");
        }
    }

    /// Append a subagent's ACP frame to its own `agent_sessions` row in
    /// `cteno.db`. Receives the raw ACP `data` payload (text-delta,
    /// thinking, tool-call, etc.) plus the subagent's own session id (which
    /// equals `SubAgent.id`, pre-created on Spawned). Builds the same
    /// `{role:agent, content:{type:acp, provider:cteno, data}}` envelope
    /// `send_acp_message` builds for the parent persona's persisted ACP, so
    /// `BaseSessionPage` (via `useSession(subagent.id)`) renders the
    /// subagent's transcript identically to a normal session.
    pub fn append_subagent_acp(&self, subagent_session_id: &str, acp_data: serde_json::Value) {
        let envelope = json!({
            "role": "agent",
            "content": {
                "type": "acp",
                "provider": "cteno",
                "data": acp_data,
            },
            "meta": { "sentFrom": "cli" },
        });
        let message_json = match serde_json::to_string(&envelope) {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "[LocalSink] failed to serialize subagent ACP envelope for {subagent_session_id}: {e}"
                );
                return;
            }
        };
        let lock = self.session_write_lock(subagent_session_id);
        let _guard = lock.lock().expect("per-session mutex poisoned");
        if let Err(e) = append_to_existing_session(
            &self.db_path,
            subagent_session_id,
            "assistant",
            message_json,
            None,
        ) {
            log::warn!(
                "[LocalSink] failed to persist subagent ACP for {subagent_session_id}: {e}"
            );
            return;
        }
        drop(_guard);
        self.emit_tauri(
            "local-session:message-appended",
            json!({ "sessionId": subagent_session_id }),
        );
    }

    /// Take (or create) the per-session write lock. Returns an `Arc<Mutex<()>>`
    /// that the caller locks for the duration of its read-modify-write.
    fn session_write_lock(&self, session_id: &str) -> Arc<Mutex<()>> {
        let mut registry = self
            .session_locks
            .lock()
            .expect("session_locks mutex poisoned");
        registry
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

impl LocalEventSink for DesktopLocalSink {
    fn on_message(&self, session_id: &str, encrypted_message: &str, local_id: Option<&str>) {
        // For local-only sessions the Plaintext codec writes UTF-8 JSON
        // verbatim, so `encrypted_message` is the raw ACP record text.
        let message_json = encrypted_message.to_string();

        // Hold the per-session write lock for the read-modify-write so
        // assistant-frame appends here don't race with synthetic
        // user-bubble appends in `append_user_message_to_local_session`.
        let lock = self.session_write_lock(session_id);
        let _guard = lock.lock().expect("per-session mutex poisoned");
        if let Err(e) = append_to_existing_session(
            &self.db_path,
            session_id,
            "assistant",
            message_json,
            local_id.map(str::to_string),
        ) {
            log::error!("[LocalSink] failed to persist message for session {session_id}: {e}");
            return;
        }
        drop(_guard);

        self.emit_tauri(
            "local-session:message-appended",
            json!({ "sessionId": session_id }),
        );
    }

    fn on_transient_message(&self, session_id: &str, encrypted_message: &str) {
        // Transient messages are not persisted — frontend consumes them in
        // real time via the Tauri event.
        self.emit_tauri(
            "local-session:transient",
            json!({
                "sessionId": session_id,
                "payload": encrypted_message,
            }),
        );
    }

    fn on_state_update(&self, session_id: &str, encrypted_state: Option<&str>, version: u32) {
        let parsed_state = match encrypted_state {
            Some(state) => match serde_json::from_str(state) {
                Ok(value) => Some(Some(value)),
                Err(e) => {
                    log::warn!(
                        "[LocalSink] failed to parse agent state for session {session_id}: {e}"
                    );
                    None
                }
            },
            None => Some(None),
        };
        if let Some(parsed_state) = parsed_state {
            let manager = AgentSessionManager::new(self.db_path.clone());
            if let Err(e) =
                manager.update_agent_state(session_id, parsed_state.as_ref(), u64::from(version))
            {
                log::warn!(
                    "[LocalSink] failed to persist agent state for session {session_id}: {e}"
                );
            }
        }

        self.emit_tauri(
            "local-session:state-update",
            json!({
                "sessionId": session_id,
                "agentState": encrypted_state,
                "version": version,
            }),
        );
    }

    fn on_metadata_update(&self, session_id: &str, encrypted_metadata: &str, version: u32) {
        self.emit_tauri(
            "local-session:metadata-update",
            json!({
                "sessionId": session_id,
                "metadata": encrypted_metadata,
                "version": version,
            }),
        );
    }

    fn on_session_alive(
        &self,
        session_id: &str,
        thinking: Option<bool>,
        thinking_status: Option<&str>,
        _context_tokens: u32,
        _compression_threshold: u32,
    ) {
        self.emit_tauri(
            "local-session:alive",
            json!({
                "sessionId": session_id,
                "active": true,
                "activeAt": chrono::Utc::now().timestamp_millis(),
                "thinking": thinking.unwrap_or(false),
                "thinkingStatus": thinking_status,
            }),
        );
    }
}

// `DesktopLocalSink` also serves as the `SessionEventSink` for the cteno
// adapter — same struct, two trait impls. The adapter's dispatcher calls
// `on_subagent_lifecycle` which we route into the process-global
// `subagent_mirror`. `on_session_message` is currently a no-op (the
// runtime-owned background ACP path it was designed for has been retired
// in favour of the explicit `AutonomousTurnStart` boundary frame; the
// trait surface stays so a future runtime can opt back in without
// re-introducing a sink type).
impl multi_agent_runtime_cteno::SessionEventSink for DesktopLocalSink {
    fn on_session_message(&self, session_id: &str, _acp_data: serde_json::Value) {
        log::debug!(
            "[LocalSink] on_session_message ({session_id}) ignored — runtime no longer emits this category"
        );
    }

    fn on_subagent_lifecycle(
        &self,
        parent_session_id: &str,
        event: multi_agent_runtime_cteno::SubAgentLifecycleEvent,
    ) {
        // For Spawned events, pre-create the agent_sessions row in cteno.db so
        // the subagent's first ACP frame (which `DesktopLocalSink::on_message`
        // appends via `append_to_existing_session`) doesn't error on a
        // missing row. cteno-agent already creates a row in its own
        // sessions.db, but desktop's cteno.db is a separate projection — we
        // need to materialise the row here. Done synchronously so it
        // happens-before any `on_message` for the same subagent_id.
        if let multi_agent_runtime_cteno::SubAgentLifecycleEvent::Spawned {
            ref subagent_id, ..
        } = event
        {
            let manager = AgentSessionManager::new(self.db_path.clone());
            if let Err(e) = manager.create_session_with_id(
                subagent_id,
                "worker",
                None,
                None,
            ) {
                if !e.contains("UNIQUE constraint failed") {
                    log::warn!(
                        "[LocalSink] failed to pre-create agent_sessions row for subagent {subagent_id}: {e}"
                    );
                }
            }
        }

        let Some(mirror) = crate::subagent_mirror::instance() else {
            log::warn!(
                "[LocalSink] subagent lifecycle for {parent_session_id} arrived before mirror install — dropping"
            );
            return;
        };
        // The `parent_session_id` we get is cteno-agent's native session id
        // (the one its SubAgentManager knows about). The desktop / frontend
        // identify the persona session by a *different* id (`session_id`
        // in `session_connections`). Translate once so the mirror — and
        // the `local-session:subagents-updated` Tauri event the frontend
        // filters on — uses the desktop-visible id. Mirrors the same
        // translation done by the autonomous_turn_handler in executor_registry.
        let parent_session_id = parent_session_id.to_string();
        tokio::spawn(async move {
            let display_session_id = match crate::local_services::spawn_config() {
                Ok(spawn_config) => {
                    if spawn_config
                        .session_connections
                        .get(&parent_session_id)
                        .await
                        .is_some()
                    {
                        // The id is already a desktop session id (no
                        // translation needed — happens when the persona's
                        // desktop id matches its native id).
                        parent_session_id.clone()
                    } else if let Some((happy_session_id, _conn)) = spawn_config
                        .session_connections
                        .get_by_executor_session_id(&parent_session_id)
                        .await
                    {
                        happy_session_id
                    } else {
                        log::warn!(
                            "[LocalSink] subagent lifecycle for native session {parent_session_id}: no matching desktop connection — applying with native id (UI may not see updates)"
                        );
                        parent_session_id.clone()
                    }
                }
                Err(e) => {
                    log::warn!(
                        "[LocalSink] spawn_config unavailable for subagent lifecycle: {e}; applying with native id"
                    );
                    parent_session_id.clone()
                }
            };
            mirror.apply_lifecycle(&display_session_id, event);
        });
    }
}

/// Append to an existing `agent_sessions` row. Errors if the session is
/// missing — session creation is handled upstream in the spawn path.
fn append_to_existing_session(
    db_path: &std::path::Path,
    session_id: &str,
    role: &str,
    content: String,
    local_id: Option<String>,
) -> Result<(), String> {
    let manager = AgentSessionManager::new(db_path.to_path_buf());
    let mut session = manager
        .get_session(session_id)?
        .ok_or_else(|| format!("local session {session_id} not found"))?;
    session.messages.push(SessionMessage {
        role: role.to_string(),
        content,
        timestamp: chrono::Utc::now().to_rfc3339(),
        local_id,
    });
    manager.update_messages(session_id, &session.messages)
}

/// Append a synthetic user-bubble to a local session and notify the frontend
/// to render it in real time. Used for the autonomous-turn synthetic
/// user-message text (e.g. concatenated `[Task Complete] X\n\n result`
/// blocks) — these don't come from the user typing in the UI, so there is
/// no optimistic frontend bubble; we have to both persist and emit the
/// `local-session:message-appended` Tauri event ourselves.
///
/// In commercial mode the equivalent is `relay_user_message_to_session`
/// (Socket.IO emit → server relay → frontend on_update). The local
/// `HappySocket::emit` is a stub, so we go straight through the
/// `LocalEventSink`'s machinery. Returns silently if no global sink is
/// installed (e.g. early boot or non-Tauri tests).
pub fn append_user_message_to_local_session(
    session_id: &str,
    text: &str,
) -> Result<(), String> {
    let Some(sink) = GLOBAL_SINK.get() else {
        log::warn!(
            "[LocalSink] no global sink installed; skipping user-message append for {session_id} ({} chars)",
            text.len()
        );
        return Ok(());
    };
    log::info!(
        "[LocalSink] append synthetic user-message to session {session_id} ({} chars)",
        text.len()
    );
    // Same per-session mutex as `on_message` — required to keep concurrent
    // assistant-frame writes from clobbering this user-bubble append (and
    // vice-versa). Without the shared lock, autonomous-turn DAG bursts that
    // emit multiple synthetic user-bubbles back-to-back interleaved with
    // streaming assistant frames lose all-but-one bubble.
    let lock = sink.session_write_lock(session_id);
    let _guard = lock.lock().expect("per-session mutex poisoned");
    if let Err(e) = append_to_existing_session(
        &sink.db_path,
        session_id,
        "user",
        text.to_string(),
        None,
    ) {
        log::warn!(
            "[LocalSink] failed to append user-message to {session_id}: {e}"
        );
        return Err(e);
    }
    drop(_guard);
    log::info!(
        "[LocalSink] emit local-session:message-appended for {session_id}"
    );
    sink.emit_tauri(
        "local-session:message-appended",
        json!({ "sessionId": session_id }),
    );
    Ok(())
}
