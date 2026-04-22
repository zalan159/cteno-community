use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use multi_agent_protocol::{
    ClaimStatus, DispatchStatus, RoleSpec, RoleTaskRequest, TaskDispatch, WorkspaceEvent,
    WorkspaceInstanceParams, WorkspaceProfile, WorkspaceSpec, WorkspaceState, WorkspaceTemplate,
    WorkspaceTurnPlan, WorkspaceTurnRequest, WorkspaceWorkflowVoteResponse,
    WorkspaceWorkflowVoteWindow, build_workflow_entry_plan, decide_coordinator_action,
    direct_workspace_turn_plan, instantiate_workspace, resolve_workflow_vote_candidate_role_ids,
    should_approve_workflow_vote, synthesize_workflow_vote_response,
};
use multi_agent_runtime_core::{RuntimeError, WorkspaceRuntime};
use multi_agent_runtime_local::{
    LocalPersistenceError, LocalWorkspacePersistence, PersistedProviderState,
};
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex as TokioMutex, mpsc, oneshot};
use tokio::time::timeout;

// ---------------------------------------------------------------------------
// Public option / result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudePermissionMode {
    Default,
    Auto,
    AcceptEdits,
    BypassPermissions,
    DontAsk,
    Plan,
}

impl ClaudePermissionMode {
    fn as_cli_value(self) -> &'static str {
        match self {
            ClaudePermissionMode::Default => "default",
            ClaudePermissionMode::Auto => "auto",
            ClaudePermissionMode::AcceptEdits => "acceptEdits",
            ClaudePermissionMode::BypassPermissions => "bypassPermissions",
            ClaudePermissionMode::DontAsk => "dontAsk",
            ClaudePermissionMode::Plan => "plan",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClaudeWorkspaceOptions {
    pub claude_path: PathBuf,
    pub permission_mode: ClaudePermissionMode,
    pub working_directory: Option<PathBuf>,
    pub additional_directories: Vec<PathBuf>,
    pub turn_timeout: Duration,
    pub max_workflow_followups: usize,
    /// Timeout for the initialize handshake when spawning a persistent
    /// subprocess.  Defaults to 60 s.
    pub spawn_ready_timeout: Duration,
    /// Optional MCP tool name to use as the CLI `--permission-prompt-tool`.
    /// If `None` (default), the flag is omitted and the CLI will fall back to
    /// the `can_use_tool` control_request flow, which this adapter handles.
    /// **Do not set this to `"stdio"`** — the CLI treats it as an MCP tool
    /// name, not a transport protocol.
    pub permission_prompt_tool_name: Option<String>,
    /// Optional agents map forwarded via the `initialize` control_request.
    pub agents: Option<Value>,
    /// Optional `excludeDynamicSections` flag forwarded via `initialize`.
    pub exclude_dynamic_sections: Option<bool>,
    /// Optional list of SDK-in-process MCP server names forwarded via
    /// `initialize` as `sdkMcpServers`. This adapter does not host SDK-side
    /// MCP servers, so callers generally leave this empty; if Claude ever
    /// requests a `mcp_message` for a server we don't know, we respond with a
    /// control_response error (rather than stalling).
    pub sdk_mcp_servers: Option<Vec<String>>,
    /// Optional system prompt forwarded via `initialize`.
    pub initialize_system_prompt: Option<String>,
    /// Optional system-prompt append forwarded via `initialize`.
    pub initialize_append_system_prompt: Option<String>,
}

impl Default for ClaudeWorkspaceOptions {
    fn default() -> Self {
        Self {
            claude_path: PathBuf::from("claude"),
            permission_mode: ClaudePermissionMode::BypassPermissions,
            working_directory: None,
            additional_directories: Vec::new(),
            turn_timeout: Duration::from_secs(240),
            max_workflow_followups: 0,
            spawn_ready_timeout: Duration::from_secs(60),
            permission_prompt_tool_name: None,
            agents: None,
            exclude_dynamic_sections: None,
            sdk_mcp_servers: None,
            initialize_system_prompt: None,
            initialize_append_system_prompt: None,
        }
    }
}

#[derive(Debug)]
pub struct ClaudeRoleTaskRun {
    pub dispatch: TaskDispatch,
    pub events: Vec<WorkspaceEvent>,
}

#[derive(Debug)]
pub struct ClaudeWorkspaceTurnRun {
    pub request: WorkspaceTurnRequest,
    pub plan: WorkspaceTurnPlan,
    pub workflow_vote_window: Option<WorkspaceWorkflowVoteWindow>,
    pub workflow_vote_responses: Vec<WorkspaceWorkflowVoteResponse>,
    pub dispatches: Vec<TaskDispatch>,
    pub events: Vec<WorkspaceEvent>,
    pub state: WorkspaceState,
}

#[derive(Debug, Error)]
pub enum ClaudeAdapterError {
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("unknown role: {0}")]
    UnknownRole(String),
    #[error("claude process missing stdout")]
    MissingStdout,
    #[error("claude process missing stderr")]
    MissingStderr,
    #[error("claude io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("claude task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("claude stderr: {0}")]
    Process(String),
    #[error("claude failed turn: {0}")]
    TurnFailed(String),
    #[error("claude timed out after {timeout:?}\n{debug}")]
    TimedOut { timeout: Duration, debug: String },
    #[error("local persistence error: {0}")]
    LocalPersistence(#[from] LocalPersistenceError),
    #[error("initialize handshake failed: {0}")]
    InitializeFailed(String),
    #[error("persistent process exited unexpectedly")]
    ProcessExited,
    #[error("control response error: {0}")]
    ControlError(String),
    #[error(
        "claude CLI version {found} is unsupported (minimum {minimum}); set \
         CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK=1 to override"
    )]
    UnsupportedCliVersion { found: String, minimum: String },
}

/// Minimum `claude` CLI version this adapter talks to.  Mirrors the Python SDK.
pub const MINIMUM_CLAUDE_CLI_VERSION: &str = "2.0.0";

/// Entry-point tag written into `CLAUDE_CODE_ENTRYPOINT` when spawning the CLI.
/// The Python SDK uses `sdk-py`, the TS SDK `sdk-ts`; this adapter reports
/// `sdk-rust-cteno` so server-side analytics can tell the Rust adapter apart.
pub const CLAUDE_CODE_ENTRYPOINT_TAG: &str = "sdk-rust-cteno";

/// Run `claude --version` (or `-v`) and compare against
/// [`MINIMUM_CLAUDE_CLI_VERSION`]. Returns `Ok(())` if the version is
/// acceptable (or couldn't be parsed — we don't want a quirky build of the
/// CLI to prevent startup). `UnsupportedCliVersion` is returned only when the
/// version was clearly parsed *and* is below the minimum.
pub async fn check_cli_version(claude_path: &std::path::Path) -> Result<(), ClaudeAdapterError> {
    if std::env::var_os("CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK").is_some() {
        return Ok(());
    }
    let output = match timeout(
        Duration::from_secs(4),
        Command::new(claude_path).arg("--version").output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        Ok(Err(_)) | Err(_) => return Ok(()), // couldn't probe – don't block startup
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    // Match the first semver-looking token.
    let mut digits = String::new();
    let mut found: Option<String> = None;
    for ch in trimmed.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            digits.push(ch);
        } else if !digits.is_empty() {
            break;
        }
    }
    if digits.matches('.').count() >= 2 {
        found = Some(digits);
    }
    let Some(found) = found else {
        return Ok(());
    };
    if version_is_below(&found, MINIMUM_CLAUDE_CLI_VERSION) {
        return Err(ClaudeAdapterError::UnsupportedCliVersion {
            found,
            minimum: MINIMUM_CLAUDE_CLI_VERSION.to_string(),
        });
    }
    Ok(())
}

fn version_is_below(found: &str, minimum: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.split('.')
            .take(3)
            .map(|p| p.parse::<u32>().unwrap_or(0))
            .collect()
    };
    let a = parse(found);
    let b = parse(minimum);
    for i in 0..std::cmp::max(a.len(), b.len()) {
        let av = a.get(i).copied().unwrap_or(0);
        let bv = b.get(i).copied().unwrap_or(0);
        if av < bv {
            return true;
        }
        if av > bv {
            return false;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Persistent subprocess handle
// ---------------------------------------------------------------------------

/// A long-lived Claude CLI subprocess that communicates via the streaming
/// JSON control protocol (`--input-format stream-json --permission-prompt-tool
/// stdio`).
#[allow(dead_code)]
struct ClaudeSessionProcess {
    /// The Claude-native session id obtained from the `system:init` frame.
    native_session_id: String,
    /// Handle kept alive so the child process is not dropped.
    _child: Child,
    /// Guarded stdin writer – only one turn writes at a time.
    stdin: Arc<TokioMutex<tokio::process::ChildStdin>>,
    /// Atomic counter for generating control request ids.  Reserved for
    /// future use when sending additional control requests (e.g. interrupt).
    request_counter: std::sync::atomic::AtomicU64,
    /// Waiters for control_response frames, keyed by request_id.
    /// The stdout reader task resolves these.  Used during initialization
    /// and reserved for future control request/response exchanges.
    pending_control: Arc<TokioMutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>,
    /// The sender half for turn-event mpsc channels. When a turn is active
    /// the holder sets `active_turn_tx` so the reader task forwards parsed
    /// `ClaudeJsonEvent` frames to it.
    active_turn_tx: Arc<TokioMutex<Option<mpsc::UnboundedSender<TurnEvent>>>>,
    /// Stderr tail (last N lines) kept for diagnostics on timeout / error.
    stderr_tail: Arc<Mutex<Vec<String>>>,
}

/// Events forwarded from the stdout reader to the current turn consumer.
#[derive(Debug)]
#[allow(dead_code)]
enum TurnEvent {
    JsonEvent(ClaudeJsonEvent),
    /// The reader saw `control_request` with subtype `can_use_tool`.
    PermissionRequest {
        request_id: String,
        tool_name: String,
        input: Value,
    },
    /// stdout EOF or parse-fatal.
    Eof,
}

/// Input bundle for [`ClaudeSessionProcess::spawn`]. Packages all
/// spawn-time parameters so we can grow the set without changing every
/// call site.
struct ClaudeSpawnParams<'a> {
    claude_path: &'a PathBuf,
    permission_mode: ClaudePermissionMode,
    permission_prompt_tool_name: Option<&'a str>,
    model: &'a str,
    working_directory: Option<&'a PathBuf>,
    additional_directories: &'a [PathBuf],
    resume_session_id: Option<&'a str>,
    spawn_ready_timeout: Duration,
    agents: Option<&'a Value>,
    exclude_dynamic_sections: Option<bool>,
    sdk_mcp_servers: Option<&'a [String]>,
    initialize_system_prompt: Option<&'a str>,
    initialize_append_system_prompt: Option<&'a str>,
}

impl ClaudeSessionProcess {
    /// Spawn the persistent subprocess, perform the initialize handshake, and
    /// return the ready handle.
    async fn spawn(params: ClaudeSpawnParams<'_>) -> Result<Self, ClaudeAdapterError> {
        let ClaudeSpawnParams {
            claude_path,
            permission_mode,
            permission_prompt_tool_name,
            model,
            working_directory,
            additional_directories,
            resume_session_id,
            spawn_ready_timeout,
            agents,
            exclude_dynamic_sections,
            sdk_mcp_servers,
            initialize_system_prompt,
            initialize_append_system_prompt,
        } = params;

        // Verify CLI version up-front so we fail fast with a readable error.
        check_cli_version(claude_path).await?;

        let mut command = Command::new(claude_path);
        command
            .arg("--output-format")
            .arg("stream-json")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--dangerously-skip-permissions")
            .arg("--permission-mode")
            .arg(permission_mode.as_cli_value())
            .arg("--model")
            .arg(model);

        // `--permission-prompt-tool stdio` is a magic sentinel (see the
        // official `@anthropic-ai/claude-agent-sdk` sdk.mjs: it literally
        // pushes `["--permission-prompt-tool", "stdio"]` whenever a
        // `canUseTool` callback is configured). It tells the CLI to route
        // permission decisions through `control_request can_use_tool` on the
        // stdio protocol instead of invoking an MCP tool. Without this flag
        // the CLI falls back to its internal permission logic and refuses
        // writes outside the cwd, surfacing as "Stream closed".
        //
        // Callers may override this with a real MCP tool name if they want to
        // route permission prompts to an actual MCP server instead.
        let prompt_tool = permission_prompt_tool_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("stdio");
        command.arg("--permission-prompt-tool").arg(prompt_tool);

        if let Some(wd) = working_directory {
            command.current_dir(wd);
        }
        for dir in additional_directories {
            command.arg("--add-dir").arg(dir);
        }
        if let Some(sid) = resume_session_id {
            command.arg("--resume").arg(sid);
        }

        // Environment adjustments:
        //   * tag this spawn so server-side analytics can distinguish the
        //     Rust adapter from sdk-py / sdk-ts,
        //   * drop inherited CLAUDECODE so the CLI doesn't mistake itself for
        //     a nested Claude Code child process (Python SDK #573).
        command.env_remove("CLAUDECODE");
        command.env("CLAUDE_CODE_ENTRYPOINT", CLAUDE_CODE_ENTRYPOINT_TAG);
        command.env("CLAUDE_AGENT_SDK_VERSION", env!("CARGO_PKG_VERSION"));

        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("claude stdin should be piped");
        let stdout = child
            .stdout
            .take()
            .ok_or(ClaudeAdapterError::MissingStdout)?;
        let stderr = child
            .stderr
            .take()
            .ok_or(ClaudeAdapterError::MissingStderr)?;

        let stdin = Arc::new(TokioMutex::new(stdin));
        let pending_control: Arc<
            TokioMutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
        > = Arc::new(TokioMutex::new(HashMap::new()));
        let active_turn_tx: Arc<TokioMutex<Option<mpsc::UnboundedSender<TurnEvent>>>> =
            Arc::new(TokioMutex::new(None));
        let stderr_tail = Arc::new(Mutex::new(Vec::<String>::new()));

        // --- oneshot for session_id from system:init ---
        let (init_tx, init_rx) = oneshot::channel::<String>();

        // --- start stderr reader ---
        let stderr_tail_clone = Arc::clone(&stderr_tail);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                push_tail(&stderr_tail_clone, line);
            }
        });

        // --- start stdout reader ---
        {
            let pending_control = Arc::clone(&pending_control);
            let active_turn_tx = Arc::clone(&active_turn_tx);
            let stdin_for_reader = Arc::clone(&stdin);
            tokio::spawn(stdout_reader_loop(
                BufReader::new(stdout),
                pending_control,
                active_turn_tx,
                Some(init_tx),
                stdin_for_reader,
            ));
        }

        // --- send initialize control request ---
        let mut init_body = serde_json::Map::new();
        init_body.insert(
            "subtype".to_string(),
            Value::String("initialize".to_string()),
        );
        init_body.insert("hooks".to_string(), Value::Null);
        if let Some(agents) = agents {
            init_body.insert("agents".to_string(), agents.clone());
        }
        if let Some(flag) = exclude_dynamic_sections {
            init_body.insert("excludeDynamicSections".to_string(), Value::Bool(flag));
        }
        if let Some(servers) = sdk_mcp_servers {
            init_body.insert(
                "sdkMcpServers".to_string(),
                Value::Array(servers.iter().map(|s| Value::String(s.clone())).collect()),
            );
        }
        if let Some(prompt) = initialize_system_prompt {
            init_body.insert(
                "systemPrompt".to_string(),
                Value::String(prompt.to_string()),
            );
        }
        if let Some(append) = initialize_append_system_prompt {
            init_body.insert(
                "appendSystemPrompt".to_string(),
                Value::String(append.to_string()),
            );
        }
        let init_request = json!({
            "type": "control_request",
            "request_id": "req_init",
            "request": Value::Object(init_body),
        });
        {
            let mut guard = stdin.lock().await;
            let payload = format!("{}\n", serde_json::to_string(&init_request).unwrap());
            guard.write_all(payload.as_bytes()).await?;
            guard.flush().await?;
        }

        // --- register a waiter for the initialize control_response ---
        let (ctrl_tx, ctrl_rx) = oneshot::channel();
        {
            let mut map = pending_control.lock().await;
            map.insert("req_init".to_string(), ctrl_tx);
        }

        // --- wait for both: control_response AND system:init session_id ---
        let native_session_id = timeout(spawn_ready_timeout, async {
            // Wait for control_response for req_init
            let ctrl_result = ctrl_rx.await.map_err(|_| {
                ClaudeAdapterError::InitializeFailed(
                    "control_response channel closed before init response".to_string(),
                )
            })?;
            ctrl_result.map_err(|e| ClaudeAdapterError::InitializeFailed(e))?;

            // Wait for session_id from system:init
            let session_id = init_rx.await.map_err(|_| {
                ClaudeAdapterError::InitializeFailed(
                    "session_id channel closed before system:init".to_string(),
                )
            })?;
            Ok::<String, ClaudeAdapterError>(session_id)
        })
        .await
        .map_err(|_| ClaudeAdapterError::TimedOut {
            timeout: spawn_ready_timeout,
            debug: format!(
                "initialize handshake timed out after {:?}",
                spawn_ready_timeout
            ),
        })??;

        // Always re-apply the requested runtime permission mode after
        // initialize. We start Claude with `--dangerously-skip-permissions`
        // so bypass can be enabled later, but that bootstrap flag should not
        // leave default/ask workspaces silently running in bypass semantics.
        let request_id = "req_spawn_set_mode";
        let set_mode_request = json!({
            "type": "control_request",
            "request_id": request_id,
            "request": {
                "subtype": "set_permission_mode",
                "mode": permission_mode.as_cli_value(),
            }
        });
        let (set_mode_tx, set_mode_rx) = oneshot::channel();
        {
            let mut map = pending_control.lock().await;
            map.insert(request_id.to_string(), set_mode_tx);
        }
        {
            let mut guard = stdin.lock().await;
            let payload = format!("{}\n", serde_json::to_string(&set_mode_request).unwrap());
            guard.write_all(payload.as_bytes()).await?;
            guard.flush().await?;
        }
        timeout(spawn_ready_timeout, set_mode_rx)
            .await
            .map_err(|_| ClaudeAdapterError::TimedOut {
                timeout: spawn_ready_timeout,
                debug: format!(
                    "set_permission_mode after initialize timed out after {:?}",
                    spawn_ready_timeout
                ),
            })?
            .map_err(|_| {
                ClaudeAdapterError::InitializeFailed(
                    "control_response channel closed before set_permission_mode response"
                        .to_string(),
                )
            })?
            .map_err(ClaudeAdapterError::InitializeFailed)?;

        Ok(Self {
            native_session_id,
            _child: child,
            stdin,
            request_counter: std::sync::atomic::AtomicU64::new(1),
            pending_control,
            active_turn_tx,
            stderr_tail,
        })
    }

    /// Send a user message and return a receiver for turn events.
    async fn send_user_message(
        &self,
        content: &str,
    ) -> Result<mpsc::UnboundedReceiver<TurnEvent>, ClaudeAdapterError> {
        let (tx, rx) = mpsc::unbounded_channel();

        // Install the turn receiver so the reader task forwards events.
        {
            let mut guard = self.active_turn_tx.lock().await;
            *guard = Some(tx);
        }

        let frame = json!({
            "type": "user",
            "session_id": "",
            "message": { "role": "user", "content": content },
            "parent_tool_use_id": null,
        });
        let payload = format!("{}\n", serde_json::to_string(&frame).unwrap());
        {
            let mut guard = self.stdin.lock().await;
            guard.write_all(payload.as_bytes()).await?;
            guard.flush().await?;
        }

        Ok(rx)
    }

    /// Respond to a permission control_request from the CLI.
    async fn respond_to_permission(
        &self,
        request_id: &str,
        allow: bool,
    ) -> Result<(), ClaudeAdapterError> {
        let response = json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "behavior": if allow { "allow" } else { "deny" },
                "updatedInput": {}
            }
        });
        let payload = format!("{}\n", serde_json::to_string(&response).unwrap());
        let mut guard = self.stdin.lock().await;
        guard.write_all(payload.as_bytes()).await?;
        guard.flush().await?;
        Ok(())
    }

    /// Detach the current turn receiver (called after turn completes).
    async fn clear_turn_receiver(&self) {
        let mut guard = self.active_turn_tx.lock().await;
        *guard = None;
    }
}

// ---------------------------------------------------------------------------
// Stdout reader – long-lived task
// ---------------------------------------------------------------------------

/// Reads stdout line by line from the persistent Claude subprocess and routes
/// frames to the appropriate consumer:
/// - `control_response` → pending_control waiters (by request_id)
/// - `control_request` (can_use_tool) → active turn channel as PermissionRequest
/// - `system:init` → init_tx (first time only, for session_id)
/// - everything else (assistant, result, etc.) → active turn channel
async fn stdout_reader_loop(
    mut reader: BufReader<tokio::process::ChildStdout>,
    pending_control: Arc<TokioMutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>,
    active_turn_tx: Arc<TokioMutex<Option<mpsc::UnboundedSender<TurnEvent>>>>,
    mut init_tx: Option<oneshot::Sender<String>>,
    stdin: Arc<TokioMutex<tokio::process::ChildStdin>>,
) {
    let mut line_buf = String::new();
    loop {
        line_buf.clear();
        match reader.read_line(&mut line_buf).await {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(_) => break,
        }

        let trimmed = line_buf.trim();
        if !trimmed.starts_with('{') {
            continue;
        }

        // First try to parse as a raw JSON value to inspect `type`.
        let raw: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let msg_type = raw.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match msg_type {
            // --- control_response from CLI (response to our control_request) ---
            "control_response" => {
                if let Some(response) = raw.get("response") {
                    let request_id = response
                        .get("request_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let mut map = pending_control.lock().await;
                    if let Some(tx) = map.remove(&request_id) {
                        let subtype = response
                            .get("subtype")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if subtype == "error" {
                            let err_msg = response
                                .get("error")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown control error")
                                .to_string();
                            let _ = tx.send(Err(err_msg));
                        } else {
                            let _ = tx.send(Ok(response.clone()));
                        }
                    }
                }
            }

            // --- control_request from CLI (permission prompt, hook callback, ...) ---
            "control_request" => {
                let request_id = raw
                    .get("request_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let subtype = raw
                    .get("request")
                    .and_then(|r| r.get("subtype"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                match subtype.as_str() {
                    "can_use_tool" => {
                        if let Some(request) = raw.get("request") {
                            let tool_name = request
                                .get("tool_name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let input = request.get("input").cloned().unwrap_or(Value::Null);

                            let guard = active_turn_tx.lock().await;
                            if let Some(tx) = guard.as_ref() {
                                let _ = tx.send(TurnEvent::PermissionRequest {
                                    request_id,
                                    tool_name,
                                    input,
                                });
                            }
                        }
                    }
                    "hook_callback" => {
                        // We don't yet support hook callbacks declared via
                        // `initialize.hooks`. Because we also don't send a
                        // non-null `hooks` payload, the CLI shouldn't fire
                        // this — but if it does, respond with a successful
                        // empty body so the CLI does not block waiting on us.
                        let response = json!({
                            "type": "control_response",
                            "response": {
                                "subtype": "success",
                                "request_id": request_id,
                                "async": false,
                                "hookSpecificOutput": null,
                            }
                        });
                        write_stdin_line(&stdin, &response).await;
                    }
                    "mcp_message" => {
                        // We don't host SDK-side MCP servers in this adapter.
                        // Respond with an explicit error so the CLI abandons
                        // the request immediately instead of stalling.
                        let response = json!({
                            "type": "control_response",
                            "response": {
                                "subtype": "error",
                                "request_id": request_id,
                                "error": "SDK MCP servers not configured in cteno Claude adapter",
                            }
                        });
                        write_stdin_line(&stdin, &response).await;
                    }
                    other => {
                        // Unknown / unsupported subtype: reply with a
                        // descriptive error rather than silently dropping.
                        let response = json!({
                            "type": "control_response",
                            "response": {
                                "subtype": "error",
                                "request_id": request_id,
                                "error": format!(
                                    "control_request subtype '{}' not implemented in cteno Claude adapter",
                                    other
                                ),
                            }
                        });
                        write_stdin_line(&stdin, &response).await;
                    }
                }
            }

            // --- regular SDK frames: system, assistant, result, user, etc. ---
            _ => {
                // Extract session_id from system:init for the initialization handshake.
                if msg_type == "system" {
                    let subtype = raw.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
                    if subtype == "init" {
                        if let Some(init_tx) = init_tx.take() {
                            if let Some(sid) = raw.get("session_id").and_then(|v| v.as_str()) {
                                let _ = init_tx.send(sid.to_string());
                            }
                        }
                    }
                }

                // Try to parse as ClaudeJsonEvent and forward to turn consumer.
                if let Ok(event) = serde_json::from_value::<ClaudeJsonEvent>(raw.clone()) {
                    let guard = active_turn_tx.lock().await;
                    if let Some(tx) = guard.as_ref() {
                        let _ = tx.send(TurnEvent::JsonEvent(event));
                    }
                }
            }
        }
    }

    // Signal EOF to the current turn consumer (if any).
    let guard = active_turn_tx.lock().await;
    if let Some(tx) = guard.as_ref() {
        let _ = tx.send(TurnEvent::Eof);
    }
}

// ---------------------------------------------------------------------------
// ClaudeWorkspace
// ---------------------------------------------------------------------------

pub struct ClaudeWorkspace {
    runtime: WorkspaceRuntime,
    options: ClaudeWorkspaceOptions,
    started: bool,
    role_session_ids: BTreeMap<String, String>,
    persistence: Option<LocalWorkspacePersistence>,
    restored_from_persistence: bool,
    /// Persistent subprocess handles, keyed by role_id.
    role_processes: BTreeMap<String, ClaudeSessionProcess>,
}

impl ClaudeWorkspace {
    pub fn new(spec: WorkspaceSpec, options: ClaudeWorkspaceOptions) -> Self {
        let persistence = LocalWorkspacePersistence::from_spec(&spec).ok();
        Self {
            runtime: WorkspaceRuntime::new(spec),
            options,
            started: false,
            role_session_ids: BTreeMap::new(),
            persistence,
            restored_from_persistence: false,
            role_processes: BTreeMap::new(),
        }
    }

    pub fn from_template(
        template: &WorkspaceTemplate,
        instance: &WorkspaceInstanceParams,
        profile: &WorkspaceProfile,
        options: ClaudeWorkspaceOptions,
    ) -> Self {
        Self::new(instantiate_workspace(template, instance, profile), options)
    }

    pub fn restore_from_local(
        cwd: impl AsRef<std::path::Path>,
        workspace_id: &str,
        options: ClaudeWorkspaceOptions,
    ) -> Result<Self, ClaudeAdapterError> {
        let persistence = LocalWorkspacePersistence::from_workspace(cwd, workspace_id);
        let spec = persistence.load_workspace_spec()?;
        let state = persistence.load_workspace_state()?;
        let history = persistence.load_events()?;
        let provider_state = persistence.load_provider_state()?;

        let mut workspace = Self::new(spec, options);
        workspace.runtime.restore_snapshot(state, history);
        workspace.role_session_ids = provider_state
            .member_bindings
            .into_iter()
            .map(|(role_id, binding)| (role_id, binding.provider_conversation_id))
            .collect();
        workspace.restored_from_persistence = true;
        Ok(workspace)
    }

    pub fn runtime(&self) -> &WorkspaceRuntime {
        &self.runtime
    }

    pub fn persistence_root(&self) -> Option<&std::path::Path> {
        self.persistence.as_ref().map(|p| p.root())
    }

    pub fn start(&mut self) -> Vec<WorkspaceEvent> {
        if self.started {
            return Vec::new();
        }

        self.started = true;
        if !self.restored_from_persistence {
            if let Some(persistence) = self.persistence.as_ref() {
                let _ = persistence.ensure_workspace_initialized(self.runtime.spec());
            }
        }
        let mut emitted = Vec::new();
        emitted.extend(self.runtime.start().emitted);
        emitted.extend(
            self.runtime
                .initialize(
                    None,
                    self.runtime
                        .spec()
                        .roles
                        .iter()
                        .map(|role| role.id.clone())
                        .collect(),
                    self.runtime
                        .spec()
                        .allowed_tools
                        .clone()
                        .unwrap_or_default(),
                    Some(vec!["print".to_string(), "resume".to_string()]),
                )
                .emitted,
        );
        let _ = self.persist_runtime(&emitted);
        emitted
    }

    pub fn delete_workspace(&mut self) -> Result<(), ClaudeAdapterError> {
        self.started = false;
        self.role_session_ids.clear();
        self.role_processes.clear();
        if let Some(persistence) = self.persistence.as_ref() {
            persistence.delete_workspace()?;
        }
        Ok(())
    }

    pub async fn run_role_task(
        &mut self,
        request: RoleTaskRequest,
    ) -> Result<ClaudeRoleTaskRun, ClaudeAdapterError> {
        let run = self.execute_assignment(request, None).await?;
        self.persist_runtime(&run.events)?;
        Ok(run)
    }

    pub async fn run_workspace_turn(
        &mut self,
        request: WorkspaceTurnRequest,
    ) -> Result<ClaudeWorkspaceTurnRun, ClaudeAdapterError> {
        let mut events = self
            .runtime
            .publish_user_message(request.message.clone())
            .emitted;
        let coordinator_decision = decide_coordinator_action(self.runtime.spec(), &request);
        if !coordinator_decision.response_text.trim().is_empty() {
            events.extend(
                self.runtime
                    .record_role_message(
                        &self
                            .runtime
                            .spec()
                            .coordinator_role_id
                            .clone()
                            .or_else(|| self.runtime.spec().default_role_id.clone())
                            .unwrap_or_else(|| "coordinator".to_string()),
                        coordinator_decision.response_text.clone(),
                        multi_agent_protocol::WorkspaceVisibility::Public,
                        None,
                        None,
                    )?
                    .emitted,
            );
        }

        let mut workflow_vote_window = None;
        let mut workflow_vote_responses = Vec::new();
        let plan = match coordinator_decision.kind {
            multi_agent_protocol::CoordinatorDecisionKind::Respond => WorkspaceTurnPlan {
                coordinator_role_id: self
                    .runtime
                    .spec()
                    .coordinator_role_id
                    .clone()
                    .or_else(|| self.runtime.spec().default_role_id.clone())
                    .unwrap_or_else(|| "coordinator".to_string()),
                response_text: coordinator_decision.response_text.clone(),
                assignments: Vec::new(),
                rationale: coordinator_decision.rationale.clone(),
            },
            multi_agent_protocol::CoordinatorDecisionKind::Delegate => {
                if let Some(target_role_id) = coordinator_decision.target_role_id.clone() {
                    direct_workspace_turn_plan(self.runtime.spec(), &request, &target_role_id)
                } else {
                    multi_agent_protocol::plan_workspace_turn(self.runtime.spec(), &request)
                }
            }
            multi_agent_protocol::CoordinatorDecisionKind::ProposeWorkflow => {
                let candidate_role_ids =
                    resolve_workflow_vote_candidate_role_ids(self.runtime.spec());
                let vote_tick = self.runtime.open_workflow_vote_window(
                    request.clone(),
                    coordinator_decision.clone(),
                    candidate_role_ids.clone(),
                );
                events.extend(vote_tick.emitted);
                let vote_window = vote_tick.state.workflow_runtime.active_vote_window.clone();
                workflow_vote_window = vote_window.clone();
                for role_id in candidate_role_ids {
                    if let Some(role) = self
                        .runtime
                        .spec()
                        .roles
                        .iter()
                        .find(|role| role.id == role_id)
                    {
                        let response = synthesize_workflow_vote_response(
                            self.runtime.spec(),
                            &request,
                            &coordinator_decision,
                            role,
                        );
                        workflow_vote_responses.push(response.clone());
                        if let Some(vote_window) = vote_window.as_ref() {
                            events.extend(
                                self.runtime
                                    .record_workflow_vote_response(vote_window, response)?
                                    .emitted,
                            );
                        }
                    }
                }
                let approved =
                    should_approve_workflow_vote(self.runtime.spec(), &workflow_vote_responses);
                if let Some(vote_window) = vote_window.clone() {
                    events.extend(
                        self.runtime
                            .close_workflow_vote_window(
                                vote_window.clone(),
                                coordinator_decision.clone(),
                                workflow_vote_responses.clone(),
                                approved,
                            )
                            .emitted,
                    );
                }
                if approved {
                    let plan = build_workflow_entry_plan(self.runtime.spec(), &request);
                    let first_assignment = plan.assignments.first();
                    events.extend(
                        self.runtime
                            .start_workflow(
                                coordinator_decision.clone(),
                                workflow_vote_window.clone(),
                                Some(request.message.clone()),
                                first_assignment
                                    .and_then(|assignment| assignment.workflow_node_id.clone()),
                                first_assignment.and_then(|assignment| assignment.stage_id.clone()),
                            )
                            .emitted,
                    );
                    plan
                } else {
                    WorkspaceTurnPlan {
                        coordinator_role_id: self
                            .runtime
                            .spec()
                            .coordinator_role_id
                            .clone()
                            .or_else(|| self.runtime.spec().default_role_id.clone())
                            .unwrap_or_else(|| "coordinator".to_string()),
                        response_text: coordinator_decision.response_text.clone(),
                        assignments: Vec::new(),
                        rationale: Some(
                            "Workflow vote rejected; staying in group chat mode.".to_string(),
                        ),
                    }
                }
            }
        };

        let mut dispatches = Vec::new();
        for assignment in &plan.assignments {
            let (mut chained_dispatches, chained_events) = self
                .execute_assignment_chain(
                    RoleTaskRequest {
                        role_id: assignment.role_id.clone(),
                        instruction: assignment.instruction.clone(),
                        summary: assignment.summary.clone(),
                        visibility: assignment.visibility,
                        source_role_id: Some(plan.coordinator_role_id.clone()),
                        workflow_node_id: assignment.workflow_node_id.clone(),
                        stage_id: assignment.stage_id.clone(),
                    },
                    Some("Claimed by runtime routing".to_string()),
                )
                .await?;
            events.extend(chained_events);
            dispatches.append(&mut chained_dispatches);
        }

        let run = ClaudeWorkspaceTurnRun {
            request,
            plan,
            workflow_vote_window,
            workflow_vote_responses,
            dispatches,
            events,
            state: self.runtime.snapshot(),
        };
        self.persist_runtime(&run.events)?;
        Ok(run)
    }

    async fn execute_assignment(
        &mut self,
        request: RoleTaskRequest,
        claim_note: Option<String>,
    ) -> Result<ClaudeRoleTaskRun, ClaudeAdapterError> {
        let role = self
            .runtime
            .spec()
            .roles
            .iter()
            .find(|role| role.id == request.role_id)
            .cloned()
            .ok_or_else(|| ClaudeAdapterError::UnknownRole(request.role_id.clone()))?;

        let (dispatch, queued_tick) = self.runtime.queue_dispatch(request)?;
        let mut emitted = queued_tick.emitted;

        let should_claim = self
            .runtime
            .snapshot()
            .dispatches
            .get(&dispatch.dispatch_id)
            .and_then(|stored| stored.claim_status)
            != Some(ClaimStatus::Claimed);
        if should_claim {
            emitted.extend(
                self.runtime
                    .claim_dispatch(
                        dispatch.dispatch_id,
                        &role.id,
                        ClaimStatus::Claimed,
                        claim_note,
                    )?
                    .emitted,
            );
        }

        let provider_result = self.execute_provider_turn(&role, &dispatch).await?;
        emitted.extend(provider_result.events);

        let snapshot = self.runtime.snapshot();
        let final_dispatch = snapshot
            .dispatches
            .get(&dispatch.dispatch_id)
            .cloned()
            .expect("dispatch should exist after provider turn");

        Ok(ClaudeRoleTaskRun {
            dispatch: final_dispatch,
            events: emitted,
        })
    }

    async fn execute_assignment_chain(
        &mut self,
        request: RoleTaskRequest,
        claim_note: Option<String>,
    ) -> Result<(Vec<TaskDispatch>, Vec<WorkspaceEvent>), ClaudeAdapterError> {
        let mut dispatches = Vec::new();
        let mut events = Vec::new();
        let mut pending = vec![(request, claim_note)];

        let mut followup_budget = self.options.max_workflow_followups;
        while let Some((request, claim_note)) = pending.pop() {
            let run = self.execute_assignment(request, claim_note).await?;
            let provider_task_id = run.dispatch.provider_task_id.clone();
            events.extend(run.events);
            dispatches.push(run.dispatch.clone());

            if let Some(provider_task_id) = provider_task_id {
                let (advance_tick, mut followups) = self
                    .runtime
                    .advance_workflow_after_dispatch(&provider_task_id)?;
                events.extend(advance_tick.emitted);
                while followup_budget > 0 {
                    let Some(followup) = followups.pop() else {
                        break;
                    };
                    followup_budget -= 1;
                    pending.push((
                        followup,
                        Some("Claimed by workflow progression".to_string()),
                    ));
                }
            }
        }

        Ok((dispatches, events))
    }

    /// Ensure a persistent subprocess exists for the given role, spawning one
    /// if necessary.  Returns a reference to the process handle.
    async fn ensure_role_process(
        &mut self,
        role: &RoleSpec,
    ) -> Result<&ClaudeSessionProcess, ClaudeAdapterError> {
        if !self.role_processes.contains_key(&role.id) {
            let resume_session_id = self.role_session_ids.get(&role.id).cloned();
            let effective_working_directory = self
                .options
                .working_directory
                .clone()
                .or_else(|| self.runtime.spec().cwd.as_ref().map(PathBuf::from));

            let process = ClaudeSessionProcess::spawn(ClaudeSpawnParams {
                claude_path: &self.options.claude_path,
                permission_mode: self.options.permission_mode,
                permission_prompt_tool_name: self.options.permission_prompt_tool_name.as_deref(),
                model: &self.runtime.spec().model,
                working_directory: effective_working_directory.as_ref(),
                additional_directories: &self.options.additional_directories,
                resume_session_id: resume_session_id.as_deref(),
                spawn_ready_timeout: self.options.spawn_ready_timeout,
                agents: self.options.agents.as_ref(),
                exclude_dynamic_sections: self.options.exclude_dynamic_sections,
                sdk_mcp_servers: self.options.sdk_mcp_servers.as_deref(),
                initialize_system_prompt: self.options.initialize_system_prompt.as_deref(),
                initialize_append_system_prompt: self
                    .options
                    .initialize_append_system_prompt
                    .as_deref(),
            })
            .await?;

            // Record the native session id.
            self.role_session_ids
                .insert(role.id.clone(), process.native_session_id.clone());
            self.role_processes.insert(role.id.clone(), process);
        }
        Ok(self.role_processes.get(&role.id).unwrap())
    }

    /// Execute a single provider turn against the persistent subprocess.
    async fn execute_provider_turn(
        &mut self,
        role: &RoleSpec,
        dispatch: &TaskDispatch,
    ) -> Result<ClaudeRoleTaskRun, ClaudeAdapterError> {
        // Ensure persistent process is running.
        self.ensure_role_process(role).await?;

        let prompt = build_dispatch_prompt(self.runtime.spec(), role, dispatch);

        // Get the process handle.  We borrow it immutably here because we
        // need &mut self later to update runtime state.  The process handle
        // only needs &self for send_user_message (it uses interior mutability).
        let process = self.role_processes.get(&role.id).unwrap();
        let process_session_id = process.native_session_id.clone();
        let stderr_tail = Arc::clone(&process.stderr_tail);

        // Record that we know this role's session_id (may have already been set
        // during ensure_role_process, but repeat for clarity).
        self.role_session_ids
            .insert(role.id.clone(), process_session_id.clone());

        // Start dispatch tracking.
        let mut emitted: Vec<WorkspaceEvent> = Vec::new();
        emitted.extend(
            self.runtime
                .start_next_dispatch(
                    process_session_id.clone(),
                    dispatch
                        .summary
                        .clone()
                        .unwrap_or_else(|| dispatch.instruction.clone()),
                    Some(format!("claude-session:{}", process_session_id)),
                )?
                .1
                .emitted,
        );

        // Send user message and get turn event receiver.
        let process = self.role_processes.get(&role.id).unwrap();
        let mut rx = process.send_user_message(&prompt).await?;

        let effective_working_directory = self
            .options
            .working_directory
            .clone()
            .or_else(|| self.runtime.spec().cwd.as_ref().map(PathBuf::from));

        let role_id = role.id.clone();
        let workspace_id = self.runtime.spec().id.clone();
        let mut final_result_text: Option<String> = None;
        let mut turn_failed: Option<String> = None;

        // Process turn events with timeout.
        let turn_processing = async {
            loop {
                let event = match rx.recv().await {
                    Some(e) => e,
                    None => break, // channel closed
                };

                match event {
                    TurnEvent::JsonEvent(claude_event) => match claude_event {
                        ClaudeJsonEvent::System {
                            subtype,
                            session_id,
                            tools,
                        } if subtype == "init" => {
                            // If we get another init (e.g. on resume), update session id.
                            if let Some(sid) = session_id {
                                self.role_session_ids.insert(role_id.clone(), sid.clone());
                                if let Some(tools) = tools {
                                    emitted.extend(
                                        self.runtime
                                            .initialize(
                                                Some(sid),
                                                self.runtime
                                                    .spec()
                                                    .roles
                                                    .iter()
                                                    .map(|r| r.id.clone())
                                                    .collect(),
                                                tools,
                                                Some(vec![
                                                    "print".to_string(),
                                                    "resume".to_string(),
                                                ]),
                                            )
                                            .emitted,
                                    );
                                }
                            }
                        }
                        ClaudeJsonEvent::Assistant {
                            message,
                            session_id,
                        } => {
                            for content in message.content {
                                match content {
                                    ClaudeContent::ToolUse { id, name, input } => {
                                        let description = summarize_tool_input(&name, &input);
                                        emitted.extend(
                                            self.runtime
                                                .progress_dispatch(
                                                    &current_task_id(
                                                        &self.role_session_ids,
                                                        &role_id,
                                                        dispatch,
                                                    ),
                                                    description,
                                                    Some(format!(
                                                        "Claude is using the {name} tool."
                                                    )),
                                                    Some(name.clone()),
                                                )?
                                                .emitted,
                                        );

                                        emitted.push(WorkspaceEvent::Message {
                                            timestamp: Utc::now().to_rfc3339(),
                                            workspace_id: workspace_id.clone(),
                                            role: role_id.clone(),
                                            text: format!("{name} tool started."),
                                            visibility: Some(
                                                multi_agent_protocol::WorkspaceVisibility::Private,
                                            ),
                                            member_id: Some(role_id.clone()),
                                            session_id: session_id.clone(),
                                            parent_tool_use_id: Some(id),
                                        });
                                    }
                                    ClaudeContent::Text { text } => {
                                        final_result_text = Some(match final_result_text.take() {
                                            Some(existing) => {
                                                format!("{existing}\n{text}")
                                            }
                                            None => text.clone(),
                                        });
                                        emitted.push(WorkspaceEvent::Message {
                                            timestamp: Utc::now().to_rfc3339(),
                                            workspace_id: workspace_id.clone(),
                                            role: "assistant".to_string(),
                                            text,
                                            visibility: Some(
                                                multi_agent_protocol::WorkspaceVisibility::Public,
                                            ),
                                            member_id: Some(role_id.clone()),
                                            session_id: session_id.clone(),
                                            parent_tool_use_id: None,
                                        });
                                    }
                                    ClaudeContent::Thinking { thinking } => {
                                        emitted.extend(
                                            self.runtime
                                                .progress_dispatch(
                                                    &current_task_id(
                                                        &self.role_session_ids,
                                                        &role_id,
                                                        dispatch,
                                                    ),
                                                    "thinking",
                                                    Some(thinking),
                                                    Some("Thinking".to_string()),
                                                )?
                                                .emitted,
                                        );
                                    }
                                    ClaudeContent::Other => {}
                                }
                            }
                        }
                        ClaudeJsonEvent::Result {
                            subtype,
                            is_error,
                            result,
                            session_id,
                        } => {
                            if is_error || subtype != "success" {
                                turn_failed = Some(result);
                                break;
                            }

                            if let Some(sid) = session_id {
                                self.role_session_ids.insert(role_id.clone(), sid);
                            }

                            let provider_task_id = self
                                .role_session_ids
                                .get(&role_id)
                                .cloned()
                                .unwrap_or_else(|| dispatch.dispatch_id.to_string());

                            emitted.extend(
                                self.runtime
                                    .complete_dispatch(
                                        &provider_task_id,
                                        DispatchStatus::Completed,
                                        None,
                                        "Claude completed the turn.".to_string(),
                                    )?
                                    .emitted,
                            );

                            let result_text = final_result_text
                                .clone()
                                .filter(|t| !t.trim().is_empty())
                                .unwrap_or(result);
                            emitted.extend(
                                self.runtime
                                    .attach_result_text(&provider_task_id, result_text)?
                                    .emitted,
                            );

                            // Turn is complete.
                            break;
                        }
                        ClaudeJsonEvent::User => {}
                        ClaudeJsonEvent::RateLimitEvent => {}
                        ClaudeJsonEvent::StreamEvent { .. } => {
                            // Partial streaming deltas – skip for workspace turn processing.
                        }
                        ClaudeJsonEvent::System { .. } => {}
                    },
                    TurnEvent::PermissionRequest {
                        request_id,
                        tool_name: _,
                        input: _,
                    } => {
                        // In workspace mode with BypassPermissions we auto-allow.
                        // For other modes a callback could be introduced later.
                        if let Some(process) = self.role_processes.get(&role_id) {
                            let _ = process.respond_to_permission(&request_id, true).await;
                        }
                    }
                    TurnEvent::Eof => {
                        // Process exited – remove from cache so next turn re-spawns.
                        self.role_processes.remove(&role_id);
                        break;
                    }
                }
            }

            Ok::<_, ClaudeAdapterError>((emitted, turn_failed))
        };

        let (emitted, turn_failed) = match timeout(self.options.turn_timeout, turn_processing).await
        {
            Ok(result) => result?,
            Err(_) => {
                // Timeout – remove the process so it gets re-spawned.
                self.role_processes.remove(&role.id);
                return Err(ClaudeAdapterError::TimedOut {
                    timeout: self.options.turn_timeout,
                    debug: render_debug_context_from_tail(
                        effective_working_directory.as_ref(),
                        &stderr_tail,
                    ),
                });
            }
        };

        // Clear the turn receiver for the next turn.
        if let Some(process) = self.role_processes.get(&role.id) {
            process.clear_turn_receiver().await;
        }

        if let Some(message) = turn_failed {
            return Err(ClaudeAdapterError::TurnFailed(message));
        }

        let snapshot = self.runtime.snapshot();
        let final_dispatch = snapshot
            .dispatches
            .get(&dispatch.dispatch_id)
            .cloned()
            .expect("dispatch should exist after claude turn");

        Ok(ClaudeRoleTaskRun {
            dispatch: final_dispatch,
            events: emitted,
        })
    }

    fn build_provider_state(&self) -> PersistedProviderState {
        PersistedProviderState {
            workspace_id: self.runtime.spec().id.clone(),
            provider: multi_agent_protocol::MultiAgentProvider::ClaudeAgentSdk,
            root_conversation_id: self.runtime.snapshot().session_id,
            member_bindings: self
                .role_session_ids
                .iter()
                .map(|(role_id, session_id)| {
                    (
                        role_id.clone(),
                        multi_agent_runtime_local::PersistedProviderBinding {
                            role_id: role_id.clone(),
                            provider_conversation_id: session_id.clone(),
                            kind: multi_agent_runtime_local::ProviderConversationKind::Session,
                            updated_at: Utc::now().to_rfc3339(),
                        },
                    )
                })
                .collect(),
            metadata: None,
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    fn persist_runtime(&self, events: &[WorkspaceEvent]) -> Result<(), ClaudeAdapterError> {
        if let Some(persistence) = self.persistence.as_ref() {
            persistence.persist_runtime(
                &self.runtime.snapshot(),
                events,
                &self.build_provider_state(),
            )?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Prompt builder
// ---------------------------------------------------------------------------

fn build_dispatch_prompt(spec: &WorkspaceSpec, role: &RoleSpec, dispatch: &TaskDispatch) -> String {
    let mut parts = vec![format!(
        "You are the {} role in the workspace \"{}\".",
        role.name, spec.name
    )];

    parts.push(
        "The current working directory is the workspace root for this task. Create or edit files using paths relative to the current directory, and avoid exploring unrelated directories.".to_string(),
    );

    if let Some(description) = role.description.as_ref() {
        parts.push(format!("Role description: {description}"));
    }
    parts.push(format!(
        "Follow this role-specific instruction set strictly:\n{}",
        role.agent.prompt
    ));
    if let Some(orchestrator_prompt) = spec.orchestrator_prompt.as_ref() {
        parts.push(format!(
            "Workspace orchestration context:\n{}",
            orchestrator_prompt
        ));
    }
    if let Some(output_root) = role.output_root.as_ref() {
        parts.push(format!(
            "Preferred output root for this role: {output_root}"
        ));
    }
    if let Some(summary) = dispatch.summary.as_ref() {
        parts.push(format!("Task summary: {summary}"));
    }
    parts.push(format!("Task instruction:\n{}", dispatch.instruction));
    parts.push(
        "Return a concise final answer after completing the task. If you create or edit files, mention the key output paths in the final answer."
            .to_string(),
    );

    parts.join("\n\n")
}

// ---------------------------------------------------------------------------
// JSON event types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClaudeJsonEvent {
    #[serde(rename = "system")]
    System {
        subtype: String,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        tools: Option<Vec<String>>,
    },
    #[serde(rename = "assistant")]
    Assistant {
        message: ClaudeAssistantMessage,
        #[serde(default)]
        session_id: Option<String>,
    },
    #[serde(rename = "user")]
    User,
    #[serde(rename = "rate_limit_event")]
    RateLimitEvent,
    #[serde(rename = "result")]
    Result {
        subtype: String,
        is_error: bool,
        result: String,
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Partial streaming event emitted when `--include-partial-messages` is
    /// active.  We capture it so serde doesn't reject the line, but workspace
    /// turn processing currently ignores the content.
    #[serde(rename = "stream_event")]
    #[allow(dead_code)]
    StreamEvent {
        #[serde(default)]
        event: Option<Value>,
    },
}

#[derive(Debug, Deserialize)]
struct ClaudeAssistantMessage {
    content: Vec<ClaudeContent>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClaudeContent {
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(other)]
    Other,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn summarize_tool_input(name: &str, input: &Value) -> String {
    match name {
        "Bash" => input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("bash")
            .to_string(),
        "Write" | "Edit" | "Read" => input
            .get("file_path")
            .or_else(|| input.get("path"))
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string(),
        "WebSearch" => input
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string(),
        "Task" | "TeamCreate" => input
            .get("prompt")
            .or_else(|| input.get("message"))
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string(),
        _ => name.to_string(),
    }
}

fn current_task_id(
    role_session_ids: &BTreeMap<String, String>,
    role_id: &str,
    dispatch: &TaskDispatch,
) -> String {
    role_session_ids
        .get(role_id)
        .cloned()
        .unwrap_or_else(|| dispatch.dispatch_id.to_string())
}

/// Best-effort single-line JSON write to the CLI's stdin. Errors are silently
/// swallowed because the reader task has nowhere to surface them — if stdin
/// is broken the rest of the session is already doomed.
async fn write_stdin_line(stdin: &Arc<TokioMutex<tokio::process::ChildStdin>>, value: &Value) {
    let Ok(payload) = serde_json::to_string(value) else {
        return;
    };
    let line = format!("{payload}\n");
    let mut guard = stdin.lock().await;
    let _ = guard.write_all(line.as_bytes()).await;
    let _ = guard.flush().await;
}

fn push_tail(buffer: &Arc<Mutex<Vec<String>>>, line: String) {
    let mut guard = buffer
        .lock()
        .expect("tail buffer mutex should not be poisoned");
    guard.push(line);
    if guard.len() > 40 {
        let overflow = guard.len() - 40;
        guard.drain(0..overflow);
    }
}

fn render_debug_context_from_tail(
    working_directory: Option<&PathBuf>,
    stderr_tail: &Arc<Mutex<Vec<String>>>,
) -> String {
    let stderr_lines = stderr_tail
        .lock()
        .expect("stderr tail mutex should not be poisoned")
        .join("\n");
    format!(
        "working_directory: {}\nstderr_tail:\n{stderr_lines}",
        working_directory
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<none>".to_string()),
    )
}
