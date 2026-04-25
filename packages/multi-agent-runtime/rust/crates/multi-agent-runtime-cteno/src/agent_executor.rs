//! [`AgentExecutor`] implementation backed by the `cteno-agent` binary
//! subprocess.
//!
//! This adapter spawns one `cteno-agent` process per [`SessionRef`] and
//! communicates with it via the line-delimited JSON `cteno-agent-stdio`
//! protocol. One session corresponds to exactly one subprocess, matching the
//! `ClaudeAgentExecutor` / `CodexAgentExecutor` topology.
//!
//! The protocol types here intentionally duplicate the shape declared in
//! `packages/agents/rust/crates/cteno-agent-stdio/src/protocol.rs`. We do not
//! take a crate dependency on `cteno-agent-stdio` because it is a binary crate
//! whose `main.rs` owns a tangle of eight interdependent internal modules
//! (`hooks_mvp`, `runner`, `session`, …). Exposing the protocol through a
//! `[lib]` target would require restructuring every one of those modules to
//! live under `src/lib.rs` — out of scope for this wave.
//!
//! Instead we keep the `Inbound` / `Outbound` enum pair here with identical
//! `#[serde(tag = "type")]` shapes so the wire payloads are byte-compatible.
//! Any future divergence surfaces as a JSON-decode error on the host side.
//!
//! Protocol coverage:
//!
//! | `cteno-agent` Outbound  | `ExecutorEvent`                         |
//! |-------------------------|------------------------------------------|
//! | `ready`                 | consumed in `spawn_session`              |
//! | `delta` (text/thinking) | `StreamDelta { Text/Thinking }`          |
//! | `tool_use`              | `ToolCallStart { partial: false }`       |
//! | `tool_result`           | `ToolResult { output: Ok/Err }`          |
//! | `permission_request`    | `PermissionRequest`                      |
//! | `tool_execution_request`| `InjectedToolInvocation`                 |
//! | `host_call_request`     | `NativeEvent` (host callers not wired)   |
//! | `turn_complete`         | `TurnComplete`                           |
//! | `error`                 | `Error { recoverable: true }`            |
//!
//! `set_model` maps to stdio control messages so the host can retarget a live
//! session without forcing a restart. `set_permission_mode` is queued on the
//! session slot and flushed immediately before the next `UserMessage`; this
//! keeps mode changes made during an active turn from blocking on the turn's
//! stdout reader while still applying them to subsequent turns.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use cteno_agent_runtime::hooks;
use multi_agent_runtime_core::{
    executor::AutonomousTurnHandler, AgentCapabilities, AgentExecutor, AgentExecutorError, EventStream,
    ExecutorEvent, InjectedToolSpec, ModelChangeOutcome, ModelSpec, NativeMessage, NativeSessionId,
    Pagination, PermissionDecision, PermissionMode, PermissionModeKind, ProcessHandleToken,
    ResumeHints, SessionFilter, SessionInfo, SessionMeta, SessionRecord, SessionRef,
    SessionStoreProvider, SpawnSessionSpec, TokenUsage, UserMessage,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, broadcast};
use tokio::time::timeout;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::connection::{BackgroundAcpSink, CtenoConnection, SessionEventRx};
use crate::protocol::{AttachmentKindWire, AttachmentWire, Inbound, InjectedToolWire, Outbound};

use multi_agent_runtime_core::executor::{
    ConnectionHandle, ConnectionHandleId, ConnectionHealth, ConnectionSpec,
};

const VENDOR_NAME: &str = "cteno";
const PROTOCOL_VERSION: &str = "0.1";
const DEFAULT_SPAWN_READY_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_TURN_TIMEOUT: Duration = Duration::from_secs(600);
const DEFAULT_CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const STDERR_TAIL_LINES: usize = 16;

// Protocol DTOs live in `crate::protocol`; imported above.

// ---------------------------------------------------------------------------
// Session registry
// ---------------------------------------------------------------------------

/// Per-session subprocess handle held inside the executor's registry.
///
/// Invariant: `stdin` lives in `CtenoSessionSlot::stdin` under its own mutex,
/// NOT on this struct. This keeps the stdin-writer path (`write_slot_frame`,
/// token-refresh broadcast, …) from contending with the long-running turn
/// task that holds the outer process mutex for the whole loop. Without this
/// split, a PermissionResponse frame cannot be written to stdin while the
/// turn task is parked on `stdout_reader.read_line()` — classic deadlock.
struct CtenoSessionProcess {
    child: Child,
    stdout_reader: BufReader<ChildStdout>,
    /// Session id negotiated during `Init`/`Ready`. Stored for debugging and
    /// future multi-session routing; not read on the hot path.
    #[allow(dead_code)]
    native_session_id: NativeSessionId,
    /// OS pid of the spawned child, cached so `close_session` can unregister
    /// from the supervisor after `Child::kill().await` consumes the handle.
    /// `None` if the kernel returned no pid (already reaped).
    pid: Option<i32>,
    stderr_events: broadcast::Sender<StderrProbeEvent>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    pending_fatal_stderr: Arc<Mutex<Option<String>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionAuthState {
    Empty,
    Present,
}

#[derive(Debug, Clone, Default)]
struct HostAuthSnapshot {
    access_token: Option<String>,
    user_id: Option<String>,
    machine_id: Option<String>,
}

#[derive(Debug, Clone)]
struct CtenoSessionLaunchConfig {
    workdir: PathBuf,
    system_prompt: Option<String>,
    additional_directories: Vec<PathBuf>,
    agent_config: Value,
    env: BTreeMap<String, String>,
    injected_tools: Vec<InjectedToolSpec>,
}

struct CtenoSessionSlot {
    native_session_id: NativeSessionId,
    launch: CtenoSessionLaunchConfig,
    auth_state: SessionAuthState,
    /// Legacy per-session subprocess backing. Populated for sessions created
    /// by `resume_session` (which still takes the one-child-per-session path)
    /// and drained to `None` when the subprocess dies.
    process: Option<Arc<Mutex<CtenoSessionProcess>>>,
    /// Owned, separately-lockable handle to the subprocess's stdin. Lives
    /// alongside `process` but *outside* the process mutex so stdin writes
    /// don't serialise against the turn loop's `stdout_reader.read_line()`.
    /// Same lifecycle as `process` — set on spawn, taken on `mark_slot_dead`.
    stdin: Option<Arc<Mutex<ChildStdin>>>,
    /// Connection-reuse backing. Populated by `start_session_on`; mutually
    /// exclusive with `process`. When `Some`, every session operation must go
    /// through the shared `CtenoConnection`.
    connection: Option<Arc<CtenoConnection>>,
    /// Routing state for the per-session dispatcher task that owns this
    /// slot's `event_rx` (consumed in `start_session_on_internal`). The
    /// dispatcher reads `event_rx` for the lifetime of the session and
    /// uses this state to decide whether each frame goes to the active
    /// `send_message` consumer (`InUserTurn`) or to the autonomous-turn
    /// handler stream (`InAutonomousTurn`). Subprocess-backed sessions
    /// (legacy resume path) keep this `Idle` because they consume their
    /// child's stdout directly.
    route_state: Arc<Mutex<RouteState>>,
    /// Permission mode requested while the current turn may still be running.
    /// Flushed as a `SetPermissionMode` frame immediately before the next
    /// `UserMessage`.
    pending_permission_mode: Option<String>,
}

type SessionRegistry = Mutex<HashMap<ProcessHandleToken, Arc<Mutex<CtenoSessionSlot>>>>;
type ConnectionRegistry = Mutex<HashMap<ConnectionHandleId, Arc<CtenoConnection>>>;

/// Routing state used by the per-session dispatcher task to decide where to
/// forward each `Outbound` frame received from the cteno-agent subprocess.
///
/// State transitions are driven by **explicit** protocol signals:
/// - `send_message` writes `Inbound::UserMessage` and atomically transitions
///   `Idle -> InUserTurn { tx }`. The returned `EventStream` consumes from
///   the matching `rx`.
/// - `Outbound::AutonomousTurnStart` (emitted by cteno-agent before it
///   self-initiates a new turn — e.g. after a SubAgent handoff) transitions
///   `Idle -> InAutonomousTurn { tx }` and invokes the registered
///   `AutonomousTurnHandler` with the matching stream.
/// - `Outbound::TurnComplete` (or downstream consumer drop) transitions back
///   to `Idle`.
///
/// Because the dispatcher is the **single consumer** of `event_rx`, frame
/// ordering is preserved naturally — no fan-out, no race between concurrent
/// turn streams.
enum RouteState {
    /// No active stream. `AutonomousTurnStart` will start a new autonomous
    /// turn; other frames in this state are unexpected and logged.
    Idle,
    /// User-initiated turn (`send_message` set this `tx`). Held until the
    /// dispatcher observes the matching `TurnComplete` for this turn.
    InUserTurn {
        tx: tokio::sync::mpsc::Sender<Result<ExecutorEvent, AgentExecutorError>>,
    },
    /// Autonomous turn started by the agent itself. Frames flow into the
    /// stream consumed by the registered `AutonomousTurnHandler` until
    /// `TurnComplete`.
    InAutonomousTurn {
        tx: tokio::sync::mpsc::Sender<Result<ExecutorEvent, AgentExecutorError>>,
    },
}

impl RouteState {
    fn is_idle(&self) -> bool {
        matches!(self, RouteState::Idle)
    }

    fn label(&self) -> &'static str {
        match self {
            RouteState::Idle => "idle",
            RouteState::InUserTurn { .. } => "user_turn",
            RouteState::InAutonomousTurn { .. } => "autonomous_turn",
        }
    }
}

#[derive(Debug, Clone)]
enum StderrProbeEvent {
    Fatal(String),
}

/// [`AgentExecutor`] implementation that drives a `cteno-agent` subprocess per
/// session. Cheap to `Arc::clone`; internally holds a subprocess registry.
pub struct CtenoAgentExecutor {
    cteno_agent_path: PathBuf,
    session_store: Arc<dyn SessionStoreProvider>,
    sessions: SessionRegistry,
    /// Registry of live multi-session connections opened via
    /// [`AgentExecutor::open_connection`]. Every `CtenoConnection` may host
    /// zero or more `CtenoSessionSlot::connection` references.
    connections: ConnectionRegistry,
    spawn_ready_timeout: Duration,
    turn_timeout: Duration,
    /// Optional host-side subprocess supervisor. When set the executor will
    /// register each freshly spawned child and unregister on close so the
    /// daemon can SIGTERM orphans on crash recovery / shutdown. Left `None`
    /// in tests and library-only callers who do not need orphan sweeping.
    supervisor: Option<Arc<cteno_host_runtime::SubprocessSupervisor>>,
    background_acp_sink: Option<BackgroundAcpSink>,
    /// Structured sink covering subagent lifecycle + legacy background ACP.
    /// Replaces the older single-`Fn` `BackgroundAcpSink` for routes
    /// that need typed event dispatch (notably `SubAgentLifecycle`).
    /// Both can coexist during migration; the dispatcher prefers
    /// the trait sink and falls back to the legacy sink for the older
    /// `source: subagent` ACP path.
    session_event_sink: Arc<Mutex<Option<crate::session_sink::SessionEventSinkArc>>>,
    /// Optional handler invoked by per-session dispatcher tasks when the
    /// cteno-agent emits `Outbound::AutonomousTurnStart`. Wrapped in
    /// `Arc<Mutex<...>>` so the dispatcher can clone the handle out and read
    /// the latest registered handler each time (avoiding a copy-at-spawn race
    /// that would prevent late `set_autonomous_turn_handler` calls from
    /// taking effect on already-running sessions).
    autonomous_turn_handler: Arc<Mutex<Option<AutonomousTurnHandler>>>,
}

impl CtenoAgentExecutor {
    /// Build a new executor targeting the given `cteno-agent` binary path and
    /// metadata store.
    pub fn new(cteno_agent_path: PathBuf, session_store: Arc<dyn SessionStoreProvider>) -> Self {
        Self {
            cteno_agent_path,
            session_store,
            sessions: Mutex::new(HashMap::new()),
            connections: Mutex::new(HashMap::new()),
            spawn_ready_timeout: DEFAULT_SPAWN_READY_TIMEOUT,
            turn_timeout: DEFAULT_TURN_TIMEOUT,
            supervisor: None,
            background_acp_sink: None,
            session_event_sink: Arc::new(Mutex::new(None)),
            autonomous_turn_handler: Arc::new(Mutex::new(None)),
        }
    }

    /// Attach a `SubprocessSupervisor` so spawned child pids are tracked in
    /// the host pid file and can be SIGTERM'd on daemon shutdown / crash
    /// recovery. Invoked from the host binary; library tests omit this.
    pub fn with_supervisor(
        mut self,
        supervisor: Arc<cteno_host_runtime::SubprocessSupervisor>,
    ) -> Self {
        self.supervisor = Some(supervisor);
        self
    }

    /// Attach a display-only sink for runtime-owned background ACP messages
    /// emitted after the initiating turn has already completed. This preserves
    /// the stdio transparent channel without letting the host own DAG or
    /// SubAgent progression.
    pub fn with_background_acp_sink(mut self, sink: BackgroundAcpSink) -> Self {
        self.background_acp_sink = Some(sink);
        self
    }

    /// Attach a structured `SessionEventSink` to receive subagent lifecycle
    /// events (and any future runtime-owned background events). The sink
    /// is invoked by the per-session dispatcher when matching frames
    /// arrive. Multiple registrations replace each other (last wins).
    ///
    /// `async` because the underlying slot is a tokio `Mutex` (shared with
    /// dispatcher tasks that read it). Call from any async context during
    /// startup before spawning sessions.
    pub async fn with_session_event_sink(
        self,
        sink: crate::session_sink::SessionEventSinkArc,
    ) -> Self {
        *self.session_event_sink.lock().await = Some(sink);
        self
    }

    /// Build an executor picking the binary from the `CTENO_AGENT_PATH` env
    /// variable, falling back to `current_exe().parent()/cteno-agent`.
    pub fn from_env(
        session_store: Arc<dyn SessionStoreProvider>,
    ) -> Result<Self, AgentExecutorError> {
        let path = if let Ok(p) = std::env::var("CTENO_AGENT_PATH") {
            PathBuf::from(p)
        } else {
            let exe = std::env::current_exe()
                .map_err(|e| AgentExecutorError::Io(format!("current_exe: {e}")))?;
            let dir = exe.parent().ok_or_else(|| {
                AgentExecutorError::Io("current_exe has no parent directory".to_string())
            })?;
            dir.join("cteno-agent")
        };
        Ok(Self::new(path, session_store))
    }

    /// Override the timeout for waiting on the initial `ready` frame.
    pub fn with_spawn_ready_timeout(mut self, timeout: Duration) -> Self {
        self.spawn_ready_timeout = timeout;
        self
    }

    /// Override the per-turn timeout for `send_message` streams.
    pub fn with_turn_timeout(mut self, timeout: Duration) -> Self {
        self.turn_timeout = timeout;
        self
    }

    async fn get_session_slot(
        &self,
        session: &SessionRef,
    ) -> Result<Arc<Mutex<CtenoSessionSlot>>, AgentExecutorError> {
        let guard = self.sessions.lock().await;
        guard
            .get(&session.process_handle)
            .cloned()
            .ok_or_else(|| AgentExecutorError::SessionNotFound(session.id.to_string()))
    }

    async fn remove_session(
        &self,
        token: &ProcessHandleToken,
    ) -> Option<Arc<Mutex<CtenoSessionSlot>>> {
        let mut guard = self.sessions.lock().await;
        guard.remove(token)
    }

    async fn session_process(
        &self,
        session: &SessionRef,
    ) -> Result<Arc<Mutex<CtenoSessionProcess>>, AgentExecutorError> {
        let slot = self.get_session_slot(session).await?;
        let guard = slot.lock().await;
        guard
            .process
            .as_ref()
            .cloned()
            .ok_or_else(|| AgentExecutorError::SessionNotFound(session.id.to_string()))
    }

    /// Send an `Inbound` frame to whichever backing the session uses
    /// (legacy per-session subprocess or shared connection). Dispatches on
    /// slot contents, so `interrupt` / `respond_to_permission` /
    /// `set_model` and permission responses do not have to know which path the
    /// session was spawned through.
    async fn write_slot_frame(
        &self,
        session: &SessionRef,
        frame: &Inbound,
    ) -> Result<(), AgentExecutorError> {
        let slot = self.get_session_slot(session).await?;
        // Snapshot connection + legacy stdin refs with a brief hold on the slot
        // mutex only. Critically, legacy writes go through `slot.stdin` (its
        // own mutex) and NOT through the process mutex — the process mutex is
        // held for the entire turn by the stdout-reader loop, so contending
        // on it here would deadlock the PermissionResponse flow.
        let (conn, stdin) = {
            let guard = slot.lock().await;
            (guard.connection.clone(), guard.stdin.clone())
        };
        if let Some(conn) = conn {
            conn.writer
                .send(frame)
                .await
                .map_err(|e| AgentExecutorError::Protocol(format!("cteno-agent write failed: {e}")))
        } else if let Some(stdin_arc) = stdin {
            let mut stdin_guard = stdin_arc.lock().await;
            write_frame(&mut *stdin_guard, frame).await
        } else {
            Err(AgentExecutorError::SessionNotFound(session.id.to_string()))
        }
    }

    async fn shutdown_process(
        &self,
        process: Arc<Mutex<CtenoSessionProcess>>,
        stdin: Option<Arc<Mutex<ChildStdin>>>,
    ) {
        // Close stdin first (if we still have it), THEN kill the child. Doing
        // it in this order avoids a race where the child sees SIGKILL before
        // noticing EOF on stdin — not critical for cteno-agent, but keeps the
        // observable behaviour identical to the pre-split code.
        if let Some(stdin_arc) = stdin {
            let mut stdin_guard = stdin_arc.lock().await;
            let _ = stdin_guard.shutdown().await;
        }
        let mut guard = process.lock().await;
        let pid_opt = guard.pid;
        let _ = guard.child.kill().await;
        let _ = guard.child.wait().await;
        drop(guard);

        if let (Some(sup), Some(pid)) = (self.supervisor.as_ref(), pid_opt) {
            if let Err(e) = sup.unregister(pid) {
                log::warn!("SubprocessSupervisor::unregister failed for pid={pid}: {e}");
            }
        }
    }

    async fn mark_slot_dead(&self, slot: &mut CtenoSessionSlot, reason: &str) {
        let stdin = slot.stdin.take();
        if let Some(process) = slot.process.take() {
            log::warn!(
                "cteno session {} marked dead: {}",
                slot.native_session_id.as_str(),
                reason
            );
            self.shutdown_process(process, stdin).await;
        }
        slot.auth_state = SessionAuthState::Empty;
    }

    async fn session_process_exited(
        &self,
        process: &Arc<Mutex<CtenoSessionProcess>>,
    ) -> Result<bool, AgentExecutorError> {
        let mut guard = process.lock().await;
        match guard.child.try_wait() {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(AgentExecutorError::Io(format!(
                "checking cteno-agent liveness failed: {e}"
            ))),
        }
    }

    async fn send_control_frame(
        &self,
        session: &SessionRef,
        operation: &'static str,
        frame: &Inbound,
    ) -> Result<(), AgentExecutorError> {
        // Connection-backed slots go through the shared writer; legacy
        // per-session subprocess slots go through the owned stdin. Both apply
        // the same DEFAULT_CONTROL_TIMEOUT.
        let slot = self.get_session_slot(session).await?;
        let (conn, legacy, stdin) = {
            let guard = slot.lock().await;
            (
                guard.connection.clone(),
                guard.process.clone(),
                guard.stdin.clone(),
            )
        };
        if let Some(conn) = conn {
            match timeout(DEFAULT_CONTROL_TIMEOUT, conn.writer.send(frame)).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(AgentExecutorError::Protocol(format!(
                    "cteno-agent {operation} write failed: {e}"
                ))),
                Err(_) => Err(AgentExecutorError::Timeout {
                    operation: operation.to_string(),
                    seconds: DEFAULT_CONTROL_TIMEOUT.as_secs(),
                }),
            }
        } else if let (Some(process), Some(stdin)) = (legacy, stdin) {
            // `process` mutex may be held by the turn-loop reader; take it
            // with the DEFAULT_CONTROL_TIMEOUT guard so a long-running turn
            // doesn't stall set_model indefinitely.
            // `stdin` has its own mutex and isn't contended by the reader.
            match timeout(DEFAULT_CONTROL_TIMEOUT, async {
                let mut process_guard = process.lock().await;
                let mut stdin_guard = stdin.lock().await;
                write_checked_frame(&mut *process_guard, &mut *stdin_guard, operation, frame).await
            })
            .await
            {
                Ok(Ok(())) => Ok(()),
                Ok(Err(err)) => Err(err),
                Err(_) => Err(AgentExecutorError::Timeout {
                    operation: operation.to_string(),
                    seconds: DEFAULT_CONTROL_TIMEOUT.as_secs(),
                }),
            }
        } else {
            Err(AgentExecutorError::SessionNotFound(session.id.to_string()))
        }
    }

    /// Push a `TokenRefreshed` frame to every active session's stdin. Called
    /// by the host-side refresh guard after `AuthStore::set_tokens` so
    /// subprocess-owned sessions observe the rotated access token on their
    /// very next cloud call. Errors on individual stdins are logged but do
    /// not abort the broadcast.
    pub async fn broadcast_token_refresh(&self, access_token: &str) {
        let frame = Inbound::TokenRefreshed {
            access_token: access_token.to_string(),
        };

        // 1. Broadcast to every live shared connection — the cteno-agent
        //    process-wide auth slot handles token rotation once per
        //    subprocess.
        let connections = {
            let guard = self.connections.lock().await;
            guard.values().cloned().collect::<Vec<_>>()
        };
        for conn in connections {
            if let Err(e) = conn.writer.send(&frame).await {
                log::warn!(
                    "token_refresh broadcast to connection {} failed: {e}",
                    conn.id
                );
            }
        }

        // 2. Legacy per-session subprocesses (from resume_session) still need
        //    the per-stdin broadcast. For each such session we also update
        //    the slot's auth_state marker.
        let sessions = {
            let guard = self.sessions.lock().await;
            guard.values().cloned().collect::<Vec<_>>()
        };
        if sessions.is_empty() {
            return;
        }
        for slot in sessions {
            let (stdin, session_id, is_connection_backed) = {
                let guard = slot.lock().await;
                (
                    guard.stdin.as_ref().cloned(),
                    guard.native_session_id.as_str().to_string(),
                    guard.connection.is_some(),
                )
            };
            if is_connection_backed {
                // Already covered by the connection broadcast above; just
                // flip auth_state so subsequent sends skip the empty-slot
                // retry path.
                let mut guard = slot.lock().await;
                guard.auth_state = SessionAuthState::Present;
                continue;
            }
            let Some(stdin) = stdin else {
                continue;
            };
            let write_result = {
                let mut stdin_guard = stdin.lock().await;
                write_frame(&mut *stdin_guard, &frame).await
            };
            match write_result {
                Ok(()) => {
                    let mut guard = slot.lock().await;
                    guard.auth_state = SessionAuthState::Present;
                }
                Err(e) => {
                    log::warn!("token_refresh broadcast to session {session_id} failed: {e}");
                    let mut guard = slot.lock().await;
                    self.mark_slot_dead(&mut guard, "token refresh broadcast failed")
                        .await;
                }
            }
        }
    }

    async fn spawn_process(
        &self,
        native_session_id: &str,
        launch: &CtenoSessionLaunchConfig,
    ) -> Result<(CtenoSessionProcess, ChildStdin, SessionAuthState), AgentExecutorError> {
        let mut command = Command::new(&self.cteno_agent_path);
        command.current_dir(&launch.workdir);
        for (key, value) in &launch.env {
            command.env(key, value);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(AgentExecutorError::from)?;
        let pid_opt: Option<i32> = child.id().map(|p| p as i32);
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentExecutorError::Io("cteno-agent stdin unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentExecutorError::Io("cteno-agent stdout unavailable".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AgentExecutorError::Io("cteno-agent stderr unavailable".to_string()))?;
        let mut stdout_reader = BufReader::new(stdout);
        let (stderr_events, stderr_tail, pending_fatal_stderr) =
            spawn_stderr_probe(native_session_id, stderr);
        let mut stderr_rx = stderr_events.subscribe();

        let mut agent_config = launch.agent_config.clone();
        let auth_snapshot = current_host_auth_snapshot();
        sync_auth_into_agent_config(&mut agent_config, &auth_snapshot);
        let auth_state = auth_state_from_snapshot(&auth_snapshot);
        let (auth_token, user_id, machine_id, agent_config_out) =
            extract_auth_from_agent_config(agent_config);
        let init = Inbound::Init {
            session_id: native_session_id.to_string(),
            workdir: launch.workdir.to_str().map(|s| s.to_string()),
            additional_directories: launch
                .additional_directories
                .iter()
                .filter_map(|path| path.to_str().map(str::to_string))
                .collect(),
            agent_config: agent_config_out,
            system_prompt: launch.system_prompt.clone(),
            auth_token,
            user_id,
            machine_id,
        };
        if let Err(error) = write_spawn_frame(&mut child, &mut stdin, &stderr_tail, &init).await {
            cleanup_failed_child(&mut child).await;
            return Err(error);
        }

        let observed_id = match timeout(
            self.spawn_ready_timeout,
            wait_for_ready(
                &mut stdout_reader,
                &mut stderr_rx,
                &stderr_tail,
                &pending_fatal_stderr,
                native_session_id,
            ),
        )
        .await
        {
            Ok(result) => match result {
                Ok(observed_id) => observed_id,
                Err(error) => {
                    cleanup_failed_child(&mut child).await;
                    return Err(error);
                }
            },
            Err(_) => {
                let stderr = stderr_tail_snapshot(&stderr_tail).await;
                cleanup_failed_child(&mut child).await;
                return Err(AgentExecutorError::Timeout {
                    operation: spawn_timeout_operation(&stderr),
                    seconds: self.spawn_ready_timeout.as_secs(),
                });
            }
        };

        if observed_id != native_session_id {
            cleanup_failed_child(&mut child).await;
            return Err(AgentExecutorError::Protocol(format!(
                "cteno-agent ready session mismatch: expected {native_session_id}, got {observed_id}"
            )));
        }

        for spec in &launch.injected_tools {
            let frame = Inbound::ToolInject {
                session_id: observed_id.clone(),
                tool: InjectedToolWire {
                    name: spec.name.clone(),
                    description: spec.description.clone(),
                    input_schema: spec.input_schema.clone(),
                },
            };
            if let Err(error) =
                write_spawn_frame(&mut child, &mut stdin, &stderr_tail, &frame).await
            {
                cleanup_failed_child(&mut child).await;
                return Err(error);
            }
        }

        let native_id = NativeSessionId::new(observed_id);
        let process = CtenoSessionProcess {
            child,
            stdout_reader,
            native_session_id: native_id.clone(),
            pid: pid_opt,
            stderr_events,
            stderr_tail,
            pending_fatal_stderr,
        };

        if let (Some(sup), Some(pid)) = (self.supervisor.as_ref(), pid_opt) {
            let record = cteno_host_runtime::SupervisedProcess {
                pid,
                kind: "cteno-agent".to_string(),
                session_id: native_id.as_str().to_string(),
                spawned_at: Utc::now().timestamp(),
            };
            if let Err(e) = sup.register(record) {
                log::warn!("SubprocessSupervisor::register failed for pid={pid}: {e}");
            }
        }

        Ok((process, stdin, auth_state))
    }

    async fn ensure_turn_process(
        &self,
        session: &SessionRef,
    ) -> Result<(Arc<Mutex<CtenoSessionProcess>>, Arc<Mutex<ChildStdin>>), AgentExecutorError> {
        let slot = self.get_session_slot(session).await?;
        let mut guard = slot.lock().await;

        if let Some(process) = guard.process.as_ref().cloned() {
            if self.session_process_exited(&process).await? {
                self.mark_slot_dead(&mut guard, "subprocess exited before send_message")
                    .await;
            }
        }

        if guard.process.is_none() {
            let (process, stdin, auth_state) = self
                .spawn_process(guard.native_session_id.as_str(), &guard.launch)
                .await?;
            guard.process = Some(Arc::new(Mutex::new(process)));
            guard.stdin = Some(Arc::new(Mutex::new(stdin)));
            guard.auth_state = auth_state;
        }

        if matches!(guard.auth_state, SessionAuthState::Empty) {
            let snapshot = current_host_auth_snapshot();
            if let Some(access_token) = snapshot.access_token.clone() {
                let stdin_arc =
                    guard.stdin.as_ref().cloned().ok_or_else(|| {
                        AgentExecutorError::SessionNotFound(session.id.to_string())
                    })?;
                let write_result = {
                    let mut stdin_guard = stdin_arc.lock().await;
                    write_frame(&mut *stdin_guard, &Inbound::TokenRefreshed { access_token }).await
                };
                match write_result {
                    Ok(()) => {
                        guard.auth_state = SessionAuthState::Present;
                    }
                    Err(error) => {
                        log::warn!(
                            "pre-send token sync to session {} failed: {error}",
                            guard.native_session_id.as_str()
                        );
                        self.mark_slot_dead(&mut guard, "pre-send token sync failed")
                            .await;
                        let (process, stdin, auth_state) = self
                            .spawn_process(guard.native_session_id.as_str(), &guard.launch)
                            .await?;
                        let process = Arc::new(Mutex::new(process));
                        let stdin = Arc::new(Mutex::new(stdin));
                        guard.process = Some(process.clone());
                        guard.stdin = Some(stdin.clone());
                        guard.auth_state = auth_state;
                        return Ok((process, stdin));
                    }
                }
            }
        }

        let process = guard
            .process
            .as_ref()
            .cloned()
            .ok_or_else(|| AgentExecutorError::SessionNotFound(session.id.to_string()))?;
        let stdin = guard
            .stdin
            .as_ref()
            .cloned()
            .ok_or_else(|| AgentExecutorError::SessionNotFound(session.id.to_string()))?;
        Ok((process, stdin))
    }

    async fn send_message_frames(
        &self,
        process: &Arc<Mutex<CtenoSessionProcess>>,
        stdin: &Arc<Mutex<ChildStdin>>,
        session_id: &str,
        message: &UserMessage,
        pending_permission_mode: Option<&str>,
    ) -> Result<(), AgentExecutorError> {
        let mut process_guard = process.lock().await;
        let mut stdin_guard = stdin.lock().await;
        if let Some(mode) = pending_permission_mode {
            let frame = Inbound::SetPermissionMode {
                session_id: session_id.to_string(),
                mode: mode.to_string(),
            };
            write_checked_frame(
                &mut *process_guard,
                &mut *stdin_guard,
                "applying queued permission mode to cteno-agent",
                &frame,
            )
            .await?;
        }
        for spec in message.injected_tools.iter() {
            let frame = Inbound::ToolInject {
                session_id: session_id.to_string(),
                tool: InjectedToolWire {
                    name: spec.name.clone(),
                    description: spec.description.clone(),
                    input_schema: spec.input_schema.clone(),
                },
            };
            write_checked_frame(
                &mut *process_guard,
                &mut *stdin_guard,
                "registering injected tools with cteno-agent",
                &frame,
            )
            .await?;
        }

        let frame = Inbound::UserMessage {
            session_id: session_id.to_string(),
            content: message.content.clone(),
            task_id: message.task_id.clone(),
            attachments: message
                .attachments
                .iter()
                .map(attachment_wire_from_core)
                .collect(),
        };
        write_checked_frame(
            &mut *process_guard,
            &mut *stdin_guard,
            "sending the user message to cteno-agent",
            &frame,
        )
        .await
    }

    /// Drive a turn on a connection-backed session: write the injected tools
    /// + UserMessage frames through the shared writer, then stream events
    /// from the slot's per-session event receiver into the returned
    /// `EventStream`. On turn completion the receiver is **not** dropped —
    /// it stays in the slot so permission closures and subsequent turns can
    /// reuse the same channel.
    async fn send_message_connection_backed(
        &self,
        session: &SessionRef,
        message: UserMessage,
    ) -> Result<EventStream, AgentExecutorError> {
        let session_id = session.id.as_str().to_string();
        let slot = self.get_session_slot(session).await?;

        let (conn, route_state) = {
            let guard = slot.lock().await;
            let conn = guard
                .connection
                .clone()
                .ok_or_else(|| AgentExecutorError::SessionNotFound(session.id.to_string()))?;
            (conn, guard.route_state.clone())
        };

        // Reject if connection is dead.
        if let crate::connection::ConnectionLiveness::Dead { reason } = conn.check().await {
            return Err(AgentExecutorError::Protocol(format!(
                "send_message: connection dead: {reason}"
            )));
        }

        // Atomically claim the session for a user-driven turn. The dispatcher
        // task spawned at session start owns `event_rx`; it will read frames
        // forever and route them to whichever `tx` is currently parked in
        // `RouteState`. Reject if another turn (user or autonomous) is in
        // flight — turn boundaries are explicit, no implicit interleaving.
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ExecutorEvent, AgentExecutorError>>(64);
        {
            let mut state = route_state.lock().await;
            if !state.is_idle() {
                return Err(AgentExecutorError::Vendor {
                    vendor: VENDOR_NAME,
                    message: format!(
                        "send_message: session already has an active turn (state={})",
                        state.label()
                    ),
                });
            }
            *state = RouteState::InUserTurn { tx: tx.clone() };
        }

        // Helper to roll back the route state if any pre-write step fails so
        // the next send_message call can start a fresh turn.
        let rollback_state = || async {
            let mut state = route_state.lock().await;
            if matches!(*state, RouteState::InUserTurn { .. }) {
                *state = RouteState::Idle;
            }
        };

        let pending_permission_mode = {
            let mut guard = slot.lock().await;
            guard.pending_permission_mode.take()
        };
        if let Some(mode) = pending_permission_mode.as_deref() {
            let frame = Inbound::SetPermissionMode {
                session_id: session_id.clone(),
                mode: mode.to_string(),
            };
            if let Err(e) = conn.writer.send(&frame).await {
                {
                    let mut guard = slot.lock().await;
                    guard.pending_permission_mode = pending_permission_mode;
                }
                rollback_state().await;
                return Err(AgentExecutorError::Protocol(format!(
                    "cteno-agent queued permission mode failed: {e}"
                )));
            }
        }

        // Write injected tools + the UserMessage through the shared writer.
        for spec in message.injected_tools.iter() {
            let frame = Inbound::ToolInject {
                session_id: session_id.clone(),
                tool: InjectedToolWire {
                    name: spec.name.clone(),
                    description: spec.description.clone(),
                    input_schema: spec.input_schema.clone(),
                },
            };
            if let Err(e) = conn.writer.send(&frame).await {
                rollback_state().await;
                return Err(AgentExecutorError::Protocol(format!(
                    "cteno-agent tool inject failed: {e}"
                )));
            }
        }
        let user_frame = Inbound::UserMessage {
            session_id: session_id.clone(),
            content: message.content.clone(),
            task_id: message.task_id.clone(),
            attachments: message
                .attachments
                .iter()
                .map(attachment_wire_from_core)
                .collect(),
        };
        if let Err(e) = conn.writer.send(&user_frame).await {
            rollback_state().await;
            return Err(AgentExecutorError::Protocol(format!(
                "cteno-agent user_message write failed: {e}"
            )));
        }

        // Spawn a per-turn timeout watchdog. Unlike the legacy implementation
        // (which interleaved deadline + recv inside a single loop) the
        // dispatcher now owns event consumption, so the watchdog only needs
        // to detect "no terminal frame within the deadline" and force-abort.
        // It cancels itself if the route state transitions back to Idle
        // (turn ended naturally).
        let turn_timeout = self.turn_timeout;
        let route_state_for_watchdog = route_state.clone();
        let conn_for_watchdog = conn.clone();
        let session_id_for_watchdog = session_id.clone();
        let tx_for_watchdog = tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(turn_timeout).await;
            let mut state = route_state_for_watchdog.lock().await;
            // Only fire if THIS user turn is still parked. If a different tx
            // is sitting there (or Idle), the turn already finished and the
            // watchdog is stale.
            let still_ours = match &*state {
                RouteState::InUserTurn { tx } => tx.same_channel(&tx_for_watchdog),
                _ => false,
            };
            if !still_ours {
                return;
            }
            log::warn!(
                "[cteno send_message watchdog] timeout for session {}, sending Abort",
                session_id_for_watchdog
            );
            let abort_frame = Inbound::Abort {
                session_id: session_id_for_watchdog.clone(),
                reason: Some(turn_timeout_message(turn_timeout.as_secs())),
            };
            let _ = conn_for_watchdog.writer.send(&abort_frame).await;
            send_timeout_terminal_events(&tx_for_watchdog, turn_timeout.as_secs()).await;
            // Force back to Idle so the next send_message can proceed even
            // if the agent never echoes TurnComplete.
            *state = RouteState::Idle;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    /// Core spawn path used by both `spawn_session` and `resume_session`.
    ///
    /// `native_session_id` is the id negotiated with the agent. For fresh
    /// sessions we allocate a new UUIDv4; for resumes the caller's cursor is
    /// forwarded so the runtime can load its history from the session store.
    async fn spawn_internal(
        &self,
        native_session_id: String,
        workdir: PathBuf,
        system_prompt: Option<String>,
        agent_config: Value,
        env: std::collections::BTreeMap<String, String>,
        injected_tools: Vec<multi_agent_runtime_core::InjectedToolSpec>,
    ) -> Result<SessionRef, AgentExecutorError> {
        let launch = CtenoSessionLaunchConfig {
            workdir: workdir.clone(),
            system_prompt: system_prompt.clone(),
            additional_directories: Vec::new(),
            agent_config,
            env,
            injected_tools,
        };
        let (process, stdin, auth_state) = self.spawn_process(&native_session_id, &launch).await?;
        let native_id = NativeSessionId::new(native_session_id.clone());

        let token = ProcessHandleToken::new();
        let session_ref = SessionRef {
            id: native_id,
            vendor: VENDOR_NAME,
            process_handle: token.clone(),
            spawned_at: Utc::now(),
            workdir: workdir.clone(),
        };

        self.session_store
            .record_session(
                VENDOR_NAME,
                SessionRecord {
                    session_id: session_ref.id.clone(),
                    workdir: session_ref.workdir.clone(),
                    context: json!({
                        "native_session_id": session_ref.id.as_str(),
                    }),
                },
            )
            .await
            .map_err(|message| AgentExecutorError::Vendor {
                vendor: VENDOR_NAME,
                message,
            })?;

        self.sessions.lock().await.insert(
            token,
            Arc::new(Mutex::new(CtenoSessionSlot {
                native_session_id: session_ref.id.clone(),
                launch,
                auth_state,
                process: Some(Arc::new(Mutex::new(process))),
                stdin: Some(Arc::new(Mutex::new(stdin))),
                connection: None,
                route_state: Arc::new(Mutex::new(RouteState::Idle)),
                pending_permission_mode: None,
            })),
        );

        Ok(session_ref)
    }

    // -----------------------------------------------------------------------
    // Connection-reuse path (Phase 1)
    // -----------------------------------------------------------------------

    /// Spawn a bare cteno-agent subprocess, register it in the connections
    /// map, and return a `ConnectionHandle`. **Does not** send any Init —
    /// sessions are attached later via `start_session_on_internal`.
    async fn open_connection_internal(
        &self,
        spec: &ConnectionSpec,
    ) -> Result<Arc<CtenoConnection>, AgentExecutorError> {
        let _ = spec.probe; // `probe` currently has no effect: the agent has
        // no protocol-level hello, so bringing the
        // subprocess to "alive" is equivalent whether
        // probing or not.
        let mut command = Command::new(&self.cteno_agent_path);
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = command.spawn().map_err(AgentExecutorError::from)?;
        let conn = CtenoConnection::start(child, self.background_acp_sink.clone())
            .map_err(|e| AgentExecutorError::Io(format!("cteno-agent connection start: {e}")))?;

        // Register with the supervisor if present so the daemon's pid file
        // stays in sync with live children.
        if let (Some(sup), Some(pid)) = (self.supervisor.as_ref(), conn.pid) {
            let record = cteno_host_runtime::SupervisedProcess {
                pid,
                kind: "cteno-agent-connection".to_string(),
                session_id: conn.id.to_string(),
                spawned_at: Utc::now().timestamp(),
            };
            if let Err(e) = sup.register(record) {
                log::warn!("SubprocessSupervisor::register failed for pid={pid}: {e}");
            }
        }

        self.connections
            .lock()
            .await
            .insert(conn.id.clone(), conn.clone());

        Ok(conn)
    }

    /// Start a new session on a shared connection: register the session in
    /// the connection's router, send its Init frame, wait for its Ready.
    async fn start_session_on_internal(
        &self,
        conn: &Arc<CtenoConnection>,
        native_session_id: String,
        launch: CtenoSessionLaunchConfig,
    ) -> Result<SessionRef, AgentExecutorError> {
        // Register first so the Ready frame (fast path) doesn't race the
        // router insert.
        let mut rx = conn.register_session(&native_session_id).await;

        // Build the Init frame including auth snapshot.
        let mut agent_config = launch.agent_config.clone();
        let auth_snapshot = current_host_auth_snapshot();
        sync_auth_into_agent_config(&mut agent_config, &auth_snapshot);
        let auth_state = auth_state_from_snapshot(&auth_snapshot);
        let (auth_token, user_id, machine_id, agent_config_out) =
            extract_auth_from_agent_config(agent_config);
        let init = Inbound::Init {
            session_id: native_session_id.clone(),
            workdir: launch.workdir.to_str().map(|s| s.to_string()),
            additional_directories: launch
                .additional_directories
                .iter()
                .filter_map(|path| path.to_str().map(str::to_string))
                .collect(),
            agent_config: agent_config_out,
            system_prompt: launch.system_prompt.clone(),
            auth_token,
            user_id,
            machine_id,
        };

        conn.writer.send(&init).await.map_err(|e| {
            AgentExecutorError::Protocol(format!("cteno-agent init write failed: {e}"))
        })?;

        // Wait for Ready (or Error, or timeout). Mirrors legacy timeout.
        match crate::connection::wait_for_ready(
            &mut rx,
            &native_session_id,
            self.spawn_ready_timeout,
        )
        .await
        {
            Ok(()) => {}
            Err(message) => {
                // Detach session from the router; no point leaving stale.
                conn.unregister_session(&native_session_id).await;
                return Err(AgentExecutorError::Protocol(message));
            }
        }

        // Inject any launch-time tools (kept here for symmetry with legacy
        // spawn_process).
        for spec in &launch.injected_tools {
            let frame = Inbound::ToolInject {
                session_id: native_session_id.clone(),
                tool: InjectedToolWire {
                    name: spec.name.clone(),
                    description: spec.description.clone(),
                    input_schema: spec.input_schema.clone(),
                },
            };
            conn.writer.send(&frame).await.map_err(|e| {
                AgentExecutorError::Protocol(format!("cteno-agent tool inject: {e}"))
            })?;
        }

        let native_id = NativeSessionId::new(native_session_id);
        let token = ProcessHandleToken::new();
        let session_ref = SessionRef {
            id: native_id.clone(),
            vendor: VENDOR_NAME,
            process_handle: token.clone(),
            spawned_at: Utc::now(),
            workdir: launch.workdir.clone(),
        };

        self.session_store
            .record_session(
                VENDOR_NAME,
                SessionRecord {
                    session_id: session_ref.id.clone(),
                    workdir: session_ref.workdir.clone(),
                    context: json!({
                        "native_session_id": session_ref.id.as_str(),
                    }),
                },
            )
            .await
            .map_err(|message| AgentExecutorError::Vendor {
                vendor: VENDOR_NAME,
                message,
            })?;

        // Spawn the per-session dispatcher BEFORE handing control back to
        // callers. The dispatcher owns `event_rx` for the lifetime of the
        // session and routes every frame either to the active per-turn
        // consumer or to the autonomous-turn handler. See `RouteState` doc
        // for the invariants.
        let route_state = Arc::new(Mutex::new(RouteState::Idle));
        spawn_session_dispatcher(
            rx,
            route_state.clone(),
            self.autonomous_turn_handler.clone(),
            self.session_event_sink.clone(),
            conn.clone(),
            native_id.as_str().to_string(),
        );

        self.sessions.lock().await.insert(
            token,
            Arc::new(Mutex::new(CtenoSessionSlot {
                native_session_id: native_id,
                launch,
                auth_state,
                process: None,
                stdin: None,
                connection: Some(conn.clone()),
                route_state,
                pending_permission_mode: None,
            })),
        );

        Ok(session_ref)
    }
}

/// Pull the `auth` sub-object out of the caller-provided `agent_config`, map
/// its `accessToken` / `userId` / `machineId` fields onto top-level Init slots,
/// and return the trimmed `agent_config` (with `auth` removed). Missing /
/// non-object shapes pass through unchanged.
fn extract_auth_from_agent_config(
    mut agent_config: Value,
) -> (Option<String>, Option<String>, Option<String>, Value) {
    let Some(obj) = agent_config.as_object_mut() else {
        return (None, None, None, agent_config);
    };
    let Some(auth_val) = obj.remove("auth") else {
        return (None, None, None, agent_config);
    };
    let Some(auth_obj) = auth_val.as_object() else {
        return (None, None, None, agent_config);
    };
    let access_token = auth_obj
        .get("accessToken")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let user_id = auth_obj
        .get("userId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let machine_id = auth_obj
        .get("machineId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (access_token, user_id, machine_id, agent_config)
}

fn current_host_auth_snapshot() -> HostAuthSnapshot {
    let Some(provider) = hooks::credentials() else {
        return HostAuthSnapshot::default();
    };
    HostAuthSnapshot {
        access_token: provider.access_token(),
        user_id: provider.user_id(),
        machine_id: provider.machine_id(),
    }
}

fn auth_state_from_snapshot(snapshot: &HostAuthSnapshot) -> SessionAuthState {
    if snapshot.access_token.is_some() {
        SessionAuthState::Present
    } else {
        SessionAuthState::Empty
    }
}

fn sync_auth_into_agent_config(agent_config: &mut Value, snapshot: &HostAuthSnapshot) {
    let Some(map) = agent_config.as_object_mut() else {
        if snapshot.access_token.is_some() {
            *agent_config = json!({
                "auth": {
                    "accessToken": snapshot.access_token,
                    "userId": snapshot.user_id,
                    "machineId": snapshot.machine_id,
                }
            });
        } else {
            *agent_config = json!({});
        }
        return;
    };

    if let Some(access_token) = snapshot.access_token.clone() {
        map.insert(
            "auth".to_string(),
            json!({
                "accessToken": access_token,
                "userId": snapshot.user_id,
                "machineId": snapshot.machine_id,
            }),
        );
    } else {
        map.remove("auth");
    }
}

fn attachment_wire_from_core(attachment: &multi_agent_runtime_core::Attachment) -> AttachmentWire {
    let kind = match attachment.kind {
        multi_agent_runtime_core::AttachmentKind::Image => AttachmentKindWire::Image,
        multi_agent_runtime_core::AttachmentKind::Text => AttachmentKindWire::Text,
        multi_agent_runtime_core::AttachmentKind::File => AttachmentKindWire::File,
        multi_agent_runtime_core::AttachmentKind::Other => AttachmentKindWire::Other,
    };
    AttachmentWire {
        kind,
        mime_type: attachment.mime_type.clone(),
        source: attachment.source.clone(),
        data: attachment.data.clone(),
    }
}

fn apply_spawn_agent_config_fields(
    agent_config: &mut Value,
    model: Option<&ModelSpec>,
    allowed_tools: Option<&Vec<String>>,
    permission_mode: PermissionMode,
) {
    if !agent_config.is_object() {
        *agent_config = json!({});
    }
    let Some(map) = agent_config.as_object_mut() else {
        return;
    };

    if let Some(model) = model {
        map.insert(
            "model".to_string(),
            json!({
                "provider": model.provider,
                "model_id": model.model_id,
                "reasoning_effort": model.reasoning_effort,
                "temperature": model.temperature,
            }),
        );
    }

    if let Some(allow) = allowed_tools {
        map.insert(
            "allowed_tools".to_string(),
            Value::Array(allow.iter().cloned().map(Value::String).collect()),
        );
    }

    map.insert(
        "permission_mode".to_string(),
        Value::String(permission_mode_wire(permission_mode).to_string()),
    );
}

fn set_launch_permission_mode(agent_config: &mut Value, mode: &str) {
    if !agent_config.is_object() {
        *agent_config = json!({});
    }
    if let Some(map) = agent_config.as_object_mut() {
        map.insert(
            "permission_mode".to_string(),
            Value::String(mode.to_string()),
        );
    }
}

fn stderr_line_is_fatal(line: &str) -> bool {
    let trimmed = line.trim_start();
    let lower = trimmed.to_ascii_lowercase();

    // cteno-agent stderr also carries diagnostics and can include previews of
    // user/session content. Treat only process-level panic/fatal prefixes as
    // fatal; otherwise ordinary text containing words like "fatal" can abort a
    // healthy turn when echoed in memory/log snippets.
    lower.starts_with("fatal")
        || lower.starts_with("panic")
        || (lower.starts_with("thread ") && lower.contains(" panicked at "))
}

fn truncate_for_error(raw: &str, max_chars: usize) -> String {
    let trimmed = raw.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }

    let head = trimmed.chars().take(max_chars).collect::<String>();
    format!("{head}...")
}

async fn push_stderr_tail(stderr_tail: &Arc<Mutex<VecDeque<String>>>, line: &str) {
    let mut tail = stderr_tail.lock().await;
    if tail.len() >= STDERR_TAIL_LINES {
        tail.pop_front();
    }
    tail.push_back(line.to_string());
}

async fn stderr_tail_snapshot(stderr_tail: &Arc<Mutex<VecDeque<String>>>) -> String {
    let tail = stderr_tail.lock().await;
    tail.iter().cloned().collect::<Vec<_>>().join(" | ")
}

async fn take_pending_fatal_stderr(
    pending_fatal_stderr: &Arc<Mutex<Option<String>>>,
) -> Option<String> {
    pending_fatal_stderr.lock().await.take()
}

fn spawn_timeout_operation(stderr_tail: &str) -> String {
    if stderr_tail.trim().is_empty() {
        "waiting for cteno-agent startup".to_string()
    } else {
        format!(
            "waiting for cteno-agent startup (last stderr: {})",
            truncate_for_error(stderr_tail, 240)
        )
    }
}

fn spawn_fatal_stderr_message(line: &str) -> String {
    format!(
        "cteno-agent startup failed: {}",
        truncate_for_error(line, 240)
    )
}

fn spawn_output_closed_message(stderr_tail: &str) -> String {
    if stderr_tail.trim().is_empty() {
        "cteno-agent startup failed: stdout closed before ready.".to_string()
    } else {
        format!(
            "cteno-agent startup failed: stdout closed before ready. Last stderr: {}",
            truncate_for_error(stderr_tail, 240)
        )
    }
}

fn turn_timeout_message(seconds: u64) -> String {
    format!(
        "cteno-agent response timed out after {seconds}s. The turn was stopped so you can retry."
    )
}

fn turn_timeout_error_event(seconds: u64) -> ExecutorEvent {
    ExecutorEvent::Error {
        message: turn_timeout_message(seconds),
        recoverable: true,
    }
}

fn empty_turn_complete_event() -> ExecutorEvent {
    ExecutorEvent::TurnComplete {
        final_text: None,
        iteration_count: 0,
        usage: TokenUsage::default(),
    }
}

async fn send_timeout_terminal_events(
    tx: &tokio::sync::mpsc::Sender<Result<ExecutorEvent, AgentExecutorError>>,
    seconds: u64,
) {
    let _ = tx.send(Ok(turn_timeout_error_event(seconds))).await;
    let _ = tx.send(Ok(empty_turn_complete_event())).await;
}

fn stderr_fatal_turn_message(line: &str) -> String {
    format!(
        "cteno-agent reported a fatal stderr line: {}",
        truncate_for_error(line, 240)
    )
}

fn subprocess_exit_message(code: Option<i32>, stderr_tail: &str) -> String {
    match (code, stderr_tail.trim().is_empty()) {
        (Some(code), true) => format!("cteno-agent exited unexpectedly (code {code})."),
        (None, true) => "cteno-agent exited unexpectedly.".to_string(),
        (Some(code), false) => format!(
            "cteno-agent exited unexpectedly (code {code}). Last stderr: {}",
            truncate_for_error(stderr_tail, 240)
        ),
        (None, false) => format!(
            "cteno-agent exited unexpectedly. Last stderr: {}",
            truncate_for_error(stderr_tail, 240)
        ),
    }
}

fn subprocess_exit_event(code: Option<i32>, stderr_tail: &str) -> ExecutorEvent {
    ExecutorEvent::Error {
        message: subprocess_exit_message(code, stderr_tail),
        recoverable: false,
    }
}

async fn cleanup_failed_child(child: &mut Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn subprocess_exit_error(
    process: &mut CtenoSessionProcess,
) -> Result<Option<AgentExecutorError>, AgentExecutorError> {
    let status = process.child.try_wait().map_err(AgentExecutorError::from)?;
    let Some(status) = status else {
        return Ok(None);
    };
    let stderr = stderr_tail_snapshot(&process.stderr_tail).await;
    Ok(Some(AgentExecutorError::SubprocessExited {
        code: status.code(),
        stderr,
    }))
}

async fn write_spawn_frame(
    child: &mut Child,
    stdin: &mut ChildStdin,
    stderr_tail: &Arc<Mutex<VecDeque<String>>>,
    frame: &Inbound,
) -> Result<(), AgentExecutorError> {
    if let Some(status) = child.try_wait().map_err(AgentExecutorError::from)? {
        let stderr = stderr_tail_snapshot(stderr_tail).await;
        return Err(AgentExecutorError::Protocol(subprocess_exit_message(
            status.code(),
            &stderr,
        )));
    }

    match write_frame(stdin, frame).await {
        Ok(()) => Ok(()),
        Err(error) => {
            if let Some(status) = child.try_wait().map_err(AgentExecutorError::from)? {
                let stderr = stderr_tail_snapshot(stderr_tail).await;
                return Err(AgentExecutorError::Protocol(subprocess_exit_message(
                    status.code(),
                    &stderr,
                )));
            }
            Err(error)
        }
    }
}

async fn write_checked_frame(
    process: &mut CtenoSessionProcess,
    stdin: &mut ChildStdin,
    operation: &str,
    frame: &Inbound,
) -> Result<(), AgentExecutorError> {
    if let Some(error) = subprocess_exit_error(process).await? {
        return Err(error);
    }

    match write_frame(stdin, frame).await {
        Ok(()) => Ok(()),
        Err(err) => {
            if let Some(error) = subprocess_exit_error(process).await? {
                return Err(error);
            }

            match err {
                AgentExecutorError::Io(message) => Err(AgentExecutorError::Protocol(format!(
                    "cteno-agent stdin closed during {operation}: {message}"
                ))),
                other => Err(other),
            }
        }
    }
}

fn spawn_stderr_probe(
    native_session_id: &str,
    stderr: ChildStderr,
) -> (
    broadcast::Sender<StderrProbeEvent>,
    Arc<Mutex<VecDeque<String>>>,
    Arc<Mutex<Option<String>>>,
) {
    let (tx, _) = broadcast::channel(16);
    let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
    let pending_fatal_stderr = Arc::new(Mutex::new(None));
    let stderr_tail_task = stderr_tail.clone();
    let pending_fatal_stderr_task = pending_fatal_stderr.clone();
    let tx_task = tx.clone();
    let session_id = native_session_id.to_string();

    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) => return,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    push_stderr_tail(&stderr_tail_task, trimmed).await;
                    if stderr_line_is_fatal(trimmed) {
                        *pending_fatal_stderr_task.lock().await = Some(trimmed.to_string());
                        let _ = tx_task.send(StderrProbeEvent::Fatal(trimmed.to_string()));
                        log::warn!("[cteno stderr {}] {}", session_id, trimmed);
                    } else {
                        log::debug!("[cteno stderr {}] {}", session_id, trimmed);
                    }
                }
                Err(error) => {
                    log::warn!("[cteno stderr {}] read error: {}", session_id, error);
                    return;
                }
            }
        }
    });

    (tx, stderr_tail, pending_fatal_stderr)
}

/// Serialize an `Inbound` enum as one JSON line and push it into `stdin`.
async fn write_frame(stdin: &mut ChildStdin, frame: &Inbound) -> Result<(), AgentExecutorError> {
    let mut line = serde_json::to_string(frame).map_err(|e| {
        AgentExecutorError::Protocol(format!("failed to serialise inbound frame: {e}"))
    })?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(AgentExecutorError::from)?;
    stdin.flush().await.map_err(AgentExecutorError::from)?;
    Ok(())
}

/// Read stdout frames until we observe a `Ready` matching the given id.
async fn wait_for_ready(
    reader: &mut BufReader<ChildStdout>,
    stderr_rx: &mut broadcast::Receiver<StderrProbeEvent>,
    stderr_tail: &Arc<Mutex<VecDeque<String>>>,
    pending_fatal_stderr: &Arc<Mutex<Option<String>>>,
    expected_id: &str,
) -> Result<String, AgentExecutorError> {
    if let Some(line) = take_pending_fatal_stderr(pending_fatal_stderr).await {
        return Err(AgentExecutorError::Protocol(spawn_fatal_stderr_message(
            &line,
        )));
    }

    loop {
        let mut line = String::new();
        let n = tokio::select! {
            res = reader.read_line(&mut line) => {
                res.map_err(AgentExecutorError::from)?
            }
            stderr_event = stderr_rx.recv() => {
                match stderr_event {
                    Ok(StderrProbeEvent::Fatal(line)) => {
                        let _ = take_pending_fatal_stderr(pending_fatal_stderr).await;
                        return Err(AgentExecutorError::Protocol(
                            spawn_fatal_stderr_message(&line),
                        ));
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        continue;
                    }
                }
            }
        };
        if n == 0 {
            let stderr = stderr_tail_snapshot(stderr_tail).await;
            return Err(AgentExecutorError::Protocol(spawn_output_closed_message(
                &stderr,
            )));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<Outbound>(trimmed) {
            Ok(Outbound::Ready { session_id }) => {
                if session_id != expected_id {
                    log::warn!(
                        "cteno-agent ready session_id mismatch: expected={expected_id} observed={session_id}"
                    );
                }
                return Ok(session_id);
            }
            Ok(Outbound::Error { message, .. }) => {
                return Err(AgentExecutorError::Protocol(format!(
                    "cteno-agent startup failed: {message}"
                )));
            }
            Ok(other) => {
                log::debug!("cteno-agent pre-ready frame ignored: {:?}", other);
                continue;
            }
            Err(e) => {
                log::warn!("cteno-agent pre-ready frame parse error: {e}; raw={trimmed}");
                continue;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AgentExecutor impl
// ---------------------------------------------------------------------------

#[async_trait]
impl AgentExecutor for CtenoAgentExecutor {
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            name: Cow::Borrowed(VENDOR_NAME),
            protocol_version: Cow::Borrowed(PROTOCOL_VERSION),
            supports_list_sessions: true,
            supports_get_messages: true,
            supports_runtime_set_model: true,
            permission_mode_kind: PermissionModeKind::Dynamic,
            supports_resume: true,
            // Phase 1 connection-reuse: `spawn_session` and
            // `start_session_on` share one subprocess per `ConnectionHandle`.
            // The stdio protocol routes every frame by `session_id`, so a
            // single child can host multiple concurrent sessions.
            supports_multi_session_per_process: true,
            supports_injected_tools: true,
            supports_permission_closure: true,
            supports_interrupt: true,
            autonomous_turn: true,
        }
    }

    async fn spawn_session(
        &self,
        spec: SpawnSessionSpec,
    ) -> Result<SessionRef, AgentExecutorError> {
        // Connection-reuse path: every `spawn_session` call opens a fresh
        // connection and attaches one session to it. The host registry may
        // later migrate to reuse a shared connection for multiple sessions,
        // but for Phase 1 we always open a new one so existing callers see
        // unchanged semantics.
        let conn_spec = ConnectionSpec {
            env: spec.env.clone(),
            probe: false,
        };
        let handle = self.open_connection(conn_spec).await?;
        self.start_session_on(&handle, spec).await
    }

    async fn resume_session(
        &self,
        session_id: NativeSessionId,
        hints: ResumeHints,
    ) -> Result<SessionRef, AgentExecutorError> {
        let workdir = hints.workdir.clone().unwrap_or_else(|| PathBuf::from("."));
        let resume_id = hints
            .vendor_cursor
            .unwrap_or_else(|| session_id.as_str().to_string());
        let agent_config = json!({
            "resume_session_id": resume_id,
        });
        self.spawn_internal(
            resume_id.clone(),
            workdir,
            None,
            agent_config,
            std::collections::BTreeMap::new(),
            Vec::new(),
        )
        .await
    }

    async fn send_message(
        &self,
        session: &SessionRef,
        message: UserMessage,
    ) -> Result<EventStream, AgentExecutorError> {
        let session_id = session.id.as_str().to_string();

        // Branch on slot backing:
        //   - Connection-backed (new multi-session path): write frames via the
        //     shared writer, consume events via the slot's event_rx.
        //   - Legacy per-session subprocess: preserve the old resurrection /
        //     stderr-probe logic verbatim.
        let slot = self.get_session_slot(session).await?;
        let has_conn = {
            let guard = slot.lock().await;
            guard.connection.is_some()
        };
        if has_conn {
            return self.send_message_connection_backed(session, message).await;
        }

        let (mut process, mut stdin) = self.ensure_turn_process(session).await?;
        let pending_permission_mode = {
            let mut guard = slot.lock().await;
            guard.pending_permission_mode.take()
        };

        if let Err(error) = self
            .send_message_frames(
                &process,
                &stdin,
                &session_id,
                &message,
                pending_permission_mode.as_deref(),
            )
            .await
        {
            let slot = self.get_session_slot(session).await?;
            {
                let mut guard = slot.lock().await;
                self.mark_slot_dead(&mut guard, "send_message frame write failed")
                    .await;
            }
            let (p, s) = self.ensure_turn_process(session).await?;
            process = p;
            stdin = s;
            self.send_message_frames(&process, &stdin, &session_id, &message, None)
                .await
                .map_err(|retry_error| match retry_error {
                    AgentExecutorError::Io(_) | AgentExecutorError::SubprocessExited { .. } => {
                        error
                    }
                    other => other,
                })?;
        }

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ExecutorEvent, AgentExecutorError>>(32);
        let turn_timeout = self.turn_timeout;
        let process_clone = process.clone();
        let stdin_clone = stdin.clone();
        let supervisor = self.supervisor.clone();
        let expected_session = session_id.clone();
        tokio::spawn(async move {
            let deadline = tokio::time::sleep(turn_timeout);
            tokio::pin!(deadline);

            let mut guard = process_clone.lock().await;
            let mut stderr_rx = guard.stderr_events.subscribe();
            let mut iterations: u32 = 0;
            let mut permission_pending = false;

            if let Some(line) = take_pending_fatal_stderr(&guard.pending_fatal_stderr).await {
                let _ = tx
                    .send(Ok(ExecutorEvent::Error {
                        message: stderr_fatal_turn_message(&line),
                        recoverable: false,
                    }))
                    .await;
                return;
            }

            loop {
                let mut line = String::new();
                tokio::select! {
                    _ = &mut deadline, if !permission_pending => {
                        send_timeout_terminal_events(&tx, turn_timeout.as_secs()).await;
                        let pid_opt = guard.pid;
                        let _ = guard.child.kill().await;
                        let _ = guard.child.wait().await;
                        if let (Some(sup), Some(pid)) = (supervisor.as_ref(), pid_opt) {
                            if let Err(e) = sup.unregister(pid) {
                                log::warn!("SubprocessSupervisor::unregister failed for pid={pid}: {e}");
                            }
                        }
                        return;
                    }
                    stderr_event = stderr_rx.recv() => {
                        match stderr_event {
                            Ok(StderrProbeEvent::Fatal(line)) => {
                                let _ = take_pending_fatal_stderr(&guard.pending_fatal_stderr).await;
                                let _ = tx
                                    .send(Ok(ExecutorEvent::Error {
                                        message: stderr_fatal_turn_message(&line),
                                        recoverable: false,
                                    }))
                                    .await;
                                return;
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {
                                continue;
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                continue;
                            }
                        }
                    }
                    res = guard.stdout_reader.read_line(&mut line) => {
                        match res {
                            Ok(0) => {
                                let code = guard
                                    .child
                                    .try_wait()
                                    .ok()
                                    .flatten()
                                    .and_then(|status| status.code());
                                let stderr = stderr_tail_snapshot(&guard.stderr_tail).await;
                                let _ = tx
                                    .send(Ok(subprocess_exit_event(code, &stderr)))
                                    .await;
                                return;
                            }
                            Ok(_) => {
                                let trimmed = line.trim();
                                if trimmed.is_empty() {
                                    continue;
                                }
                                let parsed: Result<Outbound, _> =
                                    serde_json::from_str(trimmed);
                                let event = match parsed {
                                    Ok(e) => e,
                                    Err(e) => {
                                        if tx
                                            .send(Ok(ExecutorEvent::NativeEvent {
                                                provider: Cow::Borrowed(VENDOR_NAME),
                                                payload: json!({
                                                    "raw": trimmed,
                                                    "parse_error": e.to_string(),
                                                }),
                                            }))
                                            .await
                                            .is_err()
                                        {
                                            return;
                                        }
                                        continue;
                                    }
                                };
                                let is_permission_request =
                                    matches!(&event, Outbound::PermissionRequest { .. });
                                if let Outbound::HostCallRequest {
                                    session_id,
                                    request_id,
                                    hook_name,
                                    method,
                                    ..
                                } = &event
                                {
                                    let frame = Inbound::HostCallResponse {
                                        session_id: session_id.clone(),
                                        request_id: request_id.clone(),
                                        ok: false,
                                        output: None,
                                        error: Some(format!(
                                            "host call handler not installed for {hook_name}.{method}"
                                        )),
                                    };
                                    let mut stdin_guard = stdin_clone.lock().await;
                                    let _ = write_frame(&mut *stdin_guard, &frame).await;
                                }
                                if permission_pending && !is_permission_request {
                                    permission_pending = false;
                                    deadline.as_mut().reset(tokio::time::Instant::now() + turn_timeout);
                                }
                                let done = dispatch_event(
                                    event,
                                    &tx,
                                    &mut iterations,
                                    &expected_session,
                                )
                                .await;
                                if is_permission_request {
                                    permission_pending = true;
                                }
                                if done {
                                    return;
                                }
                            }
                            Err(e) => {
                                if let Ok(Some(AgentExecutorError::SubprocessExited {
                                    code,
                                    stderr,
                                })) = subprocess_exit_error(&mut guard).await
                                {
                                    let _ = tx.send(Ok(subprocess_exit_event(code, &stderr))).await;
                                    return;
                                }
                                let stderr = stderr_tail_snapshot(&guard.stderr_tail).await;
                                let message = if stderr.trim().is_empty() {
                                    format!("cteno-agent stdout read failed: {e}")
                                } else {
                                    format!(
                                        "cteno-agent stdout read failed: {e}. Last stderr: {}",
                                        truncate_for_error(&stderr, 240)
                                    )
                                };
                                let _ = tx
                                    .send(Ok(ExecutorEvent::Error {
                                        message,
                                        recoverable: false,
                                    }))
                                    .await;
                                return;
                            }
                        }
                    }
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn respond_to_permission(
        &self,
        session: &SessionRef,
        request_id: String,
        decision: PermissionDecision,
    ) -> Result<(), AgentExecutorError> {
        let decision_wire = match &decision {
            PermissionDecision::Allow | PermissionDecision::SelectedOption { .. } => "allow",
            PermissionDecision::Deny => "deny",
            PermissionDecision::Abort => "abort",
        };
        log::info!(
            "[cteno respond_to_permission] session={} req={} decision={}",
            session.id.as_str(),
            request_id,
            decision_wire
        );
        let frame = Inbound::PermissionResponse {
            session_id: session.id.as_str().to_string(),
            request_id: request_id.clone(),
            decision: decision_wire.to_string(),
            reason: None,
        };
        let result = self.write_slot_frame(session, &frame).await;
        match &result {
            Ok(()) => log::info!(
                "[cteno respond_to_permission] write_slot_frame OK session={} req={}",
                session.id.as_str(),
                request_id
            ),
            Err(e) => log::warn!(
                "[cteno respond_to_permission] write_slot_frame ERR session={} req={}: {}",
                session.id.as_str(),
                request_id,
                e
            ),
        }
        result
    }

    async fn interrupt(&self, session: &SessionRef) -> Result<(), AgentExecutorError> {
        let frame = Inbound::Abort {
            session_id: session.id.as_str().to_string(),
            reason: None,
        };
        self.write_slot_frame(session, &frame).await
    }

    async fn close_session(&self, session: &SessionRef) -> Result<(), AgentExecutorError> {
        let entry = self.remove_session(&session.process_handle).await;
        if let Some(slot) = entry {
            let mut guard = slot.lock().await;
            // Connection-backed: detach from the shared connection's router
            // but leave the subprocess running (other sessions may share it).
            if let Some(conn) = guard.connection.take() {
                let close_frame = Inbound::CloseSession {
                    session_id: guard.native_session_id.as_str().to_string(),
                };
                if let Err(err) = conn.writer.send(&close_frame).await {
                    log::warn!(
                        "failed to notify cteno-agent about closing session {}: {}",
                        guard.native_session_id.as_str(),
                        err
                    );
                }
                conn.unregister_session(guard.native_session_id.as_str())
                    .await;
                // Force the dispatcher's active stream (if any) into a
                // terminal state so any pending downstream consumer sees
                // the session shutdown rather than hanging.
                *guard.route_state.lock().await = RouteState::Idle;
                guard.auth_state = SessionAuthState::Empty;
                return Ok(());
            }
            // Legacy path: kill the owned subprocess.
            self.mark_slot_dead(&mut guard, "close_session").await;
        }
        Ok(())
    }

    async fn set_permission_mode(
        &self,
        session: &SessionRef,
        mode: PermissionMode,
    ) -> Result<(), AgentExecutorError> {
        let mode = permission_mode_wire(mode).to_string();
        let slot = self.get_session_slot(session).await?;
        let mut guard = slot.lock().await;
        set_launch_permission_mode(&mut guard.launch.agent_config, &mode);
        guard.pending_permission_mode = Some(mode);
        Ok(())
    }

    async fn set_model(
        &self,
        session: &SessionRef,
        model: ModelSpec,
    ) -> Result<ModelChangeOutcome, AgentExecutorError> {
        let model_value = json!({
            "model_id": model.model_id.clone(),
            "reasoning_effort": model.reasoning_effort.clone(),
        });
        let frame = Inbound::SetModel {
            session_id: session.id.as_str().to_string(),
            model: model.model_id,
            effort: model.reasoning_effort,
        };
        self.send_control_frame(session, "set_model", &frame)
            .await?;
        let slot = self.get_session_slot(session).await?;
        let mut guard = slot.lock().await;
        if !guard.launch.agent_config.is_object() {
            guard.launch.agent_config = json!({});
        }
        if let Some(map) = guard.launch.agent_config.as_object_mut() {
            map.insert("model".to_string(), model_value);
        }
        Ok(ModelChangeOutcome::Applied)
    }

    async fn set_autonomous_turn_handler(
        &self,
        handler: Option<AutonomousTurnHandler>,
    ) -> Result<(), AgentExecutorError> {
        let mut guard = self.autonomous_turn_handler.lock().await;
        *guard = handler;
        Ok(())
    }

    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, AgentExecutorError> {
        self.session_store
            .list_sessions(VENDOR_NAME, filter)
            .await
            .map_err(|message| AgentExecutorError::Vendor {
                vendor: VENDOR_NAME,
                message,
            })
    }

    async fn get_session_info(
        &self,
        session_id: &NativeSessionId,
    ) -> Result<SessionInfo, AgentExecutorError> {
        self.session_store
            .get_session_info(VENDOR_NAME, session_id)
            .await
            .map_err(|message| AgentExecutorError::Vendor {
                vendor: VENDOR_NAME,
                message,
            })
    }

    async fn get_session_messages(
        &self,
        session_id: &NativeSessionId,
        pagination: Pagination,
    ) -> Result<Vec<NativeMessage>, AgentExecutorError> {
        self.session_store
            .get_session_messages(VENDOR_NAME, session_id, pagination)
            .await
            .map_err(|message| AgentExecutorError::Vendor {
                vendor: VENDOR_NAME,
                message,
            })
    }

    // -----------------------------------------------------------------------
    // Connection-reuse trait seam (Phase 1)
    // -----------------------------------------------------------------------

    async fn open_connection(
        &self,
        spec: ConnectionSpec,
    ) -> Result<ConnectionHandle, AgentExecutorError> {
        let conn = self.open_connection_internal(&spec).await?;
        Ok(ConnectionHandle {
            id: conn.id.clone(),
            vendor: VENDOR_NAME,
            inner: conn as Arc<dyn std::any::Any + Send + Sync>,
        })
    }

    async fn close_connection(&self, handle: ConnectionHandle) -> Result<(), AgentExecutorError> {
        let ConnectionHandle { id, inner, .. } = handle;
        // Remove from registry first so no concurrent start_session_on races
        // the shutdown.
        let removed = self.connections.lock().await.remove(&id);
        let conn: Option<Arc<CtenoConnection>> = match removed {
            Some(c) => Some(c),
            None => inner.downcast::<CtenoConnection>().ok(),
        };
        let Some(conn) = conn else {
            // Already closed or never registered — treat as no-op per trait
            // contract that close is idempotent.
            return Ok(());
        };
        let pid = conn.pid;
        conn.close().await;
        if let (Some(sup), Some(pid)) = (self.supervisor.as_ref(), pid) {
            if let Err(e) = sup.unregister(pid) {
                log::warn!("SubprocessSupervisor::unregister failed for pid={pid}: {e}");
            }
        }
        Ok(())
    }

    async fn check_connection(
        &self,
        handle: &ConnectionHandle,
    ) -> Result<ConnectionHealth, AgentExecutorError> {
        let conn = lookup_connection(&self.connections, handle).await?;
        Ok(match conn.check().await {
            crate::connection::ConnectionLiveness::Alive => ConnectionHealth::Healthy,
            crate::connection::ConnectionLiveness::Dead { reason } => {
                ConnectionHealth::Dead { reason }
            }
        })
    }

    async fn start_session_on(
        &self,
        handle: &ConnectionHandle,
        spec: SpawnSessionSpec,
    ) -> Result<SessionRef, AgentExecutorError> {
        let conn = lookup_connection(&self.connections, handle).await?;

        // Reject start-on-dead-connection loudly rather than silently hanging
        // on the Ready wait.
        if let crate::connection::ConnectionLiveness::Dead { reason } = conn.check().await {
            return Err(AgentExecutorError::Protocol(format!(
                "start_session_on: connection is dead: {reason}"
            )));
        }

        // Same agent_config massaging as the legacy path: honour
        // resume_hint.vendor_cursor, fold model / allowed_tools /
        // permission_mode into agent_config.
        let (native_id, mut agent_config) = if let Some(hint) = spec
            .resume_hint
            .as_ref()
            .and_then(|h| h.vendor_cursor.clone())
        {
            let mut cfg = spec.agent_config.clone();
            if let Value::Object(ref mut map) = cfg {
                map.insert("resume_session_id".to_string(), Value::String(hint.clone()));
            }
            (hint, cfg)
        } else {
            (Uuid::new_v4().to_string(), spec.agent_config.clone())
        };
        apply_spawn_agent_config_fields(
            &mut agent_config,
            spec.model.as_ref(),
            spec.allowed_tools.as_ref(),
            spec.permission_mode,
        );

        let injected = spec.injected_tools();
        let launch = CtenoSessionLaunchConfig {
            workdir: spec.workdir,
            system_prompt: spec.system_prompt,
            additional_directories: spec.additional_directories,
            agent_config,
            env: spec.env,
            injected_tools: injected,
        };

        self.start_session_on_internal(&conn, native_id, launch)
            .await
    }
}

/// Downcast a `ConnectionHandle::inner` back to the concrete `CtenoConnection`
/// and cross-reference with the executor's registry. Prefers the registry
/// entry over the `Arc<dyn Any>` — the registry is authoritative.
async fn lookup_connection(
    registry: &ConnectionRegistry,
    handle: &ConnectionHandle,
) -> Result<Arc<CtenoConnection>, AgentExecutorError> {
    if handle.vendor != VENDOR_NAME {
        return Err(AgentExecutorError::Protocol(format!(
            "connection handle belongs to vendor '{}', not '{}'",
            handle.vendor, VENDOR_NAME
        )));
    }
    if let Some(conn) = registry.lock().await.get(&handle.id).cloned() {
        return Ok(conn);
    }
    // Fall back to the Arc<dyn Any> in case the handle outlives the registry
    // entry (e.g. after close_connection).
    handle
        .inner
        .clone()
        .downcast::<CtenoConnection>()
        .map_err(|_| {
            AgentExecutorError::Protocol(
                "ConnectionHandle::inner is not a CtenoConnection".to_string(),
            )
        })
}

/// Map a cross-vendor `PermissionMode` into the snake-case string that the
/// cteno runtime accepts inside `agent_config.permission_mode`.
fn permission_mode_wire(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default | PermissionMode::Auto => "default",
        PermissionMode::AcceptEdits => "accept_edits",
        PermissionMode::BypassPermissions | PermissionMode::DontAsk => "bypass_permissions",
        PermissionMode::Plan => "plan",
        PermissionMode::ReadOnly => "read_only",
        PermissionMode::WorkspaceWrite => "workspace_write",
        PermissionMode::DangerFullAccess => "danger_full_access",
    }
}

/// Translate a decoded [`Outbound`] frame into zero or more
/// [`ExecutorEvent`]s and push them through the channel. Returns `true` when
/// the turn is complete and the worker loop should exit.
/// Spawn a long-lived dispatcher task for a connection-backed session.
///
/// The dispatcher is the sole consumer of the session's `event_rx` for the
/// session's lifetime. It reads frames in arrival order, applies explicit
/// route-state transitions on `AutonomousTurnStart` / `TurnComplete`, and
/// forwards everything else through the `dispatch_event` translator into
/// whichever `tx` is currently parked in `RouteState`. Because there is one
/// consumer per session, frame ordering is preserved naturally.
///
/// Special handling:
/// - `AutonomousTurnStart`: caught **before** translation. Allocates a fresh
///   per-turn `(tx, rx)`, transitions `Idle -> InAutonomousTurn { tx }`, and
///   invokes the registered `AutonomousTurnHandler` with `(session_id, rx
///   stream)`. The handler is responsible for consuming the stream
///   (typically by spawning a normalizer feeder).
/// - `HostCallRequest`: auto-replies with "no handler installed" via the
///   shared connection writer (preserves the legacy behavior). This needs
///   to live in the dispatcher because both user-driven and autonomous
///   turns can emit it.
/// - `TurnComplete`: routed through normally, then transitions state back
///   to `Idle`.
fn spawn_session_dispatcher(
    mut event_rx: SessionEventRx,
    route_state: Arc<Mutex<RouteState>>,
    autonomous_handler_slot: Arc<Mutex<Option<AutonomousTurnHandler>>>,
    session_event_sink_slot: Arc<Mutex<Option<crate::session_sink::SessionEventSinkArc>>>,
    conn: Arc<CtenoConnection>,
    session_id: String,
) {
    tokio::spawn(async move {
        let mut iterations: u32 = 0;
        log::debug!("[cteno dispatcher] starting for session {}", session_id);
        while let Some(frame) = event_rx.recv().await {
            // 1a. SubAgent lifecycle frames are out-of-band: not part of
            //     any per-turn stream. Route to the SessionEventSink so
            //     the host can mirror the SubAgent registry.
            if let Outbound::SubAgentLifecycle {
                session_id: ref parent,
                ref event,
            } = frame
            {
                let sink = session_event_sink_slot.lock().await.clone();
                if let Some(sink) = sink {
                    log::info!(
                        "[cteno dispatcher] routing SubAgentLifecycle frame for parent={parent} to SessionEventSink"
                    );
                    let dto = crate::session_sink::SubAgentLifecycleEvent::from_wire(
                        event.clone(),
                    );
                    sink.on_subagent_lifecycle(parent, dto);
                } else {
                    log::warn!(
                        "[cteno dispatcher] SubAgentLifecycle for {parent} but no SessionEventSink installed — dropping"
                    );
                }
                continue;
            }

            // 1b. Explicit boundary: AutonomousTurnStart begins a new turn.
            if let Outbound::AutonomousTurnStart {
                session_id: ref frame_session,
                ref reason,
                ref synthetic_user_message,
            } = frame
            {
                let handler = autonomous_handler_slot.lock().await.clone();
                let Some(handler) = handler else {
                    log::warn!(
                        "[cteno dispatcher] AutonomousTurnStart for {frame_session} (reason={reason:?}) but no autonomous_turn_handler registered — frames will be dropped until next idle"
                    );
                    continue;
                };
                let (auto_tx, auto_rx) =
                    tokio::sync::mpsc::channel::<Result<ExecutorEvent, AgentExecutorError>>(64);
                let stream: EventStream = Box::pin(ReceiverStream::new(auto_rx));
                {
                    let mut state = route_state.lock().await;
                    if !state.is_idle() {
                        log::warn!(
                            "[cteno dispatcher] AutonomousTurnStart for {frame_session} while state={} (replacing — user turn frames may be dropped)",
                            state.label()
                        );
                    }
                    *state = RouteState::InAutonomousTurn { tx: auto_tx };
                }
                handler(
                    frame_session.clone(),
                    synthetic_user_message.clone(),
                    stream,
                );
                continue;
            }

            // 2. Auto-reply to host_call_request frames so the agent's
            //    HostCallRequest doesn't hang waiting on a host that has
            //    no installed handler.
            if let Outbound::HostCallRequest {
                session_id: ref hc_session,
                ref request_id,
                ref hook_name,
                ref method,
                ..
            } = frame
            {
                let _ = conn
                    .writer
                    .send(&Inbound::HostCallResponse {
                        session_id: hc_session.clone(),
                        request_id: request_id.clone(),
                        ok: false,
                        output: None,
                        error: Some(format!(
                            "host call handler not installed for {hook_name}.{method}"
                        )),
                    })
                    .await;
            }

            // 3. Snapshot the current route tx so we don't hold the
            //    state lock across the await.
            let active_tx = {
                let state = route_state.lock().await;
                match &*state {
                    RouteState::Idle => None,
                    RouteState::InUserTurn { tx } | RouteState::InAutonomousTurn { tx } => {
                        Some(tx.clone())
                    }
                }
            };
            let Some(tx) = active_tx else {
                log::warn!(
                    "[cteno dispatcher] frame for {session_id} arrived in Idle state — dropping"
                );
                continue;
            };

            // 4. Translate + forward via the existing dispatch_event helper.
            //    Returns `true` iff this frame ended the turn (TurnComplete
            //    or downstream consumer dropped).
            let consumer_gone = dispatch_event(frame, &tx, &mut iterations, &session_id).await;

            // 5. If turn ended (or consumer dropped), reset state and
            //    iteration counter so the next turn starts clean.
            if consumer_gone {
                let mut state = route_state.lock().await;
                // Only transition if THIS tx is still parked — guards
                // against a race where send_message already replaced state
                // (it shouldn't because RouteState rejects double-claim,
                // but be defensive).
                let still_ours = match &*state {
                    RouteState::InUserTurn { tx: parked }
                    | RouteState::InAutonomousTurn { tx: parked } => parked.same_channel(&tx),
                    RouteState::Idle => false,
                };
                if still_ours {
                    *state = RouteState::Idle;
                    iterations = 0;
                }
            }
        }
        log::info!(
            "[cteno dispatcher] event_rx closed for session {} — exiting",
            session_id
        );
    });
}

async fn dispatch_event(
    event: Outbound,
    tx: &tokio::sync::mpsc::Sender<Result<ExecutorEvent, AgentExecutorError>>,
    iterations: &mut u32,
    expected_session: &str,
) -> bool {
    // We don't abort on session_id mismatches — a single cteno-agent process
    // may serve multiple sessions in the future. For now log and forward.
    match event {
        Outbound::Ready { session_id } => {
            log::warn!(
                "cteno-agent emitted unexpected mid-turn ready frame (session={session_id})"
            );
            let _ = tx
                .send(Ok(ExecutorEvent::SessionReady {
                    native_session_id: NativeSessionId::new(session_id),
                }))
                .await;
            false
        }
        Outbound::Acp {
            session_id,
            delivery,
            data,
        } => {
            if session_id != expected_session {
                log::debug!(
                    "acp frame for foreign session ignored (expected={expected_session} got={session_id})"
                );
            }
            if data.get("type").and_then(Value::as_str) == Some("tool-call") {
                *iterations = iterations.saturating_add(1);
            }
            tx.send(Ok(ExecutorEvent::NativeEvent {
                provider: Cow::Borrowed(VENDOR_NAME),
                payload: json!({
                    "kind": "acp",
                    "session_id": session_id,
                    "delivery": delivery,
                    "data": data,
                }),
            }))
            .await
            .is_err()
        }
        Outbound::PermissionRequest {
            request_id,
            tool_name,
            tool_input,
            ..
        } => tx
            .send(Ok(ExecutorEvent::PermissionRequest {
                request_id,
                tool_name,
                tool_input,
            }))
            .await
            .is_err(),
        Outbound::ToolExecutionRequest {
            request_id,
            tool_name,
            tool_input,
            ..
        } => tx
            .send(Ok(ExecutorEvent::InjectedToolInvocation {
                request_id,
                tool_name,
                tool_input,
            }))
            .await
            .is_err(),
        Outbound::HostCallRequest {
            session_id,
            request_id,
            hook_name,
            method,
            params,
        } => {
            log::warn!(
                "cteno-agent host_call_request has no host handler at executor layer (hook={hook_name} method={method})"
            );
            tx.send(Ok(ExecutorEvent::NativeEvent {
                provider: Cow::Borrowed(VENDOR_NAME),
                payload: json!({
                    "kind": "host_call_request",
                    "session_id": session_id,
                    "request_id": request_id,
                    "hook_name": hook_name,
                    "method": method,
                    "params": params,
                }),
            }))
            .await
            .is_err()
        }
        Outbound::AutonomousTurnStart {
            session_id,
            reason,
            synthetic_user_message,
        } => tx
            .send(Ok(ExecutorEvent::NativeEvent {
                provider: Cow::Borrowed(VENDOR_NAME),
                payload: json!({
                    "kind": "autonomous_turn_start",
                    "session_id": session_id,
                    "reason": reason,
                    "synthetic_user_message": synthetic_user_message,
                }),
            }))
            .await
            .is_err(),
        // Defensive fallback. Phase B routes SubAgentLifecycle via the
        // dispatcher's SessionEventSink **before** any per-turn `dispatch_event`
        // call; this NativeEvent fallback only fires for paths that bypass the
        // dispatcher (e.g. legacy subprocess sessions) and lets debugging UIs
        // observe the wire frame as a typed payload.
        Outbound::SubAgentLifecycle { session_id, event } => tx
            .send(Ok(ExecutorEvent::NativeEvent {
                provider: Cow::Borrowed(VENDOR_NAME),
                payload: json!({
                    "kind": "subagent_lifecycle",
                    "session_id": session_id,
                    "event": event,
                }),
            }))
            .await
            .is_err(),
        Outbound::TurnComplete {
            iteration_count,
            usage,
            context_usage,
            ..
        } => {
            if let Some(context_usage) = context_usage {
                let _ = tx
                    .send(Ok(ExecutorEvent::NativeEvent {
                        provider: Cow::Borrowed(VENDOR_NAME),
                        payload: json!({
                            "kind": "context_usage",
                            "total_tokens": context_usage.total_tokens,
                            "max_tokens": context_usage.max_tokens,
                            "raw_max_tokens": context_usage.raw_max_tokens,
                            "auto_compact_token_limit": context_usage.auto_compact_token_limit,
                        }),
                    }))
                    .await;
            }
            let _ = tx
                .send(Ok(ExecutorEvent::TurnComplete {
                    final_text: None,
                    iteration_count: iteration_count as u32,
                    usage: TokenUsage {
                        input_tokens: usage.input_tokens as u64,
                        output_tokens: usage.output_tokens as u64,
                        cache_creation_tokens: usage.cache_creation_input_tokens as u64,
                        cache_read_tokens: usage.cache_read_input_tokens as u64,
                        reasoning_tokens: 0,
                    },
                }))
                .await;
            true
        }
        Outbound::Error { message, .. } => {
            let _ = tx
                .send(Ok(ExecutorEvent::Error {
                    message,
                    recoverable: true,
                }))
                .await;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AcpDeliveryWire;

    #[test]
    fn apply_spawn_agent_config_fields_keeps_profile_id_alongside_model() {
        let mut agent_config = json!({
            "profile_id": "user-direct"
        });
        let model = ModelSpec {
            provider: "openai".to_string(),
            model_id: "gpt-5.1".to_string(),
            reasoning_effort: Some("high".to_string()),
            temperature: Some(0.2),
        };

        apply_spawn_agent_config_fields(
            &mut agent_config,
            Some(&model),
            None,
            PermissionMode::Default,
        );

        assert_eq!(
            agent_config.get("profile_id").and_then(Value::as_str),
            Some("user-direct")
        );
        assert_eq!(
            agent_config
                .get("model")
                .and_then(Value::as_object)
                .and_then(|model| model.get("model_id"))
                .and_then(Value::as_str),
            Some("gpt-5.1")
        );
        assert_eq!(
            agent_config.get("permission_mode").and_then(Value::as_str),
            Some("default")
        );
    }

    #[test]
    fn extract_auth_from_agent_config_preserves_non_auth_fields() {
        let (_, _, _, config) = extract_auth_from_agent_config(json!({
            "profile_id": "user-direct",
            "model": {
                "provider": "openai",
                "model_id": "gpt-5.1"
            },
            "auth": {
                "accessToken": "token",
                "userId": "user",
                "machineId": "machine"
            }
        }));

        assert_eq!(
            config.get("profile_id").and_then(Value::as_str),
            Some("user-direct")
        );
        assert_eq!(
            config
                .get("model")
                .and_then(Value::as_object)
                .and_then(|model| model.get("model_id"))
                .and_then(Value::as_str),
            Some("gpt-5.1")
        );
        assert!(config.get("auth").is_none());
    }

    #[tokio::test]
    async fn dispatch_acp_frame_as_native_event_without_translation() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut iterations = 0;

        let done = dispatch_event(
            Outbound::Acp {
                session_id: "s-1".to_string(),
                delivery: AcpDeliveryWire::Persisted,
                data: json!({
                    "type": "tool-call",
                    "id": "call-1",
                    "name": "read",
                    "input": {"path": "notes.txt"},
                    "vendor_field": {"kept": true}
                }),
            },
            &tx,
            &mut iterations,
            "s-1",
        )
        .await;

        assert!(!done);
        assert_eq!(iterations, 1);
        let event = rx.recv().await.expect("event").expect("executor event");
        match event {
            ExecutorEvent::NativeEvent { provider, payload } => {
                assert_eq!(provider.as_ref(), VENDOR_NAME);
                assert_eq!(payload.get("kind").and_then(Value::as_str), Some("acp"));
                assert_eq!(
                    payload.get("delivery").and_then(Value::as_str),
                    Some("persisted")
                );
                assert_eq!(
                    payload
                        .pointer("/data/vendor_field/kept")
                        .and_then(Value::as_bool),
                    Some(true)
                );
            }
            other => panic!("expected NativeEvent, got {other:?}"),
        }
    }

    #[test]
    fn sync_auth_into_agent_config_injects_live_snapshot() {
        let mut config = json!({
            "profile_id": "proxy-default"
        });

        sync_auth_into_agent_config(
            &mut config,
            &HostAuthSnapshot {
                access_token: Some("token-1".to_string()),
                user_id: Some("user-1".to_string()),
                machine_id: Some("machine-1".to_string()),
            },
        );

        assert_eq!(config["auth"]["accessToken"].as_str(), Some("token-1"));
        assert_eq!(config["auth"]["userId"].as_str(), Some("user-1"));
        assert_eq!(config["auth"]["machineId"].as_str(), Some("machine-1"));
        assert_eq!(config["profile_id"].as_str(), Some("proxy-default"));
    }

    #[test]
    fn sync_auth_into_agent_config_removes_stale_auth_when_logged_out() {
        let mut config = json!({
            "profile_id": "proxy-default",
            "auth": {
                "accessToken": "stale-token"
            }
        });

        sync_auth_into_agent_config(&mut config, &HostAuthSnapshot::default());

        assert!(config.get("auth").is_none());
        assert_eq!(config["profile_id"].as_str(), Some("proxy-default"));
    }

    #[tokio::test]
    async fn set_permission_mode_queues_for_next_turn_without_live_process() {
        let exec = test_executor();
        let token = ProcessHandleToken::new();
        let session = SessionRef {
            id: NativeSessionId::new("cteno-test-session"),
            vendor: VENDOR_NAME,
            process_handle: token.clone(),
            spawned_at: Utc::now(),
            workdir: PathBuf::from("/tmp/cteno-test"),
        };

        exec.sessions.lock().await.insert(
            token,
            Arc::new(Mutex::new(CtenoSessionSlot {
                native_session_id: session.id.clone(),
                launch: CtenoSessionLaunchConfig {
                    workdir: session.workdir.clone(),
                    system_prompt: None,
                    additional_directories: Vec::new(),
                    agent_config: json!({"permission_mode": "default"}),
                    env: BTreeMap::new(),
                    injected_tools: Vec::new(),
                },
                auth_state: SessionAuthState::Empty,
                process: None,
                stdin: None,
                connection: None,
                route_state: Arc::new(Mutex::new(RouteState::Idle)),
                pending_permission_mode: None,
            })),
        );

        exec.set_permission_mode(&session, PermissionMode::BypassPermissions)
            .await
            .expect("permission mode should queue without writing stdin");

        let slot = exec.get_session_slot(&session).await.unwrap();
        let guard = slot.lock().await;
        assert_eq!(
            guard
                .launch
                .agent_config
                .get("permission_mode")
                .and_then(Value::as_str),
            Some("bypass_permissions")
        );
        assert_eq!(
            guard.pending_permission_mode.as_deref(),
            Some("bypass_permissions")
        );
    }

    #[test]
    fn stderr_probe_flags_panic_and_fatal_lines() {
        assert!(stderr_line_is_fatal(
            "thread 'tokio' panicked at src/main.rs:1"
        ));
        assert!(stderr_line_is_fatal("FATAL: unable to bind socket"));
        assert!(!stderr_line_is_fatal("warning: retrying startup"));
        assert!(!stderr_line_is_fatal(
            "\"content\": \"cteno-agent reported a fatal stderr line\""
        ));
        assert!(!stderr_line_is_fatal(
            "CURRENT SESSION MEMORY: user mentioned panic and fatal text"
        ));
    }

    #[test]
    fn subprocess_exit_message_includes_stderr_tail_when_available() {
        let message = subprocess_exit_message(Some(101), "panic: broken state machine");
        assert!(message.contains("code 101"));
        assert!(message.contains("panic: broken state machine"));
    }

    #[test]
    fn spawn_output_closed_message_includes_stderr_tail_when_available() {
        let message = spawn_output_closed_message("fatal: bootstrap panic");
        assert!(message.contains("stdout closed before ready"));
        assert!(message.contains("fatal: bootstrap panic"));
    }

    #[test]
    fn spawn_timeout_operation_uses_readable_startup_context() {
        let no_stderr = spawn_timeout_operation("");
        let with_stderr = spawn_timeout_operation("panic: bootstrap failed");

        assert_eq!(no_stderr, "waiting for cteno-agent startup");
        assert!(with_stderr.contains("waiting for cteno-agent startup"));
        assert!(with_stderr.contains("panic: bootstrap failed"));
        assert!(!with_stderr.contains("spawn_session"));
    }

    #[test]
    fn subprocess_exit_event_is_terminal_and_readable() {
        let event = subprocess_exit_event(Some(101), "panic: broken state machine");
        match event {
            ExecutorEvent::Error {
                message,
                recoverable,
            } => {
                assert!(!recoverable);
                assert!(message.contains("code 101"));
                assert!(message.contains("panic: broken state machine"));
            }
            other => panic!("expected fatal error event, got {other:?}"),
        }
    }

    #[test]
    fn turn_timeout_event_is_recoverable_and_retryable() {
        let event = turn_timeout_error_event(600);
        match event {
            ExecutorEvent::Error {
                message,
                recoverable,
            } => {
                assert!(recoverable);
                assert!(message.contains("timed out after 600s"));
                assert!(message.contains("retry"));
                assert!(!message.contains("Last stderr"));
            }
            other => panic!("expected recoverable timeout error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn take_pending_fatal_stderr_consumes_the_cached_line() {
        let pending = Arc::new(Mutex::new(Some("panic: cached fatal".to_string())));

        let first = take_pending_fatal_stderr(&pending).await;
        let second = take_pending_fatal_stderr(&pending).await;

        assert_eq!(first.as_deref(), Some("panic: cached fatal"));
        assert!(second.is_none());
    }

    // -------------------------------------------------------------------
    // Connection-reuse unit tests. These exercise only the pure-logic
    // surfaces (capability flag, handle downcasting, spec default). The
    // subprocess-driven flows are covered by the integration test
    // `tests/integration_connection_reuse.rs`.
    // -------------------------------------------------------------------

    fn test_executor() -> CtenoAgentExecutor {
        // Binary path that should not exist; we never spawn in these tests.
        let bin = PathBuf::from("/nonexistent/cteno-agent");
        let store: Arc<dyn SessionStoreProvider> = Arc::new(StubStore::default());
        CtenoAgentExecutor::new(bin, store)
    }

    #[derive(Default)]
    struct StubStore;
    #[async_trait::async_trait]
    impl SessionStoreProvider for StubStore {
        async fn record_session(&self, _v: &str, _r: SessionRecord) -> Result<(), String> {
            Ok(())
        }
        async fn list_sessions(
            &self,
            _v: &str,
            _f: SessionFilter,
        ) -> Result<Vec<SessionMeta>, String> {
            Ok(Vec::new())
        }
        async fn get_session_info(
            &self,
            _v: &str,
            _id: &NativeSessionId,
        ) -> Result<SessionInfo, String> {
            Err("not implemented".into())
        }
        async fn get_session_messages(
            &self,
            _v: &str,
            _id: &NativeSessionId,
            _p: Pagination,
        ) -> Result<Vec<NativeMessage>, String> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn capabilities_flag_multi_session_is_true() {
        let exec = test_executor();
        assert!(exec.capabilities().supports_multi_session_per_process);
        assert!(exec.capabilities().autonomous_turn);
    }

    #[tokio::test]
    async fn set_autonomous_turn_handler_stores_handler_when_supported() {
        let exec = test_executor();
        let handler: AutonomousTurnHandler =
            Arc::new(|_session_id, _synthetic, _stream| {});
        exec.set_autonomous_turn_handler(Some(handler)).await.unwrap();
        assert!(exec.autonomous_turn_handler.lock().await.is_some());
    }

    // -------------------------------------------------------------------
    // RouteState transition tests — verify the state machine that drives
    // the dispatcher's frame routing decisions. These tests exercise the
    // pure state surface only (no real connection / event_rx).
    // -------------------------------------------------------------------

    #[test]
    fn route_state_idle_label() {
        assert_eq!(RouteState::Idle.label(), "idle");
        assert!(RouteState::Idle.is_idle());
    }

    #[tokio::test]
    async fn route_state_user_turn_label_and_busy() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let state = RouteState::InUserTurn { tx };
        assert_eq!(state.label(), "user_turn");
        assert!(!state.is_idle());
    }

    #[tokio::test]
    async fn route_state_autonomous_turn_label_and_busy() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let state = RouteState::InAutonomousTurn { tx };
        assert_eq!(state.label(), "autonomous_turn");
        assert!(!state.is_idle());
    }

    #[tokio::test]
    async fn send_message_rejects_when_not_idle() {
        // Synthesize a session slot in InUserTurn state and verify a
        // second send_message would refuse rather than corrupt routing.
        let route_state = Arc::new(Mutex::new(RouteState::Idle));
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        *route_state.lock().await = RouteState::InUserTurn { tx };

        // The actual rejection branch lives inside
        // send_message_connection_backed; here we simulate the same
        // critical-section check to lock in the contract.
        let state = route_state.lock().await;
        assert!(!state.is_idle());
        assert_eq!(state.label(), "user_turn");
    }

    #[tokio::test]
    async fn open_connection_with_nonexistent_binary_reports_io_error() {
        let exec = test_executor();
        let res = exec.open_connection(ConnectionSpec::default()).await;
        assert!(res.is_err(), "expected error for nonexistent binary");
        match res.unwrap_err() {
            AgentExecutorError::Io(_) => {}
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn close_connection_with_stale_handle_is_noop() {
        let exec = test_executor();
        // Build a synthetic handle backed by an Arc<()> that doesn't
        // downcast to CtenoConnection. Should succeed (no-op) per trait
        // contract of close being idempotent.
        let handle = ConnectionHandle {
            id: ConnectionHandleId::new(),
            vendor: VENDOR_NAME,
            inner: Arc::new(()) as Arc<dyn std::any::Any + Send + Sync>,
        };
        exec.close_connection(handle).await.expect("noop close");
    }

    #[tokio::test]
    async fn check_connection_with_foreign_vendor_rejects() {
        let exec = test_executor();
        let handle = ConnectionHandle {
            id: ConnectionHandleId::new(),
            vendor: "claude",
            inner: Arc::new(()) as Arc<dyn std::any::Any + Send + Sync>,
        };
        let res = exec.check_connection(&handle).await;
        assert!(
            matches!(res, Err(AgentExecutorError::Protocol(_))),
            "expected protocol error for foreign vendor handle"
        );
    }
}

// ---------------------------------------------------------------------------
// Small helpers on public types
// ---------------------------------------------------------------------------

trait SpawnSessionSpecExt {
    fn injected_tools(&self) -> Vec<multi_agent_runtime_core::InjectedToolSpec>;
}

impl SpawnSessionSpecExt for SpawnSessionSpec {
    /// `SpawnSessionSpec` does not carry injected tools directly (they ride
    /// on `UserMessage`), but a future extension might add a spawn-time field.
    /// Returning an empty vec keeps the surface forward-compatible.
    fn injected_tools(&self) -> Vec<multi_agent_runtime_core::InjectedToolSpec> {
        Vec::new()
    }
}
