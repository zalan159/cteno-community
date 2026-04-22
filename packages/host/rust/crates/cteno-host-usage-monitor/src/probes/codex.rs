//! Codex probe: persistent `codex app-server` subprocess, JSON-RPC.
//!
//! Boot sequence per poll:
//!   1. Ensure the subprocess is alive (respawn on exit / first call).
//!   2. Ensure it's been initialized (one `initialize` request per instance).
//!   3. Send `account/rateLimits/read`, wait for matching response id.
//!   4. Translate the `rateLimitsByLimitId` map into our `UsageWindow` shape.
//!
//! `windowDurationMins == 300`   → "fiveHour"
//! `windowDurationMins == 10080` → "weekly"
//! Any other duration is stored under its raw value as a fallback key so the
//! UI can still surface the data even if Codex adds a new window type.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::timeout;

use crate::probe::VendorUsageProbe;
use crate::store::{UsageCredits, UsageWindow, VendorId, VendorUsage};

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub struct CodexProbe {
    inner: Arc<Mutex<ProbeState>>,
}

struct ProbeState {
    proc: Option<ConnectedProcess>,
}

struct ConnectedProcess {
    // Held only for its `kill_on_drop` side-effect: when the probe decides
    // the subprocess is gone and drops `ProbeState.proc`, the child is SIGKILL'd.
    #[allow(dead_code)]
    child: Child,
    tx: mpsc::Sender<Request>,
}

struct Request {
    method: String,
    params: Value,
    respond: oneshot::Sender<Result<Value, String>>,
}

impl CodexProbe {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProbeState { proc: None })),
        }
    }

    async fn ensure_connection(&self) -> Result<mpsc::Sender<Request>, String> {
        let mut state = self.inner.lock().await;

        if let Some(existing) = &state.proc {
            if !existing.tx.is_closed() {
                return Ok(existing.tx.clone());
            }
            // Worker died — drop the handle and reconnect below.
            state.proc = None;
        }

        let proc = spawn_codex_app_server().await?;
        let tx = proc.tx.clone();
        state.proc = Some(proc);
        Ok(tx)
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let tx = self.ensure_connection().await?;
        let (resp_tx, resp_rx) = oneshot::channel();
        tx.send(Request {
            method: method.to_string(),
            params,
            respond: resp_tx,
        })
        .await
        .map_err(|_| "codex probe worker closed".to_string())?;
        match timeout(REQUEST_TIMEOUT, resp_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("codex probe worker dropped response channel".to_string()),
            Err(_) => Err(format!("codex {} timed out", method)),
        }
    }
}

impl Default for CodexProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VendorUsageProbe for CodexProbe {
    fn vendor(&self) -> VendorId {
        VendorId::Codex
    }

    async fn poll(&self) -> Result<VendorUsage, String> {
        let response = self
            .call("account/rateLimits/read", Value::Null)
            .await
            .map_err(|e| e)?;
        Ok(translate_snapshot(&response))
    }
}

async fn spawn_codex_app_server() -> Result<ConnectedProcess, String> {
    let mut child = Command::new("codex")
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("failed to spawn `codex app-server`: {}", e))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "codex app-server stdin closed".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "codex app-server stdout closed".to_string())?;

    // Drain stderr in the background so the child never blocks on a full pipe.
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                log::debug!("[codex-app-server stderr] {}", line);
            }
        });
    }

    let (tx, rx) = mpsc::channel::<Request>(16);
    tokio::spawn(run_worker(stdin, stdout, rx));

    // Initialize the server once. If this fails we kill the child before
    // returning so the next call tries a fresh spawn.
    let (init_tx, init_rx) = oneshot::channel();
    if tx
        .send(Request {
            method: "initialize".to_string(),
            params: json!({
                "clientInfo": {
                    "name": "cteno-usage-monitor",
                    "title": "Cteno Usage Monitor",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
            respond: init_tx,
        })
        .await
        .is_err()
    {
        let _ = child.kill().await;
        return Err("codex app-server worker not accepting requests".to_string());
    }

    match timeout(INITIALIZE_TIMEOUT, init_rx).await {
        Ok(Ok(Ok(_))) => Ok(ConnectedProcess { child, tx }),
        Ok(Ok(Err(e))) => {
            let _ = child.kill().await;
            Err(format!("codex initialize failed: {}", e))
        }
        Ok(Err(_)) => {
            let _ = child.kill().await;
            Err("codex initialize response channel dropped".to_string())
        }
        Err(_) => {
            let _ = child.kill().await;
            Err("codex initialize timed out".to_string())
        }
    }
}

/// Worker that pumps requests in/out of the subprocess. Exits when the
/// request channel closes or stdin/stdout error out; in both cases the
/// probe will respawn on the next poll.
async fn run_worker(mut stdin: ChildStdin, stdout: ChildStdout, mut rx: mpsc::Receiver<Request>) {
    use std::collections::HashMap;

    let mut next_id: i64 = 1;
    let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let pending_read = pending.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let trimmed = buf.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let parsed: Value = match serde_json::from_str(trimmed) {
                        Ok(v) => v,
                        Err(e) => {
                            log::warn!("[codex-probe] invalid JSON line: {} ({})", trimmed, e);
                            continue;
                        }
                    };
                    // Only requests have `id` + `result|error` combo; notifications don't.
                    let Some(id) = parsed.get("id").and_then(Value::as_i64) else {
                        continue;
                    };
                    let mut guard = pending_read.lock().await;
                    if let Some(tx) = guard.remove(&id) {
                        let result = if let Some(err) = parsed.get("error") {
                            Err(err
                                .get("message")
                                .and_then(Value::as_str)
                                .map(String::from)
                                .unwrap_or_else(|| err.to_string()))
                        } else {
                            Ok(parsed.get("result").cloned().unwrap_or(Value::Null))
                        };
                        let _ = tx.send(result);
                    }
                }
                Err(e) => {
                    log::warn!("[codex-probe] stdout read error: {}", e);
                    break;
                }
            }
        }
        // Failing all outstanding requests so callers aren't wedged forever.
        let mut guard = pending_read.lock().await;
        for (_, tx) in guard.drain() {
            let _ = tx.send(Err("codex app-server disconnected".to_string()));
        }
    });

    while let Some(req) = rx.recv().await {
        let id = next_id;
        next_id = next_id.saturating_add(1);

        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": req.method,
            "params": req.params,
        });
        {
            let mut guard = pending.lock().await;
            guard.insert(id, req.respond);
        }
        let line = format!("{}\n", message);
        if let Err(e) = stdin.write_all(line.as_bytes()).await {
            log::warn!("[codex-probe] stdin write error: {}", e);
            let mut guard = pending.lock().await;
            if let Some(tx) = guard.remove(&id) {
                let _ = tx.send(Err(format!("codex stdin error: {}", e)));
            }
            break;
        }
    }
}

fn window_key_for_duration(mins: Option<i64>) -> String {
    match mins {
        Some(300) => "fiveHour".to_string(),
        Some(10_080) => "weekly".to_string(),
        Some(n) => format!("window_{}m", n),
        None => "unknown".to_string(),
    }
}

fn parse_window(value: &Value) -> Option<UsageWindow> {
    let used_percent = value.get("usedPercent").and_then(Value::as_f64)?;
    let resets_at = value.get("resetsAt").and_then(Value::as_i64);
    let window_duration_mins = value.get("windowDurationMins").and_then(Value::as_i64);
    Some(UsageWindow {
        used_percent,
        resets_at,
        window_duration_mins,
        status: None,
        limit_type: None,
    })
}

fn parse_credits(value: &Value) -> Option<UsageCredits> {
    Some(UsageCredits {
        has_credits: value
            .get("hasCredits")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        unlimited: value
            .get("unlimited")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        balance: value
            .get("balance")
            .and_then(Value::as_str)
            .map(String::from),
    })
}

fn translate_snapshot(response: &Value) -> VendorUsage {
    let mut out = VendorUsage::new_windows(VendorId::Codex);

    // Prefer the newer `rateLimitsByLimitId` shape; fall back to the flat
    // `rateLimits` shape used by older codex-app-server versions.
    let limits = response
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object);

    let primary_snapshot = if let Some(map) = limits {
        // Prefer the `codex` limit id (the default/global one); if it's
        // missing, take whatever is first so the user still sees something.
        map.get("codex").or_else(|| map.values().next()).cloned()
    } else {
        response.get("rateLimits").cloned()
    };

    let Some(snap) = primary_snapshot else {
        out.error = Some("codex response missing rateLimits payload".to_string());
        return out;
    };

    if let Some(plan) = snap.get("planType").and_then(Value::as_str) {
        out.plan_type = Some(plan.to_string());
    }
    if let Some(primary) = snap.get("primary").and_then(parse_window) {
        out.windows.insert(
            window_key_for_duration(primary.window_duration_mins),
            primary,
        );
    }
    if let Some(secondary) = snap.get("secondary").and_then(parse_window) {
        out.windows.insert(
            window_key_for_duration(secondary.window_duration_mins),
            secondary,
        );
    }
    if let Some(credits) = snap.get("credits").and_then(parse_credits) {
        out.credits = Some(credits);
    }

    if out.windows.is_empty() && out.credits.is_none() {
        out.error = Some("codex returned no window or credit data".to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_rate_limits_by_limit_id() {
        let response = json!({
            "rateLimitsByLimitId": {
                "codex": {
                    "limitId": "codex",
                    "primary":   { "usedPercent": 2,  "windowDurationMins": 300,   "resetsAt": 1_776_711_831i64 },
                    "secondary": { "usedPercent": 33, "windowDurationMins": 10_080,"resetsAt": 1_777_011_635i64 },
                    "credits":   { "hasCredits": false, "unlimited": false, "balance": "0" },
                    "planType":  "pro"
                }
            }
        });
        let usage = translate_snapshot(&response);
        assert_eq!(usage.plan_type.as_deref(), Some("pro"));
        assert!(usage.error.is_none());
        let five = usage.windows.get("fiveHour").expect("fiveHour window");
        assert_eq!(five.used_percent as i64, 2);
        assert_eq!(five.resets_at, Some(1_776_711_831));
        let weekly = usage.windows.get("weekly").expect("weekly window");
        assert_eq!(weekly.used_percent as i64, 33);
        let credits = usage.credits.clone().expect("credits");
        assert!(!credits.has_credits);
        assert_eq!(credits.balance.as_deref(), Some("0"));
    }

    #[test]
    fn translate_missing_payload_sets_error() {
        let usage = translate_snapshot(&json!({}));
        assert!(usage.error.is_some());
    }
}
