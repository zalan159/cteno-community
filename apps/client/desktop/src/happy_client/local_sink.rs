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
//!   * agent-state updates → Tauri event carrying the JSON snapshot
//!   * metadata updates      → Tauri event
//!
//! The frontend's `sync.ts` subscribes to `local-session:*` Tauri events and
//! forwards them into the storage layer / invalidates the messages sync.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

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
}

impl DesktopLocalSink {
    pub fn new(db_path: PathBuf, app_handle: AppHandle) -> Self {
        Self {
            db_path,
            app_handle,
        }
    }

    fn emit_tauri(&self, event: &str, payload: serde_json::Value) {
        if let Err(e) = self.app_handle.emit(event, payload) {
            log::warn!("[LocalSink] failed to emit '{event}': {e}");
        }
    }
}

impl LocalEventSink for DesktopLocalSink {
    fn on_message(&self, session_id: &str, encrypted_message: &str, local_id: Option<&str>) {
        // For local-only sessions the Plaintext codec writes UTF-8 JSON
        // verbatim, so `encrypted_message` is the raw ACP record text.
        let message_json = encrypted_message.to_string();

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
