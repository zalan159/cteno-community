//! Transport-agnostic host event bus.
//!
//! Domain events that cross the session boundary (persona lifecycle, a2ui
//! surface updates, background-task updates, …) are expressed as typed
//! [`HostEvent`] values and dispatched through whichever
//! [`HostEventSink`] implementations the host has installed at boot.
//!
//! The desktop shell typically installs a composite sink:
//!
//! * `TauriHostEventSink`   — always on, fires a `local-host-event` Tauri
//!                             event so the frontend reacts in both community
//!                             and commercial builds.
//! * `SocketHostEventSink`  — commercial-cloud only, relays the event over
//!                             the machine socket so mobile / other devices
//!                             see it too.
//!
//! Business code never talks to a socket directly — it just calls
//! [`emit`] with a typed [`HostEvent`].  This keeps "what happened" (domain
//! semantics) independent of "how it travels" (transport).

use async_trait::async_trait;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Typed domain event emitted by host-level business code.  New variants
/// should map one-to-one to a concrete UI-facing update; add a new variant
/// rather than overloading an existing one with extra fields.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum HostEvent {
    /// A persona's pending chat session has finished spawning and its real
    /// session id has been bound.  Drives persona list / detail refresh.
    #[serde(rename = "persona-session-ready")]
    PersonaSessionReady {
        #[serde(rename = "personaId")]
        persona_id: String,
        #[serde(rename = "pendingSessionId")]
        pending_session_id: String,
        #[serde(rename = "attemptId")]
        attempt_id: String,
        vendor: String,
        #[serde(rename = "machineId")]
        machine_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        session: Option<serde_json::Value>,
    },

    /// A persona's pending chat session failed to spawn.  The pending session
    /// remains the UI anchor so queued user input can stay visible.
    #[serde(rename = "persona-session-failed")]
    PersonaSessionFailed {
        #[serde(rename = "personaId")]
        persona_id: String,
        #[serde(rename = "pendingSessionId")]
        pending_session_id: String,
        #[serde(rename = "attemptId")]
        attempt_id: String,
        vendor: String,
        #[serde(rename = "machineId")]
        machine_id: String,
        error: String,
    },

    /// An `a2ui_render` batch committed changes to a surface.  Drives UI
    /// re-render on the matching agent's view.
    #[serde(rename = "a2ui-updated")]
    A2uiUpdated {
        #[serde(rename = "agentId")]
        agent_id: String,
    },

    /// A background-task record was created or updated.  Drives the
    /// background-task list refresh for the owning session.
    #[serde(rename = "background-task-updated")]
    BackgroundTaskUpdated {
        #[serde(rename = "sessionId")]
        session_id: String,
        task: serde_json::Value,
    },
}

/// Sink that consumes host events.  Implementations must be cheap to clone
/// (held behind `Arc`) and safe to call from any async context.
#[async_trait]
pub trait HostEventSink: Send + Sync {
    async fn emit(&self, event: &HostEvent);
}

static SINK: OnceCell<Arc<dyn HostEventSink>> = OnceCell::new();

/// Install the process-wide host event sink.  First installer wins; later
/// calls are silently ignored so tests / re-init paths don't panic.
pub fn install_sink(sink: Arc<dyn HostEventSink>) {
    if SINK.set(sink).is_err() {
        log::debug!("[HostEvent] sink already installed; ignoring reinstall");
    }
}

/// Emit a host event.  Drops the event with a debug log when no sink has
/// been installed (headless / test builds that never wired one up).
pub async fn emit(event: HostEvent) {
    if let Some(sink) = SINK.get().cloned() {
        sink.emit(&event).await;
    } else {
        log::debug!("[HostEvent] emit dropped (no sink installed): {:?}", event);
    }
}

/// Composite sink that fans each event out to every sub-sink in order.
/// The desktop app uses this to dispatch to both the Tauri channel (always)
/// and the machine socket (commercial-cloud only).
pub struct CompositeHostEventSink {
    sinks: Vec<Arc<dyn HostEventSink>>,
}

impl CompositeHostEventSink {
    pub fn new(sinks: Vec<Arc<dyn HostEventSink>>) -> Self {
        Self { sinks }
    }
}

#[async_trait]
impl HostEventSink for CompositeHostEventSink {
    async fn emit(&self, event: &HostEvent) {
        for sink in &self.sinks {
            sink.emit(event).await;
        }
    }
}
