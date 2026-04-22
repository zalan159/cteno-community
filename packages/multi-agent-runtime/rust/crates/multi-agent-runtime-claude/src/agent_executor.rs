//! [`AgentExecutor`] implementation backed by the `claude` CLI subprocess.
//!
//! Each [`ClaudeAgentExecutor::spawn_session`] call forks one `claude`
//! subprocess configured with `--output-format stream-json --input-format
//! stream-json --verbose` and keeps it alive for the session's duration.
//! Subsequent `send_message` calls push one framed JSON line into the child
//! stdin and yield an [`EventStream`] reading stdout until a `result` frame
//! closes the turn.
//!
//! List / get session operations are delegated to a caller-supplied
//! [`SessionStoreProvider`] so Cteno, Claude, and Codex adapters share a
//! single ground-truth metadata store (see `agent_executor_plan.md` §4.2).
//!
//! ### Protocol coverage
//!
//! | Claude frame                             | `ExecutorEvent`            |
//! |------------------------------------------|----------------------------|
//! | `system` + `init`                        | `SessionReady`             |
//! | `assistant` + `text`                     | `StreamDelta { Text }`     |
//! | `assistant` + `thinking`                 | `StreamDelta { Thinking }` |
//! | `assistant` + `tool_use`                 | `ToolCallStart`            |
//! | `user` (tool_result piggyback)           | `ToolResult`               |
//! | `rate_limit_event`                       | `NativeEvent`              |
//! | `result` (success)                       | `TurnComplete`             |
//! | `result` (error) / unreadable            | `Error`                    |
//! | anything else                            | `NativeEvent`              |
//!
//! Tool-result payloads arrive inside a `user` envelope with a `content`
//! array of `{ type: "tool_result", tool_use_id, content, is_error }` blocks.
//! Each block is fanned out as `ExecutorEvent::ToolResult` so the normalizer
//! emits the canonical `acp/claude/tool-result` wire event and the card
//! transitions from `running` to `completed`/`error`.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use multi_agent_runtime_core::{
    AgentCapabilities, AgentExecutor, AgentExecutorError, ConnectionHandle, ConnectionHandleId,
    ConnectionHealth, ConnectionSpec, DeltaKind, EventStream, ExecutorEvent, ModelChangeOutcome,
    ModelSpec, NativeMessage, NativeSessionId, Pagination, PermissionDecision, PermissionMode,
    PermissionModeKind, ProcessHandleToken, ResumeHints, SessionFilter, SessionInfo, SessionMeta,
    SessionRecord, SessionRef, SessionStoreProvider, SpawnSessionSpec, TokenUsage, UserMessage,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, oneshot};
use tokio::time::timeout;
use tokio_stream::wrappers::ReceiverStream;

use crate::stream::{ClaudeContent, ClaudeJsonEvent, parse_stream_line};
use crate::workspace::{CLAUDE_CODE_ENTRYPOINT_TAG, ClaudeAdapterError, check_cli_version};
use uuid::Uuid;

const VENDOR_NAME: &str = "claude";
const PROTOCOL_VERSION: &str = "0.1";
const DEFAULT_SPAWN_READY_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_TURN_TIMEOUT: Duration = Duration::from_secs(600);

fn usage_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|candidate| candidate.as_u64()))
}

fn usage_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|candidate| {
            candidate
                .as_f64()
                .or_else(|| candidate.as_u64().map(|v| v as f64))
        })
    })
}

fn utilization_to_percent(value: f64) -> f64 {
    if value <= 1.0 { value * 100.0 } else { value }
}

fn merge_token_usage(value: Option<&Value>, usage: &mut TokenUsage) {
    let Some(value) = value else {
        return;
    };

    if usage.input_tokens == 0 {
        if let Some(v) = usage_u64(value, &["input_tokens", "inputTokens"]) {
            usage.input_tokens = v;
        }
    }
    if usage.output_tokens == 0 {
        if let Some(v) = usage_u64(value, &["output_tokens", "outputTokens"]) {
            usage.output_tokens = v;
        }
    }
    if usage.cache_creation_tokens == 0 {
        if let Some(v) = usage_u64(
            value,
            &["cache_creation_input_tokens", "cacheCreationInputTokens"],
        ) {
            usage.cache_creation_tokens = v;
        }
    }
    if usage.cache_read_tokens == 0 {
        if let Some(v) = usage_u64(value, &["cache_read_input_tokens", "cacheReadInputTokens"]) {
            usage.cache_read_tokens = v;
        }
    }
    if usage.reasoning_tokens == 0 {
        if let Some(v) = usage_u64(value, &["reasoning_tokens", "reasoningTokens"]) {
            usage.reasoning_tokens = v;
        }
    }

    if usage.cache_read_tokens == 0 {
        if let Some(v) = value
            .get("input_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(|candidate| candidate.as_u64())
        {
            usage.cache_read_tokens = v;
        }
    }
}

fn token_usage_from_result(usage: Option<&Value>, model_usage: Option<&Value>) -> TokenUsage {
    let mut parsed = TokenUsage::default();
    merge_token_usage(usage, &mut parsed);
    merge_token_usage(model_usage, &mut parsed);
    parsed
}

fn build_claude_task_native_event(
    kind: &str,
    task_id: Option<String>,
    description: Option<String>,
    summary: Option<String>,
    status: Option<String>,
    output_file: Option<String>,
    tool_use_id: Option<String>,
    task_type: Option<String>,
    usage: Option<Value>,
    last_tool_name: Option<String>,
    uuid: Option<String>,
    session_id: Option<String>,
) -> Value {
    json!({
        "kind": kind,
        "task_id": task_id,
        "description": description,
        "summary": summary,
        "status": status,
        "output_file": output_file,
        "tool_use_id": tool_use_id,
        "task_type": task_type,
        "usage": usage,
        "last_tool_name": last_tool_name,
        "uuid": uuid,
        "session_id": session_id,
    })
}

/// Per-session subprocess handle held inside the executor's registry.
///
/// `stdin` and `pending_permission_inputs` are behind their own `Arc<Mutex>`
/// so `respond_to_permission` can write to the CLI while the per-turn stream
/// reader task holds exclusive ownership of `stdout_reader`. Without this
/// split the two tasks deadlock: stream reader waits for Claude to emit the
/// next frame, Claude waits for the permission response that
/// `respond_to_permission` can't deliver because it can't acquire the outer
/// process lock.
///
/// `stdout_reader` lives as `Option<…>` so the stream task can `take()` it at
/// turn start and put it back when the turn completes, avoiding the need to
/// hold the outer `Mutex<ClaudeSessionProcess>` across blocking reads.
struct ClaudeSessionProcess {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    stdout_reader: Option<BufReader<ChildStdout>>,
    native_session_id: Option<NativeSessionId>,
    pending_permission_inputs: Arc<Mutex<HashMap<String, Value>>>,
    pending_control_responses: PendingControlResponses,
}

type PendingControlResponses =
    Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, AgentExecutorError>>>>>;

/// Runtime-wide registry of live Claude subprocesses keyed by
/// [`ProcessHandleToken`]. Exposed as a `Mutex` for per-handle access under
/// the shared `Arc`.
type SessionRegistry = Mutex<HashMap<ProcessHandleToken, Arc<Mutex<ClaudeSessionProcess>>>>;

/// Opaque payload stored in [`ConnectionHandle::inner`] for the Claude adapter.
///
/// **Hazard context** (see `docs/claude-p1-protocol-findings.md`): the official
/// `claude` CLI ignores the `session_id` field on inbound `type:"user"` frames
/// and enforces one session per subprocess — two user messages with different
/// `session_id` tags are appended to the SAME CLI-internal conversation. Phase
/// A empirical probes confirmed this end-to-end on CLI `2.1.114`.
///
/// Therefore the connection-reuse seam for Claude is a *shim*, not a true pool:
/// `open_connection` performs a one-shot version-probe and caches that result
/// so the registry can ask `check_connection` for cheap liveness. Every call to
/// `start_session_on` unconditionally delegates to [`spawn_internal`], which
/// forks a fresh subprocess. `close_connection` has no subprocess to kill.
///
/// The capability [`AgentCapabilities::supports_multi_session_per_process`]
/// stays `false` for the Claude adapter — callers must not assume that two
/// `SessionRef`s returned from the same `ConnectionHandle` share a transport.
#[derive(Debug)]
struct ClaudeConnectionInner {
    /// When the most recent version check succeeded. `None` means the probe
    /// has not yet completed (e.g. `probe = true` short-circuit with failure)
    /// and `check_connection` should re-probe.
    version_checked_at: Mutex<Option<DateTime<Utc>>>,
    /// Remember whether the caller requested `probe` so diagnostics can
    /// distinguish a lightweight probe from a full open. Exposed via
    /// [`ClaudeConnectionInner::probe_only`].
    probe_only: bool,
}

impl ClaudeConnectionInner {
    fn new(probe_only: bool, version_checked_at: Option<DateTime<Utc>>) -> Self {
        Self {
            version_checked_at: Mutex::new(version_checked_at),
            probe_only,
        }
    }

    /// Whether the connection was opened as a lightweight probe (vendor
    /// discovery, readiness check) rather than a persistent open.
    #[allow(dead_code)]
    fn probe_only(&self) -> bool {
        self.probe_only
    }
}

/// [`AgentExecutor`] implementation that drives a `claude` CLI subprocess per
/// session. Cheap to `Arc::clone`; internally holds a subprocess registry.
pub struct ClaudeAgentExecutor {
    claude_path: PathBuf,
    session_store: Arc<dyn SessionStoreProvider>,
    sessions: SessionRegistry,
    spawn_ready_timeout: Duration,
    turn_timeout: Duration,
}

impl ClaudeAgentExecutor {
    /// Build a new executor targeting the given `claude` binary path and
    /// metadata store.
    pub fn new(claude_path: PathBuf, session_store: Arc<dyn SessionStoreProvider>) -> Self {
        Self {
            claude_path,
            session_store,
            sessions: Mutex::new(HashMap::new()),
            spawn_ready_timeout: DEFAULT_SPAWN_READY_TIMEOUT,
            turn_timeout: DEFAULT_TURN_TIMEOUT,
        }
    }

    /// Override the timeout for waiting on the initial `system:init` frame.
    pub fn with_spawn_ready_timeout(mut self, timeout: Duration) -> Self {
        self.spawn_ready_timeout = timeout;
        self
    }

    /// Override the per-turn timeout for `send_message` streams.
    pub fn with_turn_timeout(mut self, timeout: Duration) -> Self {
        self.turn_timeout = timeout;
        self
    }

    async fn get_session(
        &self,
        session: &SessionRef,
    ) -> Result<Arc<Mutex<ClaudeSessionProcess>>, AgentExecutorError> {
        let guard = self.sessions.lock().await;
        guard
            .get(&session.process_handle)
            .cloned()
            .ok_or_else(|| AgentExecutorError::SessionNotFound(session.id.to_string()))
    }

    async fn remove_session(
        &self,
        token: &ProcessHandleToken,
    ) -> Option<Arc<Mutex<ClaudeSessionProcess>>> {
        let mut guard = self.sessions.lock().await;
        guard.remove(token)
    }

    async fn send_control_request(
        &self,
        session: &SessionRef,
        request_id: &str,
        request: Value,
        operation: &str,
    ) -> Result<Value, AgentExecutorError> {
        let process = self.get_session(session).await?;
        let frame = json!({
            "type": "control_request",
            "request_id": request_id,
            "request": request,
        });
        let mut guard = process.lock().await;
        let stdin_handle = guard.stdin.clone();
        if guard.stdout_reader.is_some() {
            write_control_line(&stdin_handle, &frame)
                .await
                .map_err(AgentExecutorError::from)?;
            timeout(
                self.spawn_ready_timeout,
                wait_for_control_response(&mut guard, Some(request_id)),
            )
            .await
            .map_err(|_| AgentExecutorError::Timeout {
                operation: operation.to_string(),
                seconds: self.spawn_ready_timeout.as_secs(),
            })?
        } else {
            let pending_control_responses = guard.pending_control_responses.clone();
            drop(guard);

            let (tx, rx) = oneshot::channel();
            pending_control_responses
                .lock()
                .await
                .insert(request_id.to_string(), tx);
            write_control_line(&stdin_handle, &frame)
                .await
                .map_err(AgentExecutorError::from)?;
            match timeout(self.spawn_ready_timeout, rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(_closed)) => Err(AgentExecutorError::Protocol(format!(
                    "claude control response channel closed for request '{}'",
                    request_id
                ))),
                Err(_timeout) => {
                    pending_control_responses.lock().await.remove(request_id);
                    Err(AgentExecutorError::Timeout {
                        operation: operation.to_string(),
                        seconds: self.spawn_ready_timeout.as_secs(),
                    })
                }
            }
        }
    }

    /// Core spawn path used by both `spawn_session` and `resume_session`.
    async fn spawn_internal(
        &self,
        workdir: PathBuf,
        system_prompt: Option<String>,
        model: Option<ModelSpec>,
        permission_mode: PermissionMode,
        additional_directories: Vec<PathBuf>,
        env: BTreeMap<String, String>,
        resume_session_id: Option<String>,
    ) -> Result<SessionRef, AgentExecutorError> {
        // Check CLI version up-front so callers get a readable error instead
        // of a mysterious handshake timeout.
        check_cli_version(&self.claude_path)
            .await
            .map_err(|e| match e {
                ClaudeAdapterError::UnsupportedCliVersion { found, minimum } => {
                    AgentExecutorError::Vendor {
                        vendor: VENDOR_NAME,
                        message: format!(
                            "claude CLI version {found} is unsupported (minimum {minimum})"
                        ),
                    }
                }
                other => AgentExecutorError::Vendor {
                    vendor: VENDOR_NAME,
                    message: other.to_string(),
                },
            })?;

        let mut command = Command::new(&self.claude_path);
        command
            .arg("--output-format")
            .arg("stream-json")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--include-partial-messages")
            .arg("--verbose")
            .arg("--dangerously-skip-permissions")
            .arg("--permission-mode")
            .arg(permission_mode_cli_value(permission_mode));

        // `--permission-prompt-tool stdio` is REQUIRED to make the CLI route
        // tool-permission decisions through `control_request can_use_tool`
        // instead of trying to run tools itself. Despite the name, `stdio` is
        // NOT an MCP tool name — it is a magic sentinel the official SDK
        // passes whenever a `canUseTool` callback is configured (see
        // `@anthropic-ai/claude-agent-sdk` sdk.mjs: the TS SDK literally does
        // `cmd.push("--permission-prompt-tool", "stdio")` when `canUseTool`
        // is set). Dropping this flag makes the CLI fall back to its internal
        // permission logic which refuses writes outside the cwd and causes
        // "Stream closed" errors.
        //
        // If a caller wants to name a real MCP tool instead, that's a
        // different code path; we'd need to thread a `permission_prompt_tool_name`
        // option down here and only use it when set.
        command.arg("--permission-prompt-tool").arg("stdio");

        if let Some(model) = model.as_ref() {
            command.arg("--model").arg(&model.model_id);
            if let Some(thinking_mode) =
                spawn_thinking_mode_cli_value(model.reasoning_effort.as_deref())
            {
                command.arg("--thinking").arg(thinking_mode);
            }
        }

        command.current_dir(&workdir);
        for dir in &additional_directories {
            command.arg("--add-dir").arg(dir);
        }

        if let Some(prompt) = system_prompt.as_ref() {
            command.arg("--system-prompt").arg(prompt);
        }

        // Generate session_id upfront (like Happy offline mode) so we don't
        // need to wait for system:init. Pass via --session-id flag.
        let generated_session_id = Uuid::new_v4().to_string();
        if let Some(sid) = resume_session_id.as_ref() {
            command.arg("--resume").arg(sid);
        } else {
            command.arg("--session-id").arg(&generated_session_id);
        }

        // Environment adjustments mirroring the Python/TS SDKs:
        //   * tag the spawn so analytics can tell us apart from sdk-py/sdk-ts,
        //   * record the adapter version,
        //   * strip inherited CLAUDECODE (SDK #573 workaround — prevents the
        //     CLI from thinking it's nested inside another Claude Code).
        command.env_remove("CLAUDECODE");
        command.env("CLAUDE_CODE_ENTRYPOINT", CLAUDE_CODE_ENTRYPOINT_TAG);
        command.env("CLAUDE_AGENT_SDK_VERSION", env!("CARGO_PKG_VERSION"));
        for (key, value) in &env {
            command.env(key, value);
        }

        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(AgentExecutorError::from)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentExecutorError::Io("claude stdin unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentExecutorError::Io("claude stdout unavailable".to_string()))?;

        let mut process = ClaudeSessionProcess {
            child,
            stdin: Arc::new(Mutex::new(stdin)),
            stdout_reader: Some(BufReader::new(stdout)),
            native_session_id: None,
            pending_permission_inputs: Arc::new(Mutex::new(HashMap::new())),
            pending_control_responses: Arc::new(Mutex::new(HashMap::new())),
        };

        // Send initialize control request (Python SDK protocol handshake).
        // CLI outputs control_response to confirm, but system:init only comes
        // after the first user message. We wait for control_response, not init.
        let init_request = serde_json::json!({
            "type": "control_request",
            "request_id": "req_init",
            "request": { "subtype": "initialize", "hooks": null }
        });
        let init_line = format!("{}\n", serde_json::to_string(&init_request).unwrap());
        {
            let mut stdin = process.stdin.lock().await;
            stdin
                .write_all(init_line.as_bytes())
                .await
                .map_err(AgentExecutorError::from)?;
            stdin.flush().await.map_err(AgentExecutorError::from)?;
        }

        // Wait for control_response confirming initialize success.
        // Skip hook frames and other system messages until we see it.
        let _init_response = timeout(
            self.spawn_ready_timeout,
            wait_for_control_response(&mut process, Some("req_init")),
        )
        .await
        .map_err(|_| AgentExecutorError::Timeout {
            operation: "spawn_session (initialize)".to_string(),
            seconds: self.spawn_ready_timeout.as_secs(),
        })??;

        // `--dangerously-skip-permissions` is required at spawn time so Claude
        // can later enter `bypassPermissions`, but it also means the CLI may
        // start in a broader allow state than the session actually requested.
        // Re-apply the requested runtime mode immediately after initialize so
        // "project default = ask" sessions do not stay silently bypassed.
        let request_id = format!("req_spawn_set_mode_{}", Uuid::new_v4());
        write_control_line(
            &process.stdin,
            &json!({
                "type": "control_request",
                "request_id": request_id,
                "request": {
                    "subtype": "set_permission_mode",
                    "mode": permission_mode_cli_value(permission_mode),
                }
            }),
        )
        .await
        .map_err(AgentExecutorError::from)?;
        timeout(
            self.spawn_ready_timeout,
            wait_for_control_response(&mut process, Some(&request_id)),
        )
        .await
        .map_err(|_| AgentExecutorError::Timeout {
            operation: "spawn_session (set_permission_mode)".to_string(),
            seconds: self.spawn_ready_timeout.as_secs(),
        })??;

        // Use the pre-generated session_id (passed via --session-id flag).
        let native_id = if let Some(sid) = resume_session_id.as_ref() {
            NativeSessionId::new(sid.clone())
        } else {
            NativeSessionId::new(generated_session_id)
        };
        process.native_session_id = Some(native_id.clone());

        let token = ProcessHandleToken::new();
        let session_ref = SessionRef {
            id: native_id,
            vendor: VENDOR_NAME,
            process_handle: token.clone(),
            spawned_at: Utc::now(),
            workdir: workdir.clone(),
        };

        if let Err(message) = self
            .session_store
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
        {
            {
                let mut stdin = process.stdin.lock().await;
                let _ = stdin.shutdown().await;
            }
            let _ = process.child.kill().await;
            let _ = process.child.wait().await;
            return Err(AgentExecutorError::Vendor {
                vendor: VENDOR_NAME,
                message,
            });
        }

        self.sessions
            .lock()
            .await
            .insert(token, Arc::new(Mutex::new(process)));

        Ok(session_ref)
    }
}

/// Read stdout frames until the first `system:init` envelope is seen and
/// return its `session_id`. Frames arriving before init are forwarded back to
/// the caller by reinserting them into the reader's internal buffer — which
/// tokio's `BufReader` doesn't let us do — so for now we simply drop them.
/// In practice `claude` sends `init` before anything else.
/// Wait for the `control_response` to our initialize request.
/// Skips all other frames (hooks, system, etc.) until we see it.
async fn wait_for_control_response(
    process: &mut ClaudeSessionProcess,
    expected_request_id: Option<&str>,
) -> Result<Value, AgentExecutorError> {
    let reader = process.stdout_reader.as_mut().ok_or_else(|| {
        AgentExecutorError::Protocol(
            "claude stdout_reader already taken by stream task".to_string(),
        )
    })?;
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(AgentExecutorError::from)?;
        if n == 0 {
            return Err(AgentExecutorError::Protocol(
                "claude stdout closed before control_response".to_string(),
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.starts_with('{') {
            continue;
        }
        match route_control_response_inline(
            trimmed,
            expected_request_id,
            &process.pending_control_responses,
        )
        .await?
        {
            InlineControlResponse::Matched(result) => return result,
            InlineControlResponse::Consumed | InlineControlResponse::NotControl => continue,
        }
    }
}

fn control_response_result(response: Value) -> Result<Value, AgentExecutorError> {
    match response
        .get("subtype")
        .and_then(|v| v.as_str())
        .unwrap_or("success")
    {
        "success" => Ok(response),
        "error" => {
            let message = response
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown Claude control error")
                .to_string();
            Err(AgentExecutorError::Vendor {
                vendor: VENDOR_NAME,
                message,
            })
        }
        other => Err(AgentExecutorError::Protocol(format!(
            "unexpected claude control_response subtype '{other}'"
        ))),
    }
}

fn claude_result_error(raw: &Value) -> Option<String> {
    if raw.get("type").and_then(|v| v.as_str()) != Some("result") {
        return None;
    }

    let is_error = raw
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let subtype = raw.get("subtype").and_then(|v| v.as_str());
    if !is_error && subtype != Some("error_during_execution") {
        return None;
    }

    if let Some(errors) = raw.get("errors").and_then(|v| v.as_array()) {
        let joined = errors
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        if !joined.is_empty() {
            return Some(joined);
        }
    }

    raw.get("error")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            subtype.map(|value| {
                format!("Claude returned result subtype '{value}' during control handshake")
            })
        })
}

enum InlineControlResponse {
    NotControl,
    Consumed,
    Matched(Result<Value, AgentExecutorError>),
}

async fn route_control_response_inline(
    raw_line: &str,
    expected_request_id: Option<&str>,
    pending_control_responses: &PendingControlResponses,
) -> Result<InlineControlResponse, AgentExecutorError> {
    let trimmed = raw_line.trim();
    if trimmed.is_empty() || !trimmed.starts_with('{') {
        return Ok(InlineControlResponse::NotControl);
    }
    let Ok(raw) = serde_json::from_str::<Value>(trimmed) else {
        return Ok(InlineControlResponse::NotControl);
    };
    if let Some(message) = claude_result_error(&raw) {
        return Ok(InlineControlResponse::Matched(Err(
            AgentExecutorError::Vendor {
                vendor: VENDOR_NAME,
                message,
            },
        )));
    }
    if raw.get("type").and_then(|v| v.as_str()) != Some("control_response") {
        return Ok(InlineControlResponse::NotControl);
    }

    let response = raw.get("response").cloned().ok_or_else(|| {
        AgentExecutorError::Protocol("claude control_response missing response payload".to_string())
    })?;
    let request_id = response
        .get("request_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let result = control_response_result(response);
    if expected_request_id.is_some_and(|expected| expected == request_id) {
        return Ok(InlineControlResponse::Matched(result));
    }
    if let Some(tx) = pending_control_responses.lock().await.remove(&request_id) {
        let _ = tx.send(result);
    }
    Ok(InlineControlResponse::Consumed)
}

fn claude_context_usage_native_event(response: &Value) -> Option<Value> {
    let payload = response.get("response")?;
    let total_tokens = usage_u64(payload, &["totalTokens", "total_tokens"])?;
    let mut event = serde_json::Map::new();
    event.insert(
        "kind".to_string(),
        Value::String("context_usage".to_string()),
    );
    event.insert("total_tokens".to_string(), Value::from(total_tokens));

    if let Some(max_tokens) = usage_u64(payload, &["maxTokens", "max_tokens"]) {
        event.insert("max_tokens".to_string(), Value::from(max_tokens));
    }
    if let Some(raw_max_tokens) = usage_u64(payload, &["rawMaxTokens", "raw_max_tokens"]) {
        event.insert("raw_max_tokens".to_string(), Value::from(raw_max_tokens));
    }
    if let Some(percentage) = payload.get("percentage").and_then(|v| v.as_f64()) {
        event.insert("percentage".to_string(), Value::from(percentage));
    }
    if let Some(model) = payload
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        event.insert("model".to_string(), Value::String(model.to_string()));
    }

    Some(Value::Object(event))
}

#[allow(dead_code)]
async fn read_until_init(
    process: &mut ClaudeSessionProcess,
) -> Result<NativeSessionId, AgentExecutorError> {
    let reader = process.stdout_reader.as_mut().ok_or_else(|| {
        AgentExecutorError::Protocol(
            "claude stdout_reader already taken by stream task".to_string(),
        )
    })?;
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(AgentExecutorError::from)?;
        if n == 0 {
            return Err(AgentExecutorError::Protocol(
                "claude stdout closed before init".to_string(),
            ));
        }
        let Some(parsed) = parse_stream_line(&line) else {
            continue;
        };
        // Skip lines that fail to parse as ClaudeJsonEvent (e.g. control_response
        // from the initialize handshake). These are expected during startup.
        let event = match parsed {
            Ok(e) => e,
            Err(_) => continue,
        };
        if let ClaudeJsonEvent::System {
            subtype,
            session_id,
            ..
        } = event
        {
            if subtype == "init" {
                if let Some(id) = session_id {
                    return Ok(NativeSessionId::new(id));
                }
            }
        }
    }
}

fn permission_mode_cli_value(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => "default",
        PermissionMode::Auto => "auto",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::BypassPermissions => "bypassPermissions",
        PermissionMode::DontAsk => "dontAsk",
        PermissionMode::Plan => "plan",
        PermissionMode::ReadOnly => "default",
        PermissionMode::WorkspaceWrite => "acceptEdits",
        PermissionMode::DangerFullAccess => "bypassPermissions",
    }
}

fn spawn_thinking_mode_cli_value(reasoning_effort: Option<&str>) -> Option<&'static str> {
    match normalized_reasoning_effort(reasoning_effort).as_deref() {
        Some("low" | "minimal" | "none" | "off" | "disabled") => Some("disabled"),
        _ => None,
    }
}

fn runtime_max_thinking_tokens(reasoning_effort: Option<&str>) -> Option<Option<u32>> {
    match normalized_reasoning_effort(reasoning_effort).as_deref() {
        // Claude's public control protocol exposes only a thinking toggle /
        // budget setter, not a first-class effort update. This adapter maps
        // the cross-vendor low/minimal bucket to "disable extended thinking",
        // and treats the default-or-higher buckets as "clear any explicit
        // thinking override and let Claude use the model's native default".
        None | Some("" | "default" | "auto" | "medium" | "high" | "max") => Some(None),
        Some("low" | "minimal" | "none" | "off" | "disabled") => Some(Some(0)),
        Some(_) => None,
    }
}

fn normalized_reasoning_effort(reasoning_effort: Option<&str>) -> Option<String> {
    reasoning_effort
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

/// Flatten a Claude `tool_result.content` field into a single rendered string.
/// Accepts:
///   * a raw `String` (most common)
///   * an array of content blocks: `[{type:"text",text:"..."}, ...]` — text
///     blocks are concatenated with blank-line separators; non-text blocks
///     (e.g. `image`) degrade to a tag placeholder so the normalizer can still
///     render something useful
///   * any other shape → `serde_json::to_string`
fn flatten_claude_tool_result_content(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    if let Some(items) = value.as_array() {
        let mut pieces: Vec<String> = Vec::with_capacity(items.len());
        for item in items {
            let ty = item.get("type").and_then(Value::as_str).unwrap_or("");
            match ty {
                "text" => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        pieces.push(text.to_string());
                    }
                }
                "image" => {
                    pieces.push("[image]".to_string());
                }
                other if !other.is_empty() => {
                    pieces.push(format!("[{other}]"));
                }
                _ => {
                    pieces.push(item.to_string());
                }
            }
        }
        return pieces.join("\n\n");
    }
    value.to_string()
}

fn user_message_to_stream_frame(message: &UserMessage) -> Value {
    // Claude's `--input-format stream-json` accepts user messages shaped like
    // the assistant frames it emits. Attachments are forwarded as untyped
    // content blocks — the vendor currently ignores unknown shapes.
    let mut content = vec![json!({
        "type": "text",
        "text": message.content,
    })];
    for attachment in &message.attachments {
        content.push(json!({
            "type": "attachment",
            "kind": attachment.kind,
            "mime_type": attachment.mime_type,
            "source": attachment.source,
            "data": attachment.data,
        }));
    }
    json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": content,
        }
    })
}

#[async_trait]
impl AgentExecutor for ClaudeAgentExecutor {
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            name: Cow::Borrowed(VENDOR_NAME),
            protocol_version: Cow::Borrowed(PROTOCOL_VERSION),
            supports_list_sessions: true,
            supports_get_messages: true,
            supports_runtime_set_model: true,
            permission_mode_kind: PermissionModeKind::Dynamic,
            supports_resume: true,
            supports_multi_session_per_process: false,
            supports_injected_tools: false,
            supports_permission_closure: true,
            supports_interrupt: true,
        }
    }

    async fn spawn_session(
        &self,
        spec: SpawnSessionSpec,
    ) -> Result<SessionRef, AgentExecutorError> {
        let resume_id = spec
            .resume_hint
            .as_ref()
            .and_then(|h| h.vendor_cursor.clone());
        self.spawn_internal(
            spec.workdir,
            spec.system_prompt,
            spec.model,
            spec.permission_mode,
            spec.additional_directories,
            spec.env,
            resume_id,
        )
        .await
    }

    async fn resume_session(
        &self,
        session_id: NativeSessionId,
        hints: ResumeHints,
    ) -> Result<SessionRef, AgentExecutorError> {
        let workdir = hints.workdir.clone().unwrap_or_else(|| PathBuf::from("."));
        self.spawn_internal(
            workdir,
            None,
            None,
            PermissionMode::Default,
            Vec::new(),
            BTreeMap::new(),
            hints
                .vendor_cursor
                .or(Some(session_id.as_str().to_string())),
        )
        .await
    }

    async fn send_message(
        &self,
        session: &SessionRef,
        message: UserMessage,
    ) -> Result<EventStream, AgentExecutorError> {
        let process = self.get_session(session).await?;
        let frame = user_message_to_stream_frame(&message);
        let line = format!(
            "{}\n",
            serde_json::to_string(&frame).map_err(|e| {
                AgentExecutorError::Protocol(format!("failed to serialise user message: {e}"))
            })?
        );

        // Split-lock discipline: grab Arc handles to stdin + pending-input
        // map, and TAKE stdout_reader out of the struct. The stream reader
        // task owns stdout_reader locally for the whole turn (no outer lock)
        // and puts it back when the turn ends. This lets `respond_to_permission`
        // lock stdin independently without deadlocking the reader.
        let stdin_handle;
        let pending_handle;
        let pending_control_handle;
        let mut stdout_reader;
        {
            let mut guard = process.lock().await;
            stdin_handle = guard.stdin.clone();
            pending_handle = guard.pending_permission_inputs.clone();
            pending_control_handle = guard.pending_control_responses.clone();
            stdout_reader = guard.stdout_reader.take().ok_or_else(|| {
                AgentExecutorError::Protocol(
                    "claude stdout_reader already taken (concurrent turn on same session?)"
                        .to_string(),
                )
            })?;
        }

        {
            let mut stdin = stdin_handle.lock().await;
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(AgentExecutorError::from)?;
            stdin.flush().await.map_err(AgentExecutorError::from)?;
        }

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ExecutorEvent, AgentExecutorError>>(32);
        let turn_timeout = self.turn_timeout;
        let process_for_return = process.clone();
        let stdin_for_task = stdin_handle.clone();
        let pending_for_task = pending_handle.clone();

        tokio::spawn(async move {
            let deadline = tokio::time::sleep(turn_timeout);
            tokio::pin!(deadline);

            let mut iterations: u32 = 0;
            let mut final_text: Option<String> = None;

            // `'turn` lets every early-exit path carry `stdout_reader` back to
            // the outer scope so we can restore it into the process struct.
            // Dropping the reader here would force a re-spawn of the CLI for
            // the next turn — which doesn't work since the child's stdout is
            // already taken at spawn.
            let returned_reader = 'turn: loop {
                let mut line = String::new();
                let read_fut = stdout_reader.read_line(&mut line);
                tokio::pin!(read_fut);

                tokio::select! {
                    _ = &mut deadline => {
                        let _ = tx
                            .send(Err(AgentExecutorError::Timeout {
                                operation: "send_message".to_string(),
                                seconds: turn_timeout.as_secs(),
                            }))
                            .await;
                        break 'turn stdout_reader;
                    }
                    res = &mut read_fut => {
                        match res {
                            Ok(0) => {
                                let _ = tx
                                    .send(Err(AgentExecutorError::Protocol(
                                        "claude stdout closed mid-turn".to_string(),
                                    )))
                                    .await;
                                break 'turn stdout_reader;
                            }
                            Ok(_) => {
                                match route_control_response_inline(
                                    &line,
                                    None,
                                    &pending_control_handle,
                                )
                                .await
                                {
                                    Ok(InlineControlResponse::Consumed) => continue,
                                    Ok(InlineControlResponse::Matched(_)) => continue,
                                    Ok(InlineControlResponse::NotControl) => {}
                                    Err(error) => {
                                        if tx.send(Err(error)).await.is_err() {
                                            break 'turn stdout_reader;
                                        }
                                        continue;
                                    }
                                }
                                // Intercept `control_request` before the
                                // ClaudeJsonEvent parser so we can surface
                                // `can_use_tool` as an `ExecutorEvent::PermissionRequest`
                                // and reply to hook_callback / mcp_message /
                                // unknown subtypes so the CLI does not hang.
                                if let Some(done) = handle_control_request_inline(
                                    &line,
                                    &tx,
                                    &stdin_for_task,
                                    &pending_for_task,
                                )
                                .await
                                {
                                    if done {
                                        break 'turn stdout_reader;
                                    }
                                    continue;
                                }
                                let Some(parsed) = parse_stream_line(&line) else { continue };
                                let should_fetch_context_usage = matches!(
                                    &parsed,
                                    Ok(ClaudeJsonEvent::Result {
                                        subtype,
                                        is_error,
                                        ..
                                    }) if subtype == "success" && !is_error
                                );
                                let event = match parsed {
                                    Ok(e) => e,
                                    Err(e) => {
                                        if tx
                                            .send(Ok(ExecutorEvent::NativeEvent {
                                                provider: Cow::Borrowed(VENDOR_NAME),
                                                payload: json!({
                                                    "raw": line.trim(),
                                                    "parse_error": e.to_string(),
                                                }),
                                            }))
                                            .await
                                            .is_err()
                                        {
                                            break 'turn stdout_reader;
                                        }
                                        continue;
                                    }
                                };
                                // Fetch context_usage BEFORE dispatching
                                // Result: the normalizer returns Ok(true) on
                                // TurnComplete and the upstream consumer
                                // breaks + drops the stream, so any
                                // NativeEvent emitted after TurnComplete
                                // would be silently discarded.
                                if should_fetch_context_usage {
                                    match request_context_usage_inline(
                                        &mut stdout_reader,
                                        &tx,
                                        &stdin_for_task,
                                        &pending_for_task,
                                        &pending_control_handle,
                                    )
                                    .await
                                    {
                                        Ok(Some(payload)) => {
                                            if tx
                                                .send(Ok(ExecutorEvent::NativeEvent {
                                                    provider: Cow::Borrowed(VENDOR_NAME),
                                                    payload,
                                                }))
                                                .await
                                                .is_err()
                                            {
                                                break 'turn stdout_reader;
                                            }
                                        }
                                        Ok(None) => {}
                                        Err(error) => {
                                            if tx.send(Err(error)).await.is_err() {
                                                break 'turn stdout_reader;
                                            }
                                        }
                                    }
                                }
                                let done = dispatch_event(
                                    event,
                                    &tx,
                                    &mut iterations,
                                    &mut final_text,
                                )
                                .await;
                                if done {
                                    break 'turn stdout_reader;
                                }
                            }
                            Err(e) => {
                                let _ = tx.send(Err(AgentExecutorError::from(e))).await;
                                break 'turn stdout_reader;
                            }
                        }
                    }
                }
            };

            // Restore stdout_reader into the process struct so the next turn
            // can `take()` it again. Brief outer lock; no other owner holds
            // it now that the reader task is about to exit.
            let mut guard = process_for_return.lock().await;
            guard.stdout_reader = Some(returned_reader);
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn respond_to_permission(
        &self,
        session: &SessionRef,
        request_id: String,
        decision: PermissionDecision,
    ) -> Result<(), AgentExecutorError> {
        let process = self.get_session(session).await?;
        // Grab the stdin + pending-input handles under a short outer lock
        // so we release it before acquiring the inner stdin/pending locks.
        // Keeping the outer lock across writes would deadlock the per-turn
        // stream reader which also holds it.
        let (stdin_handle, pending_handle) = {
            let guard = process.lock().await;
            (guard.stdin.clone(), guard.pending_permission_inputs.clone())
        };

        // Pull out the original tool input we captured when the CLI sent
        // `control_request can_use_tool`. Claude treats `updatedInput` as
        // the final args for the tool_use — not echoing it back makes the
        // downstream tool run with empty args (which silently wedges the
        // turn, since the CLI keeps waiting for a tool result). Fall back
        // to `{}` only when the original input is unknown (shouldn't
        // happen in practice).
        let original_input = pending_handle
            .lock()
            .await
            .remove(&request_id)
            .unwrap_or_else(|| json!({}));

        // SDK-shape `control_response`. Earlier drafts used
        // `permission_response` (no longer recognised by the CLI) and an
        // empty `updatedInput` (caused tool-use with no args).
        let response = match decision {
            PermissionDecision::Allow => json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": request_id,
                    "response": {
                        "behavior": "allow",
                        "updatedInput": original_input,
                    }
                }
            }),
            PermissionDecision::Deny => json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": request_id,
                    "response": {
                        "behavior": "deny",
                        "message": "Denied by user",
                    }
                }
            }),
            PermissionDecision::Abort => json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": request_id,
                    "response": {
                        "behavior": "deny",
                        "message": "Aborted by user",
                        "interrupt": true,
                    }
                }
            }),
            // Claude control_response doesn't carry a vendor-option slot;
            // the SDK only understands allow/deny. Treat any vendor-chosen
            // option as Allow — Claude sessions never surface gemini-style
            // option lists in the UI, so this branch is only hit if a
            // mixed-vendor client accidentally routes here.
            PermissionDecision::SelectedOption { .. } => json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": request_id,
                    "response": {
                        "behavior": "allow",
                        "updatedInput": original_input,
                    }
                }
            }),
        };
        let line = format!(
            "{}\n",
            serde_json::to_string(&response).map_err(|e| AgentExecutorError::Protocol(format!(
                "failed to serialise permission response: {e}"
            )))?
        );
        let mut stdin = stdin_handle.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(AgentExecutorError::from)?;
        stdin.flush().await.map_err(AgentExecutorError::from)?;
        Ok(())
    }

    async fn interrupt(&self, session: &SessionRef) -> Result<(), AgentExecutorError> {
        let process = self.get_session(session).await?;
        // SDK control_request — `/cancel` as user text was a workaround for
        // older CLI versions; current CLI honours `subtype: "interrupt"`
        // mid-turn. Reference: Python SDK `_internal/query.py::interrupt`.
        let request_id = format!("req_interrupt_{}", Uuid::new_v4());
        let frame = json!({
            "type": "control_request",
            "request_id": request_id,
            "request": { "subtype": "interrupt" }
        });
        let line = format!("{}\n", serde_json::to_string(&frame).unwrap());
        let stdin_handle = {
            let guard = process.lock().await;
            guard.stdin.clone()
        };
        let mut stdin = stdin_handle.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(AgentExecutorError::from)?;
        stdin.flush().await.map_err(AgentExecutorError::from)?;
        Ok(())
    }

    async fn close_session(&self, session: &SessionRef) -> Result<(), AgentExecutorError> {
        let entry = self.remove_session(&session.process_handle).await;
        if let Some(slot) = entry {
            let stdin_handle;
            {
                let guard = slot.lock().await;
                stdin_handle = guard.stdin.clone();
            }
            // Closing stdin signals EOF to the CLI which exits cleanly.
            {
                let mut stdin = stdin_handle.lock().await;
                let _ = stdin.shutdown().await;
            }
            let mut guard = slot.lock().await;
            let _ = guard.child.kill().await;
            let _ = guard.child.wait().await;
        }
        Ok(())
    }

    async fn set_permission_mode(
        &self,
        session: &SessionRef,
        mode: PermissionMode,
    ) -> Result<(), AgentExecutorError> {
        // Use the SDK control_request protocol — not a `/permission` user
        // slash command. In stream-json mode the CLI treats the slash as a
        // literal user message and will ignore or echo it; only a proper
        // `control_request { subtype: "set_permission_mode", mode }` actually
        // mutates the in-flight session's mode mid-turn.
        // Reference: Python SDK `_internal/query.py::set_permission_mode`.
        let request_id = format!("req_set_mode_{}", Uuid::new_v4());
        self.send_control_request(
            session,
            &request_id,
            json!({
                "subtype": "set_permission_mode",
                "mode": permission_mode_cli_value(mode),
            }),
            "set_permission_mode",
        )
        .await?;
        Ok(())
    }

    async fn set_model(
        &self,
        session: &SessionRef,
        model: ModelSpec,
    ) -> Result<ModelChangeOutcome, AgentExecutorError> {
        // SDK control_request — same reasoning as `set_permission_mode`.
        // Reference: Python SDK `_internal/query.py::set_model`.
        let request_id = format!("req_set_model_{}", Uuid::new_v4());
        self.send_control_request(
            session,
            &request_id,
            json!({
                "subtype": "set_model",
                "model": model.model_id,
            }),
            "set_model",
        )
        .await?;

        if let Some(max_thinking_tokens) =
            runtime_max_thinking_tokens(model.reasoning_effort.as_deref())
        {
            let request_id = format!("req_set_thinking_{}", Uuid::new_v4());
            self.send_control_request(
                session,
                &request_id,
                json!({
                    "subtype": "set_max_thinking_tokens",
                    "max_thinking_tokens": max_thinking_tokens,
                }),
                "set_model (thinking)",
            )
            .await?;
        }
        Ok(ModelChangeOutcome::Applied)
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
    // Connection-reuse seam (Phase 1 of OR-P1 vendor pre-connection refactor).
    //
    // **Important** (see `docs/claude-p1-protocol-findings.md`): The claude
    // CLI ignores the `session_id` on inbound user frames and enforces a
    // single session per subprocess. Two Cteno sessions CANNOT share a single
    // `claude` subprocess. Therefore this seam is a thin shim:
    //
    //   * `open_connection` runs a version probe, cheap and side-effect-free.
    //   * `start_session_on` unconditionally spawns a fresh subprocess via
    //     `spawn_internal`. The `ConnectionHandle` only guards that the CLI
    //     has been probed once — it does NOT own a subprocess.
    //   * `close_connection` is essentially a no-op.
    //   * `check_connection` re-runs the version probe for cheap liveness.
    //
    // `AgentCapabilities::supports_multi_session_per_process` stays `false`.
    // -----------------------------------------------------------------------

    async fn open_connection(
        &self,
        spec: ConnectionSpec,
    ) -> Result<ConnectionHandle, AgentExecutorError> {
        // Always run the CLI version probe first; it is cheap (~200ms) and
        // provides the authoritative "is this binary usable" answer.
        let checked_at = match check_cli_version(&self.claude_path).await {
            Ok(()) => Some(Utc::now()),
            Err(err) => {
                return Err(match err {
                    ClaudeAdapterError::UnsupportedCliVersion { found, minimum } => {
                        AgentExecutorError::Vendor {
                            vendor: VENDOR_NAME,
                            message: format!(
                                "claude CLI version {found} is unsupported (minimum {minimum})"
                            ),
                        }
                    }
                    other => AgentExecutorError::Vendor {
                        vendor: VENDOR_NAME,
                        message: other.to_string(),
                    },
                });
            }
        };

        // `ConnectionSpec::probe` is a fast-path for callers that only want
        // to confirm the CLI is usable without committing to any follow-on
        // session. Since we already stopped at the version check, `probe`
        // simply records that intent in the cached inner state.
        let inner = Arc::new(ClaudeConnectionInner::new(spec.probe, checked_at));

        Ok(ConnectionHandle {
            id: ConnectionHandleId::new(),
            vendor: VENDOR_NAME,
            inner,
        })
    }

    async fn close_connection(&self, handle: ConnectionHandle) -> Result<(), AgentExecutorError> {
        // No subprocess is tied to the connection handle — every session owns
        // its own subprocess (Phase A constraint). Verify the handle belongs
        // to us so we surface `Protocol` errors early instead of silently
        // ignoring stray handles from other vendors.
        if handle.vendor != VENDOR_NAME {
            return Err(AgentExecutorError::Protocol(format!(
                "close_connection received non-claude handle (vendor={})",
                handle.vendor
            )));
        }
        if handle
            .inner
            .downcast_ref::<ClaudeConnectionInner>()
            .is_none()
        {
            return Err(AgentExecutorError::Protocol(
                "close_connection received claude handle with unknown inner type".to_string(),
            ));
        }
        Ok(())
    }

    async fn check_connection(
        &self,
        handle: &ConnectionHandle,
    ) -> Result<ConnectionHealth, AgentExecutorError> {
        if handle.vendor != VENDOR_NAME {
            return Err(AgentExecutorError::Protocol(format!(
                "check_connection received non-claude handle (vendor={})",
                handle.vendor
            )));
        }
        let inner = handle
            .inner
            .downcast_ref::<ClaudeConnectionInner>()
            .ok_or_else(|| {
                AgentExecutorError::Protocol(
                    "check_connection received claude handle with unknown inner type".to_string(),
                )
            })?;

        // The CLI binary still has to be present. No other transport to probe.
        match check_cli_version(&self.claude_path).await {
            Ok(()) => {
                *inner.version_checked_at.lock().await = Some(Utc::now());
                Ok(ConnectionHealth::Healthy)
            }
            Err(err) => Ok(ConnectionHealth::Dead {
                reason: err.to_string(),
            }),
        }
    }

    async fn start_session_on(
        &self,
        handle: &ConnectionHandle,
        spec: SpawnSessionSpec,
    ) -> Result<SessionRef, AgentExecutorError> {
        if handle.vendor != VENDOR_NAME {
            return Err(AgentExecutorError::Protocol(format!(
                "start_session_on received non-claude handle (vendor={})",
                handle.vendor
            )));
        }
        if handle
            .inner
            .downcast_ref::<ClaudeConnectionInner>()
            .is_none()
        {
            return Err(AgentExecutorError::Protocol(
                "start_session_on received claude handle with unknown inner type".to_string(),
            ));
        }

        // The CLI funnels every session's user frames through a single
        // CLI-generated session id per subprocess (empirically confirmed in
        // Phase A). We therefore cannot multiplex multiple Cteno sessions on
        // one subprocess — unconditionally spawn a fresh one per session.
        self.spawn_session(spec).await
    }
}

/// Translate a decoded [`ClaudeJsonEvent`] into zero or more
/// [`ExecutorEvent`]s and push them through the channel. Returns `true` when
/// the turn is complete and the worker should exit.
async fn dispatch_event(
    event: ClaudeJsonEvent,
    tx: &tokio::sync::mpsc::Sender<Result<ExecutorEvent, AgentExecutorError>>,
    iterations: &mut u32,
    final_text: &mut Option<String>,
) -> bool {
    match event {
        ClaudeJsonEvent::System {
            subtype,
            session_id,
            tools,
            state,
            task_id,
            description,
            summary,
            status,
            output_file,
            tool_use_id,
            task_type,
            usage,
            last_tool_name,
            uuid,
        } => {
            let payload = match subtype.as_str() {
                "task_started" => Some(build_claude_task_native_event(
                    "task_started",
                    task_id,
                    description,
                    None,
                    None,
                    None,
                    tool_use_id,
                    task_type,
                    usage,
                    None,
                    uuid,
                    session_id,
                )),
                "task_progress" => Some(build_claude_task_native_event(
                    "task_progress",
                    task_id,
                    description,
                    summary,
                    None,
                    None,
                    tool_use_id,
                    task_type,
                    usage,
                    last_tool_name,
                    uuid,
                    session_id,
                )),
                "task_notification" => Some(build_claude_task_native_event(
                    "task_notification",
                    task_id,
                    None,
                    summary,
                    status,
                    output_file,
                    tool_use_id,
                    task_type,
                    usage,
                    None,
                    uuid,
                    session_id,
                )),
                _ => Some(json!({
                    "kind": "system",
                    "subtype": subtype,
                    "session_id": session_id,
                    "tools": tools,
                    "state": state,
                })),
            };

            if let Some(payload) = payload {
                let _ = tx
                    .send(Ok(ExecutorEvent::NativeEvent {
                        provider: Cow::Borrowed(VENDOR_NAME),
                        payload,
                    }))
                    .await;
            }
            false
        }
        ClaudeJsonEvent::Assistant { message, .. } => {
            *iterations = iterations.saturating_add(1);
            // Text and Thinking are already streamed via StreamEvent
            // (content_block_delta) because we spawn Claude CLI with
            // `--include-partial-messages`. Fall back to populating
            // final_text from the aggregated Assistant frame only when
            // streaming contributed nothing this turn (snapshot captured
            // before the loop so multi-block frames still concatenate).
            let streaming_already_populated = final_text.is_some();
            for block in message.content {
                let outgoing = match block {
                    ClaudeContent::Text { text } => {
                        if !streaming_already_populated {
                            let snapshot = final_text.clone().unwrap_or_default();
                            *final_text = Some(if snapshot.is_empty() {
                                text
                            } else {
                                format!("{snapshot}\n{text}")
                            });
                        }
                        continue;
                    }
                    ClaudeContent::Thinking { .. } => {
                        continue;
                    }
                    ClaudeContent::ToolUse { id, name, input } => ExecutorEvent::ToolCallStart {
                        tool_use_id: id,
                        name,
                        input,
                        partial: false,
                    },
                    ClaudeContent::RedactedThinking => ExecutorEvent::StreamDelta {
                        kind: DeltaKind::Thinking,
                        content: "[redacted thinking]".to_string(),
                    },
                    _ => ExecutorEvent::NativeEvent {
                        provider: Cow::Borrowed(VENDOR_NAME),
                        payload: json!({ "kind": "unknown_assistant_content" }),
                    },
                };
                if tx.send(Ok(outgoing)).await.is_err() {
                    return true;
                }
            }
            false
        }
        ClaudeJsonEvent::User { message, .. } => {
            // The CLI echoes tool results (inside content blocks) and plain
            // user text through `user` frames. Fan out `tool_result` blocks
            // as `ExecutorEvent::ToolResult` so the card transitions out of
            // the "running" state; forward anything else as a debug-only
            // native marker.
            match message.content {
                crate::stream::ClaudeUserMessageContent::Text(_) => {
                    let _ = tx
                        .send(Ok(ExecutorEvent::NativeEvent {
                            provider: Cow::Borrowed(VENDOR_NAME),
                            payload: json!({ "kind": "user_text_frame" }),
                        }))
                        .await;
                }
                crate::stream::ClaudeUserMessageContent::Blocks(blocks) => {
                    for block in blocks {
                        match block {
                            ClaudeContent::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } => {
                                let text = flatten_claude_tool_result_content(&content);
                                let output = if is_error { Err(text) } else { Ok(text) };
                                if tx
                                    .send(Ok(ExecutorEvent::ToolResult {
                                        tool_use_id,
                                        output,
                                    }))
                                    .await
                                    .is_err()
                                {
                                    return true;
                                }
                            }
                            _ => {
                                let _ = tx
                                    .send(Ok(ExecutorEvent::NativeEvent {
                                        provider: Cow::Borrowed(VENDOR_NAME),
                                        payload: json!({ "kind": "user_frame_other" }),
                                    }))
                                    .await;
                            }
                        }
                    }
                }
            }
            false
        }
        ClaudeJsonEvent::RateLimitEvent { .. } => {
            // Rate-limit / usage data now flows through the machine-level
            // `cteno-host-usage-monitor` which polls Anthropic's HTTPS
            // headers directly. The stream-json rate_limit_event is too
            // sparse (no utilization when below threshold) to drive the
            // indicator, so we drop it here.
            false
        }
        ClaudeJsonEvent::TaskStarted {
            task_id,
            description,
            tool_use_id,
            session_id,
        } => {
            let _ = tx
                .send(Ok(ExecutorEvent::NativeEvent {
                    provider: Cow::Borrowed(VENDOR_NAME),
                    payload: build_claude_task_native_event(
                        "task_started",
                        Some(task_id),
                        Some(description),
                        None,
                        None,
                        None,
                        tool_use_id,
                        None,
                        None,
                        None,
                        None,
                        session_id,
                    ),
                }))
                .await;
            false
        }
        ClaudeJsonEvent::TaskProgress {
            task_id,
            description,
            summary,
            last_tool_name,
            tool_use_id,
            session_id,
        } => {
            let _ = tx
                .send(Ok(ExecutorEvent::NativeEvent {
                    provider: Cow::Borrowed(VENDOR_NAME),
                    payload: build_claude_task_native_event(
                        "task_progress",
                        Some(task_id),
                        Some(description),
                        summary,
                        None,
                        None,
                        tool_use_id,
                        None,
                        None,
                        last_tool_name,
                        None,
                        session_id,
                    ),
                }))
                .await;
            false
        }
        ClaudeJsonEvent::TaskNotification {
            task_id,
            status,
            summary,
            output_file,
            tool_use_id,
            session_id,
        } => {
            let _ = tx
                .send(Ok(ExecutorEvent::NativeEvent {
                    provider: Cow::Borrowed(VENDOR_NAME),
                    payload: build_claude_task_native_event(
                        "task_notification",
                        Some(task_id),
                        None,
                        Some(summary),
                        Some(status),
                        output_file,
                        tool_use_id,
                        None,
                        None,
                        None,
                        None,
                        session_id,
                    ),
                }))
                .await;
            false
        }
        ClaudeJsonEvent::Result {
            subtype,
            is_error,
            result,
            usage,
            model_usage,
            ..
        } => {
            if is_error || subtype != "success" {
                let _ = tx
                    .send(Ok(ExecutorEvent::Error {
                        message: result,
                        recoverable: !is_error,
                    }))
                    .await;
            } else {
                let _ = tx
                    .send(Ok(ExecutorEvent::TurnComplete {
                        final_text: final_text.clone().or(Some(result)),
                        iteration_count: *iterations,
                        usage: token_usage_from_result(usage.as_ref(), model_usage.as_ref()),
                    }))
                    .await;
            }
            true
        }
        // Stream events from --include-partial-messages (token-level streaming)
        ClaudeJsonEvent::StreamEvent { event } => {
            if let Some(evt) = event.as_ref() {
                let evt_type = evt.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if evt_type == "content_block_delta" {
                    let delta = evt.get("delta").cloned().unwrap_or_default();
                    let delta_type = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match delta_type {
                        "text_delta" => {
                            let text = delta
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !text.is_empty() {
                                let snapshot = final_text.clone().unwrap_or_default();
                                *final_text = Some(format!("{snapshot}{text}"));
                                let _ = tx
                                    .send(Ok(ExecutorEvent::StreamDelta {
                                        kind: DeltaKind::Text,
                                        content: text,
                                    }))
                                    .await;
                            }
                        }
                        "thinking_delta" => {
                            let text = delta
                                .get("thinking")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !text.is_empty() {
                                let _ = tx
                                    .send(Ok(ExecutorEvent::StreamDelta {
                                        kind: DeltaKind::Thinking,
                                        content: text,
                                    }))
                                    .await;
                            }
                        }
                        _ => {}
                    }
                }
            }
            false
        }
        // All other event types: forward as NativeEvent
        _ => {
            let _ = tx
                .send(Ok(ExecutorEvent::NativeEvent {
                    provider: Cow::Borrowed(VENDOR_NAME),
                    payload: json!({ "kind": "unhandled_event" }),
                }))
                .await;
            false
        }
    }
}

/// Peek at a stdout line. If it is a `control_request`, surface permission
/// prompts as `ExecutorEvent::PermissionRequest`, reply to hook / mcp /
/// unknown subtypes so the CLI doesn't block waiting, and return `Some(done)`.
/// Returns `None` when the line is not a control_request (caller continues
/// normal parsing).
async fn handle_control_request_inline(
    raw_line: &str,
    tx: &tokio::sync::mpsc::Sender<Result<ExecutorEvent, AgentExecutorError>>,
    stdin_handle: &Arc<Mutex<ChildStdin>>,
    pending_permission_inputs: &Arc<Mutex<HashMap<String, Value>>>,
) -> Option<bool> {
    let trimmed = raw_line.trim();
    if trimmed.is_empty() || !trimmed.starts_with('{') {
        return None;
    }
    let Ok(raw) = serde_json::from_str::<Value>(trimmed) else {
        return None;
    };
    if raw.get("type").and_then(|v| v.as_str()) != Some("control_request") {
        return None;
    }
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
            let request = raw.get("request").cloned().unwrap_or(Value::Null);
            let tool_name = request
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tool_input = request.get("input").cloned().unwrap_or(Value::Null);
            // Remember the original input so `respond_to_permission` can
            // echo it back as `updatedInput`. Claude treats the response
            // `updatedInput` as the final tool_use args — if we reply with
            // an empty object the downstream Bash/Edit/etc. runs with no
            // args and the turn stalls.
            pending_permission_inputs
                .lock()
                .await
                .insert(request_id.clone(), tool_input.clone());
            if tx
                .send(Ok(ExecutorEvent::PermissionRequest {
                    request_id,
                    tool_name,
                    tool_input,
                }))
                .await
                .is_err()
            {
                return Some(true);
            }
            Some(false)
        }
        "hook_callback" => {
            // We do not register any hooks via `initialize.hooks`, so the CLI
            // shouldn't fire these — but if it does, respond empty-success so
            // the CLI doesn't stall waiting on us.
            let resp = json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": request_id,
                    "async": false,
                    "hookSpecificOutput": null,
                }
            });
            let _ = write_control_line(stdin_handle, &resp).await;
            Some(false)
        }
        "mcp_message" => {
            let resp = json!({
                "type": "control_response",
                "response": {
                    "subtype": "error",
                    "request_id": request_id,
                    "error": "SDK MCP servers not configured in cteno Claude adapter",
                }
            });
            let _ = write_control_line(stdin_handle, &resp).await;
            Some(false)
        }
        other => {
            let resp = json!({
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
            let _ = write_control_line(stdin_handle, &resp).await;
            Some(false)
        }
    }
}

async fn write_control_line(
    stdin_handle: &Arc<Mutex<ChildStdin>>,
    value: &Value,
) -> std::io::Result<()> {
    let payload = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    let line = format!("{payload}\n");
    let mut stdin = stdin_handle.lock().await;
    stdin.write_all(line.as_bytes()).await?;
    stdin.flush().await
}

async fn request_context_usage_inline(
    stdout_reader: &mut BufReader<ChildStdout>,
    tx: &tokio::sync::mpsc::Sender<Result<ExecutorEvent, AgentExecutorError>>,
    stdin_handle: &Arc<Mutex<ChildStdin>>,
    pending_permission_inputs: &Arc<Mutex<HashMap<String, Value>>>,
    pending_control_responses: &PendingControlResponses,
) -> Result<Option<Value>, AgentExecutorError> {
    let request_id = format!("req_context_usage_{}", Uuid::new_v4());
    write_control_line(
        stdin_handle,
        &json!({
            "type": "control_request",
            "request_id": request_id,
            "request": {
                "subtype": "get_context_usage",
            }
        }),
    )
    .await
    .map_err(AgentExecutorError::from)?;

    loop {
        let mut line = String::new();
        let n = stdout_reader
            .read_line(&mut line)
            .await
            .map_err(AgentExecutorError::from)?;
        if n == 0 {
            return Err(AgentExecutorError::Protocol(
                "claude stdout closed before get_context_usage response".to_string(),
            ));
        }
        match route_control_response_inline(&line, Some(&request_id), pending_control_responses)
            .await?
        {
            InlineControlResponse::Matched(result) => {
                return result.map(|response| claude_context_usage_native_event(&response));
            }
            InlineControlResponse::Consumed => continue,
            InlineControlResponse::NotControl => {}
        }
        if let Some(done) =
            handle_control_request_inline(&line, tx, stdin_handle, pending_permission_inputs).await
        {
            if done {
                return Ok(None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex as StdMutex};

    #[derive(Default)]
    struct RecordingStore {
        records: StdMutex<Vec<(String, SessionRecord)>>,
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
            _vendor: &str,
            session_id: &NativeSessionId,
        ) -> Result<SessionInfo, String> {
            Err(format!(
                "unexpected get_session_info for {}",
                session_id.as_str()
            ))
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

    #[cfg(unix)]
    fn write_fake_claude_cli(temp_dir: &Path, error_on_set_model: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let cli_path = temp_dir.join("fake-claude.sh");
        let set_model_response = if error_on_set_model {
            "\"subtype\":\"error\",\"request_id\":\"'$req_id'\",\"error\":\"set_model failed\""
        } else {
            "\"subtype\":\"success\",\"request_id\":\"'$req_id'\",\"response\":{}"
        };
        std::fs::write(
            &cli_path,
            format!(
                concat!(
                    "#!/bin/sh\n",
                    "if [ \"$1\" = \"--version\" ]; then echo '2.5.0'; exit 0; fi\n",
                    "if [ -n \"$FAKE_CLAUDE_LOG\" ]; then printf 'ARGS:%s\\n' \"$*\" >> \"$FAKE_CLAUDE_LOG\"; fi\n",
                    "printf '%s\\n' '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"claude-native-123\"}}'\n",
                    "while IFS= read -r line; do\n",
                    "  if [ -n \"$FAKE_CLAUDE_LOG\" ]; then printf '%s\\n' \"$line\" >> \"$FAKE_CLAUDE_LOG\"; fi\n",
                    "  req_id=$(printf '%s' \"$line\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n",
                    "  case \"$line\" in\n",
                    "    *'\"subtype\":\"initialize\"'*) printf '%s\\n' '{{\"type\":\"control_response\",\"response\":{{\"subtype\":\"success\",\"request_id\":\"'$req_id'\",\"response\":{{}}}}}}' ;;\n",
                    "    *'\"subtype\":\"set_permission_mode\"'*) printf '%s\\n' '{{\"type\":\"control_response\",\"response\":{{\"subtype\":\"success\",\"request_id\":\"'$req_id'\",\"response\":{{}}}}}}' ;;\n",
                    "    *'\"subtype\":\"set_max_thinking_tokens\"'*) printf '%s\\n' '{{\"type\":\"control_response\",\"response\":{{\"subtype\":\"success\",\"request_id\":\"'$req_id'\",\"response\":{{}}}}}}' ;;\n",
                    "    *'\"subtype\":\"set_model\"'*) printf '%s\\n' '{{\"type\":\"control_response\",\"response\":{{{}}}}}' ;;\n",
                    "  esac\n",
                    "done\n",
                ),
                set_model_response,
            ),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&cli_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&cli_path, perms).unwrap();
        cli_path
    }

    #[cfg(unix)]
    fn spawn_spec(workdir: &Path, log_path: &Path) -> SpawnSessionSpec {
        let mut env = BTreeMap::new();
        env.insert(
            "FAKE_CLAUDE_LOG".to_string(),
            log_path.to_string_lossy().into_owned(),
        );
        SpawnSessionSpec {
            workdir: workdir.to_path_buf(),
            system_prompt: None,
            model: None,
            permission_mode: PermissionMode::Default,
            allowed_tools: None,
            additional_directories: Vec::new(),
            env,
            agent_config: Value::Null,
            resume_hint: None,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn spawn_session_records_local_row() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-session-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let log_path = temp_dir.join("claude.log");

        let executor = ClaudeAgentExecutor::new(cli_path.clone(), store.clone())
            .with_spawn_ready_timeout(Duration::from_secs(5));

        let session = executor
            .spawn_session(spawn_spec(&temp_dir, &log_path))
            .await
            .unwrap();

        let records = store.records.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].0, VENDOR_NAME);
        // `spawn_internal` mints a UUID session-id up front and passes it
        // via `--session-id`, so the recorded id matches what the executor
        // reports on the returned `SessionRef`, not the `system:init`
        // payload the fake CLI emits.
        assert_eq!(records[0].1.session_id, session.id);
        assert_eq!(records[0].1.workdir, temp_dir);
        assert_eq!(
            records[0]
                .1
                .context
                .get("native_session_id")
                .and_then(|v| v.as_str()),
            Some(session.id.as_str())
        );

        drop(records);
        executor.close_session(&session).await.unwrap();
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn spawn_session_enables_dangerous_skip_permissions() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir = std::env::temp_dir().join(format!("claude-danger-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let log_path = temp_dir.join("claude.log");

        let executor = ClaudeAgentExecutor::new(cli_path, store)
            .with_spawn_ready_timeout(Duration::from_secs(5));

        let session = executor
            .spawn_session(spawn_spec(&temp_dir, &log_path))
            .await
            .unwrap();

        let log = std::fs::read_to_string(&log_path).unwrap();
        assert!(log.contains("--dangerously-skip-permissions"));

        executor.close_session(&session).await.unwrap();
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn spawn_session_reapplies_requested_permission_mode_after_initialize() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-reapply-mode-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let log_path = temp_dir.join("claude.log");

        let executor = ClaudeAgentExecutor::new(cli_path, store)
            .with_spawn_ready_timeout(Duration::from_secs(5));

        let session = executor
            .spawn_session(spawn_spec(&temp_dir, &log_path))
            .await
            .unwrap();

        let log = std::fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = log.lines().collect();
        let init_idx = lines
            .iter()
            .position(|line| line.contains("\"subtype\":\"initialize\""))
            .unwrap();
        let set_mode_idx = lines
            .iter()
            .position(|line| {
                line.contains("\"subtype\":\"set_permission_mode\"")
                    && line.contains("\"mode\":\"default\"")
            })
            .unwrap();
        assert!(set_mode_idx > init_idx);

        executor.close_session(&session).await.unwrap();
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn reasoning_effort_maps_to_claude_thinking_controls() {
        assert_eq!(spawn_thinking_mode_cli_value(None), None);
        assert_eq!(spawn_thinking_mode_cli_value(Some("low")), Some("disabled"));
        assert_eq!(
            spawn_thinking_mode_cli_value(Some("minimal")),
            Some("disabled")
        );
        assert_eq!(spawn_thinking_mode_cli_value(Some("medium")), None);

        assert_eq!(runtime_max_thinking_tokens(None), Some(None));
        assert_eq!(runtime_max_thinking_tokens(Some("medium")), Some(None));
        assert_eq!(runtime_max_thinking_tokens(Some("high")), Some(None));
        assert_eq!(runtime_max_thinking_tokens(Some("max")), Some(None));
        assert_eq!(runtime_max_thinking_tokens(Some("low")), Some(Some(0)));
        assert_eq!(runtime_max_thinking_tokens(Some("disabled")), Some(Some(0)));
        assert_eq!(runtime_max_thinking_tokens(Some("custom")), None);
    }

    #[test]
    fn context_usage_control_response_maps_to_native_event() {
        let payload = claude_context_usage_native_event(&json!({
            "subtype": "success",
            "request_id": "req_context",
            "response": {
                "totalTokens": 98200,
                "maxTokens": 155000,
                "rawMaxTokens": 200000,
                "percentage": 49.1,
                "model": "claude-sonnet-4-5"
            }
        }))
        .unwrap();

        assert_eq!(
            payload.get("kind").and_then(|v| v.as_str()),
            Some("context_usage")
        );
        assert_eq!(
            payload.get("total_tokens").and_then(|v| v.as_u64()),
            Some(98200)
        );
        assert_eq!(
            payload.get("max_tokens").and_then(|v| v.as_u64()),
            Some(155000)
        );
        assert_eq!(
            payload.get("raw_max_tokens").and_then(|v| v.as_u64()),
            Some(200000)
        );
        assert_eq!(
            payload.get("model").and_then(|v| v.as_str()),
            Some("claude-sonnet-4-5")
        );
    }

    #[tokio::test]
    async fn dispatch_event_routes_top_level_task_started_with_system_shape() {
        let event = parse_stream_line(
            r#"{"type":"task_started","task_id":"t-1","description":"top-level task","tool_use_id":"tu-1"}"#,
        )
        .unwrap()
        .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut iterations = 0;
        let mut final_text = None;

        let done = dispatch_event(event, &tx, &mut iterations, &mut final_text).await;
        assert!(!done);

        let top_level_payload = match rx.recv().await.unwrap().unwrap() {
            ExecutorEvent::NativeEvent { provider, payload } => {
                assert_eq!(provider, VENDOR_NAME);
                assert_eq!(
                    payload,
                    json!({
                        "kind": "task_started",
                        "task_id": "t-1",
                        "description": "top-level task",
                        "summary": Value::Null,
                        "status": Value::Null,
                        "output_file": Value::Null,
                        "tool_use_id": "tu-1",
                        "task_type": Value::Null,
                        "usage": Value::Null,
                        "last_tool_name": Value::Null,
                        "uuid": Value::Null,
                        "session_id": Value::Null,
                    })
                );
                payload
            }
            other => panic!("expected NativeEvent, got {other:?}"),
        };

        let system_event = ClaudeJsonEvent::System {
            subtype: "task_started".to_string(),
            session_id: None,
            tools: None,
            state: None,
            task_id: Some("t-1".to_string()),
            description: Some("top-level task".to_string()),
            summary: None,
            status: None,
            output_file: None,
            tool_use_id: Some("tu-1".to_string()),
            task_type: None,
            usage: None,
            last_tool_name: None,
            uuid: None,
        };

        let done = dispatch_event(system_event, &tx, &mut iterations, &mut final_text).await;
        assert!(!done);

        let system_payload = match rx.recv().await.unwrap().unwrap() {
            ExecutorEvent::NativeEvent { payload, .. } => payload,
            other => panic!("expected NativeEvent, got {other:?}"),
        };

        assert_eq!(top_level_payload, system_payload);
    }

    #[test]
    fn claude_task_native_event_helper_emits_canonical_schema() {
        let usage = json!({
            "input_tokens": 11,
            "output_tokens": 7
        });

        assert_eq!(
            build_claude_task_native_event(
                "task_started",
                Some("task-start".to_string()),
                Some("start description".to_string()),
                None,
                None,
                None,
                Some("tool-1".to_string()),
                Some("agent".to_string()),
                Some(usage.clone()),
                None,
                Some("uuid-start".to_string()),
                Some("session-start".to_string()),
            ),
            json!({
                "kind": "task_started",
                "task_id": "task-start",
                "description": "start description",
                "summary": Value::Null,
                "status": Value::Null,
                "output_file": Value::Null,
                "tool_use_id": "tool-1",
                "task_type": "agent",
                "usage": usage.clone(),
                "last_tool_name": Value::Null,
                "uuid": "uuid-start",
                "session_id": "session-start",
            })
        );

        assert_eq!(
            build_claude_task_native_event(
                "task_progress",
                Some("task-progress".to_string()),
                Some("progress description".to_string()),
                Some("working".to_string()),
                None,
                None,
                Some("tool-2".to_string()),
                Some("workflow".to_string()),
                Some(usage.clone()),
                Some("Bash".to_string()),
                Some("uuid-progress".to_string()),
                Some("session-progress".to_string()),
            ),
            json!({
                "kind": "task_progress",
                "task_id": "task-progress",
                "description": "progress description",
                "summary": "working",
                "status": Value::Null,
                "output_file": Value::Null,
                "tool_use_id": "tool-2",
                "task_type": "workflow",
                "usage": usage.clone(),
                "last_tool_name": "Bash",
                "uuid": "uuid-progress",
                "session_id": "session-progress",
            })
        );

        assert_eq!(
            build_claude_task_native_event(
                "task_notification",
                Some("task-note".to_string()),
                None,
                Some("done".to_string()),
                Some("completed".to_string()),
                Some("/tmp/output.txt".to_string()),
                Some("tool-3".to_string()),
                Some("remote_agent".to_string()),
                Some(usage),
                None,
                Some("uuid-note".to_string()),
                Some("session-note".to_string()),
            ),
            json!({
                "kind": "task_notification",
                "task_id": "task-note",
                "description": Value::Null,
                "summary": "done",
                "status": "completed",
                "output_file": "/tmp/output.txt",
                "tool_use_id": "tool-3",
                "task_type": "remote_agent",
                "usage": {
                    "input_tokens": 11,
                    "output_tokens": 7
                },
                "last_tool_name": Value::Null,
                "uuid": "uuid-note",
                "session_id": "session-note",
            })
        );
    }

    #[tokio::test]
    async fn route_control_response_wakes_pending_request() {
        let pending: PendingControlResponses = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert("req_set_mode".to_string(), tx);

        let outcome = route_control_response_inline(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_set_mode","response":{}}}"#,
            None,
            &pending,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, InlineControlResponse::Consumed));
        let response = rx.await.unwrap().unwrap();
        assert_eq!(
            response.get("request_id").and_then(|v| v.as_str()),
            Some("req_set_mode")
        );
    }

    #[tokio::test]
    async fn route_control_response_surfaces_result_errors_during_handshake() {
        let pending: PendingControlResponses = Arc::new(Mutex::new(HashMap::new()));

        let outcome = route_control_response_inline(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["No conversation found with session ID: native-1"]}"#,
            Some("req_init"),
            &pending,
        )
        .await
        .unwrap();

        let InlineControlResponse::Matched(Err(AgentExecutorError::Vendor { vendor, message })) =
            outcome
        else {
            panic!("expected vendor error result");
        };
        assert_eq!(vendor, VENDOR_NAME);
        assert!(message.contains("No conversation found with session ID"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn set_permission_mode_uses_runtime_control_request() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-set-mode-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let log_path = temp_dir.join("claude.log");
        let executor = ClaudeAgentExecutor::new(cli_path, store)
            .with_spawn_ready_timeout(Duration::from_secs(5));

        let session = executor
            .spawn_session(spawn_spec(&temp_dir, &log_path))
            .await
            .unwrap();
        executor
            .set_permission_mode(&session, PermissionMode::AcceptEdits)
            .await
            .unwrap();

        let log = std::fs::read_to_string(&log_path).unwrap();
        assert!(log.contains("\"subtype\":\"set_permission_mode\""));
        assert!(log.contains("\"mode\":\"acceptEdits\""));

        executor.close_session(&session).await.unwrap();
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn set_model_waits_for_success_and_applies_thinking_mapping() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-set-model-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let log_path = temp_dir.join("claude.log");
        let executor = ClaudeAgentExecutor::new(cli_path, store)
            .with_spawn_ready_timeout(Duration::from_secs(5));

        let session = executor
            .spawn_session(spawn_spec(&temp_dir, &log_path))
            .await
            .unwrap();
        let outcome = executor
            .set_model(
                &session,
                ModelSpec {
                    provider: "anthropic".to_string(),
                    model_id: "claude-sonnet-4-6".to_string(),
                    reasoning_effort: Some("low".to_string()),
                    temperature: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(outcome, ModelChangeOutcome::Applied);
        let log = std::fs::read_to_string(&log_path).unwrap();
        assert!(log.contains("\"subtype\":\"set_model\""));
        assert!(log.contains("\"model\":\"claude-sonnet-4-6\""));
        assert!(log.contains("\"subtype\":\"set_max_thinking_tokens\""));
        assert!(log.contains("\"max_thinking_tokens\":0"));

        executor.close_session(&session).await.unwrap();
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn set_model_surfaces_control_response_errors() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-set-model-error-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, true);
        let log_path = temp_dir.join("claude.log");
        let executor = ClaudeAgentExecutor::new(cli_path, store)
            .with_spawn_ready_timeout(Duration::from_secs(5));

        let session = executor
            .spawn_session(spawn_spec(&temp_dir, &log_path))
            .await
            .unwrap();
        let error = executor
            .set_model(
                &session,
                ModelSpec {
                    provider: "anthropic".to_string(),
                    model_id: "claude-haiku-4-5".to_string(),
                    reasoning_effort: None,
                    temperature: None,
                },
            )
            .await
            .unwrap_err();

        match error {
            AgentExecutorError::Vendor { vendor, message } => {
                assert_eq!(vendor, VENDOR_NAME);
                assert_eq!(message, "set_model failed");
            }
            other => panic!("unexpected error: {other:?}"),
        }

        executor.close_session(&session).await.unwrap();
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // -----------------------------------------------------------------------
    // Connection-reuse seam tests (Phase 1 OR-P1 refactor).
    //
    // These confirm the 1:1 conn:session invariant empirically discovered in
    // Phase A (docs/claude-p1-protocol-findings.md): the claude CLI cannot
    // share a subprocess across sessions, so `open_connection` is a version
    // probe and `start_session_on` spawns a fresh subprocess per call.
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    #[tokio::test]
    async fn open_connection_returns_claude_vendor_handle() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-open-conn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let executor = ClaudeAgentExecutor::new(cli_path, store)
            .with_spawn_ready_timeout(Duration::from_secs(5));

        let handle = executor
            .open_connection(ConnectionSpec::default())
            .await
            .unwrap();

        assert_eq!(handle.vendor, VENDOR_NAME);
        let inner = handle
            .inner
            .downcast_ref::<ClaudeConnectionInner>()
            .expect("inner should be ClaudeConnectionInner");
        assert!(!inner.probe_only());
        assert!(inner.version_checked_at.lock().await.is_some());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_connection_probe_flag_is_recorded() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-open-probe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let executor = ClaudeAgentExecutor::new(cli_path, store);

        let spec = ConnectionSpec {
            env: Default::default(),
            probe: true,
        };
        let handle = executor.open_connection(spec).await.unwrap();
        let inner = handle
            .inner
            .downcast_ref::<ClaudeConnectionInner>()
            .unwrap();
        assert!(inner.probe_only());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_connection_tolerates_unprobeable_binary() {
        // `check_cli_version` is deliberately lenient — it returns `Ok(())`
        // when the binary cannot be spawned so a quirky CLI build does not
        // block startup (see `workspace::check_cli_version` rustdoc). This
        // test locks in that behavior so `open_connection` is never harsher
        // than the underlying probe. The registry layer can still detect a
        // genuinely-broken CLI later via the spawn failure on
        // `start_session_on`.
        let store = Arc::new(RecordingStore::default());
        let executor = ClaudeAgentExecutor::new(
            PathBuf::from("/nonexistent/claude-binary-does-not-exist"),
            store,
        );

        let handle = executor
            .open_connection(ConnectionSpec::default())
            .await
            .expect("version probe is lenient — open_connection should not fail");
        assert_eq!(handle.vendor, VENDOR_NAME);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn two_open_connections_get_distinct_handle_ids() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-conn-ids-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let executor = ClaudeAgentExecutor::new(cli_path, store);

        let h1 = executor
            .open_connection(ConnectionSpec::default())
            .await
            .unwrap();
        let h2 = executor
            .open_connection(ConnectionSpec::default())
            .await
            .unwrap();
        assert_ne!(h1.id, h2.id);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn check_connection_reports_healthy_for_working_cli() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-check-conn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let executor = ClaudeAgentExecutor::new(cli_path, store);

        let handle = executor
            .open_connection(ConnectionSpec::default())
            .await
            .unwrap();
        let health = executor.check_connection(&handle).await.unwrap();
        assert_eq!(health, ConnectionHealth::Healthy);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn check_connection_rejects_non_claude_handle() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-foreign-handle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let executor = ClaudeAgentExecutor::new(cli_path, store);

        let foreign = ConnectionHandle {
            id: ConnectionHandleId::new(),
            vendor: "codex",
            inner: Arc::new(()),
        };
        let err = executor.check_connection(&foreign).await.unwrap_err();
        match err {
            AgentExecutorError::Protocol(msg) => {
                assert!(msg.contains("non-claude"), "unexpected msg: {msg}");
            }
            other => panic!("expected Protocol error, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn close_connection_is_noop_for_valid_handle() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-close-conn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let executor = ClaudeAgentExecutor::new(cli_path, store);

        let handle = executor
            .open_connection(ConnectionSpec::default())
            .await
            .unwrap();
        executor.close_connection(handle).await.unwrap();
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn close_connection_rejects_non_claude_handle() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-close-foreign-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let executor = ClaudeAgentExecutor::new(cli_path, store);

        let foreign = ConnectionHandle {
            id: ConnectionHandleId::new(),
            vendor: "codex",
            inner: Arc::new(()),
        };
        let err = executor.close_connection(foreign).await.unwrap_err();
        assert!(matches!(err, AgentExecutorError::Protocol(_)));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn start_session_on_spawns_fresh_subprocess_per_call() {
        // Phase A constraint: the claude CLI enforces one session per
        // subprocess. Every `start_session_on` call must therefore fork a
        // brand-new process, and the adapter's per-session registry should
        // hold two distinct `ProcessHandleToken`s after two calls.
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-start-fresh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let log_path = temp_dir.join("claude.log");
        let executor = ClaudeAgentExecutor::new(cli_path, store)
            .with_spawn_ready_timeout(Duration::from_secs(5));

        let handle = executor
            .open_connection(ConnectionSpec::default())
            .await
            .unwrap();

        let s1 = executor
            .start_session_on(&handle, spawn_spec(&temp_dir, &log_path))
            .await
            .unwrap();
        let s2 = executor
            .start_session_on(&handle, spawn_spec(&temp_dir, &log_path))
            .await
            .unwrap();

        // Two sessions → two distinct process_handle tokens, two registry
        // entries. This is the critical invariant: sharing would violate the
        // CLI's one-session-per-subprocess enforcement.
        assert_ne!(s1.process_handle, s2.process_handle);
        {
            let sessions = executor.sessions.lock().await;
            assert_eq!(sessions.len(), 2, "expected two live subprocesses");
            assert!(sessions.contains_key(&s1.process_handle));
            assert!(sessions.contains_key(&s2.process_handle));
        }

        // The CLI invocation log should contain two separate spawn ARGS
        // lines (one per subprocess).
        let log = std::fs::read_to_string(&log_path).unwrap();
        let arg_lines = log.lines().filter(|l| l.starts_with("ARGS:")).count();
        assert_eq!(arg_lines, 2, "expected two spawns, log=\n{log}");

        executor.close_session(&s1).await.unwrap();
        executor.close_session(&s2).await.unwrap();
        executor.close_connection(handle).await.unwrap();
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn start_session_on_rejects_non_claude_handle() {
        let store = Arc::new(RecordingStore::default());
        let temp_dir =
            std::env::temp_dir().join(format!("claude-start-foreign-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cli_path = write_fake_claude_cli(&temp_dir, false);
        let log_path = temp_dir.join("claude.log");
        let executor = ClaudeAgentExecutor::new(cli_path, store);

        let foreign = ConnectionHandle {
            id: ConnectionHandleId::new(),
            vendor: "codex",
            inner: Arc::new(()),
        };
        let err = executor
            .start_session_on(&foreign, spawn_spec(&temp_dir, &log_path))
            .await
            .unwrap_err();
        assert!(matches!(err, AgentExecutorError::Protocol(_)));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capabilities_do_not_advertise_multi_session_per_process() {
        // Guard against accidental flip. Phase A proved the CLI cannot host
        // more than one session per subprocess, so the capability must stay
        // false — callers gate shared-subprocess code paths on this bit.
        let store = Arc::new(RecordingStore::default());
        let executor = ClaudeAgentExecutor::new(PathBuf::from("/unused"), store);
        let caps = executor.capabilities();
        assert!(
            !caps.supports_multi_session_per_process,
            "claude adapter must not advertise multi-session per process"
        );
    }
}
