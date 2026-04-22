//! One-shot loopback HTTP listener for OAuth callback capture (RFC 8252 §7.3).
//!
//! The desktop Tauri app previously used `cteno://auth/callback` as the OAuth
//! redirect URI and relied on the OS scheme handler. That handler is only
//! registered when the app is *installed* (via `tauri bundle`), so in
//! `cargo tauri dev` the browser returns "scheme has no registered handler"
//! and the callback is lost. Loopback redirect is the industry-standard
//! fallback for native OAuth clients:
//!
//!   1. Client binds 127.0.0.1:<ephemeral port> *before* opening the browser.
//!   2. redirect_uri = http://127.0.0.1:<port>/callback
//!   3. Browser hits the listener; listener stores the request path+query in
//!      a shared map keyed by a handle and serves a "you can close this tab"
//!      HTML page.
//!   4. JS calls `oauth_loopback_wait(handle)` which blocks (async) until the
//!      captured path is available, then returns it.
//!
//! Returning the captured URL through the command's own Promise (rather than a
//! Tauri event) is critical: webview reloads (Metro HMR, dev rebuild, window
//! recreation) blow away JS event listeners but they don't drop the pending
//! `invoke` Promise, which is resolved from Rust directly. Event-based capture
//! is racy in dev.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::oneshot;
use uuid::Uuid;

const LISTEN_TIMEOUT_SECS: u64 = 300;
const ACCEPT_POLL_MS: u64 = 250;
const READ_TIMEOUT_MS: u64 = 15_000;

const SUCCESS_HTML: &str = r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>Login successful</title>
<style>
body { font-family: -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif;
       padding: 48px; text-align: center; color: #111; background: #fafafa; }
.card { max-width: 420px; margin: 0 auto; padding: 32px;
        background: #fff; border: 1px solid #e5e5e5; border-radius: 12px; }
h1 { font-size: 20px; margin: 0 0 12px; }
p { color: #555; margin: 0; line-height: 1.5; }
</style></head>
<body><div class="card">
<h1>Login successful</h1>
<p>You can close this tab and return to Cteno.</p>
</div></body></html>"#;

type ResultSender = oneshot::Sender<Result<String, String>>;
type ResultReceiver = oneshot::Receiver<Result<String, String>>;

struct PendingCallback {
    sender: Option<ResultSender>,
    receiver: Option<ResultReceiver>,
}

fn pending_map() -> &'static Mutex<HashMap<String, PendingCallback>> {
    static MAP: OnceLock<Mutex<HashMap<String, PendingCallback>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopbackStartResponse {
    pub handle: String,
    pub port: u16,
    pub redirect_uri: String,
}

#[tauri::command]
pub fn oauth_loopback_start() -> Result<LoopbackStartResponse, String> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind 127.0.0.1:0 failed: {e}"))?;
    let addr: SocketAddr = listener
        .local_addr()
        .map_err(|e| format!("local_addr failed: {e}"))?;
    let port = addr.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let handle = Uuid::new_v4().to_string();

    let (tx, rx) = oneshot::channel();
    pending_map()
        .lock()
        .map_err(|e| format!("pending_map lock poisoned: {e}"))?
        .insert(
            handle.clone(),
            PendingCallback {
                sender: None,
                receiver: Some(rx),
            },
        );

    listener
        .set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking(true) failed: {e}"))?;

    let handle_for_thread = handle.clone();
    std::thread::Builder::new()
        .name(format!("oauth-loopback-{port}"))
        .spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(LISTEN_TIMEOUT_SECS);
            let result = loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        break match read_request_line(stream) {
                            Some(path) => Ok(path),
                            None => {
                                Err("loopback stream closed before sending request".to_string())
                            }
                        };
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            break Err("loopback listener timed out".to_string());
                        }
                        std::thread::sleep(Duration::from_millis(ACCEPT_POLL_MS));
                    }
                    Err(e) => {
                        break Err(format!("loopback accept failed: {e}"));
                    }
                }
            };

            // Deliver to whatever path the JS has taken. The receiver may
            // either be waiting in `oauth_loopback_wait` (result goes straight
            // through the oneshot) or not yet subscribed (we stash the
            // completed result under the handle for later pickup).
            let _ = tx.send(result);
            // Don't remove the map entry here; `oauth_loopback_wait` removes
            // it once it consumes the receiver.
            let _ = handle_for_thread; // silence unused warning when cfg'd out
        })
        .map_err(|e| format!("spawn loopback thread failed: {e}"))?;

    Ok(LoopbackStartResponse {
        handle,
        port,
        redirect_uri,
    })
}

#[tauri::command]
pub async fn oauth_loopback_wait(handle: String) -> Result<String, String> {
    let receiver = {
        let mut map = pending_map()
            .lock()
            .map_err(|e| format!("pending_map lock poisoned: {e}"))?;
        let entry = map
            .get_mut(&handle)
            .ok_or_else(|| format!("unknown loopback handle: {handle}"))?;
        entry
            .receiver
            .take()
            .ok_or_else(|| "loopback already consumed".to_string())?
    };

    let outcome = receiver
        .await
        .map_err(|_| "loopback sender dropped before delivering a result".to_string())?;

    // Clean up regardless of success/failure.
    let _ = pending_map().lock().map(|mut map| map.remove(&handle));

    outcome
}

fn read_request_line(mut stream: TcpStream) -> Option<String> {
    // The listener is non-blocking for polling, so accepted sockets inherit
    // that. Switch back to blocking for a short read/write burst.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(READ_TIMEOUT_MS)));
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).ok()?;
    if n == 0 {
        return None;
    }

    // The request line is `METHOD PATH HTTP/x.y\r\n…`; we only need PATH to
    // extract `code` and `state`. Anything fancier (headers, body) is ignored.
    let head = std::str::from_utf8(&buf[..n]).ok()?;
    let first_line = head.split("\r\n").next()?;
    let mut parts = first_line.split_whitespace();
    let _method = parts.next()?;
    let path = parts.next()?.to_string();

    let body = SUCCESS_HTML.as_bytes();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);

    Some(path)
}
