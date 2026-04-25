//! [`AgentExecutor`] implementation for the Codex CLI.
//!
//! The preferred transport is Codex's persistent `app-server` JSON-RPC mode
//! (`codex app-server --listen stdio://`). Each runtime session owns one
//! long-lived subprocess and reuses it across turns via `thread/start` +
//! `turn/start`.
//!
//! Older Codex builds do not expose `app-server`; in that case the adapter
//! falls back to the legacy `codex exec --experimental-json` one-shot path so
//! the runtime still works, including the existing per-turn timeout guard.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use async_stream::try_stream;
use async_trait::async_trait;
use chrono::Utc;
use multi_agent_runtime_core::executor::{
    AgentCapabilities, AgentExecutor, AgentExecutorError, ConnectionHandle, ConnectionHandleId,
    ConnectionHealth, ConnectionSpec, DeltaKind, EventStream, ExecutorEvent, ModelChangeOutcome,
    ModelSpec, NativeMessage, NativeSessionId, Pagination, PermissionDecision, PermissionMode,
    PermissionModeKind, ProcessHandleToken, ResumeHints, SessionFilter, SessionInfo, SessionMeta,
    SessionRecord, SessionRef, SessionStoreProvider, SpawnSessionSpec, TokenUsage, UserMessage,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::stream::{
    CodexCommandExecOutputDelta, CodexItem, CodexJsonEvent, CodexMcpToolCallProgress,
    CodexPlanDelta, CodexPlanItem, CodexTurnPlanUpdate,
};

/// Vendor tag used by this adapter when talking to [`SessionStoreProvider`].
const VENDOR: &str = "codex";
const CODEX_GUARDIAN_REVIEW_MARKER: &str = "__codex_guardian_review";
const CODEX_TURN_PLAN_TOOL_PREFIX: &str = "__codex_turn_plan";
const CODEX_RESUME_CONFIG_KEY: &str = "codex_resume_config";

/// Per-turn timeout for Codex `send_message`. Codex can run lengthy
/// operations so we allow 5 minutes before giving up.
const DEFAULT_TURN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Default)]
struct CommandOutputBuffers {
    stdout: String,
    stderr: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandOutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone)]
struct ExecFallbackState {
    current_child: Arc<Mutex<Option<Child>>>,
    current_stdin: Arc<Mutex<Option<ChildStdin>>>,
}

struct AppServerPendingRequest {
    method: String,
    params: Value,
}

/// Message routed from the demuxer task into a per-thread turn runner.
#[derive(Debug)]
enum ThreadFrame {
    /// A server→client JSON-RPC request targeting this thread (approval /
    /// elicitation). `id` is the transport-level request id to respond with.
    ServerRequest {
        id: u64,
        method: String,
        params: Value,
    },
    /// A server→client notification targeting this thread.
    Notification { method: String, params: Value },
    /// The connection died while this thread still had a live turn.
    ConnectionClosed { reason: String },
}

/// Outcome of a client→server JSON-RPC call awaiting its matching id.
struct PendingRequest {
    #[allow(dead_code)]
    method: String,
    tx: oneshot::Sender<Result<Value, AgentExecutorError>>,
}

/// Per-thread state held by the connection. One thread ≡ one session.
struct ThreadState {
    /// Frames routed from the demuxer (approvals, notifications, closures).
    /// Drained by `send_message_via_app_server` inside its per-turn loop.
    frames_rx: Mutex<Option<mpsc::UnboundedReceiver<ThreadFrame>>>,
    frames_tx: mpsc::UnboundedSender<ThreadFrame>,
    /// Pending server→client approval / elicitation requests keyed by
    /// JSON-RPC `id` (string form for cross-adapter consistency).
    pending_approvals: Mutex<HashMap<String, AppServerPendingRequest>>,
    /// Current in-flight turn id, if any.
    current_turn_id: Arc<Mutex<Option<String>>>,
    /// Cached session config for resume / restart decisions.
    #[allow(dead_code)]
    config: Mutex<SessionConfig>,
}

impl ThreadState {
    fn new(config: SessionConfig) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            frames_rx: Mutex::new(Some(rx)),
            frames_tx: tx,
            pending_approvals: Mutex::new(HashMap::new()),
            current_turn_id: Arc::new(Mutex::new(None)),
            config: Mutex::new(config),
        }
    }
}

/// Per-turn lease for a thread's frame receiver.
///
/// `send_message_via_app_server` takes ownership of `frames_rx` so only one
/// turn can run concurrently. The stream consumer often stops after
/// `TurnComplete`, which drops the stream early. This guard ensures the
/// receiver is returned to `ThreadState` even on early drop/error.
struct TurnFrameLease {
    thread_state: Arc<ThreadState>,
    frames_rx: Option<mpsc::UnboundedReceiver<ThreadFrame>>,
}

impl TurnFrameLease {
    fn new(
        thread_state: Arc<ThreadState>,
        frames_rx: mpsc::UnboundedReceiver<ThreadFrame>,
    ) -> Self {
        Self {
            thread_state,
            frames_rx: Some(frames_rx),
        }
    }

    fn receiver_mut(&mut self) -> &mut mpsc::UnboundedReceiver<ThreadFrame> {
        self.frames_rx
            .as_mut()
            .expect("turn frame receiver must exist while lease is active")
    }

    async fn return_now(&mut self) {
        if let Some(frames_rx) = self.frames_rx.take() {
            *self.thread_state.frames_rx.lock().await = Some(frames_rx);
        }
    }
}

impl Drop for TurnFrameLease {
    fn drop(&mut self) {
        let Some(frames_rx) = self.frames_rx.take() else {
            return;
        };
        let thread_state = self.thread_state.clone();
        tokio::spawn(async move {
            *thread_state.frames_rx.lock().await = Some(frames_rx);
        });
    }
}

/// Long-lived reusable `codex app-server` subprocess hosting multiple threads.
///
/// Returned from [`CodexAgentExecutor::open_connection`] and reused across
/// `start_session_on` calls. The demultiplexer task reads stdout line-by-line,
/// routes responses via `pending_requests`, and fans out thread-scoped
/// notifications / requests to [`ThreadState::frames_tx`] so each session's
/// turn stream stays independent.
pub(crate) struct CodexAppServerConnection {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    next_request_id: Arc<AtomicU64>,
    pending_requests: Arc<Mutex<HashMap<u64, PendingRequest>>>,
    threads: Arc<RwLock<HashMap<String, Arc<ThreadState>>>>,
    last_frame_seen: Arc<RwLock<Instant>>,
    demux_task: Mutex<Option<JoinHandle<()>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
    #[allow(dead_code)]
    codex_path: PathBuf,
}

/// Thin per-thread handle held by a `SessionTransport::AppServer`. Points
/// back to the shared connection, plus the vendor-native `thread_id`.
#[derive(Clone)]
struct ConnectionThreadHandle {
    connection: Arc<CodexAppServerConnection>,
    thread_id: String,
}

#[derive(Clone)]
enum SessionTransport {
    AppServer(ConnectionThreadHandle),
    ExecFallback(ExecFallbackState),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SessionConfig {
    workdir: PathBuf,
    #[serde(default)]
    additional_directories: Vec<PathBuf>,
    permission_mode: PermissionMode,
    #[serde(default)]
    model: Option<ModelSpec>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

struct SessionRuntimeState {
    desired_config: SessionConfig,
    transport_config: SessionConfig,
    transport: SessionTransport,
}

/// Handle bookkeeping for a spawned Codex session.
struct SessionState {
    /// Vendor-native thread id (stable across resumes).
    native_id: Arc<Mutex<NativeSessionId>>,
    /// Desired session config plus the currently active transport binding.
    runtime: Mutex<SessionRuntimeState>,
}

/// [`AgentExecutor`] adapter for the Codex CLI.
pub struct CodexAgentExecutor {
    /// Path to the `codex` binary.
    codex_path: PathBuf,
    /// Shared session store used for host-side listing / message fetch.
    session_store: Arc<dyn SessionStoreProvider>,
    /// Registry of live sessions keyed by process-handle token.
    sessions: Arc<RwLock<HashMap<ProcessHandleToken, Arc<SessionState>>>>,
    /// Cached feature probe for `codex app-server`.
    app_server_available: OnceLock<bool>,
    /// Per-turn timeout for `send_message` event streams.
    turn_timeout: Duration,
    /// Shared app-server connection reused across sessions when the
    /// registry has not opened one explicitly. Guarded by a mutex to
    /// serialise concurrent opens.
    shared_connection: Arc<Mutex<Option<(ConnectionHandleId, Arc<CodexAppServerConnection>)>>>,
}

impl CodexAgentExecutor {
    /// Build a new executor bound to the given `codex` binary path and
    /// session store.
    pub fn new(codex_path: PathBuf, session_store: Arc<dyn SessionStoreProvider>) -> Self {
        Self {
            codex_path,
            session_store,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            app_server_available: OnceLock::new(),
            turn_timeout: DEFAULT_TURN_TIMEOUT,
            shared_connection: Arc::new(Mutex::new(None)),
        }
    }

    /// Override the per-turn timeout for `send_message` streams.
    pub fn with_turn_timeout(mut self, timeout: Duration) -> Self {
        self.turn_timeout = timeout;
        self
    }

    /// Look up the child state for a session handle.
    async fn state_for(
        &self,
        session: &SessionRef,
    ) -> Result<Arc<SessionState>, AgentExecutorError> {
        let guard = self.sessions.read().await;
        guard
            .get(&session.process_handle)
            .cloned()
            .ok_or_else(|| AgentExecutorError::SessionNotFound(session.id.as_str().to_string()))
    }

    fn app_server_available(&self) -> bool {
        *self
            .app_server_available
            .get_or_init(|| probe_app_server(&self.codex_path))
    }

    /// Translate a [`PermissionMode`] into the Codex CLI's `--sandbox` +
    /// `approval_policy` pair.
    ///
    /// Codex does not have a 1:1 mapping — we pick the closest-safer
    /// combination and document the choice inline.
    fn permission_mode_args(mode: PermissionMode) -> (&'static str, &'static str) {
        match mode {
            // Default: lock down writes to workspace, require approval on request.
            PermissionMode::Default => ("workspace-write", "on-request"),
            // Auto: Codex has no auto mode; default is the closest safe mapping.
            PermissionMode::Auto => ("workspace-write", "on-request"),
            // AcceptEdits: auto-approve edits inside workspace.
            PermissionMode::AcceptEdits => ("workspace-write", "never"),
            // BypassPermissions: full access, never ask.
            PermissionMode::BypassPermissions => ("danger-full-access", "never"),
            // DontAsk: Codex has no dedicated mode; full access without prompts
            // is the closest equivalent.
            PermissionMode::DontAsk => ("danger-full-access", "never"),
            // Plan: read-only with prompts so no state changes.
            PermissionMode::Plan => ("read-only", "untrusted"),
            // ReadOnly: pure read-only.
            PermissionMode::ReadOnly => ("read-only", "never"),
            // WorkspaceWrite: writes inside workspace, prompt elsewhere.
            PermissionMode::WorkspaceWrite => ("workspace-write", "on-request"),
            // DangerFullAccess: full access, never ask.
            PermissionMode::DangerFullAccess => ("danger-full-access", "never"),
        }
    }

    fn synthetic_exec_fallback_id() -> NativeSessionId {
        NativeSessionId::new(format!("codex-{}", uuid::Uuid::new_v4()))
    }

    fn default_session_config(workdir: PathBuf) -> SessionConfig {
        SessionConfig {
            workdir,
            additional_directories: Vec::new(),
            permission_mode: PermissionMode::Default,
            model: None,
            system_prompt: None,
            env: BTreeMap::new(),
        }
    }

    fn persisted_context(session_id: &NativeSessionId, config: &SessionConfig) -> Value {
        json!({
            "native_session_id": session_id.as_str(),
            CODEX_RESUME_CONFIG_KEY: config,
        })
    }

    async fn persist_session_record(
        &self,
        session_id: &NativeSessionId,
        config: &SessionConfig,
    ) -> Result<(), AgentExecutorError> {
        self.session_store
            .record_session(
                VENDOR,
                SessionRecord {
                    session_id: session_id.clone(),
                    workdir: config.workdir.clone(),
                    context: Self::persisted_context(session_id, config),
                },
            )
            .await
            .map_err(|message| AgentExecutorError::Vendor {
                vendor: VENDOR,
                message,
            })
    }

    async fn load_persisted_config(
        &self,
        session_id: &NativeSessionId,
        hints: &ResumeHints,
    ) -> SessionConfig {
        let fallback_workdir = hints
            .workdir
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let mut config = Self::default_session_config(fallback_workdir);

        if let Ok(info) = self
            .session_store
            .get_session_info(VENDOR, session_id)
            .await
        {
            if let Some(persisted) = info
                .extras
                .get(CODEX_RESUME_CONFIG_KEY)
                .cloned()
                .and_then(|value| serde_json::from_value::<SessionConfig>(value).ok())
            {
                config = persisted;
            } else {
                config.workdir = info.meta.workdir;
            }
        }

        if let Some(workdir) = hints.workdir.clone() {
            config.workdir = workdir;
        }

        config
    }

    /// Dial a brand-new app-server subprocess and run the `initialize` +
    /// `initialized` handshake. No `thread/start` is issued here — the
    /// returned connection is ready to host any number of threads via
    /// [`Self::attach_thread`].
    async fn dial_connection(
        &self,
        env: &BTreeMap<String, String>,
    ) -> Result<Arc<CodexAppServerConnection>, AgentExecutorError> {
        let mut command = Command::new(&self.codex_path);
        command
            .arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        for (key, value) in env {
            command.env(key, value);
        }

        let mut child = command
            .spawn()
            .map_err(|e| AgentExecutorError::Io(format!("spawn codex app-server: {e}")))?;
        let mut stdin = child.stdin.take().ok_or_else(|| {
            AgentExecutorError::Protocol("codex app-server missing stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AgentExecutorError::Protocol("codex app-server missing stdout".to_string())
        })?;
        let mut stdout_reader = BufReader::new(stdout);

        // initialize handshake — blocking read before the demuxer starts.
        app_server_write_request(
            &mut stdin,
            1,
            "initialize",
            json!({
                "clientInfo": {
                    "name": "multi-agent-runtime-codex",
                    "title": "multi-agent-runtime-codex",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "experimentalApi": true,
                },
            }),
        )
        .await?;
        let _ = app_server_wait_for_response(&mut stdin, &mut stdout_reader, 1).await?;

        app_server_write_notification(&mut stdin, "initialized", Value::Null).await?;

        let pending_requests = Arc::new(Mutex::new(HashMap::new()));
        let threads: Arc<RwLock<HashMap<String, Arc<ThreadState>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let last_frame_seen = Arc::new(RwLock::new(Instant::now()));
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let connection = Arc::new(CodexAppServerConnection {
            child: Arc::new(Mutex::new(child)),
            stdin: Arc::new(Mutex::new(stdin)),
            next_request_id: Arc::new(AtomicU64::new(2)),
            pending_requests: pending_requests.clone(),
            threads: threads.clone(),
            last_frame_seen: last_frame_seen.clone(),
            demux_task: Mutex::new(None),
            closed: closed.clone(),
            codex_path: self.codex_path.clone(),
        });

        // Spawn the demuxer task — owns the stdout reader and fans frames
        // out to per-thread channels / pending-request oneshots. Gets a
        // clone of the stdin handle so it can ack connection-level
        // server→client requests (e.g. `account/chatgptAuthTokens/refresh`
        // in external-auth mode) instead of silently dropping them.
        let stdin_for_demux = connection.stdin.clone();
        let task = tokio::spawn(run_app_server_demux(
            stdout_reader,
            pending_requests,
            threads,
            last_frame_seen,
            closed,
            stdin_for_demux,
        ));
        *connection.demux_task.lock().await = Some(task);

        Ok(connection)
    }

    /// Issue `thread/start` (or `thread/resume`) on an already-dialed
    /// connection. Registers a fresh [`ThreadState`] in the connection's
    /// thread map and returns the vendor-native id.
    async fn attach_thread(
        connection: &Arc<CodexAppServerConnection>,
        config: &SessionConfig,
        resume_thread_id: Option<String>,
    ) -> Result<NativeSessionId, AgentExecutorError> {
        let (sandbox, approval) = Self::permission_mode_args(config.permission_mode);
        let model_id = config
            .model
            .as_ref()
            .map(|m| m.model_id.clone())
            .unwrap_or_else(|| "gpt-5.4".to_string());

        let thread_method = if resume_thread_id.is_some() {
            "thread/resume"
        } else {
            "thread/start"
        };
        let thread_params = if let Some(thread_id) = resume_thread_id.as_ref() {
            json!({
                "threadId": thread_id,
                "model": model_id,
                "modelProvider": Value::Null,
                "cwd": config.workdir,
                "approvalPolicy": approval,
                "sandbox": sandbox,
                "config": Value::Null,
                "baseInstructions": Value::Null,
                "developerInstructions": Value::Null,
                "persistExtendedHistory": true,
            })
        } else {
            json!({
                "model": model_id,
                "modelProvider": Value::Null,
                "profile": Value::Null,
                "cwd": config.workdir,
                "approvalPolicy": approval,
                "sandbox": sandbox,
                "config": Value::Null,
                "baseInstructions": config.system_prompt,
                "developerInstructions": Value::Null,
                "compactPrompt": Value::Null,
                "includeApplyPatchTool": Value::Null,
                "experimentalRawEvents": true,
                "persistExtendedHistory": true,
            })
        };

        let result = connection
            .call_request(thread_method, thread_params)
            .await?;
        let native_id = app_server_thread_id(&result).ok_or_else(|| {
            AgentExecutorError::Protocol(
                "codex app-server thread response missing thread.id".into(),
            )
        })?;

        // Register the thread *before* any notifications can arrive. The
        // demuxer peeks at the thread map each frame — if this thread id
        // is missing, a rogue `thread/started` would be dropped.
        let thread_state = Arc::new(ThreadState::new(config.clone()));
        connection
            .threads
            .write()
            .await
            .insert(native_id.clone(), thread_state);

        Ok(NativeSessionId::new(native_id))
    }

    /// Reuse the executor-held shared connection, creating one lazily the
    /// first time an app-server session spawns. Registry callers that
    /// opened a connection via [`AgentExecutor::open_connection`] use
    /// [`AgentExecutor::start_session_on`] instead and bypass this cache.
    async fn shared_app_server_connection(
        &self,
        config: &SessionConfig,
    ) -> Result<Arc<CodexAppServerConnection>, AgentExecutorError> {
        let mut guard = self.shared_connection.lock().await;
        if let Some((_, connection)) = guard.as_ref() {
            if !connection.closed.load(Ordering::SeqCst) {
                // Cheap liveness check; if the child died we fall through
                // to re-dial below.
                let mut child = connection.child.lock().await;
                if matches!(child.try_wait(), Ok(None)) {
                    return Ok(connection.clone());
                }
            }
        }

        let connection = self.dial_connection(&config.env).await?;
        *guard = Some((ConnectionHandleId::new(), connection.clone()));
        Ok(connection)
    }

    async fn spawn_session_state(
        &self,
        config: SessionConfig,
        native_id: NativeSessionId,
        transport: SessionTransport,
    ) -> Result<SessionRef, AgentExecutorError> {
        self.persist_session_record(&native_id, &config).await?;

        let process_handle = ProcessHandleToken::new();
        let session_ref = SessionRef {
            id: native_id.clone(),
            vendor: VENDOR,
            process_handle,
            spawned_at: Utc::now(),
            workdir: config.workdir.clone(),
        };

        let state = Arc::new(SessionState {
            native_id: Arc::new(Mutex::new(native_id)),
            runtime: Mutex::new(SessionRuntimeState {
                desired_config: config.clone(),
                transport_config: config,
                transport,
            }),
        });

        self.sessions
            .write()
            .await
            .insert(session_ref.process_handle.clone(), state);

        Ok(session_ref)
    }

    async fn send_message_via_exec_fallback(
        &self,
        state: Arc<SessionState>,
        transport: ExecFallbackState,
        config: SessionConfig,
        message: UserMessage,
    ) -> Result<EventStream, AgentExecutorError> {
        let codex_path = self.codex_path.clone();

        let resume_thread_id = {
            let native_id = state.native_id.lock().await;
            let id = native_id.as_str();
            if id.starts_with("codex-") && id.len() == "codex-".len() + 36 {
                None
            } else {
                Some(id.to_string())
            }
        };

        let (sandbox, approval) = Self::permission_mode_args(config.permission_mode);
        let model_id = config
            .model
            .as_ref()
            .map(|m| m.model_id.clone())
            .unwrap_or_else(|| "gpt-5.4".to_string());

        let mut command = Command::new(&codex_path);
        command
            .arg("exec")
            .arg("--experimental-json")
            .arg("--model")
            .arg(&model_id)
            .arg("--sandbox")
            .arg(sandbox)
            .arg("--config")
            .arg(format!("approval_policy=\"{approval}\""))
            .arg("--cd")
            .arg(&config.workdir)
            .arg("--skip-git-repo-check");

        for add_dir in &config.additional_directories {
            command.arg("--add-dir").arg(add_dir);
        }
        for (key, value) in &config.env {
            command.env(key, value);
        }
        command.env(
            "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
            "multi_agent_runtime_rust",
        );

        if let Some(thread_id) = resume_thread_id {
            command.arg("resume").arg(thread_id);
        }

        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut prompt = String::new();
        if let Some(sys) = config.system_prompt.as_ref() {
            prompt.push_str(sys);
            prompt.push_str("\n\n");
        }
        prompt.push_str(&message.content);
        prompt.push('\n');
        command.arg(&prompt);

        let child_slot = transport.current_child.clone();
        let stdin_slot = transport.current_stdin.clone();
        let native_id_slot = state.native_id.clone();
        let session_store = self.session_store.clone();
        let persist_config = config.clone();
        let turn_timeout = self.turn_timeout;

        let stream = try_stream! {
            let mut child = command
                .spawn()
                .map_err(|e| AgentExecutorError::Io(format!("spawn codex: {e}")))?;

            let stdin = child.stdin.take().ok_or_else(|| {
                AgentExecutorError::Protocol("codex child missing stdin".into())
            })?;
            let stdout = child.stdout.take().ok_or_else(|| {
                AgentExecutorError::Protocol("codex child missing stdout".into())
            })?;

            *stdin_slot.lock().await = Some(stdin);
            *child_slot.lock().await = Some(child);

            let mut lines = BufReader::new(stdout).lines();
            let mut iteration_count: u32 = 0;
            let mut final_text: Option<String> = None;
            let mut aggregate_usage = TokenUsage::default();
            let mut active_turn_plan_tool_use_id: Option<String> = None;
            let mut plan_text_by_tool_use_id: HashMap<String, String> = HashMap::new();
            let mut command_output_by_tool_use_id: HashMap<String, CommandOutputBuffers> =
                HashMap::new();

            let turn_deadline = tokio::time::Instant::now() + turn_timeout;

            loop {
                let remaining = turn_deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    Err(AgentExecutorError::Timeout {
                        operation: "send_message".to_string(),
                        seconds: turn_timeout.as_secs(),
                    })?;
                    unreachable!();
                }

                let line = match tokio::time::timeout(remaining, lines.next_line()).await {
                    Ok(Ok(Some(line))) => line,
                    Ok(Ok(None)) => break,
                    Ok(Err(e)) => {
                        Err(AgentExecutorError::Io(e.to_string()))?;
                        unreachable!();
                    }
                    Err(_) => {
                        Err(AgentExecutorError::Timeout {
                            operation: "send_message".to_string(),
                            seconds: turn_timeout.as_secs(),
                        })?;
                        unreachable!();
                    }
                };

                let trimmed = line.trim();
                if !trimmed.starts_with('{') {
                    continue;
                }

                let event: CodexJsonEvent = match serde_json::from_str(trimmed) {
                    Ok(event) => event,
                    Err(_) => {
                        yield ExecutorEvent::NativeEvent {
                            provider: Cow::Borrowed(VENDOR),
                            payload: Value::String(trimmed.to_string()),
                        };
                        continue;
                    }
                };

                match event {
                    CodexJsonEvent::ThreadStarted { thread_id } => {
                        let native_id = NativeSessionId::new(thread_id);
                        *native_id_slot.lock().await = native_id.clone();
                        session_store
                            .record_session(
                                VENDOR,
                                SessionRecord {
                                    session_id: native_id.clone(),
                                    workdir: persist_config.workdir.clone(),
                                    context: Self::persisted_context(&native_id, &persist_config),
                                },
                            )
                            .await
                            .map_err(|message| AgentExecutorError::Vendor {
                                vendor: VENDOR,
                                message,
                            })?;
                        yield ExecutorEvent::SessionReady { native_session_id: native_id };
                    }
                    CodexJsonEvent::TurnStarted => {
                        iteration_count = iteration_count.saturating_add(1);
                        active_turn_plan_tool_use_id = None;
                        plan_text_by_tool_use_id.clear();
                        command_output_by_tool_use_id.clear();
                    }
                    CodexJsonEvent::TurnCompleted { usage } => {
                        if let Some(tool_use_id) = active_turn_plan_tool_use_id.take() {
                            yield codex_plan_tool_result(tool_use_id);
                        }
                        if let Some(usage) = usage.as_ref() {
                            update_usage_from_object(usage, &mut aggregate_usage);
                        }
                        yield ExecutorEvent::UsageUpdate(aggregate_usage.clone());
                        yield ExecutorEvent::TurnComplete {
                            final_text: final_text.clone(),
                            iteration_count,
                            usage: aggregate_usage.clone(),
                        };
                    }
                    CodexJsonEvent::TurnFailed { error } => {
                        if let Some(tool_use_id) = active_turn_plan_tool_use_id.take() {
                            yield codex_plan_tool_result(tool_use_id);
                        }
                        yield ExecutorEvent::Error {
                            message: error.message,
                            recoverable: true,
                        };
                    }
                    CodexJsonEvent::TurnPlanUpdated { update } => {
                        let tool_use_id =
                            codex_turn_plan_tool_use_id(update.id(), iteration_count);
                        active_turn_plan_tool_use_id = Some(tool_use_id.clone());
                        yield translate_turn_plan_update(&tool_use_id, &update);
                    }
                    CodexJsonEvent::McpToolCallProgress { progress } => {
                        if let Some(event) = translate_mcp_tool_call_progress(&progress) {
                            yield event;
                        } else {
                            yield ExecutorEvent::NativeEvent {
                                provider: Cow::Borrowed(VENDOR),
                                payload: progress.raw_payload(),
                            };
                        }
                    }
                    CodexJsonEvent::CommandExecOutputDelta { delta } => {
                        if let Some(event) = translate_command_exec_output_delta(
                            &delta,
                            &mut command_output_by_tool_use_id,
                        ) {
                            yield event;
                        } else {
                            yield ExecutorEvent::NativeEvent {
                                provider: Cow::Borrowed(VENDOR),
                                payload: delta.raw_payload(),
                            };
                        }
                    }
                    CodexJsonEvent::PlanDelta { delta } => {
                        if let Some(event) =
                            translate_plan_delta(&delta, &mut plan_text_by_tool_use_id)
                        {
                            yield event;
                        } else {
                            yield ExecutorEvent::NativeEvent {
                                provider: Cow::Borrowed(VENDOR),
                                payload: delta.raw_payload(),
                            };
                        }
                    }
                    CodexJsonEvent::Error { message } => {
                        if let Some(tool_use_id) = active_turn_plan_tool_use_id.take() {
                            yield codex_plan_tool_result(tool_use_id);
                        }
                        yield ExecutorEvent::Error { message, recoverable: false };
                    }
                    CodexJsonEvent::ItemStarted { item }
                    | CodexJsonEvent::ItemUpdated { item } => {
                        if let Some(event) = translate_item(&item, &model_id) {
                            yield event;
                        }
                        if let Some(event) = translate_reasoning_summary(&item) {
                            yield event;
                        }
                    }
                    CodexJsonEvent::ItemCompleted { item } => {
                        if let CodexItem::AgentMessage { text, .. } = &item {
                            final_text = Some(text.clone());
                            yield ExecutorEvent::StreamDelta {
                                kind: DeltaKind::Text,
                                content: text.clone(),
                            };
                        }
                        if let Some(event) = translate_reasoning_summary(&item) {
                            yield event;
                        }
                        for event in translate_item_completed(&item) {
                            yield event;
                        }
                    }
                }
            }

            if let Some(mut stdin) = stdin_slot.lock().await.take() {
                let _ = stdin.shutdown().await;
            }

            if let Some(mut child) = child_slot.lock().await.take() {
                let _ = child.wait().await;
            }
        };

        Ok(Box::pin(stream))
    }

    async fn send_message_via_app_server(
        &self,
        state: Arc<SessionState>,
        handle: ConnectionThreadHandle,
        config: SessionConfig,
        message: UserMessage,
    ) -> Result<EventStream, AgentExecutorError> {
        let model_id = config
            .model
            .as_ref()
            .map(|m| m.model_id.clone())
            .unwrap_or_else(|| "gpt-5.4".to_string());
        let reasoning_effort = config
            .model
            .as_ref()
            .and_then(|m| m.reasoning_effort.clone());
        let (sandbox, approval) = Self::permission_mode_args(config.permission_mode);
        let turn_timeout = self.turn_timeout;
        let workdir = config.workdir.clone();
        let additional_directories = config.additional_directories.clone();

        let connection = handle.connection.clone();
        let thread_id = handle.thread_id.clone();

        // Acquire the per-thread frame receiver exclusively for this turn.
        // A second concurrent turn on the same thread is a protocol error.
        let thread_state = connection
            .threads
            .read()
            .await
            .get(&thread_id)
            .cloned()
            .ok_or_else(|| {
                AgentExecutorError::Protocol(format!(
                    "codex app-server thread {thread_id} is not registered"
                ))
            })?;
        let frames_rx = thread_state.frames_rx.lock().await.take().ok_or_else(|| {
            AgentExecutorError::Protocol(format!(
                "codex app-server thread {thread_id} already has a turn in flight"
            ))
        })?;
        let mut frame_lease = TurnFrameLease::new(thread_state.clone(), frames_rx);

        // Account-level rate limits are now polled by the daemon's
        // machine-scoped `cteno-host-usage-monitor` — no per-turn probe.
        let initial_native_events: Vec<ExecutorEvent> = Vec::new();

        // Fire `turn/start` via the connection's `call_request` helper so
        // the response (carrying `turn.id`) is demuxed into our oneshot.
        let sandbox_policy = sandbox_policy_json(sandbox, &workdir, &additional_directories);
        let turn_params = json!({
            "threadId": thread_id,
            "input": [{ "type": "text", "text": message.content }],
            "cwd": workdir,
            "approvalPolicy": approval,
            "sandboxPolicy": sandbox_policy,
            "model": model_id,
            "effort": reasoning_effort,
            "summary": "auto",
            "outputSchema": Value::Null,
        });
        let _ = state; // keep `state` reachable for future native-id updates

        let turn_response = connection.call_request("turn/start", turn_params).await?;
        let initial_turn_id = app_server_turn_id_from_response(Some(&turn_response));
        if let Some(id) = initial_turn_id.clone() {
            *thread_state.current_turn_id.lock().await = Some(id);
        }

        let stream = try_stream! {
            let mut iteration_count: u32 = 0;
            let mut final_text: Option<String> = None;
            let mut aggregate_usage = TokenUsage::default();
            let mut active_turn_plan_tool_use_id: Option<String> = None;
            let mut plan_text_by_tool_use_id: HashMap<String, String> = HashMap::new();
            let mut command_output_by_tool_use_id: HashMap<String, CommandOutputBuffers> =
                HashMap::new();
            let deadline = tokio::time::sleep(turn_timeout);
            tokio::pin!(deadline);
            let mut permission_pending = false;

            for event in initial_native_events {
                yield event;
            }

            loop {
                let frame = tokio::select! {
                    _ = &mut deadline, if !permission_pending => {
                        Err(AgentExecutorError::Timeout {
                            operation: "send_message".to_string(),
                            seconds: turn_timeout.as_secs(),
                        })
                    }
                    frame = frame_lease.receiver_mut().recv() => {
                        match frame {
                            Some(frame) => Ok(frame),
                            None => Err(AgentExecutorError::Protocol(
                                "codex app-server thread channel closed mid-turn".to_string(),
                            )),
                        }
                    }
                }?;

                match frame {
                    ThreadFrame::ConnectionClosed { reason } => {
                        Err(AgentExecutorError::Protocol(format!(
                            "codex app-server connection closed: {reason}"
                        )))?;
                        unreachable!();
                    }
                    ThreadFrame::ServerRequest { id, method, params } => {
                        if let Some(event) = handle_app_server_request(
                            id,
                            &method,
                            &params,
                            &thread_state.pending_approvals,
                        ).await? {
                            let is_permission_request =
                                matches!(&event, ExecutorEvent::PermissionRequest { .. });
                            if permission_pending && !is_permission_request {
                                permission_pending = false;
                                deadline
                                    .as_mut()
                                    .reset(tokio::time::Instant::now() + turn_timeout);
                            }
                            yield event;
                            if is_permission_request {
                                permission_pending = true;
                            }
                        }
                    }
                    ThreadFrame::Notification { method, params } => {
                        let outcome = handle_app_server_notification(
                            &method,
                            &params,
                            &model_id,
                            &thread_state.current_turn_id,
                            &mut final_text,
                            &mut aggregate_usage,
                            &mut iteration_count,
                            &mut active_turn_plan_tool_use_id,
                            &mut plan_text_by_tool_use_id,
                            &mut command_output_by_tool_use_id,
                        ).await?;
                        let has_permission_request = outcome
                            .events
                            .iter()
                            .any(|event| matches!(event, ExecutorEvent::PermissionRequest { .. }));
                        if permission_pending
                            && !has_permission_request
                            && !outcome.events.is_empty()
                        {
                            permission_pending = false;
                            deadline
                                .as_mut()
                                .reset(tokio::time::Instant::now() + turn_timeout);
                        }
                        for event in outcome.events {
                            yield event;
                        }
                        if has_permission_request {
                            permission_pending = true;
                        }
                        if outcome.done {
                            break;
                        }
                    }
                }
            }

            // Return explicitly on normal stream completion. Early stream
            // drops/errors are handled by TurnFrameLease::drop.
            frame_lease.return_now().await;
        };

        Ok(Box::pin(stream))
    }

    /// Release the transport attached to a single session. For app-server
    /// sessions this only drops the thread entry — the underlying
    /// connection stays alive so other sessions on the same subprocess are
    /// unaffected. Killing the subprocess is only the job of
    /// [`AgentExecutor::close_connection`].
    async fn close_transport(transport: SessionTransport) -> Result<(), AgentExecutorError> {
        match transport {
            SessionTransport::AppServer(handle) => {
                handle
                    .connection
                    .threads
                    .write()
                    .await
                    .remove(&handle.thread_id);
            }
            SessionTransport::ExecFallback(transport) => {
                if let Some(mut stdin) = transport.current_stdin.lock().await.take() {
                    let _ = stdin.shutdown().await;
                }
                if let Some(mut child) = transport.current_child.lock().await.take() {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
            }
        }
        Ok(())
    }

    async fn prepare_transport_for_turn(
        &self,
        state: &Arc<SessionState>,
    ) -> Result<(SessionTransport, SessionConfig), AgentExecutorError> {
        let (transport, desired_config, transport_config) = {
            let runtime = state.runtime.lock().await;
            (
                runtime.transport.clone(),
                runtime.desired_config.clone(),
                runtime.transport_config.clone(),
            )
        };

        if desired_config == transport_config {
            return Ok((transport, desired_config));
        }

        match transport.clone() {
            SessionTransport::ExecFallback(_) => {
                let mut runtime = state.runtime.lock().await;
                runtime.transport_config = runtime.desired_config.clone();
                Ok((runtime.transport.clone(), runtime.desired_config.clone()))
            }
            SessionTransport::AppServer(handle) => {
                // Look up this thread's in-flight turn on the shared
                // connection. If a turn is live, defer re-config to the
                // caller — the same way the legacy transport did.
                let thread_state = handle
                    .connection
                    .threads
                    .read()
                    .await
                    .get(&handle.thread_id)
                    .cloned();
                if let Some(thread_state) = thread_state {
                    if thread_state.current_turn_id.lock().await.is_some() {
                        return Err(AgentExecutorError::Protocol(
                            "codex app-server is busy; pending config changes apply after the current turn"
                                .to_string(),
                        ));
                    }
                }

                // Restart the thread in place by issuing `thread/resume`
                // with the new config on the same connection. The demuxer
                // continues to fan frames into the *new* thread id.
                let resume_id = state.native_id.lock().await.as_str().to_string();
                let new_native_id = Self::attach_thread(
                    &handle.connection,
                    &desired_config,
                    Some(resume_id.clone()),
                )
                .await?;
                self.persist_session_record(&new_native_id, &desired_config)
                    .await?;

                // Retire the old thread entry so future demux frames under
                // the old id are dropped cleanly.
                handle
                    .connection
                    .threads
                    .write()
                    .await
                    .remove(&handle.thread_id);

                let new_handle = ConnectionThreadHandle {
                    connection: handle.connection.clone(),
                    thread_id: new_native_id.as_str().to_string(),
                };
                let new_transport = SessionTransport::AppServer(new_handle);
                *state.native_id.lock().await = new_native_id;
                {
                    let mut runtime = state.runtime.lock().await;
                    runtime.transport = new_transport.clone();
                    runtime.transport_config = desired_config.clone();
                }
                Ok((new_transport, desired_config))
            }
        }
    }
}

impl CodexAppServerConnection {
    /// Send a client→server JSON-RPC request on this connection and wait
    /// for the matching response. The demuxer routes the result into the
    /// oneshot registered in `pending_requests`.
    async fn call_request(
        self: &Arc<Self>,
        method: &str,
        params: Value,
    ) -> Result<Value, AgentExecutorError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(AgentExecutorError::Protocol(
                "codex app-server connection is closed".to_string(),
            ));
        }

        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending_requests.lock().await.insert(
            id,
            PendingRequest {
                method: method.to_string(),
                tx,
            },
        );

        {
            let mut stdin = self.stdin.lock().await;
            if let Err(error) = app_server_write_request(&mut stdin, id, method, params).await {
                // Remove the pending entry to avoid a slow-memory leak.
                self.pending_requests.lock().await.remove(&id);
                return Err(error);
            }
        }

        match rx.await {
            Ok(result) => result,
            Err(_) => Err(AgentExecutorError::Protocol(format!(
                "codex app-server connection dropped before responding to {method}"
            ))),
        }
    }

    /// Write a raw JSON-RPC response to a server→client request (used by
    /// `respond_to_permission` / `respond_to_elicitation`).
    async fn call_response(&self, id: u64, result: Value) -> Result<(), AgentExecutorError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(AgentExecutorError::Protocol(
                "codex app-server connection is closed".to_string(),
            ));
        }
        let mut stdin = self.stdin.lock().await;
        app_server_write_response(&mut stdin, id, result).await
    }

    /// Best-effort shutdown. Drops stdin, waits briefly, then kills.
    /// Cancels the demuxer task and drains any pending oneshots with a
    /// `ConnectionClosed` error.
    async fn shutdown(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }

        // Drop stdin so the server sees EOF.
        {
            let mut stdin = self.stdin.lock().await;
            let _ = stdin.shutdown().await;
        }

        {
            let mut child = self.child.lock().await;
            if tokio::time::timeout(Duration::from_secs(1), child.wait())
                .await
                .is_err()
            {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        }

        // Cancel the demuxer task.
        if let Some(task) = self.demux_task.lock().await.take() {
            task.abort();
        }

        // Drain pending requests with an error so callers unblock.
        let mut pending = self.pending_requests.lock().await;
        for (_, request) in pending.drain() {
            let _ = request.tx.send(Err(AgentExecutorError::Protocol(
                "codex app-server connection closed".to_string(),
            )));
        }

        // Notify every registered thread.
        let threads = self.threads.read().await.clone();
        for state in threads.values() {
            let _ = state.frames_tx.send(ThreadFrame::ConnectionClosed {
                reason: "connection shut down".to_string(),
            });
        }
    }
}

#[async_trait]
impl AgentExecutor for CodexAgentExecutor {
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            name: Cow::Borrowed(VENDOR),
            protocol_version: Cow::Borrowed("0.1"),
            // list/get/info are implemented via SessionStoreProvider, so
            // they look native to callers even though Codex itself cannot
            // answer them.
            supports_list_sessions: true,
            supports_get_messages: true,
            // Model changes are applied by restarting the transport in place.
            supports_runtime_set_model: true,
            // Permission mode changes use the same restart-in-place path.
            permission_mode_kind: PermissionModeKind::Dynamic,
            // `codex exec resume <thread_id>` restores conversation state.
            supports_resume: true,
            // With the Phase-1 connection-reuse refactor the app-server
            // path hosts N threads on one subprocess. The exec-fallback
            // path still spawns per-turn, but callers pick it only when
            // app-server is unavailable.
            supports_multi_session_per_process: true,
            // The CLI has no caller-injected-tool surface.
            supports_injected_tools: false,
            // Permission flow is through `approval_policy` + stdin prompts.
            supports_permission_closure: true,
            // Children can be killed to interrupt the turn.
            supports_interrupt: true,
            autonomous_turn: false,
        }
    }

    async fn spawn_session(
        &self,
        spec: SpawnSessionSpec,
    ) -> Result<SessionRef, AgentExecutorError> {
        let config = SessionConfig {
            workdir: spec.workdir,
            additional_directories: spec.additional_directories,
            permission_mode: spec.permission_mode,
            model: spec.model,
            system_prompt: spec.system_prompt,
            env: spec.env,
        };
        if self.app_server_available() {
            // Connection-reuse path: share one app-server subprocess
            // across sessions on this executor.
            let connection = self.shared_app_server_connection(&config).await?;
            let native_id = Self::attach_thread(&connection, &config, None).await?;
            let transport = SessionTransport::AppServer(ConnectionThreadHandle {
                connection,
                thread_id: native_id.as_str().to_string(),
            });
            return self.spawn_session_state(config, native_id, transport).await;
        }

        self.spawn_session_state(
            config,
            Self::synthetic_exec_fallback_id(),
            SessionTransport::ExecFallback(ExecFallbackState {
                current_child: Arc::new(Mutex::new(None)),
                current_stdin: Arc::new(Mutex::new(None)),
            }),
        )
        .await
    }

    async fn resume_session(
        &self,
        session_id: NativeSessionId,
        hints: ResumeHints,
    ) -> Result<SessionRef, AgentExecutorError> {
        let config = self.load_persisted_config(&session_id, &hints).await;
        let resume_id = hints
            .vendor_cursor
            .clone()
            .unwrap_or_else(|| session_id.as_str().to_string());
        if self.app_server_available() {
            let connection = self.shared_app_server_connection(&config).await?;
            let native_id = Self::attach_thread(&connection, &config, Some(resume_id)).await?;
            let transport = SessionTransport::AppServer(ConnectionThreadHandle {
                connection,
                thread_id: native_id.as_str().to_string(),
            });
            return self.spawn_session_state(config, native_id, transport).await;
        }

        self.spawn_session_state(
            config,
            NativeSessionId::new(resume_id),
            SessionTransport::ExecFallback(ExecFallbackState {
                current_child: Arc::new(Mutex::new(None)),
                current_stdin: Arc::new(Mutex::new(None)),
            }),
        )
        .await
    }

    async fn send_message(
        &self,
        session: &SessionRef,
        message: UserMessage,
    ) -> Result<EventStream, AgentExecutorError> {
        let state = self.state_for(session).await?;
        let (transport, config) = self.prepare_transport_for_turn(&state).await?;
        match transport {
            SessionTransport::AppServer(transport) => {
                self.send_message_via_app_server(state.clone(), transport, config, message)
                    .await
            }
            SessionTransport::ExecFallback(transport) => {
                self.send_message_via_exec_fallback(state.clone(), transport, config, message)
                    .await
            }
        }
    }

    async fn respond_to_permission(
        &self,
        session: &SessionRef,
        request_id: String,
        decision: PermissionDecision,
    ) -> Result<(), AgentExecutorError> {
        let state = self.state_for(session).await?;
        let transport = {
            let runtime = state.runtime.lock().await;
            runtime.transport.clone()
        };
        match transport {
            SessionTransport::AppServer(handle) => {
                let thread_state = handle
                    .connection
                    .threads
                    .read()
                    .await
                    .get(&handle.thread_id)
                    .cloned()
                    .ok_or_else(|| {
                        AgentExecutorError::Protocol(format!(
                            "codex app-server thread {} is not registered",
                            handle.thread_id
                        ))
                    })?;
                let pending = thread_state
                    .pending_approvals
                    .lock()
                    .await
                    .remove(&request_id);
                let pending = pending.ok_or_else(|| {
                    AgentExecutorError::Protocol(format!(
                        "codex app-server permission request {request_id} is no longer pending"
                    ))
                })?;
                let request_id_num = request_id.parse::<u64>().map_err(|_| {
                    AgentExecutorError::Protocol(format!(
                        "codex app-server permission request id {request_id} is not numeric"
                    ))
                })?;
                handle
                    .connection
                    .call_response(
                        request_id_num,
                        app_server_permission_response(&pending.method, &pending.params, decision),
                    )
                    .await
            }
            SessionTransport::ExecFallback(_) => Err(AgentExecutorError::Unsupported {
                capability: "respond_to_permission".to_string(),
            }),
        }
    }

    async fn respond_to_elicitation(
        &self,
        session: &SessionRef,
        request_id: String,
        response: Value,
    ) -> Result<(), AgentExecutorError> {
        let state = self.state_for(session).await?;
        let transport = {
            let runtime = state.runtime.lock().await;
            runtime.transport.clone()
        };
        match transport {
            SessionTransport::AppServer(handle) => {
                let thread_state = handle
                    .connection
                    .threads
                    .read()
                    .await
                    .get(&handle.thread_id)
                    .cloned()
                    .ok_or_else(|| {
                        AgentExecutorError::Protocol(format!(
                            "codex app-server thread {} is not registered",
                            handle.thread_id
                        ))
                    })?;
                let pending = thread_state
                    .pending_approvals
                    .lock()
                    .await
                    .remove(&request_id);
                let pending = pending.ok_or_else(|| {
                    AgentExecutorError::Protocol(format!(
                        "codex app-server elicitation request {request_id} is no longer pending"
                    ))
                })?;
                let request_id_num = request_id.parse::<u64>().map_err(|_| {
                    AgentExecutorError::Protocol(format!(
                        "codex app-server elicitation request id {request_id} is not numeric"
                    ))
                })?;
                let result = if pending.method == "mcpServer/elicitation/request" {
                    codex_mcp_elicitation_result(&response)
                } else {
                    response
                };
                handle
                    .connection
                    .call_response(request_id_num, result)
                    .await
            }
            SessionTransport::ExecFallback(transport) => {
                let stdin_text =
                    codex_terminal_interaction_response_text(&response).ok_or_else(|| {
                        AgentExecutorError::Protocol(
                            "codex terminal interaction response missing accepted text content"
                                .to_string(),
                        )
                    })?;
                let mut guard = transport.current_stdin.lock().await;
                let stdin = guard.as_mut().ok_or_else(|| {
                    AgentExecutorError::Protocol(
                        "codex terminal interaction requested without a running stdin pipe"
                            .to_string(),
                    )
                })?;
                stdin
                    .write_all(stdin_text.as_bytes())
                    .await
                    .map_err(|e| AgentExecutorError::Io(e.to_string()))?;
                if !stdin_text.ends_with('\n') {
                    stdin
                        .write_all(b"\n")
                        .await
                        .map_err(|e| AgentExecutorError::Io(e.to_string()))?;
                }
                stdin
                    .flush()
                    .await
                    .map_err(|e| AgentExecutorError::Io(e.to_string()))?;
                Ok(())
            }
        }
    }

    async fn interrupt(&self, session: &SessionRef) -> Result<(), AgentExecutorError> {
        let state = self.state_for(session).await?;
        let transport = {
            let runtime = state.runtime.lock().await;
            runtime.transport.clone()
        };
        match transport {
            SessionTransport::AppServer(handle) => {
                let thread_state = handle
                    .connection
                    .threads
                    .read()
                    .await
                    .get(&handle.thread_id)
                    .cloned();
                let turn_id = match thread_state {
                    Some(s) => s.current_turn_id.lock().await.clone(),
                    None => None,
                };
                // Interrupt is thread+turn-scoped — issuing it on thread A
                // does not touch thread B on the same connection.
                if let Some(turn_id) = turn_id {
                    let _ = handle
                        .connection
                        .call_request(
                            "turn/interrupt",
                            json!({
                                "threadId": handle.thread_id,
                                "turnId": turn_id,
                            }),
                        )
                        .await?;
                }
            }
            SessionTransport::ExecFallback(transport) => {
                if let Some(mut stdin) = transport.current_stdin.lock().await.take() {
                    let _ = stdin.shutdown().await;
                }
                if let Some(mut child) = transport.current_child.lock().await.take() {
                    child
                        .kill()
                        .await
                        .map_err(|e| AgentExecutorError::Io(e.to_string()))?;
                }
            }
        }
        Ok(())
    }

    async fn close_session(&self, session: &SessionRef) -> Result<(), AgentExecutorError> {
        if let Some(state) = self.sessions.write().await.remove(&session.process_handle) {
            let transport = {
                let runtime = state.runtime.lock().await;
                runtime.transport.clone()
            };
            Self::close_transport(transport).await?;
        }
        Ok(())
    }

    async fn set_permission_mode(
        &self,
        session: &SessionRef,
        mode: PermissionMode,
    ) -> Result<(), AgentExecutorError> {
        let state = self.state_for(session).await?;
        let (desired_config, transport, transport_config) = {
            let mut runtime = state.runtime.lock().await;
            runtime.desired_config.permission_mode = mode;
            (
                runtime.desired_config.clone(),
                runtime.transport.clone(),
                runtime.transport_config.clone(),
            )
        };
        let native_id = state.native_id.lock().await.clone();
        self.persist_session_record(&native_id, &desired_config)
            .await?;

        if matches!(transport, SessionTransport::AppServer(_)) && desired_config != transport_config
        {
            if let Err(error) = self.prepare_transport_for_turn(&state).await {
                match error {
                    AgentExecutorError::Protocol(message)
                        if message.contains("busy") || message.contains("current turn") => {}
                    other => return Err(other),
                }
            }
        }
        Ok(())
    }

    async fn set_model(
        &self,
        session: &SessionRef,
        model: ModelSpec,
    ) -> Result<ModelChangeOutcome, AgentExecutorError> {
        let state = self.state_for(session).await?;
        let (desired_config, transport, transport_config) = {
            let mut runtime = state.runtime.lock().await;
            runtime.desired_config.model = Some(model);
            (
                runtime.desired_config.clone(),
                runtime.transport.clone(),
                runtime.transport_config.clone(),
            )
        };
        let native_id = state.native_id.lock().await.clone();
        self.persist_session_record(&native_id, &desired_config)
            .await?;

        if matches!(transport, SessionTransport::AppServer(_)) && desired_config != transport_config
        {
            if let Err(error) = self.prepare_transport_for_turn(&state).await {
                match error {
                    AgentExecutorError::Protocol(message)
                        if message.contains("busy") || message.contains("current turn") => {}
                    other => return Err(other),
                }
            }
        }

        Ok(ModelChangeOutcome::Applied)
    }

    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, AgentExecutorError> {
        self.session_store
            .list_sessions(VENDOR, filter)
            .await
            .map_err(|e| AgentExecutorError::Vendor {
                vendor: VENDOR,
                message: e,
            })
    }

    async fn get_session_info(
        &self,
        session_id: &NativeSessionId,
    ) -> Result<SessionInfo, AgentExecutorError> {
        self.session_store
            .get_session_info(VENDOR, session_id)
            .await
            .map_err(|e| AgentExecutorError::Vendor {
                vendor: VENDOR,
                message: e,
            })
    }

    async fn get_session_messages(
        &self,
        session_id: &NativeSessionId,
        pagination: Pagination,
    ) -> Result<Vec<NativeMessage>, AgentExecutorError> {
        self.session_store
            .get_session_messages(VENDOR, session_id, pagination)
            .await
            .map_err(|e| AgentExecutorError::Vendor {
                vendor: VENDOR,
                message: e,
            })
    }

    // -------------------------------------------------------------------
    // Connection-reuse seam (Phase 1 pre-connection refactor).
    //
    // Hazards to remember when extending these:
    //   * `probe = true` short-circuits without spawning a subprocess. A
    //     callers wanting a real handshake must set `probe = false`.
    //   * Killing the app-server subprocess (or receiving EOF) cascades a
    //     `ConnectionClosed` frame to every live thread. Callers in the
    //     middle of a turn see `AgentExecutorError::Protocol(..)`.
    //   * `check_connection` is intentionally lenient: it only reports
    //     `Dead` when the child process has exited. The registry recovers
    //     from stale-but-alive transports by surfacing the stream error
    //     on the next `send_message`.
    // -------------------------------------------------------------------

    async fn open_connection(
        &self,
        spec: ConnectionSpec,
    ) -> Result<ConnectionHandle, AgentExecutorError> {
        if !self.app_server_available() {
            return Err(AgentExecutorError::Unsupported {
                capability: "open_connection".to_string(),
            });
        }

        if spec.probe {
            // Caller only wants to know the binary is installed. We have
            // already confirmed that via `probe_app_server` during the
            // capability check — return a sentinel handle backed by an
            // empty, never-used state so the registry can count it.
            let (_tx, _rx) = mpsc::unbounded_channel::<ThreadFrame>();
            // Empty probe-only connection; `start_session_on` on this
            // handle will fail loudly (child.try_wait == Ok(Some)).
            let connection = self.dial_connection(&spec.env).await?;
            // Immediately tear it down — we only wanted to prove spawn
            // + handshake succeed.
            connection.shutdown().await;
            return Ok(ConnectionHandle {
                id: ConnectionHandleId::new(),
                vendor: VENDOR,
                inner: connection as Arc<dyn std::any::Any + Send + Sync>,
            });
        }

        let connection = self.dial_connection(&spec.env).await?;
        Ok(ConnectionHandle {
            id: ConnectionHandleId::new(),
            vendor: VENDOR,
            inner: connection as Arc<dyn std::any::Any + Send + Sync>,
        })
    }

    async fn close_connection(&self, handle: ConnectionHandle) -> Result<(), AgentExecutorError> {
        let connection = handle
            .inner
            .downcast::<CodexAppServerConnection>()
            .map_err(|_| {
                AgentExecutorError::Protocol(
                    "close_connection: ConnectionHandle.inner not a CodexAppServerConnection"
                        .into(),
                )
            })?;
        connection.shutdown().await;

        // If this was the executor-held shared connection, clear it so
        // subsequent spawns redial.
        let mut guard = self.shared_connection.lock().await;
        if let Some((id, existing)) = guard.as_ref() {
            if *id == handle.id || Arc::ptr_eq(existing, &connection) {
                *guard = None;
            }
        }
        Ok(())
    }

    async fn check_connection(
        &self,
        handle: &ConnectionHandle,
    ) -> Result<ConnectionHealth, AgentExecutorError> {
        let connection = handle
            .inner
            .clone()
            .downcast::<CodexAppServerConnection>()
            .map_err(|_| {
                AgentExecutorError::Protocol(
                    "check_connection: ConnectionHandle.inner not a CodexAppServerConnection"
                        .into(),
                )
            })?;

        if connection.closed.load(Ordering::SeqCst) {
            return Ok(ConnectionHealth::Dead {
                reason: "connection shut down".to_string(),
            });
        }

        let mut child = connection.child.lock().await;
        match child.try_wait() {
            Ok(Some(status)) => Ok(ConnectionHealth::Dead {
                reason: format!("app-server exited (code={:?})", status.code()),
            }),
            Ok(None) => Ok(ConnectionHealth::Healthy),
            Err(error) => Ok(ConnectionHealth::Dead {
                reason: format!("app-server child error: {error}"),
            }),
        }
    }

    async fn start_session_on(
        &self,
        handle: &ConnectionHandle,
        spec: SpawnSessionSpec,
    ) -> Result<SessionRef, AgentExecutorError> {
        let connection = handle
            .inner
            .clone()
            .downcast::<CodexAppServerConnection>()
            .map_err(|_| {
                AgentExecutorError::Protocol(
                    "start_session_on: ConnectionHandle.inner not a CodexAppServerConnection"
                        .into(),
                )
            })?;

        if connection.closed.load(Ordering::SeqCst) {
            return Err(AgentExecutorError::Protocol(
                "codex app-server connection is closed; reopen before starting a session".into(),
            ));
        }

        let config = SessionConfig {
            workdir: spec.workdir,
            additional_directories: spec.additional_directories,
            permission_mode: spec.permission_mode,
            model: spec.model,
            system_prompt: spec.system_prompt,
            env: spec.env,
        };

        let native_id = Self::attach_thread(&connection, &config, None).await?;
        let transport = SessionTransport::AppServer(ConnectionThreadHandle {
            connection,
            thread_id: native_id.as_str().to_string(),
        });
        self.spawn_session_state(config, native_id, transport).await
    }
}

struct AppServerNotificationOutcome {
    events: Vec<ExecutorEvent>,
    done: bool,
}

fn probe_app_server(codex_path: &PathBuf) -> bool {
    std::process::Command::new(codex_path)
        .arg("app-server")
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Demultiplexer task.
///
/// Reads one JSON-RPC frame per line from the `codex app-server` stdout
/// and routes it:
///   * Response with matching `id` → `pending_requests[id]` oneshot.
///   * Server→client request carrying `threadId` → that thread's frame
///     channel (so the per-turn runner can register a
///     `PermissionRequest` event and store the approval in the thread's
///     `pending_approvals`).
///   * Notification with `threadId` → same thread channel.
///   * Frames without a threadId are connection-level. We currently forward
///     `account/rateLimits/*` notifications to all live threads so per-turn
///     streams can surface quota usage, and drop the rest.
///
/// On EOF / read error, all pending oneshots fail with `Protocol` and
/// every thread receives a `ConnectionClosed` frame.
async fn run_app_server_demux(
    mut stdout: BufReader<ChildStdout>,
    pending_requests: Arc<Mutex<HashMap<u64, PendingRequest>>>,
    threads: Arc<RwLock<HashMap<String, Arc<ThreadState>>>>,
    last_frame_seen: Arc<RwLock<Instant>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
    stdin: Arc<Mutex<ChildStdin>>,
) {
    let close_reason: String;
    loop {
        let mut line = String::new();
        match stdout.read_line(&mut line).await {
            Ok(0) => {
                close_reason = "codex app-server stdout EOF".to_string();
                break;
            }
            Ok(_) => {}
            Err(error) => {
                close_reason = format!("codex app-server stdout error: {error}");
                break;
            }
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        *last_frame_seen.write().await = Instant::now();

        let payload: Value = match serde_json::from_str(trimmed) {
            Ok(p) => p,
            Err(_) => {
                // Non-JSON line — skip silently (matches existing adapter
                // behavior that drops non-JSON stdout lines).
                continue;
            }
        };

        let Some(object) = payload.as_object() else {
            continue;
        };

        // Response to a client-initiated request?
        if let Some(response_id) = object.get("id").and_then(Value::as_u64) {
            if object.contains_key("result") || object.contains_key("error") {
                // `method` is absent on a pure response, present on a
                // server-initiated request.
                if !object.contains_key("method") {
                    if let Some(pending) = pending_requests.lock().await.remove(&response_id) {
                        let outcome = if let Some(error) = object.get("error") {
                            Err(app_server_response_error(&pending.method, error))
                        } else {
                            Ok(object.get("result").cloned().unwrap_or(Value::Null))
                        };
                        let _ = pending.tx.send(outcome);
                    }
                    continue;
                }
            }

            // Server→client request targeting a thread.
            if let Some(method) = object.get("method").and_then(Value::as_str) {
                let params = object.get("params").cloned().unwrap_or(Value::Null);
                let thread_id = frame_thread_id(&params);

                if let Some(thread_id) = thread_id {
                    if let Some(state) = threads.read().await.get(&thread_id).cloned() {
                        let _ = state.frames_tx.send(ThreadFrame::ServerRequest {
                            id: response_id,
                            method: method.to_string(),
                            params,
                        });
                        continue;
                    }
                }

                // No target thread — connection-level server→client
                // request. Real-world example:
                // `account/chatgptAuthTokens/refresh` fires when codex is
                // in external-auth mode and sees a 401 mid-turn. The
                // server waits `EXTERNAL_AUTH_REFRESH_TIMEOUT` (10s) for
                // our response before surfacing an error on the
                // underlying request; prior to this patch we silently
                // dropped the frame, which blocked the caller's turn for
                // those 10s every time the server reached out.
                //
                // Verified behavior (manual JSON-RPC drive against
                // codex-cli 0.120.0 app-server): this request never
                // fires in the default managed-auth / api-key flow, so
                // the ack below is strictly defensive — but it avoids a
                // latent 10s stall for users who flip on external auth.
                let send_result = {
                    let mut stdin = stdin.lock().await;
                    app_server_write_response(
                        &mut stdin,
                        response_id,
                        // Empty object; server-side tolerant of
                        // unknown-shape results when its own handler
                        // rejects it. We would otherwise need per-method
                        // response schemas that we do not own.
                        json!({}),
                    )
                    .await
                };
                if let Err(err) = send_result {
                    eprintln!("codex app-server connection-level {method} ack failed: {err}");
                }
                continue;
            }
        }

        // Notifications.
        if let Some(method) = object.get("method").and_then(Value::as_str) {
            let params = object.get("params").cloned().unwrap_or(Value::Null);
            let thread_id = frame_thread_id(&params);

            if let Some(thread_id) = thread_id {
                if let Some(state) = threads.read().await.get(&thread_id).cloned() {
                    let _ = state.frames_tx.send(ThreadFrame::Notification {
                        method: method.to_string(),
                        params,
                    });
                }
                continue;
            }
            // Connection-level notifications are mostly informational and do
            // not belong to a specific thread. Forward account/rateLimits/*
            // to all registered threads so active turns can surface quota
            // updates; drop everything else.
            if method.starts_with("account/rateLimits/") {
                let snapshot = threads.read().await.clone();
                for state in snapshot.values() {
                    let _ = state.frames_tx.send(ThreadFrame::Notification {
                        method: method.to_string(),
                        params: params.clone(),
                    });
                }
            }
            continue;
        }
    }

    closed.store(true, Ordering::SeqCst);

    // Drain pending requests with an error.
    let mut pending = pending_requests.lock().await;
    for (_, request) in pending.drain() {
        let _ = request
            .tx
            .send(Err(AgentExecutorError::Protocol(close_reason.clone())));
    }

    // Notify every registered thread.
    let threads_snapshot = threads.read().await.clone();
    for state in threads_snapshot.values() {
        let _ = state.frames_tx.send(ThreadFrame::ConnectionClosed {
            reason: close_reason.clone(),
        });
    }
}

/// Extract the thread id for routing. Primary key is `params.threadId`;
/// for `thread/started` the id is nested under `params.thread.id`.
fn frame_thread_id(params: &Value) -> Option<String> {
    if let Some(id) = params.get("threadId").and_then(Value::as_str) {
        return Some(id.to_string());
    }
    if let Some(id) = params
        .get("thread")
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str)
    {
        return Some(id.to_string());
    }
    None
}

async fn app_server_write_request(
    stdin: &mut ChildStdin,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), AgentExecutorError> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    stdin
        .write_all(payload.to_string().as_bytes())
        .await
        .map_err(|e| AgentExecutorError::Io(e.to_string()))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|e| AgentExecutorError::Io(e.to_string()))?;
    stdin
        .flush()
        .await
        .map_err(|e| AgentExecutorError::Io(e.to_string()))
}

async fn app_server_write_notification(
    stdin: &mut ChildStdin,
    method: &str,
    params: Value,
) -> Result<(), AgentExecutorError> {
    let payload = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    stdin
        .write_all(payload.to_string().as_bytes())
        .await
        .map_err(|e| AgentExecutorError::Io(e.to_string()))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|e| AgentExecutorError::Io(e.to_string()))?;
    stdin
        .flush()
        .await
        .map_err(|e| AgentExecutorError::Io(e.to_string()))
}

async fn app_server_write_response(
    stdin: &mut ChildStdin,
    id: u64,
    result: Value,
) -> Result<(), AgentExecutorError> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    });
    stdin
        .write_all(payload.to_string().as_bytes())
        .await
        .map_err(|e| AgentExecutorError::Io(e.to_string()))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|e| AgentExecutorError::Io(e.to_string()))?;
    stdin
        .flush()
        .await
        .map_err(|e| AgentExecutorError::Io(e.to_string()))
}

async fn app_server_wait_for_response(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    expected_id: u64,
) -> Result<Value, AgentExecutorError> {
    loop {
        let mut line = String::new();
        let bytes_read = stdout
            .read_line(&mut line)
            .await
            .map_err(|e| AgentExecutorError::Io(e.to_string()))?;
        if bytes_read == 0 {
            return Err(AgentExecutorError::Protocol(
                "codex app-server stdout closed during handshake".to_string(),
            ));
        }

        let payload: Value = match serde_json::from_str(line.trim()) {
            Ok(payload) => payload,
            Err(_) => continue,
        };
        let Some(object) = payload.as_object() else {
            continue;
        };

        if object
            .get("id")
            .and_then(Value::as_u64)
            .is_some_and(|id| id == expected_id)
        {
            if let Some(error) = object.get("error") {
                return Err(app_server_response_error("handshake", error));
            }
            if let Some(result) = object.get("result") {
                return Ok(result.clone());
            }
        }

        if let (Some(id), Some(method)) = (
            object.get("id").and_then(Value::as_u64),
            object.get("method").and_then(Value::as_str),
        ) {
            let result = match method {
                "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                    json!({ "decision": "decline" })
                }
                "item/permissions/requestApproval" => json!({ "permissions": {} }),
                "mcpServer/elicitation/request" => json!({
                    "action": "decline",
                    "content": Value::Null,
                    "_meta": Value::Null,
                }),
                _ => json!({}),
            };
            app_server_write_response(stdin, id, result).await?;
        }
    }
}

fn app_server_thread_id(result: &Value) -> Option<String> {
    result
        .get("thread")
        .and_then(Value::as_object)
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn app_server_turn_id_from_response(result: Option<&Value>) -> Option<String> {
    result
        .and_then(|result| result.get("turn"))
        .and_then(Value::as_object)
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn app_server_response_error(method: &str, error: &Value) -> AgentExecutorError {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown codex app-server error");
    let code = error
        .get("code")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    AgentExecutorError::Vendor {
        vendor: VENDOR,
        message: format!("{method}: {message} (code={code})"),
    }
}

fn sandbox_policy_json(mode: &str, workdir: &PathBuf, additional_directories: &[PathBuf]) -> Value {
    match mode {
        "read-only" => json!({ "type": "readOnly" }),
        "danger-full-access" => json!({ "type": "dangerFullAccess" }),
        "workspace-write" => {
            let mut writable_roots = vec![workdir.display().to_string()];
            writable_roots.extend(
                additional_directories
                    .iter()
                    .map(|path| path.display().to_string()),
            );
            if writable_roots.len() > 1 {
                json!({
                    "type": "workspaceWrite",
                    "writableRoots": writable_roots,
                })
            } else {
                json!({ "type": "workspaceWrite" })
            }
        }
        _ => json!({ "type": "workspaceWrite" }),
    }
}

fn app_server_permission_response(
    method: &str,
    params: &Value,
    decision: PermissionDecision,
) -> Value {
    match method {
        "item/permissions/requestApproval" => match decision {
            // SelectedOption is opaque to codex's approval model; map it to
            // Allow. Codex's frontend never surfaces vendor option lists
            // through this adapter today.
            PermissionDecision::Allow | PermissionDecision::SelectedOption { .. } => json!({
                "permissions": params.get("permissions").cloned().unwrap_or_else(|| json!({})),
                "scope": "turn",
            }),
            PermissionDecision::Deny | PermissionDecision::Abort => json!({
                "permissions": {},
                "scope": "turn",
            }),
        },
        "mcpServer/elicitation/request" => {
            let action = match decision {
                PermissionDecision::Allow | PermissionDecision::SelectedOption { .. } => "accept",
                PermissionDecision::Deny => "decline",
                PermissionDecision::Abort => "cancel",
            };
            json!({
                "action": action,
                "content": Value::Null,
                "_meta": Value::Null,
            })
        }
        _ => {
            let decision = match decision {
                PermissionDecision::Allow | PermissionDecision::SelectedOption { .. } => "accept",
                PermissionDecision::Deny => "decline",
                PermissionDecision::Abort => "cancel",
            };
            json!({ "decision": decision })
        }
    }
}

async fn handle_app_server_request(
    id: u64,
    method: &str,
    params: &Value,
    pending_requests: &Mutex<HashMap<String, AppServerPendingRequest>>,
) -> Result<Option<ExecutorEvent>, AgentExecutorError> {
    let request_id = id.to_string();
    match method {
        "item/commandExecution/requestApproval" => {
            pending_requests.lock().await.insert(
                request_id.clone(),
                AppServerPendingRequest {
                    method: method.to_string(),
                    params: params.clone(),
                },
            );
            Ok(Some(ExecutorEvent::PermissionRequest {
                request_id,
                tool_name: "Bash".to_string(),
                tool_input: json!({
                    "command": params.get("command").cloned().unwrap_or(Value::Null),
                    "cwd": params.get("cwd").cloned().unwrap_or(Value::Null),
                    "reason": params.get("reason").cloned().unwrap_or(Value::Null),
                }),
            }))
        }
        "item/fileChange/requestApproval" => {
            pending_requests.lock().await.insert(
                request_id.clone(),
                AppServerPendingRequest {
                    method: method.to_string(),
                    params: params.clone(),
                },
            );
            Ok(Some(ExecutorEvent::PermissionRequest {
                request_id,
                tool_name: "CodexPatch".to_string(),
                tool_input: json!({
                    "fileChanges": params.get("fileChanges").cloned().unwrap_or(Value::Null),
                    "reason": params.get("reason").cloned().unwrap_or(Value::Null),
                }),
            }))
        }
        "item/permissions/requestApproval" => {
            pending_requests.lock().await.insert(
                request_id.clone(),
                AppServerPendingRequest {
                    method: method.to_string(),
                    params: params.clone(),
                },
            );
            Ok(Some(ExecutorEvent::PermissionRequest {
                request_id,
                tool_name: "CodexPermissions".to_string(),
                tool_input: json!({
                    "permissions": params.get("permissions").cloned().unwrap_or_else(|| json!({})),
                    "reason": params.get("reason").cloned().unwrap_or(Value::Null),
                    "threadId": params.get("threadId").cloned().unwrap_or(Value::Null),
                    "turnId": params.get("turnId").cloned().unwrap_or(Value::Null),
                }),
            }))
        }
        "mcpServer/elicitation/request" => {
            pending_requests.lock().await.insert(
                request_id.clone(),
                AppServerPendingRequest {
                    method: method.to_string(),
                    params: params.clone(),
                },
            );
            let meta = params.get("_meta").and_then(Value::as_object);
            Ok(Some(ExecutorEvent::PermissionRequest {
                request_id,
                tool_name: "McpTool".to_string(),
                tool_input: json!({
                    "message": params.get("message").cloned().unwrap_or(Value::Null),
                    "description": meta.and_then(|m| m.get("tool_description")).cloned().unwrap_or(Value::Null),
                    "params": meta.and_then(|m| m.get("tool_params")).cloned().unwrap_or(Value::Null),
                    "persist": meta.and_then(|m| m.get("persist")).cloned().unwrap_or(Value::Null),
                }),
            }))
        }
        _ => Ok(Some(ExecutorEvent::NativeEvent {
            provider: Cow::Borrowed(VENDOR),
            payload: json!({
                "request_id": request_id,
                "method": method,
                "params": params,
            }),
        })),
    }
}

async fn handle_app_server_notification(
    method: &str,
    params: &Value,
    current_model: &str,
    current_turn_id: &Arc<Mutex<Option<String>>>,
    final_text: &mut Option<String>,
    aggregate_usage: &mut TokenUsage,
    iteration_count: &mut u32,
    active_turn_plan_tool_use_id: &mut Option<String>,
    plan_text_by_tool_use_id: &mut HashMap<String, String>,
    command_output_by_tool_use_id: &mut HashMap<String, CommandOutputBuffers>,
) -> Result<AppServerNotificationOutcome, AgentExecutorError> {
    let mut outcome = AppServerNotificationOutcome {
        events: Vec::new(),
        done: false,
    };

    if let Some(notification_turn_id) = app_server_notification_turn_id(method, params) {
        let active_turn_id = current_turn_id.lock().await.clone();
        match active_turn_id {
            Some(active_turn_id) if active_turn_id != notification_turn_id => return Ok(outcome),
            None if method != "turn/started" => return Ok(outcome),
            _ => {}
        }
    }

    match method {
        "turn/started" => {
            *iteration_count = iteration_count.saturating_add(1);
            *active_turn_plan_tool_use_id = None;
            plan_text_by_tool_use_id.clear();
            command_output_by_tool_use_id.clear();
            *current_turn_id.lock().await = params
                .get("turn")
                .or_else(|| params.get("turnId"))
                .and_then(|value| {
                    value
                        .get("id")
                        .and_then(Value::as_str)
                        .or_else(|| value.as_str())
                })
                .map(ToOwned::to_owned);
        }
        "thread/tokenUsage/updated" => {
            if let Some(token_usage) = params.get("tokenUsage") {
                update_usage_from_object(token_usage, aggregate_usage);
                outcome
                    .events
                    .push(ExecutorEvent::UsageUpdate(aggregate_usage.clone()));
                if let Some(event) = codex_context_usage_native_event(token_usage) {
                    outcome.events.push(event);
                }
            }
        }
        "turn/completed" => {
            if let Some(tool_use_id) = active_turn_plan_tool_use_id.take() {
                outcome.events.push(codex_plan_tool_result(tool_use_id));
            }
            if let Some(turn) = params.get("turn") {
                update_usage_from_object(turn, aggregate_usage);
            }
            *current_turn_id.lock().await = None;
            let status = params
                .get("turn")
                .and_then(|turn| turn.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("completed");
            if matches!(status, "cancelled" | "canceled" | "aborted" | "interrupted") {
                outcome.events.push(ExecutorEvent::Error {
                    message: format!("codex turn ended with status {status}"),
                    recoverable: true,
                });
            } else {
                outcome
                    .events
                    .push(ExecutorEvent::UsageUpdate(aggregate_usage.clone()));
                outcome.events.push(ExecutorEvent::TurnComplete {
                    final_text: final_text.clone(),
                    iteration_count: *iteration_count,
                    usage: aggregate_usage.clone(),
                });
            }
            outcome.done = true;
        }
        "thread/status/changed" => {
            let is_idle = params
                .get("status")
                .and_then(|status| status.get("type"))
                .and_then(Value::as_str)
                == Some("idle");
            if is_idle && current_turn_id.lock().await.is_some() {
                let has_turn_activity = *iteration_count > 0
                    || final_text
                        .as_ref()
                        .map(|text| !text.is_empty())
                        .unwrap_or(false)
                    || usage_has_activity(aggregate_usage);
                if !has_turn_activity {
                    // Guard against stale `thread/status=idle` notifications
                    // from the previous turn arriving before the current turn
                    // has emitted any activity.
                    return Ok(outcome);
                }
                if let Some(tool_use_id) = active_turn_plan_tool_use_id.take() {
                    outcome.events.push(codex_plan_tool_result(tool_use_id));
                }
                *current_turn_id.lock().await = None;
                outcome
                    .events
                    .push(ExecutorEvent::UsageUpdate(aggregate_usage.clone()));
                outcome.events.push(ExecutorEvent::TurnComplete {
                    final_text: final_text.clone(),
                    iteration_count: *iteration_count,
                    usage: aggregate_usage.clone(),
                });
                outcome.done = true;
            }
        }
        "item/started" | "item/updated" => {
            if let Some(item) = params.get("item") {
                if let Ok(item) = serde_json::from_value::<CodexItem>(item.clone()) {
                    if let Some(event) = translate_item(&item, current_model) {
                        outcome.events.push(event);
                    }
                    if let Some(event) = translate_reasoning_summary(&item) {
                        outcome.events.push(event);
                    }
                } else {
                    outcome.events.push(ExecutorEvent::NativeEvent {
                        provider: Cow::Borrowed(VENDOR),
                        payload: params.clone(),
                    });
                }
            }
        }
        "item/completed" => {
            if let Some(item) = params.get("item") {
                if let Ok(item) = serde_json::from_value::<CodexItem>(item.clone()) {
                    if let CodexItem::AgentMessage { text, .. } = &item {
                        *final_text = Some(text.clone());
                        outcome.events.push(ExecutorEvent::StreamDelta {
                            kind: DeltaKind::Text,
                            content: text.clone(),
                        });
                    }
                    if let Some(event) = translate_reasoning_summary(&item) {
                        outcome.events.push(event);
                    }
                    outcome.events.extend(translate_item_completed(&item));
                } else {
                    outcome.events.push(ExecutorEvent::NativeEvent {
                        provider: Cow::Borrowed(VENDOR),
                        payload: params.clone(),
                    });
                }
            }
        }
        "item/agentMessage/delta" => {
            // Real-time text streaming delta from Codex app-server v2.
            // params: { threadId, turnId, itemId, delta: "text chunk" }
            if let Some(delta_text) = params.get("delta").and_then(Value::as_str) {
                if !delta_text.is_empty() {
                    let snapshot = final_text.clone().unwrap_or_default();
                    *final_text = Some(format!("{snapshot}{delta_text}"));
                    outcome.events.push(ExecutorEvent::StreamDelta {
                        kind: DeltaKind::Text,
                        content: delta_text.to_string(),
                    });
                }
            }
        }
        "item/agentReasoning/delta"
        | "item/reasoningSummaryText/delta"
        | "item/reasoning/summaryTextDelta" => {
            // Thinking/reasoning streaming delta
            if let Some(delta_text) = params.get("delta").and_then(Value::as_str) {
                if !delta_text.is_empty() {
                    outcome.events.push(ExecutorEvent::StreamDelta {
                        kind: DeltaKind::Thinking,
                        content: delta_text.to_string(),
                    });
                }
            }
        }
        "turn/plan/updated" | "turn.plan.updated" => {
            if let Ok(update) = serde_json::from_value::<CodexTurnPlanUpdate>(params.clone()) {
                let tool_use_id = codex_turn_plan_tool_use_id(update.id(), *iteration_count);
                *active_turn_plan_tool_use_id = Some(tool_use_id.clone());
                outcome
                    .events
                    .push(translate_turn_plan_update(&tool_use_id, &update));
            }
        }
        "item/mcpToolCall/progress"
        | "item.mcpToolCall.progress"
        | "item.mcp_tool_call.progress"
        | "item/mcp_tool_call/progress"
        | "mcpToolCall/progress"
        | "mcpToolCall.progress"
        | "mcp_tool_call/progress"
        | "mcp_tool_call.progress" => {
            if let Ok(progress) = serde_json::from_value::<CodexMcpToolCallProgress>(params.clone())
            {
                if let Some(event) = translate_mcp_tool_call_progress(&progress) {
                    outcome.events.push(event);
                } else {
                    outcome.events.push(ExecutorEvent::NativeEvent {
                        provider: Cow::Borrowed(VENDOR),
                        payload: progress.raw_payload(),
                    });
                }
            }
        }
        "command/exec/outputDelta"
        | "command.exec.outputDelta"
        | "command/exec/output_delta"
        | "command.exec.output_delta" => {
            if let Ok(delta) = serde_json::from_value::<CodexCommandExecOutputDelta>(params.clone())
            {
                if let Some(event) =
                    translate_command_exec_output_delta(&delta, command_output_by_tool_use_id)
                {
                    outcome.events.push(event);
                } else {
                    outcome.events.push(ExecutorEvent::NativeEvent {
                        provider: Cow::Borrowed(VENDOR),
                        payload: delta.raw_payload(),
                    });
                }
            }
        }
        "item/plan/delta" | "item.plan.delta" | "plan/delta" | "plan.delta" => {
            if let Ok(delta) = serde_json::from_value::<CodexPlanDelta>(params.clone()) {
                if let Some(event) = translate_plan_delta(&delta, plan_text_by_tool_use_id) {
                    outcome.events.push(event);
                } else {
                    outcome.events.push(ExecutorEvent::NativeEvent {
                        provider: Cow::Borrowed(VENDOR),
                        payload: delta.raw_payload(),
                    });
                }
            }
        }
        "codex/event" | "codex/event/task_complete" | "codex/event/turn_aborted" => {
            if let Some(msg) = params.get("msg") {
                let msg_type = msg.get("type").and_then(Value::as_str).unwrap_or("");
                match msg_type {
                    "task_started" => {
                        *iteration_count = iteration_count.saturating_add(1);
                    }
                    "agent_message" => {
                        if let Some(text) = msg.get("message").and_then(Value::as_str) {
                            *final_text = Some(text.to_string());
                            outcome.events.push(ExecutorEvent::StreamDelta {
                                kind: DeltaKind::Text,
                                content: text.to_string(),
                            });
                        }
                    }
                    "token_count" => {
                        update_usage_from_object(msg, aggregate_usage);
                        outcome
                            .events
                            .push(ExecutorEvent::UsageUpdate(aggregate_usage.clone()));
                    }
                    "task_complete" => {
                        outcome
                            .events
                            .push(ExecutorEvent::UsageUpdate(aggregate_usage.clone()));
                        outcome.events.push(ExecutorEvent::TurnComplete {
                            final_text: final_text.clone(),
                            iteration_count: *iteration_count,
                            usage: aggregate_usage.clone(),
                        });
                        *current_turn_id.lock().await = None;
                        outcome.done = true;
                    }
                    "turn_aborted" => {
                        outcome.events.push(ExecutorEvent::Error {
                            message: "codex turn aborted".to_string(),
                            recoverable: true,
                        });
                        *current_turn_id.lock().await = None;
                        outcome.done = true;
                    }
                    _ => outcome.events.push(ExecutorEvent::NativeEvent {
                        provider: Cow::Borrowed(VENDOR),
                        payload: msg.clone(),
                    }),
                }
            }
        }
        "account/rateLimits/updated" => {
            // Rate-limit snapshots now flow through the machine-level
            // `cteno-host-usage-monitor` (polls `account/rateLimits/read`
            // against a dedicated codex-app-server). Drop the per-session
            // notification to avoid duplicating that data.
        }
        "error" => {
            let base_message = params
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("codex error (details unavailable)")
                .to_string();
            let additional_details = params
                .get("error")
                .and_then(|error| error.get("additionalDetails"))
                .and_then(Value::as_str)
                .filter(|details| *details != base_message);
            let message = if let Some(details) = additional_details {
                format!("{base_message}: {details}")
            } else {
                base_message
            };
            let will_retry = params
                .get("willRetry")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            outcome.events.push(ExecutorEvent::Error {
                message,
                recoverable: will_retry,
            });
            outcome.events.push(ExecutorEvent::NativeEvent {
                provider: Cow::Borrowed(VENDOR),
                payload: json!({
                    "method": method,
                    "params": params,
                }),
            });
        }
        _ => outcome.events.push(ExecutorEvent::NativeEvent {
            provider: Cow::Borrowed(VENDOR),
            payload: json!({
                "method": method,
                "params": params,
            }),
        }),
    }

    Ok(outcome)
}

fn usage_has_activity(usage: &TokenUsage) -> bool {
    usage.input_tokens > 0
        || usage.output_tokens > 0
        || usage.cache_creation_tokens > 0
        || usage.cache_read_tokens > 0
        || usage.reasoning_tokens > 0
}

fn app_server_notification_turn_id(method: &str, params: &Value) -> Option<String> {
    let turn_id = params
        .get("turn")
        .and_then(Value::as_object)
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .or_else(|| params.get("turnId").and_then(Value::as_str))
        .map(ToOwned::to_owned);

    match method {
        "turn/started"
        | "turn/completed"
        | "item/started"
        | "item/updated"
        | "item/completed"
        | "item/agentMessage/delta"
        | "item/agentReasoning/delta"
        | "item/reasoningSummaryText/delta"
        | "item/reasoning/summaryTextDelta" => turn_id,
        _ => None,
    }
}

fn update_usage_from_object(value: &Value, usage: &mut TokenUsage) {
    let Some(object) = value.as_object() else {
        return;
    };

    for candidate in usage_objects(object) {
        if let Some(v) = json_u64(candidate, &["input_tokens", "inputTokens"]) {
            usage.input_tokens = v;
        }
        if let Some(v) = json_u64(candidate, &["output_tokens", "outputTokens"]) {
            usage.output_tokens = v;
        }
        if let Some(v) = json_u64(
            candidate,
            &[
                "cache_creation_tokens",
                "cacheCreationTokens",
                "cache_creation_input_tokens",
                "cacheCreationInputTokens",
            ],
        ) {
            usage.cache_creation_tokens = v;
        }
        if let Some(v) = json_u64(
            candidate,
            &[
                "cache_read_tokens",
                "cacheReadTokens",
                "cache_read_input_tokens",
                "cacheReadInputTokens",
                "cached_input_tokens",
                "cachedInputTokens",
            ],
        ) {
            usage.cache_read_tokens = v;
        }
        if let Some(v) = json_u64(
            candidate,
            &[
                "reasoning_tokens",
                "reasoningTokens",
                "reasoning_output_tokens",
                "reasoningOutputTokens",
            ],
        ) {
            usage.reasoning_tokens = v;
        }
    }
}

fn codex_context_usage_native_event(token_usage: &Value) -> Option<ExecutorEvent> {
    let object = token_usage.as_object()?;
    let context_window =
        positive_json_u64(object, &["modelContextWindow", "model_context_window"])?;
    let usage = object
        .get("last")
        .or_else(|| object.get("lastTokenUsage"))
        .or_else(|| object.get("last_token_usage"))
        .or_else(|| object.get("total"))
        .or_else(|| object.get("totalTokenUsage"))
        .or_else(|| object.get("total_token_usage"))?;
    let usage_object = usage.as_object()?;
    let total_tokens = positive_json_u64(usage_object, &["totalTokens", "total_tokens"])?;

    Some(ExecutorEvent::NativeEvent {
        provider: Cow::Borrowed(VENDOR),
        payload: json!({
            "kind": "context_usage",
            "total_tokens": total_tokens,
            "max_tokens": context_window,
            "raw_max_tokens": context_window,
        }),
    })
}

fn usage_objects<'a>(object: &'a Map<String, Value>) -> Vec<&'a Map<String, Value>> {
    let mut candidates = vec![object];
    for key in ["total", "totalTokenUsage", "total_token_usage"] {
        if let Some(nested) = object.get(key).and_then(Value::as_object) {
            candidates.push(nested);
        }
    }
    candidates
}

fn json_u64(object: &Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_u64)
}

fn positive_json_u64(object: &Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()))
        })
        .filter(|value| *value > 0)
}

fn json_f64(object: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(|value| value.as_f64().or_else(|| value.as_u64().map(|n| n as f64)))
    })
}

fn json_string(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn codex_mcp_elicitation_result(response: &Value) -> Value {
    let action = response
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("accept");
    let content = response
        .get("content")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let meta = response.get("_meta").cloned().unwrap_or(Value::Null);
    json!({
        "action": action,
        "content": content,
        "_meta": meta,
    })
}

/// Map an in-flight [`CodexItem`] to an [`ExecutorEvent`].
///
/// Returns `None` for items that do not emit anything on start/update
/// (Codex often streams `item.started` before content is present).
fn translate_item(item: &CodexItem, current_model: &str) -> Option<ExecutorEvent> {
    match item {
        CodexItem::CommandExecution {
            id, command, data, ..
        } => Some(ExecutorEvent::ToolCallStart {
            tool_use_id: id.clone(),
            name: "Bash".to_string(),
            input: codex_command_input(command, data),
            partial: true,
        }),
        CodexItem::McpToolCall {
            id, server, tool, ..
        } => Some(ExecutorEvent::ToolCallStart {
            tool_use_id: id.clone(),
            name: format!("mcp__{server}__{tool}"),
            input: Value::Null,
            partial: true,
        }),
        CodexItem::DynamicToolCall {
            id,
            tool,
            arguments,
            ..
        } => Some(ExecutorEvent::ToolCallStart {
            tool_use_id: id.clone(),
            name: tool.clone(),
            input: arguments.clone().unwrap_or(Value::Null),
            partial: true,
        }),
        CodexItem::CollabAgentToolCall {
            id, tool, prompt, ..
        } => Some(ExecutorEvent::ToolCallStart {
            tool_use_id: id.clone(),
            name: "Task".to_string(),
            input: codex_collab_task_input(tool, prompt.as_deref(), current_model),
            partial: true,
        }),
        CodexItem::WebSearch { id, query } => Some(ExecutorEvent::ToolCallStart {
            tool_use_id: id.clone(),
            name: "WebSearch".to_string(),
            input: serde_json::json!({ "query": query }),
            partial: true,
        }),
        CodexItem::ImageGeneration { id, data } => Some(ExecutorEvent::ToolCallStart {
            tool_use_id: id.clone(),
            name: "image_generation".to_string(),
            input: Value::Object(data.clone()),
            partial: true,
        }),
        CodexItem::ImageView { id, data } => Some(ExecutorEvent::ToolCallStart {
            tool_use_id: id.clone(),
            name: "screenshot".to_string(),
            input: Value::Object(data.clone()),
            partial: true,
        }),
        CodexItem::Plan {
            id,
            explanation,
            items,
        } => Some(ExecutorEvent::ToolCallStart {
            tool_use_id: id.clone(),
            name: "update_plan".to_string(),
            input: codex_plan_input(explanation.as_deref(), items),
            partial: true,
        }),
        CodexItem::TodoList { id, items } => Some(ExecutorEvent::ToolCallStart {
            tool_use_id: id.clone(),
            name: "update_plan".to_string(),
            input: codex_todo_input(items),
            partial: true,
        }),
        CodexItem::AutoApprovalReview { id, data } => Some(ExecutorEvent::PermissionRequest {
            request_id: id.clone(),
            tool_name: "CodexGuardian".to_string(),
            tool_input: codex_guardian_review_input(data),
        }),
        CodexItem::Reasoning { text, .. } => Some(ExecutorEvent::StreamDelta {
            kind: DeltaKind::Reasoning,
            content: text.clone(),
        }),
        CodexItem::AgentMessage { .. } | CodexItem::FileChange { .. } | CodexItem::Error { .. } => {
            None
        }
    }
}

fn translate_reasoning_summary(item: &CodexItem) -> Option<ExecutorEvent> {
    let summary = match item {
        CodexItem::Reasoning { summary, .. } => summary,
        _ => return None,
    };

    Some(ExecutorEvent::StreamDelta {
        kind: DeltaKind::Thinking,
        content: codex_reasoning_summary(summary)?,
    })
}

fn translate_turn_plan_update(tool_use_id: &str, update: &CodexTurnPlanUpdate) -> ExecutorEvent {
    ExecutorEvent::ToolCallStart {
        tool_use_id: tool_use_id.to_string(),
        name: "update_plan".to_string(),
        input: codex_plan_input(update.explanation(), update.items()),
        partial: true,
    }
}

fn translate_mcp_tool_call_progress(progress: &CodexMcpToolCallProgress) -> Option<ExecutorEvent> {
    let tool_use_id = progress.tool_use_id()?;
    let message = progress.message()?;

    Some(ExecutorEvent::ToolCallInputDelta {
        tool_use_id: tool_use_id.to_string(),
        json_patch: json!({
            "message": message,
        }),
    })
}

fn translate_command_exec_output_delta(
    delta: &CodexCommandExecOutputDelta,
    accumulated_output: &mut HashMap<String, CommandOutputBuffers>,
) -> Option<ExecutorEvent> {
    let tool_use_id = delta.tool_use_id()?.to_string();
    let stream = normalize_command_output_stream(delta.stream()?)?;
    let chunk = decode_command_output_chunk(delta.chunk_base64()?)?;

    let buffer = accumulated_output.entry(tool_use_id.clone()).or_default();
    let json_patch = match stream {
        CommandOutputStream::Stdout => {
            buffer.stdout.push_str(&chunk);
            json!({ "stdout": buffer.stdout })
        }
        CommandOutputStream::Stderr => {
            buffer.stderr.push_str(&chunk);
            json!({ "stderr": buffer.stderr })
        }
    };

    Some(ExecutorEvent::ToolCallInputDelta {
        tool_use_id,
        json_patch,
    })
}

fn translate_plan_delta(
    delta: &CodexPlanDelta,
    accumulated_text: &mut HashMap<String, String>,
) -> Option<ExecutorEvent> {
    let tool_use_id = delta.tool_use_id()?;
    let delta_text = delta.delta()?;

    let explanation = append_delta_text(
        accumulated_text.entry(tool_use_id.to_string()).or_default(),
        delta_text,
    );

    Some(ExecutorEvent::ToolCallInputDelta {
        tool_use_id: tool_use_id.to_string(),
        json_patch: json!({
            "explanation": explanation,
        }),
    })
}

/// Map a completed [`CodexItem`] to zero-or-more terminal events.
fn translate_item_completed(item: &CodexItem) -> Vec<ExecutorEvent> {
    match item {
        CodexItem::CommandExecution {
            id,
            aggregated_output,
            exit_code,
            ..
        } => {
            let output = match exit_code {
                Some(0) | None => Ok(aggregated_output.clone().unwrap_or_default()),
                Some(code) => Err(format!(
                    "command exited with code {code}: {}",
                    aggregated_output.clone().unwrap_or_default()
                )),
            };
            vec![ExecutorEvent::ToolResult {
                tool_use_id: id.clone(),
                output,
            }]
        }
        CodexItem::McpToolCall {
            id, status, error, ..
        } => {
            let output = if status.as_deref() == Some("failed") {
                Err(error
                    .as_ref()
                    .and_then(|e| e.message.clone())
                    .unwrap_or_else(|| "mcp tool call failed".to_string()))
            } else {
                Ok(String::new())
            };
            vec![ExecutorEvent::ToolResult {
                tool_use_id: id.clone(),
                output,
            }]
        }
        CodexItem::DynamicToolCall {
            id,
            status,
            content_items,
            error,
            ..
        } => {
            let output = if status.as_deref() == Some("failed") {
                Err(error
                    .as_ref()
                    .and_then(|e| e.message.clone())
                    .unwrap_or_else(|| "dynamic tool call failed".to_string()))
            } else {
                Ok(codex_dynamic_tool_result_payload(content_items))
            };
            vec![ExecutorEvent::ToolResult {
                tool_use_id: id.clone(),
                output,
            }]
        }
        CodexItem::CollabAgentToolCall {
            id, tool, status, ..
        } => vec![ExecutorEvent::ToolResult {
            tool_use_id: id.clone(),
            output: if status.as_deref() == Some("failed") {
                Err(codex_collab_task_result(tool, false))
            } else {
                Ok(codex_collab_task_result(tool, true))
            },
        }],
        CodexItem::WebSearch { id, .. } => vec![ExecutorEvent::ToolResult {
            tool_use_id: id.clone(),
            output: Ok(String::new()),
        }],
        CodexItem::ImageGeneration { id, data } => vec![ExecutorEvent::ToolResult {
            tool_use_id: id.clone(),
            output: Ok(codex_item_result_payload("image_generation", data)),
        }],
        CodexItem::ImageView { id, data } => vec![ExecutorEvent::ToolResult {
            tool_use_id: id.clone(),
            output: Ok(codex_item_result_payload("screenshot", data)),
        }],
        CodexItem::Plan { id, .. } => vec![ExecutorEvent::ToolResult {
            tool_use_id: id.clone(),
            output: Ok("Plan updated".to_string()),
        }],
        CodexItem::TodoList { id, items } => vec![ExecutorEvent::ToolResult {
            tool_use_id: id.clone(),
            output: Ok(format!("Updated {} todo item(s)", items.len())),
        }],
        CodexItem::AutoApprovalReview { id, data } => vec![ExecutorEvent::NativeEvent {
            provider: Cow::Borrowed(VENDOR),
            payload: codex_guardian_review_completion_payload(id, data),
        }],
        CodexItem::FileChange { id, changes, .. } => {
            // Surface as a synthetic tool call for the file-change batch so
            // downstream normalisers can render it like `ApplyPatch`.
            vec![
                ExecutorEvent::ToolCallStart {
                    tool_use_id: id.clone(),
                    name: "CodexPatch".to_string(),
                    input: serde_json::json!({ "changes": codex_patch_input(changes) }),
                    partial: false,
                },
                ExecutorEvent::ToolResult {
                    tool_use_id: id.clone(),
                    output: Ok(codex_patch_summary(changes)),
                },
            ]
        }
        CodexItem::Error { message, .. } => vec![ExecutorEvent::Error {
            message: message.clone(),
            recoverable: true,
        }],
        CodexItem::Reasoning { .. } | CodexItem::AgentMessage { .. } => Vec::new(),
    }
}

fn codex_plan_input(explanation: Option<&str>, items: &[CodexPlanItem]) -> Value {
    let todos = items
        .iter()
        .filter_map(|item| {
            let content = item
                .content
                .as_deref()
                .or(item.text.as_deref())
                .or(item.step.as_deref())
                .or(item.description.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            Some(serde_json::json!({
                "content": content,
                "status": normalize_todo_status(item.status.as_deref()),
            }))
        })
        .collect();

    let mut input = serde_json::Map::new();
    input.insert("todos".to_string(), Value::Array(todos));
    if let Some(explanation) = explanation.map(str::trim).filter(|value| !value.is_empty()) {
        input.insert(
            "explanation".to_string(),
            Value::String(explanation.to_string()),
        );
    }

    Value::Object(input)
}

fn codex_turn_plan_tool_use_id(vendor_id: Option<&str>, iteration_count: u32) -> String {
    vendor_id
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("{CODEX_TURN_PLAN_TOOL_PREFIX}_{}", iteration_count.max(1)))
}

fn codex_plan_tool_result(tool_use_id: String) -> ExecutorEvent {
    ExecutorEvent::ToolResult {
        tool_use_id,
        output: Ok("Plan updated".to_string()),
    }
}

fn append_delta_text(buffer: &mut String, delta: &str) -> String {
    buffer.push_str(delta);
    buffer.clone()
}

fn normalize_command_output_stream(stream: &str) -> Option<CommandOutputStream> {
    match stream.trim().to_ascii_lowercase().as_str() {
        "stdout" | "out" | "1" => Some(CommandOutputStream::Stdout),
        "stderr" | "err" | "2" => Some(CommandOutputStream::Stderr),
        _ => None,
    }
}

fn decode_command_output_chunk(chunk_base64: &str) -> Option<String> {
    let decoded = decode_base64(chunk_base64)?;
    Some(String::from_utf8_lossy(&decoded).into_owned())
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut quartet = [0u8; 4];
    let mut quartet_len = 0usize;
    let mut saw_padding = false;

    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                saw_padding = true;
                64
            }
            b'\r' | b'\n' | b'\t' | b' ' => continue,
            _ => return None,
        };

        if saw_padding && value != 64 {
            return None;
        }

        quartet[quartet_len] = value;
        quartet_len += 1;

        if quartet_len == 4 {
            output.push((quartet[0] << 2) | (quartet[1] >> 4));
            if quartet[2] != 64 {
                output.push((quartet[1] << 4) | (quartet[2] >> 2));
            }
            if quartet[3] != 64 {
                output.push((quartet[2] << 6) | quartet[3]);
            }

            if quartet[0] == 64 || quartet[1] == 64 {
                return None;
            }
            if quartet[2] == 64 && quartet[3] != 64 {
                return None;
            }

            quartet_len = 0;
        }
    }

    if quartet_len != 0 {
        return None;
    }

    Some(output)
}

fn codex_command_input(command: &str, data: &Map<String, Value>) -> Value {
    let mut input =
        serde_json::Map::from_iter([("command".to_string(), Value::String(command.to_string()))]);

    if let Some(interaction) = data
        .get("terminalInteraction")
        .or_else(|| data.get("terminal_interaction"))
    {
        input.insert("terminalInteraction".to_string(), interaction.clone());
    }

    Value::Object(input)
}

fn codex_item_result_payload(
    normalized_type: &str,
    data: &serde_json::Map<String, Value>,
) -> String {
    let mut payload = data.clone();
    payload.insert(
        "type".to_string(),
        Value::String(normalized_type.to_string()),
    );
    Value::Object(payload).to_string()
}

fn codex_dynamic_tool_result_payload(content_items: &[Value]) -> String {
    json!({ "contentItems": content_items }).to_string()
}

fn codex_todo_input(items: &[crate::stream::CodexTodoItem]) -> Value {
    Value::Object(serde_json::Map::from_iter([(
        "todos".to_string(),
        Value::Array(
            items
                .iter()
                .map(|item| {
                    serde_json::json!({
                        "content": item.text,
                        "status": if item.completed { "completed" } else { "pending" },
                    })
                })
                .collect(),
        ),
    )]))
}

fn codex_guardian_review_input(data: &Map<String, Value>) -> Value {
    let mut input = data.clone();
    input.insert(CODEX_GUARDIAN_REVIEW_MARKER.to_string(), Value::Bool(true));
    if let Some(risk_level) = codex_guardian_risk_level(data) {
        input
            .entry("riskLevel".to_string())
            .or_insert_with(|| Value::String(risk_level.to_string()));
    }
    Value::Object(input)
}

fn codex_guardian_review_completion_payload(id: &str, data: &Map<String, Value>) -> Value {
    json!({
        "kind": "codex_guardian_review_completed",
        "request_id": id,
        "review": Value::Object(data.clone()),
    })
}

fn codex_guardian_risk_level(data: &Map<String, Value>) -> Option<&str> {
    data.get("riskLevel")
        .or_else(|| data.get("risk_level"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn codex_collab_task_input(action: &str, prompt: Option<&str>, model: &str) -> Value {
    serde_json::json!({
        "action": normalize_collab_action(action),
        "prompt": prompt.map(str::trim).unwrap_or_default(),
        "model": model,
    })
}

fn codex_collab_task_result(action: &str, completed: bool) -> String {
    let action = normalize_collab_action(action);
    if completed {
        format!("Task {action} completed")
    } else {
        format!("Task {action} failed")
    }
}

fn codex_patch_input(changes: &[crate::stream::CodexFileChange]) -> Value {
    Value::Object(serde_json::Map::from_iter(changes.iter().map(|change| {
        (
            change.path.clone(),
            serde_json::json!({
                "kind": change.kind,
                "diff": change.diff.clone().unwrap_or_default(),
            }),
        )
    })))
}

fn codex_patch_summary(changes: &[crate::stream::CodexFileChange]) -> String {
    match changes {
        [] => "Applied file changes".to_string(),
        [change] => format!("Applied {} to {}", change.kind, change.path),
        _ => format!("Applied {} file changes", changes.len()),
    }
}

fn codex_reasoning_summary(summary: &[Value]) -> Option<String> {
    let parts: Vec<String> = summary
        .iter()
        .filter_map(codex_reasoning_summary_part)
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn codex_reasoning_summary_part(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        Value::Object(object) => ["text", "summary", "content", "value"]
            .iter()
            .find_map(|key| object.get(*key).and_then(Value::as_str))
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(ToOwned::to_owned),
        _ => None,
    }
}

fn normalize_todo_status(status: Option<&str>) -> &'static str {
    match status
        .unwrap_or("pending")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "completed" | "complete" | "done" | "success" | "succeeded" => "completed",
        "in_progress" | "in-progress" | "inprogress" | "running" | "active" => "in_progress",
        _ => "pending",
    }
}

fn normalize_collab_action(action: &str) -> &str {
    match action {
        "spawn_agent" | "spawnAgent" => "spawnAgent",
        "send_input" | "sendInput" => "sendInput",
        "resume_agent" | "resumeAgent" => "resumeAgent",
        "wait" => "wait",
        "close_agent" | "closeAgent" => "closeAgent",
        _ => action,
    }
}

fn codex_terminal_interaction_response_text(response: &Value) -> Option<String> {
    let object = response.as_object()?;
    match object.get("action").and_then(Value::as_str) {
        Some("accept") => {}
        _ => return None,
    }

    let content = object.get("content")?;
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Object(content) => ["text", "input", "value", "stdin", "response", "message"]
            .iter()
            .find_map(|key| content.get(*key).and_then(Value::as_str))
            .map(ToOwned::to_owned),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures_util::StreamExt;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingStore {
        records: Mutex<Vec<(String, SessionRecord)>>,
    }

    impl RecordingStore {
        fn latest_record(
            &self,
            vendor: &str,
            session_id: &NativeSessionId,
        ) -> Option<SessionRecord> {
            self.records
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|(stored_vendor, record)| {
                    stored_vendor == vendor && record.session_id == *session_id
                })
                .map(|(_, record)| record.clone())
        }
    }

    #[async_trait]
    impl SessionStoreProvider for RecordingStore {
        async fn record_session(&self, vendor: &str, session: SessionRecord) -> Result<(), String> {
            self.records
                .lock()
                .unwrap()
                .push((vendor.to_string(), session));
            Ok(())
        }

        async fn list_sessions(
            &self,
            _vendor: &str,
            _filter: SessionFilter,
        ) -> Result<Vec<SessionMeta>, String> {
            Ok(Vec::new())
        }

        async fn get_session_info(
            &self,
            vendor: &str,
            session_id: &NativeSessionId,
        ) -> Result<SessionInfo, String> {
            let record = self
                .latest_record(vendor, session_id)
                .ok_or_else(|| format!("missing session {}", session_id.as_str()))?;
            Ok(SessionInfo {
                meta: SessionMeta {
                    id: record.session_id.clone(),
                    workdir: record.workdir.clone(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    title: None,
                },
                permission_mode: None,
                model: None,
                usage: TokenUsage::default(),
                extras: record.context,
            })
        }

        async fn get_session_messages(
            &self,
            _vendor: &str,
            _session_id: &NativeSessionId,
            _pagination: Pagination,
        ) -> Result<Vec<NativeMessage>, String> {
            Ok(Vec::new())
        }
    }

    fn write_mock_codex_app_server(
        script_path: &Path,
        launch_count_path: &Path,
        request_log_path: &Path,
    ) {
        let script = format!(
            r#"#!/bin/sh
set -eu
count_file='{count_file}'
log_file='{log_file}'

if [ "$#" -ge 2 ] && [ "$1" = "app-server" ] && [ "$2" = "--help" ]; then
  exit 0
fi

if [ "$#" -ge 3 ] && [ "$1" = "app-server" ] && [ "$2" = "--listen" ] && [ "$3" = "stdio://" ]; then
  launches=0
  if [ -f "$count_file" ]; then
    launches=$(cat "$count_file")
  fi
  launches=$((launches + 1))
  printf '%s' "$launches" > "$count_file"
  turn=0

  while IFS= read -r line; do
    printf '%s\n' "$line" >> "$log_file"
    id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$line" in
      *'"method":"initialize"'*)
        printf '{{"jsonrpc":"2.0","id":%s,"result":{{"userAgent":"mock"}}}}\n' "$id"
        ;;
      *'"method":"initialized"'*)
        ;;
      *'"method":"thread/start"'*|*'"method":"thread/resume"'*)
        printf '{{"jsonrpc":"2.0","id":%s,"result":{{"thread":{{"id":"mock-thread-1","path":"/tmp/mock-thread-1"}}}}}}\n' "$id"
        ;;
      *'"method":"turn/start"'*)
        turn=$((turn + 1))
        printf '{{"jsonrpc":"2.0","id":%s,"result":{{"turn":{{"id":"mock-turn-%s","status":"inProgress"}}}}}}\n' "$id" "$turn"
        printf '{{"jsonrpc":"2.0","method":"turn/started","params":{{"threadId":"mock-thread-1","turn":{{"id":"mock-turn-%s","status":"inProgress"}}}}}}\n' "$turn"
        printf '{{"jsonrpc":"2.0","method":"item/completed","params":{{"threadId":"mock-thread-1","turnId":"mock-turn-%s","item":{{"type":"agentMessage","id":"msg-%s","text":"reply %s"}}}}}}\n' "$turn" "$turn" "$turn"
        printf '{{"jsonrpc":"2.0","method":"turn/completed","params":{{"threadId":"mock-thread-1","turn":{{"id":"mock-turn-%s","status":"completed","inputTokens":3,"outputTokens":5}}}}}}\n' "$turn"
        ;;
      *'"method":"turn/interrupt"'*)
        printf '{{"jsonrpc":"2.0","id":%s,"result":{{"abortReason":"interrupted"}}}}\n' "$id"
        ;;
    esac
  done
  exit 0
fi

exit 1
"#,
            count_file = launch_count_path.display(),
            log_file = request_log_path.display(),
        );

        fs::write(script_path, script).unwrap();
        let mut perms = fs::metadata(script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(script_path, perms).unwrap();
    }

    async fn collect_events(stream: EventStream) -> Vec<ExecutorEvent> {
        let mut events = Vec::new();
        let mut stream = Box::pin(stream);
        while let Some(item) = stream.next().await {
            events.push(item.unwrap());
        }
        events
    }

    #[tokio::test]
    async fn app_server_spawn_records_native_id_and_reuses_process() {
        let root = std::env::temp_dir().join(format!("codex-app-server-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let workdir = root.join("workdir");
        fs::create_dir_all(&workdir).unwrap();
        let script_path = root.join("mock-codex");
        let launch_count_path = root.join("launch-count.txt");
        let request_log_path = root.join("requests.log");
        write_mock_codex_app_server(&script_path, &launch_count_path, &request_log_path);

        let store = Arc::new(RecordingStore::default());
        let executor = CodexAgentExecutor::new(script_path, store.clone());

        let session = executor
            .spawn_session(SpawnSessionSpec {
                workdir: workdir.clone(),
                system_prompt: Some("System prompt".to_string()),
                model: None,
                permission_mode: PermissionMode::Default,
                allowed_tools: None,
                additional_directories: Vec::new(),
                env: Default::default(),
                agent_config: Value::Null,
                resume_hint: None,
            })
            .await
            .unwrap();

        {
            let records = store.records.lock().unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].0, VENDOR);
            assert_eq!(records[0].1.session_id, session.id);
            assert_eq!(records[0].1.workdir, workdir);
            assert_eq!(
                records[0]
                    .1
                    .context
                    .get("native_session_id")
                    .and_then(Value::as_str),
                Some("mock-thread-1")
            );
            assert!(records[0].1.context.get(CODEX_RESUME_CONFIG_KEY).is_some());
        }

        let first_events = collect_events(
            executor
                .send_message(
                    &session,
                    UserMessage {
                        content: "first".to_string(),
                        task_id: None,
                        attachments: Vec::new(),
                        parent_tool_use_id: None,
                        injected_tools: Vec::new(),
                    },
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(first_events.iter().any(|event| matches!(
            event,
            ExecutorEvent::StreamDelta { kind: DeltaKind::Text, content } if content == "reply 1"
        )));
        assert!(first_events.iter().any(|event| matches!(
            event,
            ExecutorEvent::TurnComplete {
                usage,
                ..
            } if usage.input_tokens == 3 && usage.output_tokens == 5
        )));

        let second_events = collect_events(
            executor
                .send_message(
                    &session,
                    UserMessage {
                        content: "second".to_string(),
                        task_id: None,
                        attachments: Vec::new(),
                        parent_tool_use_id: None,
                        injected_tools: Vec::new(),
                    },
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(second_events.iter().any(|event| matches!(
            event,
            ExecutorEvent::StreamDelta { kind: DeltaKind::Text, content } if content == "reply 2"
        )));

        executor.close_session(&session).await.unwrap();

        assert_eq!(fs::read_to_string(&launch_count_path).unwrap(), "1");
        let _ = fs::remove_dir_all(&root);
    }

    // ---------------------------------------------------------------
    // Phase 1 connection-reuse unit tests.
    //
    // These exercise the new CodexAppServerConnection seam against a
    // mock `codex app-server` that emits distinct thread ids per
    // `thread/start` call. They use multiple ThreadState channels and
    // verify the demuxer routes by `params.threadId`.
    // ---------------------------------------------------------------

    /// Mock app-server that allocates a fresh `mock-thread-N` per
    /// `thread/start` and echoes each `turn/start` with a thread-scoped
    /// `turn/completed`. Used by the multi-session tests below.
    fn write_multi_thread_mock(
        script_path: &Path,
        launch_count_path: &Path,
        request_log_path: &Path,
    ) {
        let script = format!(
            r#"#!/bin/sh
set -eu
count_file='{count_file}'
log_file='{log_file}'

if [ "$#" -ge 2 ] && [ "$1" = "app-server" ] && [ "$2" = "--help" ]; then
  exit 0
fi

if [ "$#" -ge 3 ] && [ "$1" = "app-server" ] && [ "$2" = "--listen" ] && [ "$3" = "stdio://" ]; then
  launches=0
  if [ -f "$count_file" ]; then
    launches=$(cat "$count_file")
  fi
  launches=$((launches + 1))
  printf '%s' "$launches" > "$count_file"
  thread=0
  turn=0

  while IFS= read -r line; do
    printf '%s\n' "$line" >> "$log_file"
    id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$line" in
      *'"method":"initialize"'*)
        printf '{{"jsonrpc":"2.0","id":%s,"result":{{"userAgent":"mock"}}}}\n' "$id"
        ;;
      *'"method":"initialized"'*)
        ;;
      *'"method":"thread/start"'*|*'"method":"thread/resume"'*)
        thread=$((thread + 1))
        printf '{{"jsonrpc":"2.0","id":%s,"result":{{"thread":{{"id":"mock-thread-%s","path":"/tmp/mock-thread-%s"}}}}}}\n' "$id" "$thread" "$thread"
        ;;
      *'"method":"turn/start"'*)
        turn=$((turn + 1))
        tid=$(printf '%s\n' "$line" | sed -n 's/.*"threadId":"\([^"]*\)".*/\1/p')
        printf '{{"jsonrpc":"2.0","id":%s,"result":{{"turn":{{"id":"mock-turn-%s","status":"inProgress"}}}}}}\n' "$id" "$turn"
        printf '{{"jsonrpc":"2.0","method":"turn/started","params":{{"threadId":"%s","turn":{{"id":"mock-turn-%s","status":"inProgress"}}}}}}\n' "$tid" "$turn"
        printf '{{"jsonrpc":"2.0","method":"item/completed","params":{{"threadId":"%s","turnId":"mock-turn-%s","item":{{"type":"agentMessage","id":"msg-%s","text":"reply-%s-from-%s"}}}}}}\n' "$tid" "$turn" "$turn" "$turn" "$tid"
        printf '{{"jsonrpc":"2.0","method":"turn/completed","params":{{"threadId":"%s","turn":{{"id":"mock-turn-%s","status":"completed","inputTokens":3,"outputTokens":5}}}}}}\n' "$tid" "$turn"
        ;;
      *'"method":"turn/interrupt"'*)
        printf '{{"jsonrpc":"2.0","id":%s,"result":{{}}}}\n' "$id"
        ;;
    esac
  done
  exit 0
fi

exit 1
"#,
            count_file = launch_count_path.display(),
            log_file = request_log_path.display(),
        );

        fs::write(script_path, script).unwrap();
        let mut perms = fs::metadata(script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(script_path, perms).unwrap();
    }

    fn test_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("codex-p1-{label}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn default_spawn(workdir: PathBuf) -> SpawnSessionSpec {
        SpawnSessionSpec {
            workdir,
            system_prompt: None,
            model: None,
            permission_mode: PermissionMode::Default,
            allowed_tools: None,
            additional_directories: Vec::new(),
            env: BTreeMap::new(),
            agent_config: Value::Null,
            resume_hint: None,
        }
    }

    #[tokio::test]
    async fn open_connection_runs_initialize_and_initialized() {
        let root = test_root("open-conn");
        let script_path = root.join("mock-codex");
        let count_path = root.join("launch-count.txt");
        let log_path = root.join("requests.log");
        write_multi_thread_mock(&script_path, &count_path, &log_path);

        let store = Arc::new(RecordingStore::default());
        let executor = CodexAgentExecutor::new(script_path, store);
        let handle = executor
            .open_connection(ConnectionSpec::default())
            .await
            .expect("open_connection should succeed on probe=false");
        assert_eq!(handle.vendor, VENDOR);

        // Close first, then read the log — the mock script appends to
        // the log from a read loop that only fully drains after stdin
        // closes.
        executor.close_connection(handle).await.unwrap();

        // Brief settle so the child's fs writes land.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let log = fs::read_to_string(&log_path).unwrap();
        assert!(
            log.contains(r#""method":"initialize""#),
            "initialize handshake frame missing: {log}"
        );
        assert!(
            log.contains(r#""method":"initialized""#),
            "initialized notification missing: {log}"
        );
        // No thread/start was issued during open_connection.
        assert!(
            !log.contains(r#""method":"thread/start""#),
            "open_connection must not issue thread/start: {log}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn start_session_on_returns_thread_id_from_response() {
        let root = test_root("start-session-on");
        let script_path = root.join("mock-codex");
        let count_path = root.join("launch-count.txt");
        let log_path = root.join("requests.log");
        write_multi_thread_mock(&script_path, &count_path, &log_path);
        let workdir = root.join("workdir");
        fs::create_dir_all(&workdir).unwrap();

        let store = Arc::new(RecordingStore::default());
        let executor = CodexAgentExecutor::new(script_path, store);
        let handle = executor
            .open_connection(ConnectionSpec::default())
            .await
            .unwrap();
        let session = executor
            .start_session_on(&handle, default_spawn(workdir))
            .await
            .unwrap();
        assert_eq!(session.id.as_str(), "mock-thread-1");

        let _ = executor.close_connection(handle).await;
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn two_sessions_on_one_connection_have_independent_event_streams() {
        let root = test_root("two-sessions");
        let script_path = root.join("mock-codex");
        let count_path = root.join("launch-count.txt");
        let log_path = root.join("requests.log");
        write_multi_thread_mock(&script_path, &count_path, &log_path);
        let w1 = root.join("w1");
        let w2 = root.join("w2");
        fs::create_dir_all(&w1).unwrap();
        fs::create_dir_all(&w2).unwrap();

        let store = Arc::new(RecordingStore::default());
        let executor = CodexAgentExecutor::new(script_path, store);
        let handle = executor
            .open_connection(ConnectionSpec::default())
            .await
            .unwrap();
        let sess_a = executor
            .start_session_on(&handle, default_spawn(w1))
            .await
            .unwrap();
        let sess_b = executor
            .start_session_on(&handle, default_spawn(w2))
            .await
            .unwrap();

        // Two sessions on one connection must get distinct thread ids.
        assert_ne!(sess_a.id.as_str(), sess_b.id.as_str());

        let events_a = collect_events(
            executor
                .send_message(
                    &sess_a,
                    UserMessage {
                        content: "hello A".to_string(),
                        task_id: None,
                        attachments: Vec::new(),
                        parent_tool_use_id: None,
                        injected_tools: Vec::new(),
                    },
                )
                .await
                .unwrap(),
        )
        .await;
        let events_b = collect_events(
            executor
                .send_message(
                    &sess_b,
                    UserMessage {
                        content: "hello B".to_string(),
                        task_id: None,
                        attachments: Vec::new(),
                        parent_tool_use_id: None,
                        injected_tools: Vec::new(),
                    },
                )
                .await
                .unwrap(),
        )
        .await;

        // Each event stream carries a StreamDelta stamped with the
        // thread's own reply text (routed by threadId).
        let has_a = events_a.iter().any(|e| {
            matches!(
                e,
                ExecutorEvent::StreamDelta { content, .. } if content.contains(sess_a.id.as_str())
            )
        });
        let has_b = events_b.iter().any(|e| {
            matches!(
                e,
                ExecutorEvent::StreamDelta { content, .. } if content.contains(sess_b.id.as_str())
            )
        });
        assert!(
            has_a,
            "session A did not see its own thread's reply: {events_a:?}"
        );
        assert!(
            has_b,
            "session B did not see its own thread's reply: {events_b:?}"
        );

        // Same subprocess was reused for both sessions.
        assert_eq!(fs::read_to_string(&count_path).unwrap(), "1");

        let _ = executor.close_connection(handle).await;
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn interrupt_one_thread_does_not_cancel_other() {
        let root = test_root("interrupt-iso");
        let script_path = root.join("mock-codex");
        let count_path = root.join("launch-count.txt");
        let log_path = root.join("requests.log");
        write_multi_thread_mock(&script_path, &count_path, &log_path);
        let w1 = root.join("w1");
        let w2 = root.join("w2");
        fs::create_dir_all(&w1).unwrap();
        fs::create_dir_all(&w2).unwrap();

        let store = Arc::new(RecordingStore::default());
        let executor = CodexAgentExecutor::new(script_path, store);
        let handle = executor
            .open_connection(ConnectionSpec::default())
            .await
            .unwrap();
        let sess_a = executor
            .start_session_on(&handle, default_spawn(w1))
            .await
            .unwrap();
        let sess_b = executor
            .start_session_on(&handle, default_spawn(w2))
            .await
            .unwrap();

        // Interrupt before a turn is live — should be a no-op rather than
        // an error, confirming interrupt is scoped and tolerant.
        executor.interrupt(&sess_a).await.unwrap();

        // Session B still happily streams.
        let events_b = collect_events(
            executor
                .send_message(
                    &sess_b,
                    UserMessage {
                        content: "hello B".to_string(),
                        task_id: None,
                        attachments: Vec::new(),
                        parent_tool_use_id: None,
                        injected_tools: Vec::new(),
                    },
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(events_b
            .iter()
            .any(|e| matches!(e, ExecutorEvent::TurnComplete { .. })));

        let _ = executor.close_connection(handle).await;
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn close_connection_kills_child_and_drains_pending_requests() {
        let root = test_root("close-conn");
        let script_path = root.join("mock-codex");
        let count_path = root.join("launch-count.txt");
        let log_path = root.join("requests.log");
        write_multi_thread_mock(&script_path, &count_path, &log_path);

        let store = Arc::new(RecordingStore::default());
        let executor = CodexAgentExecutor::new(script_path, store);
        let handle = executor
            .open_connection(ConnectionSpec::default())
            .await
            .unwrap();

        // Grab a clone so we can still poke the underlying struct after
        // close_connection takes ownership of the outer handle.
        let inner_any = handle.inner.clone();
        let inner: Arc<CodexAppServerConnection> =
            inner_any.downcast::<CodexAppServerConnection>().unwrap();

        executor.close_connection(handle).await.unwrap();

        assert!(
            inner.closed.load(Ordering::SeqCst),
            "closed flag should be set"
        );
        let mut child = inner.child.lock().await;
        let status = child.try_wait().expect("try_wait after close");
        assert!(status.is_some(), "child should have exited");

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn check_connection_reports_dead_after_child_exit() {
        let root = test_root("check-conn");
        let script_path = root.join("mock-codex");
        let count_path = root.join("launch-count.txt");
        let log_path = root.join("requests.log");
        write_multi_thread_mock(&script_path, &count_path, &log_path);

        let store = Arc::new(RecordingStore::default());
        let executor = CodexAgentExecutor::new(script_path, store);
        let handle = executor
            .open_connection(ConnectionSpec::default())
            .await
            .unwrap();

        // Freshly opened handle should be healthy.
        let health = executor.check_connection(&handle).await.unwrap();
        assert_eq!(health, ConnectionHealth::Healthy);

        // Kill the child out from under us.
        {
            let inner_any = handle.inner.clone();
            let inner: Arc<CodexAppServerConnection> =
                inner_any.downcast::<CodexAppServerConnection>().unwrap();
            let mut child = inner.child.lock().await;
            let _ = child.kill().await;
            let _ = child.wait().await;
        }

        let health = executor.check_connection(&handle).await.unwrap();
        match health {
            ConnectionHealth::Dead { reason } => {
                assert!(
                    reason.contains("app-server exited") || reason.contains("connection shut down"),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected Dead, got {other:?}"),
        }

        // Cleanup (close_connection should still succeed even though
        // shutdown is a no-op since closed flag may already be set).
        let _ = executor.close_connection(handle).await;
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn exec_fallback_path_still_spawns_per_session() {
        // Mock binary that pretends `app-server --help` is unknown,
        // forcing the adapter onto the exec-fallback branch.
        let root = test_root("exec-fallback");
        let script_path = root.join("mock-codex");
        fs::write(&script_path, "#!/bin/sh\nexit 1\n").unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();

        let store = Arc::new(RecordingStore::default());
        let executor = CodexAgentExecutor::new(script_path, store);
        // app_server_available must be false for this mock.
        assert!(!executor.app_server_available());

        let workdir = root.join("workdir");
        fs::create_dir_all(&workdir).unwrap();
        let session = executor
            .spawn_session(default_spawn(workdir))
            .await
            .expect("spawn_session should fall back to exec path");
        // In exec-fallback the native id is a synthetic `codex-<uuid>`
        // prefix — that is how we know the fallback branch ran.
        assert!(
            session.id.as_str().starts_with("codex-"),
            "unexpected fallback id: {}",
            session.id.as_str()
        );
        let _ = executor.close_session(&session).await;
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn capabilities_report_runtime_restart_managed_controls() {
        let executor =
            CodexAgentExecutor::new(PathBuf::from("codex"), Arc::new(RecordingStore::default()));
        let caps = executor.capabilities();
        assert!(caps.supports_runtime_set_model);
        assert_eq!(caps.permission_mode_kind, PermissionModeKind::Dynamic);
        assert!(caps.supports_resume);
    }

    #[test]
    fn capabilities_do_not_advertise_autonomous_turn() {
        let executor =
            CodexAgentExecutor::new(PathBuf::from("codex"), Arc::new(RecordingStore::default()));
        assert!(!executor.capabilities().autonomous_turn);
    }

    #[tokio::test]
    async fn set_autonomous_turn_handler_returns_unsupported() {
        let executor =
            CodexAgentExecutor::new(PathBuf::from("codex"), Arc::new(RecordingStore::default()));
        let err = executor
            .set_autonomous_turn_handler(Some(Arc::new(|_session_id, _synthetic, _stream| {})))
            .await
            .unwrap_err();
        match err {
            AgentExecutorError::Unsupported { capability } => {
                assert_eq!(capability, "set_autonomous_turn_handler");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn permission_request_for_additional_permissions_maps_to_executor_event() {
        let pending_requests = tokio::sync::Mutex::new(HashMap::new());
        let event = handle_app_server_request(
            41,
            "item/permissions/requestApproval",
            &json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "perm-1",
                "permissions": {
                    "fileSystem": {
                        "write": ["/tmp/outside-workspace"]
                    }
                },
                "reason": "needs extra write access"
            }),
            &pending_requests,
        )
        .await
        .expect("request should map")
        .expect("event should exist");

        match event {
            ExecutorEvent::PermissionRequest {
                request_id,
                tool_name,
                tool_input,
            } => {
                assert_eq!(request_id, "41");
                assert_eq!(tool_name, "CodexPermissions");
                assert_eq!(
                    tool_input,
                    json!({
                        "permissions": {
                            "fileSystem": {
                                "write": ["/tmp/outside-workspace"]
                            }
                        },
                        "reason": "needs extra write access",
                        "threadId": "thread-1",
                        "turnId": "turn-1",
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let pending = pending_requests.lock().await;
        assert_eq!(
            pending.get("41").map(|request| request.method.as_str()),
            Some("item/permissions/requestApproval")
        );
    }

    #[tokio::test]
    async fn handle_app_server_request_elicitation_emits_permission_request() {
        let pending_requests = tokio::sync::Mutex::new(HashMap::new());
        let params = json!({
            "_meta": {
                "codex_approval_kind": "mcp_tool_call",
                "tool_description": "list memory",
                "tool_params": {},
                "persist": ["session", "always"]
            },
            "message": "Allow the cteno-memory MCP server to run tool \"memory_list\"?"
        });

        let event = handle_app_server_request(
            42,
            "mcpServer/elicitation/request",
            &params,
            &pending_requests,
        )
        .await
        .expect("request should map")
        .expect("event should exist");

        match event {
            ExecutorEvent::PermissionRequest {
                request_id,
                tool_name,
                tool_input,
            } => {
                assert_eq!(request_id, "42");
                assert_eq!(tool_name, "McpTool");
                assert_eq!(
                    tool_input["message"],
                    json!("Allow the cteno-memory MCP server to run tool \"memory_list\"?")
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let pending = pending_requests.lock().await;
        assert!(pending.contains_key("42"));
    }

    #[tokio::test]
    async fn handle_error_notification_fatal_emits_unrecoverable_error() {
        let current_turn_id = Arc::new(tokio::sync::Mutex::new(None));
        let mut final_text = None;
        let mut aggregate_usage = TokenUsage::default();
        let mut iteration_count = 0;
        let mut active_turn_plan_tool_use_id = None;
        let mut plan_text_by_tool_use_id = HashMap::new();
        let mut command_output_by_tool_use_id = HashMap::new();

        let outcome = handle_app_server_notification(
            "error",
            &json!({
                "error": {
                    "message": "stream disconnected"
                },
                "willRetry": false
            }),
            "gpt-5",
            &current_turn_id,
            &mut final_text,
            &mut aggregate_usage,
            &mut iteration_count,
            &mut active_turn_plan_tool_use_id,
            &mut plan_text_by_tool_use_id,
            &mut command_output_by_tool_use_id,
        )
        .await
        .expect("error notification should parse");

        assert_eq!(outcome.events.len(), 2);
        match &outcome.events[0] {
            ExecutorEvent::Error {
                message,
                recoverable,
            } => {
                assert_eq!(message, "stream disconnected");
                assert!(!recoverable);
            }
            other => panic!("first event should be error, got {other:?}"),
        }
        assert!(matches!(
            &outcome.events[1],
            ExecutorEvent::NativeEvent { .. }
        ));
        assert!(!outcome.done);
    }

    #[tokio::test]
    async fn handle_error_notification_retrying_emits_recoverable_error() {
        let current_turn_id = Arc::new(tokio::sync::Mutex::new(None));
        let mut final_text = None;
        let mut aggregate_usage = TokenUsage::default();
        let mut iteration_count = 0;
        let mut active_turn_plan_tool_use_id = None;
        let mut plan_text_by_tool_use_id = HashMap::new();
        let mut command_output_by_tool_use_id = HashMap::new();

        let outcome = handle_app_server_notification(
            "error",
            &json!({
                "error": {
                    "message": "Reconnecting... 2/5"
                },
                "willRetry": true
            }),
            "gpt-5",
            &current_turn_id,
            &mut final_text,
            &mut aggregate_usage,
            &mut iteration_count,
            &mut active_turn_plan_tool_use_id,
            &mut plan_text_by_tool_use_id,
            &mut command_output_by_tool_use_id,
        )
        .await
        .expect("error notification should parse");

        assert_eq!(outcome.events.len(), 2);
        match &outcome.events[0] {
            ExecutorEvent::Error {
                message,
                recoverable,
            } => {
                assert_eq!(message, "Reconnecting... 2/5");
                assert!(*recoverable);
            }
            other => panic!("first event should be error, got {other:?}"),
        }
        assert!(matches!(
            &outcome.events[1],
            ExecutorEvent::NativeEvent { .. }
        ));
        assert!(!outcome.done);
    }

    #[tokio::test]
    async fn stale_turn_completed_notification_is_ignored() {
        let current_turn_id = Arc::new(tokio::sync::Mutex::new(Some("turn-live".to_string())));
        let mut final_text = None;
        let mut aggregate_usage = TokenUsage::default();
        let mut iteration_count = 1;
        let mut active_turn_plan_tool_use_id = None;
        let mut plan_text_by_tool_use_id = HashMap::new();
        let mut command_output_by_tool_use_id = HashMap::new();

        let outcome = handle_app_server_notification(
            "turn/completed",
            &json!({
                "turn": {
                    "id": "turn-stale",
                    "status": "completed",
                }
            }),
            "gpt-5",
            &current_turn_id,
            &mut final_text,
            &mut aggregate_usage,
            &mut iteration_count,
            &mut active_turn_plan_tool_use_id,
            &mut plan_text_by_tool_use_id,
            &mut command_output_by_tool_use_id,
        )
        .await
        .expect("notification should parse");

        assert!(outcome.events.is_empty());
        assert!(!outcome.done);
        assert_eq!(
            current_turn_id.lock().await.clone(),
            Some("turn-live".to_string())
        );
    }

    #[tokio::test]
    async fn idle_status_without_turn_activity_does_not_complete_turn() {
        let current_turn_id = Arc::new(tokio::sync::Mutex::new(Some("turn-live".to_string())));
        let mut final_text = None;
        let mut aggregate_usage = TokenUsage::default();
        let mut iteration_count = 0;
        let mut active_turn_plan_tool_use_id = None;
        let mut plan_text_by_tool_use_id = HashMap::new();
        let mut command_output_by_tool_use_id = HashMap::new();

        let outcome = handle_app_server_notification(
            "thread/status/changed",
            &json!({
                "status": {
                    "type": "idle"
                }
            }),
            "gpt-5",
            &current_turn_id,
            &mut final_text,
            &mut aggregate_usage,
            &mut iteration_count,
            &mut active_turn_plan_tool_use_id,
            &mut plan_text_by_tool_use_id,
            &mut command_output_by_tool_use_id,
        )
        .await
        .expect("notification should parse");

        assert!(outcome.events.is_empty());
        assert!(!outcome.done);
        assert_eq!(
            current_turn_id.lock().await.clone(),
            Some("turn-live".to_string())
        );
    }

    #[test]
    fn additional_permissions_response_grants_or_denies_requested_scope() {
        let params = json!({
            "permissions": {
                "fileSystem": {
                    "write": ["/tmp/outside-workspace"]
                },
                "network": {
                    "enabled": true
                }
            }
        });

        assert_eq!(
            app_server_permission_response(
                "item/permissions/requestApproval",
                &params,
                PermissionDecision::Allow,
            ),
            json!({
                "permissions": {
                    "fileSystem": {
                        "write": ["/tmp/outside-workspace"]
                    },
                    "network": {
                        "enabled": true
                    }
                },
                "scope": "turn",
            })
        );
        assert_eq!(
            app_server_permission_response(
                "item/permissions/requestApproval",
                &params,
                PermissionDecision::Deny,
            ),
            json!({
                "permissions": {},
                "scope": "turn",
            })
        );
    }

    #[test]
    fn app_server_permission_response_elicitation_accept() {
        assert_eq!(
            app_server_permission_response(
                "mcpServer/elicitation/request",
                &json!({}),
                PermissionDecision::Allow,
            ),
            json!({
                "action": "accept",
                "content": Value::Null,
                "_meta": Value::Null,
            })
        );
    }

    #[test]
    fn app_server_permission_response_elicitation_deny() {
        assert_eq!(
            app_server_permission_response(
                "mcpServer/elicitation/request",
                &json!({}),
                PermissionDecision::Deny,
            ),
            json!({
                "action": "decline",
                "content": Value::Null,
                "_meta": Value::Null,
            })
        );
    }

    #[tokio::test]
    async fn set_model_and_permission_mode_restart_and_resume_with_persisted_config() {
        let root =
            std::env::temp_dir().join(format!("codex-runtime-control-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let workdir = root.join("workdir");
        let extra_dir = root.join("extra");
        fs::create_dir_all(&workdir).unwrap();
        fs::create_dir_all(&extra_dir).unwrap();
        let script_path = root.join("mock-codex");
        let launch_count_path = root.join("launch-count.txt");
        let request_log_path = root.join("requests.log");
        write_mock_codex_app_server(&script_path, &launch_count_path, &request_log_path);

        let store = Arc::new(RecordingStore::default());
        let executor = CodexAgentExecutor::new(script_path.clone(), store.clone());

        let session = executor
            .spawn_session(SpawnSessionSpec {
                workdir: workdir.clone(),
                system_prompt: Some("System prompt".to_string()),
                model: Some(ModelSpec {
                    provider: "openai".to_string(),
                    model_id: "gpt-5.4".to_string(),
                    reasoning_effort: Some("medium".to_string()),
                    temperature: None,
                }),
                permission_mode: PermissionMode::Default,
                allowed_tools: None,
                additional_directories: vec![extra_dir.clone()],
                env: BTreeMap::from([("CODEX_TEST_FLAG".to_string(), "1".to_string())]),
                agent_config: Value::Null,
                resume_hint: None,
            })
            .await
            .unwrap();

        let outcome = executor
            .set_model(
                &session,
                ModelSpec {
                    provider: "openai".to_string(),
                    model_id: "gpt-5.4-codex".to_string(),
                    reasoning_effort: Some("high".to_string()),
                    temperature: Some(0.2),
                },
            )
            .await
            .unwrap();
        assert_eq!(outcome, ModelChangeOutcome::Applied);

        executor
            .set_permission_mode(&session, PermissionMode::AcceptEdits)
            .await
            .unwrap();

        let persisted = store
            .latest_record(VENDOR, &session.id)
            .expect("updated session should be persisted under the native id");
        assert_eq!(persisted.workdir, workdir);
        assert_eq!(
            persisted
                .context
                .get("native_session_id")
                .and_then(Value::as_str),
            Some("mock-thread-1")
        );
        let persisted_config: SessionConfig = serde_json::from_value(
            persisted
                .context
                .get(CODEX_RESUME_CONFIG_KEY)
                .cloned()
                .expect("persisted config"),
        )
        .unwrap();
        assert_eq!(
            persisted_config.permission_mode,
            PermissionMode::AcceptEdits
        );
        assert_eq!(
            persisted_config
                .model
                .as_ref()
                .map(|model| model.model_id.as_str()),
            Some("gpt-5.4-codex")
        );
        assert_eq!(
            persisted_config.additional_directories,
            vec![extra_dir.clone()]
        );
        assert_eq!(
            persisted_config
                .env
                .get("CODEX_TEST_FLAG")
                .map(String::as_str),
            Some("1")
        );
        // Connection-reuse (Phase 1): set_model + set_permission_mode issue
        // `thread/resume` on the shared subprocess instead of re-spawning.
        // Only the original spawn is reflected in the launch counter.
        assert_eq!(fs::read_to_string(&launch_count_path).unwrap(), "1");

        executor.close_session(&session).await.unwrap();
        fs::write(&request_log_path, "").unwrap();

        let resumed_executor = CodexAgentExecutor::new(script_path, store);
        let resumed = resumed_executor
            .resume_session(
                session.id.clone(),
                ResumeHints {
                    vendor_cursor: Some(session.id.as_str().to_string()),
                    workdir: None,
                    metadata: BTreeMap::new(),
                },
            )
            .await
            .unwrap();
        assert_eq!(resumed.id, session.id);
        assert_eq!(resumed.workdir, workdir);

        let _events = collect_events(
            resumed_executor
                .send_message(
                    &resumed,
                    UserMessage {
                        content: "resume check".to_string(),
                        task_id: None,
                        attachments: Vec::new(),
                        parent_tool_use_id: None,
                        injected_tools: Vec::new(),
                    },
                )
                .await
                .unwrap(),
        )
        .await;

        let request_log = fs::read_to_string(&request_log_path).unwrap();
        assert!(request_log.contains(r#""method":"thread/resume""#));
        assert!(request_log.contains(r#""model":"gpt-5.4-codex""#));
        assert!(request_log.contains(r#""approvalPolicy":"never""#));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn command_execution_starts_as_bash_tool() {
        let event = translate_item(
            &CodexItem::CommandExecution {
                id: "cmd-1".to_string(),
                command: "ls".to_string(),
                aggregated_output: None,
                exit_code: None,
                status: None,
                data: Map::new(),
            },
            "gpt-5",
        )
        .expect("command execution should emit a tool-start event");

        match event {
            ExecutorEvent::ToolCallStart { name, .. } => assert_eq!(name, "Bash"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn command_execution_carries_terminal_interaction_into_bash_input() {
        let event = translate_item(
            &CodexItem::CommandExecution {
                id: "cmd-2".to_string(),
                command: "python3 -c 'input()'".to_string(),
                aggregated_output: None,
                exit_code: None,
                status: Some("running".to_string()),
                data: Map::from_iter([(
                    "terminalInteraction".to_string(),
                    serde_json::json!({
                        "prompt": "Enter stdin",
                        "placeholder": "stdin"
                    }),
                )]),
            },
            "gpt-5",
        )
        .expect("command execution should emit a tool-start event");

        match event {
            ExecutorEvent::ToolCallStart { input, .. } => {
                assert_eq!(
                    input,
                    serde_json::json!({
                        "command": "python3 -c 'input()'",
                        "terminalInteraction": {
                            "prompt": "Enter stdin",
                            "placeholder": "stdin"
                        }
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn web_search_starts_as_websearch_tool() {
        let event = translate_item(
            &CodexItem::WebSearch {
                id: "search-1".to_string(),
                query: "rpc parity".to_string(),
            },
            "gpt-5",
        )
        .expect("web search should emit a tool-start event");

        match event {
            ExecutorEvent::ToolCallStart { name, .. } => assert_eq!(name, "WebSearch"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn dynamic_tool_call_starts_as_generic_tool() {
        let event = translate_item(
            &CodexItem::DynamicToolCall {
                id: "dynamic-tool-1".to_string(),
                tool: "acme_lookup".to_string(),
                arguments: Some(serde_json::json!({
                    "query": "rpc parity"
                })),
                status: Some("running".to_string()),
                content_items: Vec::new(),
                error: None,
            },
            "gpt-5",
        )
        .expect("dynamic tool call should emit a tool-start event");

        match event {
            ExecutorEvent::ToolCallStart { name, input, .. } => {
                assert_eq!(name, "acme_lookup");
                assert_eq!(input, serde_json::json!({ "query": "rpc parity" }));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn reasoning_summary_emits_condensed_thinking_delta() {
        let event = translate_reasoning_summary(&CodexItem::Reasoning {
            id: "reason-1".to_string(),
            text: "Longer private reasoning".to_string(),
            summary: vec![
                serde_json::json!({ "text": "Check current adapter behavior." }),
                serde_json::json!({ "content": "Emit a condensed thinking delta." }),
            ],
        })
        .expect("reasoning summary should emit a thinking delta");

        match event {
            ExecutorEvent::StreamDelta { kind, content } => {
                assert_eq!(kind, DeltaKind::Thinking);
                assert_eq!(
                    content,
                    "Check current adapter behavior.\nEmit a condensed thinking delta."
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn image_generation_starts_as_image_generation_tool() {
        let event = translate_item(
            &CodexItem::ImageGeneration {
                id: "img-gen-1".to_string(),
                data: serde_json::Map::from_iter([
                    (
                        "prompt".to_string(),
                        Value::String("A parity gate robot".to_string()),
                    ),
                    (
                        "model".to_string(),
                        Value::String("gpt-image-1".to_string()),
                    ),
                ]),
            },
            "gpt-5",
        )
        .expect("image generation should emit a tool-start event");

        match event {
            ExecutorEvent::ToolCallStart { name, input, .. } => {
                assert_eq!(name, "image_generation");
                assert_eq!(
                    input,
                    serde_json::json!({
                        "prompt": "A parity gate robot",
                        "model": "gpt-image-1"
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn image_view_starts_as_screenshot_tool() {
        let event = translate_item(
            &CodexItem::ImageView {
                id: "img-view-1".to_string(),
                data: serde_json::Map::from_iter([
                    (
                        "image_url".to_string(),
                        Value::String("https://example.com/screenshot.png".to_string()),
                    ),
                    ("screen_size".to_string(), serde_json::json!([1440, 900])),
                ]),
            },
            "gpt-5",
        )
        .expect("image view should emit a tool-start event");

        match event {
            ExecutorEvent::ToolCallStart { name, input, .. } => {
                assert_eq!(name, "screenshot");
                assert_eq!(
                    input,
                    serde_json::json!({
                        "image_url": "https://example.com/screenshot.png",
                        "screen_size": [1440, 900]
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn mcp_tool_call_progress_emits_tool_input_delta() {
        let event = translate_mcp_tool_call_progress(&CodexMcpToolCallProgress {
            data: Map::from_iter([
                (
                    "item".to_string(),
                    serde_json::json!({
                        "id": "mcp-1"
                    }),
                ),
                (
                    "progress".to_string(),
                    serde_json::json!({
                        "message": "Waiting for MCP server"
                    }),
                ),
            ]),
        })
        .expect("mcp tool progress should emit a tool delta");

        match event {
            ExecutorEvent::ToolCallInputDelta {
                tool_use_id,
                json_patch,
            } => {
                assert_eq!(tool_use_id, "mcp-1");
                assert_eq!(
                    json_patch,
                    serde_json::json!({
                        "message": "Waiting for MCP server"
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn command_exec_output_delta_decodes_and_accumulates_stdout_and_stderr() {
        let mut accumulated = HashMap::new();

        let stdout_event = translate_command_exec_output_delta(
            &CodexCommandExecOutputDelta {
                data: Map::from_iter([
                    ("id".to_string(), Value::String("cmd-1".to_string())),
                    ("stream".to_string(), Value::String("stdout".to_string())),
                    ("chunk".to_string(), Value::String("aGVsbG8K".to_string())),
                ]),
            },
            &mut accumulated,
        )
        .expect("stdout delta should emit a tool delta");

        match stdout_event {
            ExecutorEvent::ToolCallInputDelta {
                tool_use_id,
                json_patch,
            } => {
                assert_eq!(tool_use_id, "cmd-1");
                assert_eq!(json_patch, serde_json::json!({ "stdout": "hello\n" }));
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let stderr_event = translate_command_exec_output_delta(
            &CodexCommandExecOutputDelta {
                data: Map::from_iter([
                    ("id".to_string(), Value::String("cmd-1".to_string())),
                    ("stream".to_string(), Value::String("stderr".to_string())),
                    ("chunk".to_string(), Value::String("d2Fybg==".to_string())),
                ]),
            },
            &mut accumulated,
        )
        .expect("stderr delta should emit a tool delta");

        match stderr_event {
            ExecutorEvent::ToolCallInputDelta {
                tool_use_id,
                json_patch,
            } => {
                assert_eq!(tool_use_id, "cmd-1");
                assert_eq!(json_patch, serde_json::json!({ "stderr": "warn" }));
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let second_stdout_event = translate_command_exec_output_delta(
            &CodexCommandExecOutputDelta {
                data: Map::from_iter([
                    ("id".to_string(), Value::String("cmd-1".to_string())),
                    ("stream".to_string(), Value::String("stdout".to_string())),
                    ("chunk".to_string(), Value::String("d29ybGQ=".to_string())),
                ]),
            },
            &mut accumulated,
        )
        .expect("second stdout delta should emit a tool delta");

        match second_stdout_event {
            ExecutorEvent::ToolCallInputDelta {
                tool_use_id,
                json_patch,
            } => {
                assert_eq!(tool_use_id, "cmd-1");
                assert_eq!(json_patch, serde_json::json!({ "stdout": "hello\nworld" }));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn plan_delta_emits_accumulated_tool_input_delta() {
        let mut accumulated = HashMap::new();
        let delta = CodexPlanDelta {
            data: Map::from_iter([
                (
                    "item".to_string(),
                    serde_json::json!({
                        "id": "plan-1"
                    }),
                ),
                (
                    "delta".to_string(),
                    Value::String("Break the task".to_string()),
                ),
            ]),
        };

        let event = translate_plan_delta(&delta, &mut accumulated)
            .expect("plan delta should emit a tool delta");

        match event {
            ExecutorEvent::ToolCallInputDelta {
                tool_use_id,
                json_patch,
            } => {
                assert_eq!(tool_use_id, "plan-1");
                assert_eq!(
                    json_patch,
                    serde_json::json!({
                        "explanation": "Break the task"
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let second_delta = CodexPlanDelta {
            data: Map::from_iter([
                ("itemId".to_string(), Value::String("plan-1".to_string())),
                (
                    "delta".to_string(),
                    Value::String(" into checkpoints".to_string()),
                ),
            ]),
        };

        let second_event = translate_plan_delta(&second_delta, &mut accumulated)
            .expect("second plan delta should emit a tool delta");

        match second_event {
            ExecutorEvent::ToolCallInputDelta {
                tool_use_id,
                json_patch,
            } => {
                assert_eq!(tool_use_id, "plan-1");
                assert_eq!(
                    json_patch,
                    serde_json::json!({
                        "explanation": "Break the task into checkpoints"
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn file_change_completes_as_codex_patch_tool() {
        let events = translate_item_completed(&CodexItem::FileChange {
            id: "patch-1".to_string(),
            changes: vec![crate::stream::CodexFileChange {
                path: "src/main.rs".to_string(),
                kind: "update".to_string(),
                diff: Some("@@ -1 +1 @@\n-old\n+new".to_string()),
            }],
            status: Some("completed".to_string()),
        });

        match &events[0] {
            ExecutorEvent::ToolCallStart { name, input, .. } => {
                assert_eq!(name, "CodexPatch");
                assert_eq!(
                    input,
                    &serde_json::json!({
                        "changes": {
                            "src/main.rs": {
                                "kind": "update",
                                "diff": "@@ -1 +1 @@\n-old\n+new"
                            }
                        }
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }

        match &events[1] {
            ExecutorEvent::ToolResult {
                tool_use_id,
                output: Ok(output),
            } => {
                assert_eq!(tool_use_id, "patch-1");
                assert_eq!(output, "Applied update to src/main.rs");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn image_view_completion_emits_screenshot_result() {
        let events = translate_item_completed(&CodexItem::ImageView {
            id: "img-view-1".to_string(),
            data: serde_json::Map::from_iter([
                (
                    "image_url".to_string(),
                    Value::String("https://example.com/screenshot.png".to_string()),
                ),
                ("screen_size".to_string(), serde_json::json!([1440, 900])),
            ]),
        });

        match &events[0] {
            ExecutorEvent::ToolResult {
                tool_use_id,
                output: Ok(output),
            } => {
                assert_eq!(tool_use_id, "img-view-1");
                assert_eq!(
                    serde_json::from_str::<Value>(output)
                        .expect("screenshot result should be json"),
                    serde_json::json!({
                        "type": "screenshot",
                        "image_url": "https://example.com/screenshot.png",
                        "screen_size": [1440, 900]
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn plan_starts_as_update_plan_tool() {
        let event = translate_item(
            &CodexItem::Plan {
                id: "plan-1".to_string(),
                explanation: Some("Break the task down".to_string()),
                items: vec![
                    crate::stream::CodexPlanItem {
                        content: Some("Inspect current mapping".to_string()),
                        text: None,
                        step: None,
                        description: None,
                        status: Some("completed".to_string()),
                    },
                    crate::stream::CodexPlanItem {
                        content: None,
                        text: Some("Add frontend tool translation".to_string()),
                        step: None,
                        description: None,
                        status: Some("running".to_string()),
                    },
                ],
            },
            "gpt-5",
        )
        .expect("plan item should emit a tool-start event");

        match event {
            ExecutorEvent::ToolCallStart { name, input, .. } => {
                assert_eq!(name, "update_plan");
                assert_eq!(
                    input,
                    serde_json::json!({
                        "todos": [
                            { "content": "Inspect current mapping", "status": "completed" },
                            { "content": "Add frontend tool translation", "status": "in_progress" }
                        ],
                        "explanation": "Break the task down"
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn turn_plan_update_starts_as_update_plan_tool() {
        let event = translate_turn_plan_update(
            "turn-plan-1",
            &crate::stream::CodexTurnPlanUpdate {
                explanation: Some("Track the execution plan".to_string()),
                items: vec![
                    crate::stream::CodexPlanItem {
                        content: Some("Inspect current mapping".to_string()),
                        text: None,
                        step: None,
                        description: None,
                        status: Some("completed".to_string()),
                    },
                    crate::stream::CodexPlanItem {
                        content: None,
                        text: None,
                        step: Some("Forward top-level plan updates".to_string()),
                        description: None,
                        status: Some("inProgress".to_string()),
                    },
                ],
                ..Default::default()
            },
        );

        match event {
            ExecutorEvent::ToolCallStart {
                tool_use_id,
                name,
                input,
                partial,
            } => {
                assert_eq!(tool_use_id, "turn-plan-1");
                assert_eq!(name, "update_plan");
                assert!(partial);
                assert_eq!(
                    input,
                    serde_json::json!({
                        "todos": [
                            { "content": "Inspect current mapping", "status": "completed" },
                            { "content": "Forward top-level plan updates", "status": "in_progress" }
                        ],
                        "explanation": "Track the execution plan"
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn todo_list_starts_as_update_plan_tool() {
        let event = translate_item(
            &CodexItem::TodoList {
                id: "todo-1".to_string(),
                items: vec![
                    crate::stream::CodexTodoItem {
                        text: "Read the target file".to_string(),
                        completed: true,
                    },
                    crate::stream::CodexTodoItem {
                        text: "Map todo_list into tool events".to_string(),
                        completed: false,
                    },
                ],
            },
            "gpt-5",
        )
        .expect("todo list should emit a tool-start event");

        match event {
            ExecutorEvent::ToolCallStart { name, input, .. } => {
                assert_eq!(name, "update_plan");
                assert_eq!(
                    input,
                    serde_json::json!({
                        "todos": [
                            { "content": "Read the target file", "status": "completed" },
                            { "content": "Map todo_list into tool events", "status": "pending" }
                        ]
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn guardian_review_starts_as_permission_request_with_risk_level() {
        let event = translate_item(
            &CodexItem::AutoApprovalReview {
                id: "guardian-1".to_string(),
                data: serde_json::Map::from_iter([
                    ("riskLevel".to_string(), Value::String("medium".to_string())),
                    ("target".to_string(), Value::String("Bash".to_string())),
                ]),
            },
            "gpt-5",
        )
        .expect("guardian review should emit a permission request");

        match event {
            ExecutorEvent::PermissionRequest {
                request_id,
                tool_name,
                tool_input,
            } => {
                assert_eq!(request_id, "guardian-1");
                assert_eq!(tool_name, "CodexGuardian");
                assert_eq!(
                    tool_input,
                    serde_json::json!({
                        "riskLevel": "medium",
                        "target": "Bash",
                        "__codex_guardian_review": true
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn collab_tool_call_starts_as_task_tool() {
        let event = translate_item(
            &CodexItem::CollabAgentToolCall {
                id: "task-1".to_string(),
                tool: "spawn_agent".to_string(),
                status: Some("in_progress".to_string()),
                prompt: Some("Inspect the frontend Task mapping".to_string()),
            },
            "gpt-5.4",
        )
        .expect("collab tool call should emit a task tool-start event");

        match event {
            ExecutorEvent::ToolCallStart { name, input, .. } => {
                assert_eq!(name, "Task");
                assert_eq!(
                    input,
                    serde_json::json!({
                        "action": "spawnAgent",
                        "prompt": "Inspect the frontend Task mapping",
                        "model": "gpt-5.4"
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn plan_completion_emits_tool_result() {
        let events = translate_item_completed(&CodexItem::Plan {
            id: "plan-1".to_string(),
            explanation: None,
            items: Vec::new(),
        });

        match &events[0] {
            ExecutorEvent::ToolResult {
                tool_use_id,
                output: Ok(output),
            } => {
                assert_eq!(tool_use_id, "plan-1");
                assert_eq!(output, "Plan updated");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn todo_list_completion_emits_tool_result() {
        let events = translate_item_completed(&CodexItem::TodoList {
            id: "todo-1".to_string(),
            items: vec![crate::stream::CodexTodoItem {
                text: "Done".to_string(),
                completed: true,
            }],
        });

        match &events[0] {
            ExecutorEvent::ToolResult {
                tool_use_id,
                output: Ok(output),
            } => {
                assert_eq!(tool_use_id, "todo-1");
                assert_eq!(output, "Updated 1 todo item(s)");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn guardian_review_completion_emits_native_completion_payload() {
        let events = translate_item_completed(&CodexItem::AutoApprovalReview {
            id: "guardian-1".to_string(),
            data: serde_json::Map::from_iter([
                ("riskLevel".to_string(), Value::String("medium".to_string())),
                ("autoApproved".to_string(), Value::Bool(true)),
            ]),
        });

        match &events[0] {
            ExecutorEvent::NativeEvent { provider, payload } => {
                assert_eq!(provider.as_ref(), VENDOR);
                assert_eq!(
                    payload,
                    &serde_json::json!({
                        "kind": "codex_guardian_review_completed",
                        "request_id": "guardian-1",
                        "review": {
                            "riskLevel": "medium",
                            "autoApproved": true
                        }
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn token_usage_notification_maps_last_usage_to_context_usage_native_event() {
        let event = codex_context_usage_native_event(&json!({
            "total": {
                "totalTokens": 180_000,
                "inputTokens": 150_000,
                "cachedInputTokens": 20_000,
                "outputTokens": 8_000,
                "reasoningOutputTokens": 2_000
            },
            "last": {
                "totalTokens": 158_000,
                "inputTokens": 130_000,
                "cachedInputTokens": 18_000,
                "outputTokens": 7_000,
                "reasoningOutputTokens": 3_000
            },
            "modelContextWindow": 258_400
        }))
        .expect("context usage event");

        match event {
            ExecutorEvent::NativeEvent { provider, payload } => {
                assert_eq!(provider.as_ref(), VENDOR);
                assert_eq!(
                    payload,
                    json!({
                        "kind": "context_usage",
                        "total_tokens": 158_000_u64,
                        "max_tokens": 258_400_u64,
                        "raw_max_tokens": 258_400_u64,
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn collab_tool_call_completion_emits_tool_result() {
        let events = translate_item_completed(&CodexItem::CollabAgentToolCall {
            id: "task-1".to_string(),
            tool: "spawnAgent".to_string(),
            status: Some("completed".to_string()),
            prompt: Some("Inspect the frontend Task mapping".to_string()),
        });

        match &events[0] {
            ExecutorEvent::ToolResult {
                tool_use_id,
                output: Ok(output),
            } => {
                assert_eq!(tool_use_id, "task-1");
                assert_eq!(output, "Task spawnAgent completed");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn dynamic_tool_call_completion_emits_content_items_result() {
        let events = translate_item_completed(&CodexItem::DynamicToolCall {
            id: "dynamic-tool-1".to_string(),
            tool: "acme_lookup".to_string(),
            arguments: Some(serde_json::json!({
                "query": "rpc parity"
            })),
            status: Some("completed".to_string()),
            content_items: vec![serde_json::json!({
                "type": "text",
                "text": "Found a match"
            })],
            error: None,
        });

        match &events[0] {
            ExecutorEvent::ToolResult {
                tool_use_id,
                output: Ok(output),
            } => {
                assert_eq!(tool_use_id, "dynamic-tool-1");
                assert_eq!(
                    output,
                    &serde_json::json!({
                        "contentItems": [
                            {
                                "type": "text",
                                "text": "Found a match"
                            }
                        ]
                    })
                    .to_string()
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn terminal_interaction_response_extracts_text() {
        assert_eq!(
            codex_terminal_interaction_response_text(&serde_json::json!({
                "action": "accept",
                "content": {
                    "text": "hello stdin"
                }
            })),
            Some("hello stdin".to_string())
        );
        assert_eq!(
            codex_terminal_interaction_response_text(&serde_json::json!({
                "action": "cancel",
                "content": {
                    "text": "hello stdin"
                }
            })),
            None
        );
    }

    #[test]
    fn turn_plan_tool_uses_stable_per_turn_fallback_id() {
        assert_eq!(
            codex_turn_plan_tool_use_id(None, 0),
            "__codex_turn_plan_1".to_string()
        );
        assert_eq!(
            codex_turn_plan_tool_use_id(Some("vendor-plan-1"), 3),
            "vendor-plan-1".to_string()
        );

        match codex_plan_tool_result("__codex_turn_plan_2".to_string()) {
            ExecutorEvent::ToolResult {
                tool_use_id,
                output: Ok(output),
            } => {
                assert_eq!(tool_use_id, "__codex_turn_plan_2");
                assert_eq!(output, "Plan updated");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn update_usage_from_app_server_total_payload() {
        let mut usage = TokenUsage::default();
        update_usage_from_object(
            &serde_json::json!({
                "total": {
                    "inputTokens": 1200,
                    "outputTokens": 80,
                    "cachedInputTokens": 640,
                    "reasoningOutputTokens": 32
                }
            }),
            &mut usage,
        );

        assert_eq!(usage.input_tokens, 1200);
        assert_eq!(usage.output_tokens, 80);
        assert_eq!(usage.cache_read_tokens, 640);
        assert_eq!(usage.reasoning_tokens, 32);
    }
}
