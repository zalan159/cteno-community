//! Shared, multi-session cteno-agent subprocess connection.
//!
//! Backs the Phase 1 connection-reuse trait seam (`open_connection` /
//! `close_connection` / `check_connection` / `start_session_on`). Every
//! [`CtenoConnection`] owns exactly one `cteno-agent` child process and can
//! host multiple concurrent sessions multiplexed over the shared stdio
//! channel.
//!
//! # Design
//!
//! The cteno-agent stdio protocol already routes every inbound / outbound
//! frame by `session_id`, so the connection is a thin shim that:
//!
//! 1. Owns the `Child`, `ChildStdin`, `ChildStdout`, `ChildStderr` handles.
//! 2. Runs a **writer task** that owns `ChildStdin`. Producers push frames
//!    through an `mpsc::Sender<WriterCmd>`; this eliminates head-of-line
//!    blocking between sessions even when the child's stdin pipe is slow to
//!    drain. The channel's capacity bounds the queue.
//! 3. Runs a **demultiplexer task** that reads lines off `ChildStdout`,
//!    parses them as [`protocol::Outbound`] frames, looks up
//!    `session_id → mpsc::Sender<Outbound>` in the connection's registry,
//!    and forwards. Unknown session_id is logged and dropped (never a panic).
//! 4. Exposes `register_session` / `unregister_session` so the adapter can
//!    attach and detach sessions, each receiving an [`mpsc::Receiver<Outbound>`]
//!    that carries only its own frames.
//!
//! # Invariants
//!
//! - One session's failure does not affect others. When a session's event
//!   receiver is dropped (channel closed) the demuxer logs and drops any
//!   subsequent frames tagged with that session_id.
//! - Subprocess death is surfaced uniformly: the demuxer's `read_line` returns
//!   EOF, the connection flips to `Dead`, and all remaining sessions see
//!   their channels closed.
//! - The writer task accepts a `Shutdown` command that closes `ChildStdin`
//!   and exits cleanly, used by `close_connection`.
//!
//! # What this file does NOT do
//!
//! - Own any session-level state (agent_config, auth snapshot, turn loops).
//!   That stays in `agent_executor.rs` keyed by `session_id` on the
//!   connection.
//! - Handle permission closure / tool injection / per-session timers.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use multi_agent_runtime_core::executor::ConnectionHandleId;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::protocol::{Inbound, Outbound};

/// Capacity of the shared stdin writer channel. Producers (session worker
/// tasks) push frames into this channel and the writer task serialises them
/// to the child. If the child stops reading, producers backpressure uniformly
/// via `Sender::send` awaiting channel space.
const WRITER_QUEUE_CAPACITY: usize = 256;

/// Capacity of each per-session Outbound event channel.
const PER_SESSION_QUEUE_CAPACITY: usize = 64;

/// Number of recent stderr lines kept for error reporting.
const STDERR_TAIL_LINES: usize = 16;

/// Outcome of the last write attempt to the writer task's channel.
pub type WriteResult = Result<(), WriteError>;

/// Errors the writer task or producers may surface.
#[derive(Debug)]
pub enum WriteError {
    /// The writer task's channel is closed — the connection is dead.
    Closed,
    /// Backpressure hit the timeout before the channel accepted the frame.
    Timeout,
    /// Serialisation failure (should never happen for well-formed Inbound).
    Serialisation(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Closed => write!(f, "cteno-agent connection closed"),
            WriteError::Timeout => write!(f, "cteno-agent writer channel backpressure timeout"),
            WriteError::Serialisation(e) => write!(f, "serialisation error: {e}"),
        }
    }
}

impl std::error::Error for WriteError {}

/// Commands the writer task accepts on its input channel.
enum WriterCmd {
    Frame(String),
    Shutdown,
}

/// Handle used by the adapter (and by session workers) to push frames into
/// the shared cteno-agent subprocess. Cheap to clone.
#[derive(Clone)]
pub struct ConnectionWriter {
    tx: mpsc::Sender<WriterCmd>,
}

impl ConnectionWriter {
    /// Send an [`Inbound`] frame. Bounded-timeout; returns `WriteError::Closed`
    /// if the writer task has already exited.
    pub async fn send(&self, frame: &Inbound) -> WriteResult {
        let mut line =
            serde_json::to_string(frame).map_err(|e| WriteError::Serialisation(e.to_string()))?;
        line.push('\n');
        // 5s is a generous upper bound — the writer task flushes after every
        // frame, so in healthy state the channel drains immediately.
        match timeout(Duration::from_secs(5), self.tx.send(WriterCmd::Frame(line))).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(WriteError::Closed),
            Err(_) => Err(WriteError::Timeout),
        }
    }

    /// Tell the writer task to close stdin and exit. Used by
    /// `close_connection` for graceful shutdown.
    pub async fn shutdown(&self) {
        let _ = self.tx.send(WriterCmd::Shutdown).await;
    }
}

/// Per-session outbound event receiver vended by [`CtenoConnection::register_session`].
pub type SessionEventRx = mpsc::Receiver<Outbound>;
pub type SessionEventTx = mpsc::Sender<Outbound>;

/// Shared state the demultiplexer uses to route outbound frames.
struct SessionRouter {
    /// session_id → per-session event sender.
    entries: Mutex<HashMap<String, SessionEventTx>>,
}

impl SessionRouter {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    async fn register(&self, session_id: String, tx: SessionEventTx) {
        let mut guard = self.entries.lock().await;
        guard.insert(session_id, tx);
    }

    async fn unregister(&self, session_id: &str) -> Option<SessionEventTx> {
        let mut guard = self.entries.lock().await;
        guard.remove(session_id)
    }

    async fn route(&self, session_id: &str, frame: Outbound) {
        let sender = {
            let guard = self.entries.lock().await;
            guard.get(session_id).cloned()
        };
        match sender {
            Some(tx) => {
                // `try_send` first so a slow consumer never blocks the demuxer
                // — slow consumers only starve themselves, never the
                // connection. If full, spawn a bounded-timeout send so we
                // don't silently drop a frame.
                if let Err(err) = tx.try_send(frame) {
                    match err {
                        mpsc::error::TrySendError::Full(frame) => {
                            log::warn!("cteno session {} backpressured; awaiting slot", session_id);
                            if let Err(_e) = timeout(Duration::from_secs(2), tx.send(frame)).await {
                                log::warn!(
                                    "cteno session {} backpressure timeout; frame dropped",
                                    session_id
                                );
                            }
                        }
                        mpsc::error::TrySendError::Closed(_) => {
                            log::debug!(
                                "cteno session {} channel closed; dropping frame",
                                session_id
                            );
                        }
                    }
                }
            }
            None => {
                log::warn!(
                    "cteno-agent emitted frame for unknown session_id={} — dropping",
                    session_id
                );
            }
        }
    }

    /// Drain every registered sender, closing all per-session channels.
    /// Called when the subprocess dies or the connection closes.
    async fn drain_all(&self) -> Vec<(String, SessionEventTx)> {
        let mut guard = self.entries.lock().await;
        guard.drain().collect()
    }
}

/// Health state observed on the connection. Mirrors
/// `multi_agent_runtime_core::ConnectionHealth` so the trait impl can build a
/// core-facing value by mapping this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionLiveness {
    Alive,
    Dead { reason: String },
}

/// The owned, shared cteno-agent subprocess backing a
/// `ConnectionHandle::inner`.
///
/// Access is via `Arc<CtenoConnection>`. Internal state is behind individual
/// `Mutex`es so independent surfaces (session registration, liveness probe,
/// frame writing) do not serialise on the same lock.
pub struct CtenoConnection {
    pub id: ConnectionHandleId,
    pub writer: ConnectionWriter,
    router: Arc<SessionRouter>,
    /// Live-or-dead flag flipped by the demuxer on EOF / error.
    liveness: Mutex<ConnectionLiveness>,
    /// Cached pid for supervisor unregistration.
    pub pid: Option<i32>,
    /// Rolling buffer of recent stderr lines.
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    /// The child handle, kept alive for `try_wait` / `kill` in
    /// `check_connection` and `close`.
    child: Mutex<Option<Child>>,
    /// Handles for the spawned writer + demux + stderr tasks so we can abort
    /// them on close.
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl CtenoConnection {
    /// Build a new connection around an already-spawned child process.
    ///
    /// Takes ownership of the stdio handles so the caller cannot race with
    /// the connection's internal tasks. Does NOT send any protocol frames —
    /// sessions are attached via `register_session` after this returns.
    pub fn start(mut child: Child) -> Result<Arc<Self>, String> {
        let pid = child.id().map(|p| p as i32);
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "cteno-agent stdin unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "cteno-agent stdout unavailable".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "cteno-agent stderr unavailable".to_string())?;

        let router = Arc::new(SessionRouter::new());
        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));

        let (writer_tx, writer_rx) = mpsc::channel::<WriterCmd>(WRITER_QUEUE_CAPACITY);
        let writer_handle = tokio::spawn(writer_task(stdin, writer_rx));

        let router_for_demux = router.clone();
        let demux_handle = tokio::spawn(demux_task(stdout, router_for_demux));

        let stderr_tail_for_task = stderr_tail.clone();
        let stderr_handle = tokio::spawn(stderr_task(stderr, stderr_tail_for_task));

        let conn = Arc::new(Self {
            id: ConnectionHandleId::new(),
            writer: ConnectionWriter { tx: writer_tx },
            router,
            liveness: Mutex::new(ConnectionLiveness::Alive),
            pid,
            stderr_tail,
            child: Mutex::new(Some(child)),
            tasks: Mutex::new(vec![writer_handle, demux_handle, stderr_handle]),
        });

        // Spawn a liveness watcher: when the demuxer exits (subprocess stdout
        // EOF) or when a periodic try_wait detects exit, flip liveness and
        // close all per-session channels.
        let conn_weak = Arc::downgrade(&conn);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            loop {
                interval.tick().await;
                let Some(conn) = conn_weak.upgrade() else {
                    return;
                };
                // Already marked dead by some other path; stop watching.
                {
                    let guard = conn.liveness.lock().await;
                    if !matches!(*guard, ConnectionLiveness::Alive) {
                        return;
                    }
                }
                let status = {
                    let mut guard = conn.child.lock().await;
                    match guard.as_mut() {
                        Some(c) => c.try_wait().ok().flatten(),
                        None => None,
                    }
                };
                if let Some(status) = status {
                    let reason = format!(
                        "cteno-agent subprocess exited with status {:?}",
                        status.code()
                    );
                    conn.mark_dead(&reason).await;
                    return;
                }
            }
        });

        Ok(conn)
    }

    /// Attach a new session: register its event receiver in the router and
    /// return the receiver end. Caller keeps the receiver for the lifetime of
    /// the session; dropping it detaches automatically via
    /// `unregister_session`.
    pub async fn register_session(&self, session_id: &str) -> SessionEventRx {
        let (tx, rx) = mpsc::channel::<Outbound>(PER_SESSION_QUEUE_CAPACITY);
        self.router.register(session_id.to_string(), tx).await;
        rx
    }

    /// Remove a session's entry from the router. Any in-flight frames queued
    /// on its channel remain until the receiver is dropped.
    pub async fn unregister_session(&self, session_id: &str) -> Option<SessionEventTx> {
        self.router.unregister(session_id).await
    }

    /// Returns the current liveness snapshot.
    pub async fn liveness(&self) -> ConnectionLiveness {
        self.liveness.lock().await.clone()
    }

    /// Check if the child process is still alive. Updates `liveness` and
    /// returns the up-to-date state.
    pub async fn check(&self) -> ConnectionLiveness {
        // Fast path: already flipped dead.
        {
            let guard = self.liveness.lock().await;
            if !matches!(*guard, ConnectionLiveness::Alive) {
                return guard.clone();
            }
        }

        let maybe_status = {
            let mut guard = self.child.lock().await;
            match guard.as_mut() {
                Some(c) => c.try_wait().ok().flatten(),
                None => {
                    return ConnectionLiveness::Dead {
                        reason: "child handle missing".to_string(),
                    };
                }
            }
        };
        if let Some(status) = maybe_status {
            let reason = format!(
                "cteno-agent subprocess exited with status {:?}",
                status.code()
            );
            self.mark_dead(&reason).await;
            return ConnectionLiveness::Dead { reason };
        }

        // Also probe the writer channel: if the writer task has exited, the
        // connection is effectively dead even if try_wait hasn't noticed yet.
        if self.writer.tx.is_closed() {
            let reason = "cteno-agent writer channel closed".to_string();
            self.mark_dead(&reason).await;
            return ConnectionLiveness::Dead { reason };
        }

        ConnectionLiveness::Alive
    }

    async fn mark_dead(&self, reason: &str) {
        {
            let mut guard = self.liveness.lock().await;
            if !matches!(*guard, ConnectionLiveness::Alive) {
                return;
            }
            *guard = ConnectionLiveness::Dead {
                reason: reason.to_string(),
            };
        }
        // Close all per-session channels so waiting receivers wake up.
        let drained = self.router.drain_all().await;
        drop(drained); // Dropping each sender triggers receiver close.
    }

    /// Snapshot the most recent stderr lines for diagnostics.
    pub async fn stderr_tail(&self) -> String {
        let tail = self.stderr_tail.lock().await;
        tail.iter().cloned().collect::<Vec<_>>().join(" | ")
    }

    /// Gracefully terminate the subprocess: issue writer Shutdown (closes
    /// stdin → cteno-agent exits on EOF), wait briefly, kill if still alive,
    /// abort all background tasks.
    pub async fn close(&self) {
        self.writer.shutdown().await;

        // Give the child up to 2s to exit on its own after stdin close.
        let mut child_opt = {
            let mut guard = self.child.lock().await;
            guard.take()
        };
        if let Some(ref mut child) = child_opt {
            let wait_res = timeout(Duration::from_secs(2), child.wait()).await;
            if wait_res.is_err() {
                // Timed out — force kill.
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        }

        self.mark_dead("connection closed by host").await;

        let tasks = {
            let mut guard = self.tasks.lock().await;
            std::mem::take(&mut *guard)
        };
        for h in tasks {
            h.abort();
        }
    }
}

/// Writer task: owns `ChildStdin` and drains the WriterCmd channel.
async fn writer_task(mut stdin: ChildStdin, mut rx: mpsc::Receiver<WriterCmd>) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            WriterCmd::Frame(line) => {
                if let Err(e) = stdin.write_all(line.as_bytes()).await {
                    log::warn!("cteno-agent stdin write failed: {e}; writer task exiting");
                    return;
                }
                if let Err(e) = stdin.flush().await {
                    log::warn!("cteno-agent stdin flush failed: {e}; writer task exiting");
                    return;
                }
            }
            WriterCmd::Shutdown => {
                // Drop stdin to signal EOF to the agent, then drain any
                // remaining queued frames (ignoring errors).
                let _ = stdin.shutdown().await;
                return;
            }
        }
    }
}

/// Demux task: reads JSON-lines from stdout and routes each by session_id.
async fn demux_task(stdout: ChildStdout, router: Arc<SessionRouter>) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                log::info!("cteno-agent stdout EOF; demux task exiting");
                return;
            }
            Err(e) => {
                log::warn!("cteno-agent stdout read error: {e}; demux task exiting");
                return;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Outbound>(trimmed) {
                    Ok(frame) => {
                        let session_id = outbound_session_id(&frame).to_string();
                        router.route(&session_id, frame).await;
                    }
                    Err(e) => {
                        log::warn!("cteno-agent outbound frame parse error: {e}; raw={trimmed}");
                    }
                }
            }
        }
    }
}

/// Stderr task: tails the subprocess's stderr into a bounded ring buffer.
async fn stderr_task(stderr: ChildStderr, tail: Arc<Mutex<VecDeque<String>>>) {
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => return,
            Err(_) => return,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let mut guard = tail.lock().await;
                if guard.len() >= STDERR_TAIL_LINES {
                    guard.pop_front();
                }
                guard.push_back(trimmed.to_string());
                log::debug!("[cteno-agent stderr] {}", trimmed);
            }
        }
    }
}

/// Extract the `session_id` string out of any outbound frame so the demuxer
/// can route. Safe because every outbound variant carries it.
fn outbound_session_id(frame: &Outbound) -> &str {
    match frame {
        Outbound::Ready { session_id } => session_id,
        Outbound::Delta { session_id, .. } => session_id,
        Outbound::ToolUse { session_id, .. } => session_id,
        Outbound::ToolResult { session_id, .. } => session_id,
        Outbound::PermissionRequest { session_id, .. } => session_id,
        Outbound::ToolExecutionRequest { session_id, .. } => session_id,
        Outbound::TurnComplete { session_id, .. } => session_id,
        Outbound::Error { session_id, .. } => session_id,
        Outbound::HostCallRequest { session_id, .. } => session_id,
    }
}

// Re-export internal protocol types through a helper module so tests and the
// adapter can use `crate::protocol::Inbound` / `Outbound` consistently.
pub use crate::protocol::Outbound as OutboundEvent;

/// Helper: receive the `Ready` frame for a session, returning Err on any
/// other matching-session Error / unexpected frame, or on channel close.
pub async fn wait_for_ready(
    rx: &mut SessionEventRx,
    expected_id: &str,
    wait: Duration,
) -> Result<(), String> {
    let deadline = tokio::time::sleep(wait);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => {
                return Err(format!("timed out after {:?} waiting for ready", wait));
            }
            next = rx.recv() => {
                match next {
                    Some(Outbound::Ready { session_id }) => {
                        if session_id != expected_id {
                            log::warn!(
                                "ready session_id mismatch: expected={expected_id} observed={session_id}"
                            );
                        }
                        return Ok(());
                    }
                    Some(Outbound::Error { message, .. }) => {
                        return Err(format!("cteno-agent reported error before ready: {message}"));
                    }
                    Some(other) => {
                        log::debug!("cteno pre-ready frame ignored: {:?}", other);
                        continue;
                    }
                    None => {
                        return Err("connection closed before ready".to_string());
                    }
                }
            }
        }
    }
}
