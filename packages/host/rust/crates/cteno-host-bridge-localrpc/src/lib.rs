use cteno_host_rpc_core::{RpcRegistry, RpcRequest};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Mutex};

type CompletionMap = Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>;
type KindMap = Arc<Mutex<HashMap<String, String>>>;
const DAEMON_ROOT_ENV: &str = "CTENO_DAEMON_ROOT";

static CLI_COMPLETIONS: OnceLock<CompletionMap> = OnceLock::new();
static SESSION_KINDS: OnceLock<KindMap> = OnceLock::new();

fn completions() -> &'static CompletionMap {
    CLI_COMPLETIONS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

fn session_kinds() -> &'static KindMap {
    SESSION_KINDS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub async fn register_completion(session_id: String) -> oneshot::Receiver<String> {
    let (tx, rx) = oneshot::channel();
    completions().lock().await.insert(session_id, tx);
    rx
}

pub async fn try_complete_cli_session(session_id: &str, final_text: &str) -> bool {
    let mut map = completions().lock().await;
    if let Some(tx) = map.remove(session_id) {
        let _ = tx.send(final_text.to_string());
        true
    } else {
        false
    }
}

pub async fn set_session_kind_label(session_id: String, kind: String) {
    session_kinds().lock().await.insert(session_id, kind);
}

pub fn get_session_kind_label(session_id: &str) -> Option<String> {
    session_kinds()
        .try_lock()
        .ok()
        .and_then(|map| map.get(session_id).cloned())
}

pub async fn remove_session_kind_label(session_id: &str) {
    session_kinds().lock().await.remove(session_id);
}

fn socket_filename(env_tag: &str) -> String {
    if env_tag.is_empty() {
        "daemon.sock".to_string()
    } else {
        format!("daemon.{}.sock", env_tag)
    }
}

pub fn env_tag_from_data_dir(app_data_dir: &str) -> String {
    let dir_name = std::path::Path::new(app_data_dir)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if dir_name.ends_with(".dev") {
        "dev".to_string()
    } else if dir_name.ends_with(".preview") {
        "preview".to_string()
    } else {
        String::new()
    }
}

pub fn socket_path_for_env(env_tag: &str) -> PathBuf {
    daemon_root_dir().join(socket_filename(env_tag))
}

pub fn socket_path() -> PathBuf {
    fn is_connectable(path: &std::path::Path) -> bool {
        std::os::unix::net::UnixStream::connect(path).is_ok()
    }

    if let Ok(env) = std::env::var("CTENO_ENV") {
        return socket_path_for_env(if env == "release" { "" } else { &env });
    }

    let agentd_sock = socket_path_for_env("agentd");
    if agentd_sock.exists() && is_connectable(&agentd_sock) {
        return agentd_sock;
    }

    let dev_sock = socket_path_for_env("dev");
    if dev_sock.exists() && is_connectable(&dev_sock) {
        return dev_sock;
    }

    let release_sock = socket_path_for_env("");
    if release_sock.exists() && is_connectable(&release_sock) {
        return release_sock;
    }

    if agentd_sock.exists() {
        return agentd_sock;
    }
    if dev_sock.exists() {
        return dev_sock;
    }
    release_sock
}

fn daemon_root_dir() -> PathBuf {
    match std::env::var_os(DAEMON_ROOT_ENV) {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".agents"),
    }
}

type InterceptFuture = Pin<Box<dyn Future<Output = Result<Option<Value>, String>> + Send>>;
pub type RpcInterceptor = Arc<dyn Fn(String, String) -> InterceptFuture + Send + Sync>;

/// App-side hook invoked before falling back to the generic `RpcRegistry`.
///
/// Returning `Ok(Some(value))` short-circuits with a successful response;
/// returning `Ok(None)` lets the generic registry handle the method normally.
/// `Err(msg)` is surfaced back to the caller as an RPC error.
#[async_trait::async_trait]
pub trait LocalRpcAuthGate: Send + Sync {
    async fn handle(&self, method: &str, machine_id: &str) -> Result<Option<Value>, String>;
}

/// Preferred entry point: socket binding + generic RPC dispatch with an
/// app-provided auth gate.
pub async fn start_with_gate(
    registry: Arc<RpcRegistry>,
    machine_id: String,
    env_tag: String,
    gate: Arc<dyn LocalRpcAuthGate>,
) {
    let interceptor: RpcInterceptor = Arc::new(move |method: String, machine_id: String| {
        let gate = gate.clone();
        Box::pin(async move { gate.handle(&method, &machine_id).await })
    });
    start(registry, machine_id, env_tag, interceptor).await;
}

pub async fn start(
    registry: Arc<RpcRegistry>,
    machine_id: String,
    env_tag: String,
    interceptor: RpcInterceptor,
) {
    let sock = socket_path_for_env(&env_tag);

    if let Some(parent) = sock.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(&sock);

    let listener = match tokio::net::UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            log::error!("[LocalRPC] Failed to bind {}: {}", sock.display(), e);
            return;
        }
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600));
    }

    log::info!("[LocalRPC] Listening on {}", sock.display());

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let reg = registry.clone();
                let mid = machine_id.clone();
                let intercept = interceptor.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, reg, &mid, intercept).await {
                        log::debug!("[LocalRPC] Connection error: {}", e);
                    }
                });
            }
            Err(e) => {
                log::warn!("[LocalRPC] Accept error: {}", e);
            }
        }
    }
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    registry: Arc<RpcRegistry>,
    machine_id: &str,
    interceptor: RpcInterceptor,
) -> Result<(), String> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err_resp = json!({"id": null, "error": format!("Invalid JSON: {}", e)});
                let mut out = serde_json::to_string(&err_resp).unwrap_or_default();
                out.push('\n');
                let _ = writer.write_all(out.as_bytes()).await;
                continue;
            }
        };

        let id = request["id"].as_str().unwrap_or("").to_string();
        let method = request["method"].as_str().unwrap_or("").to_string();
        let params = request.get("params").cloned().unwrap_or(json!({}));

        if method.is_empty() {
            let err_resp = json!({"id": id, "error": "Missing 'method' field"});
            let mut out = serde_json::to_string(&err_resp).unwrap_or_default();
            out.push('\n');
            let _ = writer.write_all(out.as_bytes()).await;
            continue;
        }

        let response =
            if let Some(result) = interceptor(method.clone(), machine_id.to_string()).await? {
                json!({"id": id, "result": result})
            } else {
                let full_method = format!("{}:{}", machine_id, method);
                let rpc_request = RpcRequest {
                    request_id: id.clone(),
                    method: full_method,
                    params,
                };
                let response = registry.handle(rpc_request).await;
                if let Some(error) = response.error {
                    json!({"id": id, "error": error})
                } else {
                    json!({"id": id, "result": response.result})
                }
            };

        let mut out = serde_json::to_string(&response).unwrap_or_default();
        out.push('\n');
        if writer.write_all(out.as_bytes()).await.is_err() {
            break;
        }
    }

    Ok(())
}

pub fn cleanup(env_tag: &str) {
    let sock = socket_path_for_env(env_tag);
    if sock.exists() {
        let _ = std::fs::remove_file(&sock);
        log::info!("[LocalRPC] Removed socket {}", sock.display());
    }
}
