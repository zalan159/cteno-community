//! CDP WebSocket Connection
//!
//! Async CDP client using tokio-tungstenite. Supports command/response
//! with automatic ID matching and event subscriptions.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

/// CDP protocol error.
#[derive(Debug)]
pub struct CdpError {
    pub message: String,
    pub code: Option<i64>,
}

impl std::fmt::Display for CdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CDP error: {}", self.message)
    }
}

/// Async CDP WebSocket connection.
pub struct CdpConnection {
    msg_counter: AtomicU32,
    tx: mpsc::UnboundedSender<String>,
    pending: Arc<Mutex<HashMap<u32, oneshot::Sender<Result<Value, CdpError>>>>>,
    event_subs: Arc<Mutex<HashMap<String, Vec<mpsc::UnboundedSender<Value>>>>>,
    reader_handle: JoinHandle<()>,
    /// Whether the connection is still alive.
    alive: Arc<std::sync::atomic::AtomicBool>,
}

impl CdpConnection {
    /// Connect to Chrome CDP WebSocket on the given port.
    pub async fn connect(port: u16) -> Result<Self, String> {
        // Get WebSocket URL from /json/version
        let version_url = format!("http://127.0.0.1:{}/json/version", port);
        let resp: Value = reqwest::get(&version_url)
            .await
            .map_err(|e| format!("Failed to get CDP version: {}", e))?
            .json()
            .await
            .map_err(|e| format!("Failed to parse CDP version: {}", e))?;

        let ws_url = resp["webSocketDebuggerUrl"]
            .as_str()
            .ok_or("Missing webSocketDebuggerUrl in CDP version response")?;

        log::info!("[CDP] Connecting to {}", ws_url);

        let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| format!("Failed to connect CDP WebSocket: {}", e))?;

        let (mut ws_write, mut ws_read) = ws_stream.split();

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let pending: Arc<Mutex<HashMap<u32, oneshot::Sender<Result<Value, CdpError>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let event_subs: Arc<Mutex<HashMap<String, Vec<mpsc::UnboundedSender<Value>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));

        let pending_clone = pending.clone();
        let event_subs_clone = event_subs.clone();
        let alive_clone = alive.clone();

        // Background reader/writer task
        let reader_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Forward outgoing messages to WebSocket
                    msg = rx.recv() => {
                        match msg {
                            Some(text) => {
                                if let Err(e) = ws_write.send(Message::Text(text.into())).await {
                                    log::error!("[CDP] WebSocket send error: {}", e);
                                    break;
                                }
                            }
                            None => break, // channel closed
                        }
                    }
                    // Read incoming messages from WebSocket
                    msg = ws_read.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                let text_str: &str = &text;
                                if let Ok(v) = serde_json::from_str::<Value>(text_str) {
                                    // Response to a command (has "id" field)
                                    if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                                        let mut p = pending_clone.lock().await;
                                        if let Some(sender) = p.remove(&(id as u32)) {
                                            if let Some(error) = v.get("error") {
                                                let _ = sender.send(Err(CdpError {
                                                    message: error["message"].as_str().unwrap_or("Unknown CDP error").to_string(),
                                                    code: error["code"].as_i64(),
                                                }));
                                            } else {
                                                let _ = sender.send(Ok(v.get("result").cloned().unwrap_or(json!({}))));
                                            }
                                        }
                                    }
                                    // Event (has "method" but no "id")
                                    else if let Some(method) = v.get("method").and_then(|m| m.as_str()) {
                                        let params = v.get("params").cloned().unwrap_or(json!({}));
                                        let mut subs = event_subs_clone.lock().await;
                                        if let Some(listeners) = subs.get_mut(method) {
                                            listeners.retain(|tx| tx.send(params.clone()).is_ok());
                                        }
                                    }
                                }
                            }
                            Some(Ok(Message::Close(_))) | None => {
                                log::warn!("[CDP] WebSocket closed");
                                break;
                            }
                            Some(Err(e)) => {
                                log::error!("[CDP] WebSocket read error: {}", e);
                                break;
                            }
                            _ => {} // ping/pong/binary
                        }
                    }
                }
            }

            alive_clone.store(false, Ordering::SeqCst);

            // Notify all pending requests that connection died
            let mut p = pending_clone.lock().await;
            for (_, sender) in p.drain() {
                let _ = sender.send(Err(CdpError {
                    message: "CDP connection closed".to_string(),
                    code: None,
                }));
            }
        });

        log::info!("[CDP] Connected successfully");

        Ok(Self {
            msg_counter: AtomicU32::new(0),
            tx,
            pending,
            event_subs,
            reader_handle,
            alive,
        })
    }

    /// Send a CDP command and wait for its response (default 30s timeout).
    /// `session_id` is the CDP Target session ID (for target-scoped commands).
    pub async fn send(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value, CdpError> {
        self.send_with_timeout(method, params, session_id, 30).await
    }

    /// Send a CDP command with a custom timeout in seconds.
    pub async fn send_with_timeout(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, CdpError> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(CdpError {
                message: "CDP connection is closed".to_string(),
                code: None,
            });
        }

        let id = self.msg_counter.fetch_add(1, Ordering::SeqCst) + 1;

        let mut msg = json!({
            "id": id,
            "method": method,
            "params": params,
        });

        if let Some(sid) = session_id {
            msg["sessionId"] = json!(sid);
        }

        let (resp_tx, resp_rx) = oneshot::channel();

        {
            let mut p = self.pending.lock().await;
            p.insert(id, resp_tx);
        }

        self.tx.send(msg.to_string()).map_err(|_| CdpError {
            message: "Failed to send CDP command (channel closed)".to_string(),
            code: None,
        })?;

        match tokio::time::timeout(tokio::time::Duration::from_secs(timeout_secs), resp_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CdpError {
                message: format!("CDP response channel dropped for {}", method),
                code: None,
            }),
            Err(_) => {
                // Remove pending on timeout
                let mut p = self.pending.lock().await;
                p.remove(&id);
                Err(CdpError {
                    message: format!("CDP command timed out: {}", method),
                    code: None,
                })
            }
        }
    }

    /// Subscribe to a CDP event. Returns a receiver that yields event params.
    pub async fn subscribe(&self, event: &str) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut subs = self.event_subs.lock().await;
        subs.entry(event.to_string())
            .or_insert_with(Vec::new)
            .push(tx);
        rx
    }

    /// Check if the connection is still alive.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Get a clone of the raw WebSocket sender for fire-and-forget CDP commands.
    /// Used by background tasks (e.g. dialog handlers) that need to send commands
    /// without holding a reference to the full CdpConnection.
    pub fn raw_sender(
        &self,
    ) -> (
        mpsc::UnboundedSender<String>,
        Arc<std::sync::atomic::AtomicBool>,
    ) {
        (self.tx.clone(), Arc::clone(&self.alive))
    }

    /// Close the CDP connection.
    pub async fn close(&self) {
        self.alive.store(false, Ordering::SeqCst);
        // Dropping tx will cause the reader task to exit
    }
}

impl Drop for CdpConnection {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        self.reader_handle.abort();
    }
}
