//! stdin / stdout transport for the line-delimited JSON protocol.

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex};

use crate::protocol::{Inbound, Outbound};

/// Handle used by the rest of the binary to emit Outbound events.
#[derive(Clone)]
pub struct OutboundWriter {
    inner: Arc<Mutex<tokio::io::Stdout>>,
}

impl OutboundWriter {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(tokio::io::stdout())),
        }
    }

    /// Serialize and write one outbound message, followed by a newline.
    pub async fn send(&self, msg: Outbound) {
        let line = match serde_json::to_string(&msg) {
            Ok(s) => s,
            Err(err) => {
                // Fall back to a minimal handcrafted error envelope. We
                // cannot surface this via the protocol if serialization
                // itself is broken, so log to stderr and bail.
                log::error!("failed to serialize outbound message: {err}");
                return;
            }
        };
        let mut guard = self.inner.lock().await;
        if let Err(err) = guard.write_all(line.as_bytes()).await {
            log::error!("stdout write failed: {err}");
            return;
        }
        if let Err(err) = guard.write_all(b"\n").await {
            log::error!("stdout write failed: {err}");
            return;
        }
        let _ = guard.flush().await;
    }
}

impl Default for OutboundWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// Spawn a task that reads stdin line-by-line, parses each line as an
/// `Inbound`, and forwards it to the given channel. Returns the receiver side.
///
/// When stdin reaches EOF the channel is dropped, which signals downstream
/// consumers to shut down gracefully.
pub fn spawn_stdin_reader(writer: OutboundWriter) -> mpsc::Receiver<Inbound> {
    let (tx, rx) = mpsc::channel::<Inbound>(32);
    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();
        loop {
            line.clear();
            let read = match reader.read_line(&mut line).await {
                Ok(n) => n,
                Err(err) => {
                    log::error!("stdin read error: {err}");
                    break;
                }
            };
            if read == 0 {
                log::info!("stdin EOF, shutting down reader");
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Inbound>(trimmed) {
                Ok(msg) => {
                    if tx.send(msg).await.is_err() {
                        log::warn!("session worker gone; stdin reader exiting");
                        break;
                    }
                }
                Err(err) => {
                    // Surface parse errors out to the host but keep going.
                    writer
                        .send(Outbound::Error {
                            session_id: String::new(),
                            message: format!("invalid inbound message: {err}"),
                        })
                        .await;
                }
            }
        }
    });
    rx
}
