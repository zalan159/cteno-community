//! `GeminiAcpConnection` — a single `gemini --acp` subprocess hosting many
//! sessions over a shared JSON-RPC 2.0 / ndJSON transport.
//!
//! The connection is created by `open_connection` (subprocess spawn +
//! `initialize` handshake + optional `authenticate`) and re-used for every
//! subsequent `session/new`. A per-connection demuxer task reads stdout, splits
//! frames into three buckets:
//!
//! 1. JSON-RPC responses → resolve the matching oneshot in
//!    `pending_requests`.
//! 2. Server-initiated JSON-RPC requests (permission / FS / terminal) →
//!    route to the addressed session's `pending_inbound` map so the adapter
//!    can echo an id-matched response back later.
//! 3. Server-initiated notifications (`session/update`) → emit
//!    `ExecutorEvent` frames into the session's broadcast channel.
//!
//! Closing the subprocess (stdin EOF, `Abort`, process exit) drains every
//! in-flight pending so callers see `ConnectionClosed` instead of hanging.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use multi_agent_runtime_core::{AgentExecutorError, ExecutorEvent};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::stream::{FrameKind, JsonRpcError, JsonRpcFrame, JsonRpcId};

/// How long `open_connection` waits for the `initialize` response before
/// failing with `AgentExecutorError::Timeout`.
pub const DEFAULT_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a single prompt turn is allowed to run before the caller times
/// out. Individual sessions can override via spec, but this is the default
/// ceiling applied by the adapter.
pub const DEFAULT_TURN_TIMEOUT: Duration = Duration::from_secs(600);

/// Channel capacity for per-session `ExecutorEvent` broadcasts.
const EVENT_BROADCAST_CAPACITY: usize = 256;

/// Channel capacity for outbound frames written to stdin.
const WRITER_QUEUE_CAPACITY: usize = 64;

/// Per-session state kept inside the shared connection.
///
/// Events flow via a `broadcast::Sender` so multiple consumers (e.g. the
/// normalizer + a debug tap) can subscribe simultaneously. `pending_inbound`
/// matches server-initiated requests (permission prompts) to the oneshot that
/// `respond_to_permission` / `respond_to_elicitation` will fulfill.
pub struct SessionState {
    pub events_tx: broadcast::Sender<ExecutorEvent>,
    pub pending_inbound: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    /// Best known active Gemini model for this session. Prompt responses may
    /// refine this via `_meta.quota.model_usage`, but this gives us a
    /// session-scoped hint before the first response arrives.
    pub current_model: Mutex<Option<String>>,
    /// Most recently seen prompt request id (u64). Used to cancel the right
    /// in-flight turn on `interrupt`, although Gemini resolves it on its own
    /// in response to `session/cancel`.
    pub in_flight_prompt_id: Mutex<Option<JsonRpcId>>,
    /// Number of permission requests that are blocking this session. Prompt
    /// watchdogs must pause while this is non-zero because Gemini intentionally
    /// moves the task into input-required until the user replies.
    pub pending_permission_count: AtomicU64,
    /// Whether the session is still considered live inside the connection.
    pub alive: AtomicBool,
}

impl SessionState {
    pub fn new(current_model: Option<String>) -> Self {
        let (events_tx, _) = broadcast::channel(EVENT_BROADCAST_CAPACITY);
        Self {
            events_tx,
            pending_inbound: Mutex::new(HashMap::new()),
            current_model: Mutex::new(current_model),
            in_flight_prompt_id: Mutex::new(None),
            pending_permission_count: AtomicU64::new(0),
            alive: AtomicBool::new(true),
        }
    }
}

/// Registered pending outbound request. `method` kept for diagnostics.
pub struct PendingRequest {
    pub method: String,
    pub tx: oneshot::Sender<Result<Value, JsonRpcError>>,
}

/// Shared connection object held behind `ConnectionHandle::inner` as
/// `Arc<dyn Any>`.
pub struct GeminiAcpConnection {
    pub gemini_path: PathBuf,
    pub child: Mutex<Option<Child>>,
    /// Writer side — wraps `ChildStdin` behind an mpsc so writes never block
    /// on the stdin mutex between sessions.
    writer_tx: mpsc::Sender<Vec<u8>>,
    writer_task: Mutex<Option<JoinHandle<()>>>,
    demux_task: Mutex<Option<JoinHandle<()>>>,
    stderr_task: Mutex<Option<JoinHandle<()>>>,
    next_request_id: AtomicU64,
    pending_requests: Mutex<HashMap<u64, PendingRequest>>,
    sessions: RwLock<HashMap<String, Arc<SessionState>>>,
    authenticated: AtomicBool,
    last_frame_seen: RwLock<Instant>,
    closed: AtomicBool,
    /// Agent capabilities reported in the `initialize` response. Kept for
    /// adapter-level feature detection (e.g. `loadSession: true` before
    /// issuing `session/load`).
    agent_capabilities: RwLock<Value>,
    auth_methods: RwLock<Vec<AuthMethod>>,
    /// Union of `models.availableModels[*].modelId` seen from every
    /// `session/new` response on this connection. Used to gate
    /// `session/set_model` so a bogus id (e.g. a profile_id from another
    /// vendor) never reaches Gemini's backend — which would otherwise
    /// return `[500] Requested entity was not found.` on the next
    /// `session/prompt`.
    known_models: RwLock<std::collections::HashSet<String>>,
}

/// Subset of the `authMethods` array entries we care about.
#[derive(Debug, Clone)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
}

impl GeminiAcpConnection {
    /// Spawn `gemini --acp`, wire the writer / demuxer tasks, and send
    /// `initialize`. Returns the connection with the handshake result applied.
    pub async fn open(gemini_path: PathBuf) -> Result<Arc<Self>, AgentExecutorError> {
        let mut command = Command::new(&gemini_path);
        command
            .arg("--acp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(AgentExecutorError::from)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentExecutorError::Io("gemini stdin unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentExecutorError::Io("gemini stdout unavailable".to_string()))?;
        let stderr = child.stderr.take();

        let (writer_tx, writer_rx) = mpsc::channel::<Vec<u8>>(WRITER_QUEUE_CAPACITY);

        let connection = Arc::new(Self {
            gemini_path,
            child: Mutex::new(Some(child)),
            writer_tx,
            writer_task: Mutex::new(None),
            demux_task: Mutex::new(None),
            stderr_task: Mutex::new(None),
            next_request_id: AtomicU64::new(1),
            pending_requests: Mutex::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            authenticated: AtomicBool::new(false),
            last_frame_seen: RwLock::new(Instant::now()),
            closed: AtomicBool::new(false),
            agent_capabilities: RwLock::new(Value::Null),
            auth_methods: RwLock::new(Vec::new()),
            known_models: RwLock::new(std::collections::HashSet::new()),
        });

        // Spawn writer task.
        let writer_task = tokio::spawn(writer_loop(stdin, writer_rx));
        *connection.writer_task.lock().await = Some(writer_task);

        // Spawn demuxer task.
        let demux_conn = Arc::clone(&connection);
        let demux_task = tokio::spawn(async move { demuxer_loop(demux_conn, stdout).await });
        *connection.demux_task.lock().await = Some(demux_task);

        // Spawn stderr drain task — we just pipe it into log::debug! so
        // gemini's noisy startup banner doesn't pollute the host's stdout.
        if let Some(stderr) = stderr {
            let task = tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut buf = String::new();
                loop {
                    buf.clear();
                    match reader.read_line(&mut buf).await {
                        Ok(0) => break,
                        Ok(_) => log::debug!("gemini[stderr] {}", buf.trim_end()),
                        Err(_) => break,
                    }
                }
            });
            *connection.stderr_task.lock().await = Some(task);
        }

        Ok(connection)
    }

    /// Allocate the next JSON-RPC request id.
    pub fn next_id(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Register a pending request and queue the envelope for stdin.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, AgentExecutorError> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(AgentExecutorError::Protocol(
                "gemini connection already closed".to_string(),
            ));
        }
        let id = self.next_id();
        let (tx, rx) = oneshot::channel();
        self.pending_requests.lock().await.insert(
            id,
            PendingRequest {
                method: method.to_string(),
                tx,
            },
        );
        let frame = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_frame(&frame).await?;
        match rx.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(AgentExecutorError::Vendor {
                vendor: "gemini",
                message: format!("[{}] {}", err.code, err.message),
            }),
            Err(_) => Err(AgentExecutorError::Protocol(
                "gemini connection dropped the pending request".to_string(),
            )),
        }
    }

    /// Send a notification (no `id`, no response awaited).
    pub async fn notify(&self, method: &str, params: Value) -> Result<(), AgentExecutorError> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(AgentExecutorError::Protocol(
                "gemini connection already closed".to_string(),
            ));
        }
        let frame = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_frame(&frame).await
    }

    /// Emit an id-matched response to a server-initiated request.
    pub async fn respond(&self, id: &JsonRpcId, result: Value) -> Result<(), AgentExecutorError> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(AgentExecutorError::Protocol(
                "gemini connection already closed".to_string(),
            ));
        }
        let frame = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        });
        self.write_frame(&frame).await
    }

    /// Send a raw pre-built JSON-RPC frame to stdin.
    async fn write_frame(&self, frame: &Value) -> Result<(), AgentExecutorError> {
        let mut line = serde_json::to_vec(frame)
            .map_err(|e| AgentExecutorError::Protocol(format!("serialize frame: {e}")))?;
        line.push(b'\n');
        self.writer_tx
            .send(line)
            .await
            .map_err(|_| AgentExecutorError::Protocol("gemini writer channel closed".to_string()))
    }

    pub async fn register_session(
        &self,
        session_id: String,
        current_model: Option<String>,
    ) -> Arc<SessionState> {
        let state = Arc::new(SessionState::new(current_model));
        self.sessions
            .write()
            .await
            .insert(session_id, Arc::clone(&state));
        state
    }

    pub async fn get_session(&self, session_id: &str) -> Option<Arc<SessionState>> {
        self.sessions.read().await.get(session_id).cloned()
    }

    pub async fn remove_session(&self, session_id: &str) -> Option<Arc<SessionState>> {
        self.sessions.write().await.remove(session_id)
    }

    pub fn mark_authenticated(&self) {
        self.authenticated.store(true, Ordering::Relaxed);
    }

    pub fn is_authenticated(&self) -> bool {
        self.authenticated.load(Ordering::Relaxed)
    }

    pub fn mark_closed(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    pub async fn set_agent_capabilities(&self, value: Value) {
        *self.agent_capabilities.write().await = value;
    }

    pub async fn agent_capabilities(&self) -> Value {
        self.agent_capabilities.read().await.clone()
    }

    pub async fn set_auth_methods(&self, methods: Vec<AuthMethod>) {
        *self.auth_methods.write().await = methods;
    }

    pub async fn auth_methods(&self) -> Vec<AuthMethod> {
        self.auth_methods.read().await.clone()
    }

    /// Merge a `models.availableModels` array (as returned by `session/new` /
    /// `session/load`) into the connection's known-models cache.
    pub async fn ingest_available_models(&self, available: &Value) {
        let Some(arr) = available.as_array() else {
            return;
        };
        let mut guard = self.known_models.write().await;
        for entry in arr {
            if let Some(model_id) = entry.get("modelId").and_then(Value::as_str) {
                guard.insert(model_id.to_string());
            }
        }
    }

    /// `true` when `model_id` has been reported by at least one
    /// `session/new` / `session/load` response on this connection. When the
    /// cache is empty (e.g. never observed a response yet) this returns
    /// `false` — callers should fall through to a permissive default in that
    /// case so fresh connections don't block legitimate `set_model` calls.
    pub async fn is_known_model(&self, model_id: &str) -> bool {
        self.known_models.read().await.contains(model_id)
    }

    /// Snapshot the known-models set; used by `list_vendor_models` to
    /// surface gemini's model list to the host UI.
    pub async fn known_models_snapshot(&self) -> Vec<String> {
        let mut out: Vec<String> = self.known_models.read().await.iter().cloned().collect();
        out.sort();
        out
    }

    pub async fn touch(&self) {
        *self.last_frame_seen.write().await = Instant::now();
    }

    pub async fn last_frame_age(&self) -> Duration {
        self.last_frame_seen.read().await.elapsed()
    }

    /// Drain every outstanding pending request / session broadcast with
    /// `ConnectionClosed`. Called on subprocess death or explicit close.
    pub async fn drain_and_close(&self, reason: &str) {
        if self.closed.swap(true, Ordering::Relaxed) {
            return;
        }

        // Resolve all pendings with an error.
        let mut pendings = self.pending_requests.lock().await;
        for (_, pending) in pendings.drain() {
            let _ = pending.tx.send(Err(JsonRpcError {
                code: -32603,
                message: format!("connection closed: {reason}"),
                data: None,
            }));
        }
        drop(pendings);

        // Mark every session dead and close broadcasters by dropping the sender.
        let mut sessions = self.sessions.write().await;
        for (_, state) in sessions.drain() {
            state.alive.store(false, Ordering::Relaxed);
            // broadcast::Sender::send returns SendError if no receivers,
            // which is fine.
            let _ = state.events_tx.send(ExecutorEvent::Error {
                message: format!("gemini connection closed: {reason}"),
                recoverable: false,
            });
        }
    }

    /// Attempt a graceful shutdown — close stdin to let gemini exit by itself,
    /// then kill if it lingers.
    pub async fn shutdown(self: Arc<Self>) {
        self.drain_and_close("shutdown requested").await;

        // Close the writer channel so the writer task drains and exits.
        // We drop the Sender by replacing through Mutex — but writer_tx
        // is owned by the Arc, so we force-close by aborting the writer task.
        if let Some(task) = self.writer_task.lock().await.take() {
            task.abort();
        }
        if let Some(task) = self.demux_task.lock().await.take() {
            task.abort();
        }
        if let Some(task) = self.stderr_task.lock().await.take() {
            task.abort();
        }

        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

/// Writer task — shovels outbound frames from the mpsc into stdin.
async fn writer_loop(mut stdin: ChildStdin, mut rx: mpsc::Receiver<Vec<u8>>) {
    while let Some(bytes) = rx.recv().await {
        if let Err(err) = stdin.write_all(&bytes).await {
            log::warn!("gemini writer: {err}");
            break;
        }
        if let Err(err) = stdin.flush().await {
            log::warn!("gemini writer flush: {err}");
            break;
        }
    }
    // Close stdin cleanly.
    let _ = stdin.shutdown().await;
}

/// Demuxer task — reads stdout line-by-line and routes frames.
async fn demuxer_loop(conn: Arc<GeminiAcpConnection>, stdout: ChildStdout) {
    let mut reader = BufReader::new(stdout);
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = match reader.read_line(&mut buf).await {
            Ok(n) => n,
            Err(err) => {
                log::warn!("gemini stdout read error: {err}");
                break;
            }
        };
        if n == 0 {
            log::debug!("gemini stdout EOF");
            break;
        }

        conn.touch().await;

        let frame = match JsonRpcFrame::parse_line(&buf) {
            Some(Ok(frame)) => frame,
            Some(Err(err)) => {
                log::debug!("gemini non-JSON stdout line: {} ({err})", buf.trim());
                continue;
            }
            None => continue,
        };

        match frame.classify() {
            FrameKind::Response { id, result } => {
                let id_u64 = match id.as_u64() {
                    Some(n) => n,
                    None => {
                        log::warn!("gemini response with non-numeric id {id:?}");
                        continue;
                    }
                };
                let pending = conn.pending_requests.lock().await.remove(&id_u64);
                if let Some(pending) = pending {
                    let _ = pending.tx.send(result);
                } else {
                    log::debug!("gemini response for unknown id {id_u64}");
                }
            }
            FrameKind::IncomingRequest { id, method, params } => {
                route_incoming_request(Arc::clone(&conn), id, method, params).await;
            }
            FrameKind::Notification { method, params } => {
                route_notification(&conn, method, params).await;
            }
            FrameKind::Invalid => {
                log::warn!("gemini invalid JSON-RPC frame: {}", buf.trim());
            }
        }
    }

    conn.drain_and_close("stdout EOF").await;
}

async fn route_incoming_request(
    conn: Arc<GeminiAcpConnection>,
    id: JsonRpcId,
    method: String,
    params: Value,
) {
    // We currently care about session/request_permission — future ACP FS /
    // terminal methods land on the same code path, but our clientCapabilities
    // don't advertise those so Gemini shouldn't send them.
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let session = match session_id.as_deref() {
        Some(sid) => conn.get_session(sid).await,
        None => None,
    };

    match method.as_str() {
        "session/request_permission" => {
            let Some(session) = session else {
                log::warn!("gemini session/request_permission for unknown session {session_id:?}");
                // Reply with cancelled outcome so gemini doesn't hang.
                let _ = conn
                    .respond(&id, json!({"outcome":{"outcome":"cancelled"}}))
                    .await;
                return;
            };
            // Correlation key = inbound JSON-RPC id, stringified. Exposed to
            // callers via the ExecutorEvent::PermissionRequest.request_id
            // field so `respond_to_permission` can look it up.
            let corr = match &id {
                JsonRpcId::Number(n) => n.to_string(),
                JsonRpcId::String(s) => s.clone(),
            };
            let tool_call = params.get("toolCall").cloned().unwrap_or(Value::Null);
            let tool_name = gemini_tool_name(
                tool_call.get("title").and_then(Value::as_str),
                tool_call.get("kind").and_then(Value::as_str),
            );

            // Stash an oneshot so respond_to_permission can fulfil it.
            let (reply_tx, reply_rx) = oneshot::channel::<Value>();
            session
                .pending_inbound
                .lock()
                .await
                .insert(corr.clone(), reply_tx);
            session
                .pending_permission_count
                .fetch_add(1, Ordering::SeqCst);

            // Bundle the full vendor payload for the UI. Frontend branches
            // on `_vendor="gemini"` and renders one button per entry in
            // `_vendor_options[]` (shape: `{optionId, name, kind}`).
            // Fallback UI still reads the flat toolCall fields.
            let vendor_options = params.get("options").cloned().unwrap_or(Value::Null);
            let mut tool_input = match tool_call.clone() {
                Value::Object(m) => Value::Object(m),
                other => {
                    let mut m = serde_json::Map::new();
                    m.insert("_raw_tool_call".to_string(), other);
                    Value::Object(m)
                }
            };
            if let Value::Object(ref mut m) = tool_input {
                m.insert("_vendor".to_string(), Value::String("gemini".to_string()));
                m.insert("_vendor_options".to_string(), vendor_options);
            }

            let _ = session.events_tx.send(ExecutorEvent::PermissionRequest {
                request_id: corr.clone(),
                tool_name,
                tool_input,
            });

            // Spawn a watcher that, once the oneshot resolves, forwards the
            // selected outcome back to gemini as a JSON-RPC response.
            let reply_conn = Arc::clone(&conn);
            let reply_id = id.clone();
            tokio::spawn(async move {
                match reply_rx.await {
                    Ok(outcome) => {
                        if let Err(err) = reply_conn.respond(&reply_id, outcome).await {
                            log::warn!("gemini permission reply failed for corr={corr}: {err}");
                        }
                    }
                    Err(_) => {
                        let _ = session.pending_permission_count.fetch_update(
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                            |count| Some(count.saturating_sub(1)),
                        );
                        // Fallback — respond with cancelled so gemini proceeds.
                        let _ = reply_conn
                            .respond(&reply_id, json!({"outcome":{"outcome":"cancelled"}}))
                            .await;
                    }
                }
            });
        }
        other => {
            log::debug!("gemini unhandled server request {other} params={params}");
            // Reply with a "method not found" error so gemini doesn't hang.
            let err = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not supported: {other}") }
            });
            let _ = conn.write_frame(&err).await;
        }
    }
}

async fn route_notification(conn: &GeminiAcpConnection, method: String, params: Value) {
    if method != "session/update" {
        log::debug!("gemini unhandled notification {method} params={params}");
        return;
    }
    let session_id = match params.get("sessionId").and_then(Value::as_str) {
        Some(sid) => sid,
        None => {
            log::warn!("gemini session/update without sessionId");
            return;
        }
    };
    let session = match conn.get_session(session_id).await {
        Some(s) => s,
        None => {
            log::debug!("gemini session/update for unknown session {session_id}");
            return;
        }
    };

    let update = match params.get("update") {
        Some(v) => v.clone(),
        None => return,
    };

    let parsed: Result<crate::stream::SessionUpdate, _> = serde_json::from_value(update.clone());
    let event = match parsed {
        Ok(crate::stream::SessionUpdate::AgentMessageChunk { content }) => {
            content.text().map(|t| ExecutorEvent::StreamDelta {
                kind: multi_agent_runtime_core::DeltaKind::Text,
                content: t.to_string(),
            })
        }
        Ok(crate::stream::SessionUpdate::AgentThoughtChunk { content }) => {
            content.text().map(|t| ExecutorEvent::StreamDelta {
                kind: multi_agent_runtime_core::DeltaKind::Thinking,
                content: t.to_string(),
            })
        }
        Ok(crate::stream::SessionUpdate::UserMessageChunk { .. }) => None, // ignore
        Ok(crate::stream::SessionUpdate::ToolCall {
            tool_call_id,
            title,
            kind,
            content,
            extra,
            ..
        }) => Some(ExecutorEvent::ToolCallStart {
            tool_use_id: tool_call_id,
            name: gemini_tool_name(title.as_deref(), kind.as_deref()),
            input: content.unwrap_or_else(|| Value::Object(extra)),
            partial: false,
        }),
        Ok(crate::stream::SessionUpdate::ToolCallUpdate {
            tool_call_id,
            status,
            content,
            ..
        }) => match status.as_deref() {
            Some("completed") => Some(ExecutorEvent::ToolResult {
                tool_use_id: tool_call_id,
                output: Ok(content.as_ref().map(|v| v.to_string()).unwrap_or_default()),
            }),
            Some("failed") => Some(ExecutorEvent::ToolResult {
                tool_use_id: tool_call_id,
                output: Err(content
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "tool failed".to_string())),
            }),
            _ => None,
        },
        Ok(_) => Some(ExecutorEvent::NativeEvent {
            provider: std::borrow::Cow::Borrowed("gemini"),
            payload: update,
        }),
        Err(err) => {
            log::debug!("gemini session/update parse error: {err}; raw={update}");
            Some(ExecutorEvent::NativeEvent {
                provider: std::borrow::Cow::Borrowed("gemini"),
                payload: update,
            })
        }
    };

    if let Some(event) = event {
        let _ = session.events_tx.send(event);
    }
}

fn gemini_tool_name(title: Option<&str>, kind: Option<&str>) -> String {
    [title, kind]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .next()
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::gemini_tool_name;

    #[test]
    fn tool_name_falls_back_to_kind_for_update_plan() {
        assert_eq!(gemini_tool_name(None, Some("update_plan")), "update_plan");
        assert_eq!(
            gemini_tool_name(Some("update_plan"), Some("ignored")),
            "update_plan"
        );
        assert_eq!(
            gemini_tool_name(Some("  "), Some("update_plan")),
            "update_plan"
        );
    }
}
