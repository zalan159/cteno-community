//! The `AgentExecutor` trait — unified session-level abstraction shared by
//! every agent vendor (Cteno, Claude, Codex, …).

use async_trait::async_trait;
use std::sync::Arc;

use super::capabilities::{AgentCapabilities, ExecutorSemanticCapabilities};
use super::error::AgentExecutorError;
use super::event::EventStream;
use super::types::{
    ConnectionHandleId, ConnectionHealth, ConnectionSpec, ModelChangeOutcome, ModelSpec,
    NativeMessage, NativeSessionId, NormalizedModelSpec, Pagination, PermissionDecision,
    PermissionMode, ResumeHints, SessionFilter, SessionInfo, SessionMeta, SessionRef,
    SpawnSessionSpec, UserMessage,
};

/// Opaque handle to a live, reusable vendor connection.
///
/// A connection is the vendor's persistent transport after the global
/// initialize handshake completes. It can host multiple concurrent sessions
/// (when [`AgentCapabilities::supports_multi_session_per_process`] is true).
///
/// `inner` holds adapter-owned state (subprocess, stdin/stdout mutexes, the
/// pending-request map, etc.). Each adapter downcasts `inner` to its concrete
/// state type in `start_session_on` / `check_connection` / `close_connection`.
pub struct ConnectionHandle {
    pub id: ConnectionHandleId,
    pub vendor: &'static str,
    pub inner: std::sync::Arc<dyn std::any::Any + Send + Sync>,
}

impl std::fmt::Debug for ConnectionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionHandle")
            .field("id", &self.id)
            .field("vendor", &self.vendor)
            .finish_non_exhaustive()
    }
}

impl Clone for ConnectionHandle {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            vendor: self.vendor,
            inner: self.inner.clone(),
        }
    }
}

/// Callback invoked when a vendor autonomously starts a new turn (i.e. a
/// turn not initiated by `send_message` — e.g. a background subagent
/// completed and woke the session, or a scheduled task fired).
///
/// Receives:
/// - the native session id
/// - an optional **synthetic user-message text** that triggered the turn
///   (e.g. concatenated `[Task Complete] X\n\n result` blocks for queued
///   subagent handoffs). The host SHOULD render this as a user-bubble in
///   the session transcript before consuming the stream — without it, the
///   autonomous turn looks like the agent talking to itself.
/// - an event stream that ends when the autonomous turn completes
///   (`ExecutorEvent::TurnComplete` observed). The host is responsible for
///   consuming the stream — typically by spawning a task that feeds events
///   into a normalizer / UI renderer keyed by the session id.
///
/// Vendors that lack this capability return `Unsupported` from
/// `set_autonomous_turn_handler` and never invoke the callback.
pub type AutonomousTurnHandler =
    Arc<dyn Fn(String, Option<String>, EventStream) + Send + Sync>;

/// Vendor-agnostic session-level contract.
///
/// Every method is `async` and can fail with [`AgentExecutorError`]. Callers
/// should consult [`AgentExecutor::capabilities`] first for optional surfaces
/// (`list_sessions`, runtime model change, …) — unsupported capabilities
/// return `AgentExecutorError::Unsupported` rather than panicking.
///
/// Implementations are owned as `Arc<dyn AgentExecutor>` and are expected to
/// be cheap to clone. Concrete implementations live in:
///
/// - `multi-agent-runtime-cteno::CtenoAgentExecutor`
/// - `multi-agent-runtime-claude::ClaudeAgentExecutor`
/// - `multi-agent-runtime-codex::CodexAgentExecutor`
#[async_trait]
pub trait AgentExecutor: Send + Sync + 'static {
    /// Static capability manifest. Must be pure — same result every call.
    fn capabilities(&self) -> AgentCapabilities;

    /// Additive semantic contract for model / effort / permission handling.
    ///
    /// The default implementation derives this from the legacy
    /// [`AgentCapabilities`] bits so current adapters stay source-compatible.
    /// Adapter crates can override this as they migrate to the richer core DTOs.
    fn semantic_capabilities(&self) -> ExecutorSemanticCapabilities {
        self.capabilities().semantic_capabilities()
    }

    /// Spawn a fresh session and return a handle identifying it.
    async fn spawn_session(&self, spec: SpawnSessionSpec)
    -> Result<SessionRef, AgentExecutorError>;

    /// Resume a previously-closed session.
    async fn resume_session(
        &self,
        session_id: NativeSessionId,
        hints: ResumeHints,
    ) -> Result<SessionRef, AgentExecutorError>;

    /// Send a user message and return a stream of events driving the turn.
    async fn send_message(
        &self,
        session: &SessionRef,
        message: UserMessage,
    ) -> Result<EventStream, AgentExecutorError>;

    /// Reply to a pending permission prompt (see `ExecutorEvent::PermissionRequest`).
    async fn respond_to_permission(
        &self,
        session: &SessionRef,
        request_id: String,
        decision: PermissionDecision,
    ) -> Result<(), AgentExecutorError>;

    /// Reply to a pending elicitation prompt (structured user input request).
    async fn respond_to_elicitation(
        &self,
        _session: &SessionRef,
        _request_id: String,
        _response: serde_json::Value,
    ) -> Result<(), AgentExecutorError> {
        Err(AgentExecutorError::Unsupported {
            capability: "respond_to_elicitation".to_string(),
        })
    }

    /// Interrupt the in-flight turn (if any).
    async fn interrupt(&self, session: &SessionRef) -> Result<(), AgentExecutorError>;

    /// Close the session and release transport resources.
    async fn close_session(&self, session: &SessionRef) -> Result<(), AgentExecutorError>;

    /// Change the session's permission mode mid-flight if supported.
    async fn set_permission_mode(
        &self,
        session: &SessionRef,
        mode: PermissionMode,
    ) -> Result<(), AgentExecutorError>;

    /// Attempt to change the session's model.
    ///
    /// Outcome may be `Applied`, `RestartRequired { reason }`, or `Unsupported`
    /// depending on vendor capability.
    async fn set_model(
        &self,
        session: &SessionRef,
        model: ModelSpec,
    ) -> Result<ModelChangeOutcome, AgentExecutorError>;

    /// Register or replace the autonomous-turn stream callback.
    ///
    /// Implementors may ignore handler updates when `autonomous_turn` capability
    /// is `false`. The default implementation rejects the request with
    /// `Unsupported`.
    async fn set_autonomous_turn_handler(
        &self,
        _handler: Option<AutonomousTurnHandler>,
    ) -> Result<(), AgentExecutorError> {
        Err(AgentExecutorError::Unsupported {
            capability: "set_autonomous_turn_handler".to_string(),
        })
    }

    /// Additive normalized model-change seam for future adapter migration.
    ///
    /// The default implementation converts into the legacy [`ModelSpec`] and
    /// delegates to [`AgentExecutor::set_model`].
    async fn set_model_selection(
        &self,
        session: &SessionRef,
        model: NormalizedModelSpec,
    ) -> Result<ModelChangeOutcome, AgentExecutorError> {
        self.set_model(session, model.into()).await
    }

    /// List vendor-native sessions matching a filter (host-side query).
    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, AgentExecutorError>;

    /// Fetch full detail for a specific native session.
    async fn get_session_info(
        &self,
        session_id: &NativeSessionId,
    ) -> Result<SessionInfo, AgentExecutorError>;

    /// Fetch a page of native messages for the given session.
    async fn get_session_messages(
        &self,
        session_id: &NativeSessionId,
        pagination: Pagination,
    ) -> Result<Vec<NativeMessage>, AgentExecutorError>;

    // -----------------------------------------------------------------------
    // Connection-reuse seam (Phase 1 of vendor pre-connection refactor).
    //
    // All four default impls return `Unsupported` or delegate to the existing
    // `spawn_session` path so adapters that have not yet migrated keep
    // compiling. Adapters opt in by:
    //   1. Setting `AgentCapabilities::supports_multi_session_per_process = true`
    //   2. Overriding `open_connection` / `close_connection` / `check_connection`
    //   3. Overriding `start_session_on` to attach a new session to the shared
    //      connection instead of spawning a fresh subprocess.
    // -----------------------------------------------------------------------

    /// Open a reusable vendor connection: spawn the subprocess (or equivalent
    /// transport) and run the global handshake that brings the vendor to a
    /// "ready for any session" state.
    ///
    /// The returned [`ConnectionHandle`] is held by the host registry and
    /// passed to [`AgentExecutor::start_session_on`] for each new session.
    ///
    /// Adapters without connection reuse keep the default `Unsupported`
    /// implementation — callers fall back to [`AgentExecutor::spawn_session`].
    async fn open_connection(
        &self,
        _spec: ConnectionSpec,
    ) -> Result<ConnectionHandle, AgentExecutorError> {
        Err(AgentExecutorError::Unsupported {
            capability: "open_connection".to_string(),
        })
    }

    /// Close a previously-opened connection, killing its transport and
    /// releasing adapter-owned resources. After this returns the
    /// [`ConnectionHandle`] must not be reused.
    async fn close_connection(&self, _handle: ConnectionHandle) -> Result<(), AgentExecutorError> {
        Err(AgentExecutorError::Unsupported {
            capability: "close_connection".to_string(),
        })
    }

    /// Probe a live connection's health by sending the vendor's lightweight
    /// heartbeat (control request / JSON-RPC ping / whatever the protocol
    /// offers). Used by the registry to detect dead transports before
    /// attempting a new session on them.
    async fn check_connection(
        &self,
        _handle: &ConnectionHandle,
    ) -> Result<ConnectionHealth, AgentExecutorError> {
        Err(AgentExecutorError::Unsupported {
            capability: "check_connection".to_string(),
        })
    }

    /// Start a new session on an existing connection.
    ///
    /// Default implementation delegates to [`AgentExecutor::spawn_session`]
    /// so adapters without connection reuse ignore the handle. Adapters that
    /// do support reuse override this to register a new session on the
    /// shared transport and return a [`SessionRef`] whose
    /// `process_handle` points back to the connection.
    async fn start_session_on(
        &self,
        _handle: &ConnectionHandle,
        spec: SpawnSessionSpec,
    ) -> Result<SessionRef, AgentExecutorError> {
        self.spawn_session(spec).await
    }
}
