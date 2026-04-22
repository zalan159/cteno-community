use super::{ExecutionState, PermissionHandler, SessionRegistry};
use crate::happy_client::permission::PermissionRpcResponse;
use crate::happy_client::RpcRegistry;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use multi_agent_runtime_core::{AgentExecutor, ModelChangeOutcome, SessionRef};
use serde::Serialize;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tokio::process::Command;
use tokio::sync::RwLock;

const SESSION_LOCAL_RPC_METHODS: [&str; 17] = [
    "permission",
    "elicitation",
    "set-model",
    "set-permission-mode",
    "set-sandbox-policy",
    "abort",
    "killSession",
    "send-to-background",
    "get-session-mcp-servers",
    "set-session-mcp-servers",
    "bash",
    "readFile",
    "writeFile",
    "listDirectory",
    "getDirectoryTree",
    "ripgrep",
    "switch",
];

#[derive(Clone)]
pub(crate) struct SessionLocalRpcContext {
    pub(crate) session_id: String,
    pub(crate) workdir: PathBuf,
    pub(crate) db_path: PathBuf,
    pub(crate) server_url: String,
    pub(crate) auth_token: String,
    pub(crate) execution_state: ExecutionState,
    pub(crate) permission_handler: Arc<PermissionHandler>,
    pub(crate) session_connections: SessionRegistry,
    pub(crate) mcp_session_ids: Arc<RwLock<Vec<String>>>,
    pub(crate) executor: Option<Arc<dyn AgentExecutor>>,
    pub(crate) session_ref: Option<SessionRef>,
}

fn session_local_method(session_id: &str, method: &str) -> String {
    format!("{session_id}:{method}")
}

#[derive(Serialize)]
struct DirectoryEntryResponse {
    name: String,
    #[serde(rename = "type")]
    entry_type: &'static str,
    size: Option<u64>,
    modified: Option<u64>,
}

#[derive(Serialize)]
struct DirectoryTreeNode {
    name: String,
    path: String,
    #[serde(rename = "type")]
    node_type: &'static str,
    size: Option<u64>,
    modified: Option<u64>,
    children: Option<Vec<DirectoryTreeNode>>,
}

fn resolve_path(workdir: &Path, path: &str) -> PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workdir.join(candidate)
    }
}

fn metadata_modified_ms(metadata: &std::fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
}

fn list_entry_type(file_type: &std::fs::FileType) -> &'static str {
    if file_type.is_file() {
        "file"
    } else if file_type.is_dir() {
        "directory"
    } else {
        "other"
    }
}

fn tree_node_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn compute_sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

async fn read_dir_entries(path: &Path) -> Result<Vec<DirectoryEntryResponse>, String> {
    let mut entries = Vec::new();
    let mut read_dir = tokio::fs::read_dir(path)
        .await
        .map_err(|e| format!("Failed to read directory {}: {e}", path.display()))?;

    while let Some(entry) = read_dir
        .next_entry()
        .await
        .map_err(|e| format!("Failed to read directory entry in {}: {e}", path.display()))?
    {
        let metadata = entry.metadata().await.map_err(|e| {
            format!(
                "Failed to read metadata for {}: {e}",
                entry.path().display()
            )
        })?;
        entries.push(DirectoryEntryResponse {
            name: entry.file_name().to_string_lossy().into_owned(),
            entry_type: list_entry_type(&metadata.file_type()),
            size: Some(metadata.len()),
            modified: metadata_modified_ms(&metadata),
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

fn build_directory_tree(path: &Path, max_depth: usize) -> Result<DirectoryTreeNode, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("Failed to read metadata for {}: {e}", path.display()))?;
    let is_directory = metadata.is_dir();
    let mut node = DirectoryTreeNode {
        name: tree_node_name(path),
        path: path.to_string_lossy().into_owned(),
        node_type: if is_directory { "directory" } else { "file" },
        size: Some(metadata.len()),
        modified: metadata_modified_ms(&metadata),
        children: None,
    };

    if !is_directory || max_depth == 0 {
        return Ok(node);
    }

    let mut children = Vec::new();
    let read_dir = std::fs::read_dir(path)
        .map_err(|e| format!("Failed to read directory {}: {e}", path.display()))?;

    for child in read_dir {
        let child = child.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        children.push(build_directory_tree(&child.path(), max_depth - 1)?);
    }

    children.sort_by(|a, b| a.name.cmp(&b.name));
    node.children = Some(children);
    Ok(node)
}

pub(crate) async fn register_session_local_rpcs(
    registry: &RpcRegistry,
    context: SessionLocalRpcContext,
) {
    let permission_method = session_local_method(&context.session_id, "permission");
    let permission_handler = context.permission_handler.clone();
    registry
        .register(&permission_method, move |params: Value| {
            let permission_handler = permission_handler.clone();
            async move {
                let response: PermissionRpcResponse = serde_json::from_value(params)
                    .map_err(|e| format!("Failed to parse PermissionRpcResponse: {e}"))?;
                permission_handler.handle_rpc_response(response);
                Ok(json!({"status": "ok"}))
            }
        })
        .await;

    let elicitation_method = session_local_method(&context.session_id, "elicitation");
    let executor = context.executor.clone();
    let session_ref = context.session_ref.clone();
    registry
        .register(&elicitation_method, move |params: Value| {
            let executor = executor.clone();
            let session_ref = session_ref.clone();
            async move {
                let request_id = params
                    .get("id")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| "Missing elicitation id".to_string())?
                    .to_string();
                let response = params
                    .get("response")
                    .cloned()
                    .ok_or_else(|| "Missing elicitation response".to_string())?;
                let executor = executor
                    .as_ref()
                    .ok_or_else(|| "Session executor unavailable".to_string())?;
                let session_ref = session_ref
                    .as_ref()
                    .ok_or_else(|| "Session reference unavailable".to_string())?;
                executor
                    .respond_to_elicitation(session_ref, request_id, response)
                    .await
                    .map_err(|e| format!("Failed to send elicitation response: {e}"))?;
                Ok(json!({"status": "ok"}))
            }
        })
        .await;

    let set_permission_mode_method =
        session_local_method(&context.session_id, "set-permission-mode");
    let permission_handler = context.permission_handler.clone();
    let set_mode_executor = context.executor.clone();
    let set_mode_session_ref = context.session_ref.clone();
    let set_mode_session_id = context.session_id.clone();
    let set_mode_db_path = context.db_path.clone();
    let set_mode_server_url = context.server_url.clone();
    let set_mode_auth_token = context.auth_token.clone();
    registry
        .register(&set_permission_mode_method, move |params: Value| {
            let permission_handler = permission_handler.clone();
            let executor = set_mode_executor.clone();
            let session_ref = set_mode_session_ref.clone();
            let session_id = set_mode_session_id.clone();
            let db_path = set_mode_db_path.clone();
            let server_url = set_mode_server_url.clone();
            let auth_token = set_mode_auth_token.clone();
            async move {
                let mode_str = params
                    .get("mode")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| "Missing mode".to_string())?;
                let (exec_mode, host_mode) =
                    crate::happy_client::permission::parse_runtime_permission_mode(mode_str)
                        .ok_or_else(|| format!("Unknown mode: {mode_str}"))?;

                // Forward to the vendor subprocess so the CLI honours the
                // new mode on future tool calls. Runtime permission changes
                // should only report success once both the executor and the
                // host-side gatekeeper accepted the update.
                let executor = executor
                    .as_ref()
                    .ok_or_else(|| format!("Session {} has no executor handle", session_id))?;
                let session_ref = session_ref
                    .as_ref()
                    .ok_or_else(|| format!("Session {} has no session reference", session_id))?;
                executor
                    .set_permission_mode(session_ref, exec_mode)
                    .await
                    .map_err(|e| {
                        format!(
                            "Failed to update permission mode for session {}: {}",
                            session_id, e
                        )
                    })?;

                crate::happy_client::session_helpers::upsert_agent_session_permission_mode_value(
                    &db_path,
                    &session_id,
                    mode_str,
                )
                .map_err(|error| {
                    format!(
                        "Failed to persist permission mode for session {}: {}",
                        session_id, error
                    )
                })?;

                if let Some(mode) = host_mode {
                    permission_handler.set_mode(mode);
                    if !server_url.is_empty() && !auth_token.is_empty() {
                        if let Err(error) = super::persist_session_permission_mode_to_kv(
                            &server_url,
                            &auth_token,
                            &session_id,
                            mode,
                        )
                        .await
                        {
                            log::warn!(
                                "[Permission] Failed to persist mode to KV for {}: {}",
                                session_id,
                                error
                            );
                        }
                    }
                }

                Ok(json!({"status": "ok"}))
            }
        })
        .await;

    let set_model_method = session_local_method(&context.session_id, "set-model");
    let model_session_connections = context.session_connections.clone();
    let model_session_id = context.session_id.clone();
    registry
        .register(&set_model_method, move |params: Value| {
            let session_connections = model_session_connections.clone();
            let session_id = model_session_id.clone();
            async move {
                let model_id = params
                    .get("modelId")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| "Missing modelId".to_string())?
                    .to_string();
                let reasoning_effort = params
                    .get("reasoningEffort")
                    .or_else(|| params.get("effort"))
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string());

                let connection = session_connections
                    .get(&session_id)
                    .await
                    .ok_or_else(|| format!("Session {} not found", session_id))?;
                let outcome = connection
                    .switch_profile(model_id, reasoning_effort)
                    .await?;
                let (outcome_kind, reason) = match outcome {
                    ModelChangeOutcome::Applied => ("applied", None),
                    ModelChangeOutcome::RestartRequired { reason } => {
                        ("restart_required", Some(reason))
                    }
                    ModelChangeOutcome::Unsupported => ("unsupported", None),
                };

                Ok(json!({
                    "status": "ok",
                    "outcome": outcome_kind,
                    "reason": reason,
                }))
            }
        })
        .await;

    let set_sandbox_policy_method = session_local_method(&context.session_id, "set-sandbox-policy");
    let sandbox_session_id = context.session_id.clone();
    registry
        .register(&set_sandbox_policy_method, move |params: Value| {
            let session_id = sandbox_session_id.clone();
            async move {
                log::info!(
                    "[Sandbox] Received set-sandbox-policy RPC for session {}: {:?}",
                    session_id,
                    params
                );
                Ok(json!({"status": "ok"}))
            }
        })
        .await;

    let abort_method = session_local_method(&context.session_id, "abort");
    let execution_state = context.execution_state.clone();
    registry
        .register(&abort_method, move |_params: Value| {
            let execution_state = execution_state.clone();
            async move {
                execution_state.request_abort();
                Ok(json!({"status": "ok"}))
            }
        })
        .await;

    let kill_method = session_local_method(&context.session_id, "killSession");
    let kill_session_id = context.session_id.clone();
    let kill_session_connections = context.session_connections.clone();
    registry
        .register(&kill_method, move |_params: Value| {
            let cleanup_sid = kill_session_id.clone();
            let cleanup_conns = kill_session_connections.clone();
            async move {
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                    if let Ok(run_manager) = crate::local_services::run_manager() {
                        let _ = run_manager.kill_by_session(&cleanup_sid).await;
                    }

                    if let Ok(browser_manager) = crate::local_services::browser_manager() {
                        browser_manager.close_session(&cleanup_sid).await;
                    }

                    match crate::local_services::scheduler() {
                        Ok(scheduler) => match scheduler.delete_tasks_by_session(&cleanup_sid) {
                            Ok(count) if count > 0 => {
                                log::info!(
                                    "[Session] Deleted {} scheduled tasks for session {}",
                                    count,
                                    cleanup_sid
                                );
                            }
                            Ok(_) => {}
                            Err(e) => {
                                log::warn!(
                                    "[Session] Failed to delete scheduled tasks for session {}: {}",
                                    cleanup_sid,
                                    e
                                );
                            }
                        },
                        Err(e) => log::warn!(
                            "[Session] Scheduler service unavailable for {}: {}",
                            cleanup_sid,
                            e
                        ),
                    }

                    crate::subagent::manager::global()
                        .unregister_session(&cleanup_sid)
                        .await;

                    if let Ok(persona_manager) = crate::local_services::persona_manager() {
                        persona_manager.on_task_complete(&cleanup_sid).await;
                    }

                    if cleanup_conns.remove(&cleanup_sid).await.is_some() {
                        log::info!(
                            "[Session] Removed session {} from active connections",
                            cleanup_sid
                        );
                    }

                    log::info!("[Session] Session {} archived and cleaned up", cleanup_sid);
                });

                Ok(json!({"success": true, "message": "Session archived"}))
            }
        })
        .await;

    let send_to_background_method = session_local_method(&context.session_id, "send-to-background");
    let send_to_background_session_id = context.session_id.clone();
    registry
        .register(&send_to_background_method, move |params: Value| {
            let session_id = send_to_background_session_id.clone();
            async move {
                let call_id = params
                    .get("callId")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");

                if call_id.is_empty() {
                    return Ok(json!({"status": "error", "message": "missing callId"}));
                }

                let triggered = match crate::local_services::run_manager() {
                    Ok(run_manager) => run_manager.trigger_background_signal(call_id).await,
                    Err(e) => {
                        log::error!("[SendToBackground] RunManager unavailable: {}", e);
                        return Ok(json!({"status": "error", "message": e}));
                    }
                };

                if triggered {
                    log::info!(
                        "[SendToBackground] Triggered background for callId={} in session {}",
                        call_id,
                        session_id
                    );
                    Ok(json!({"status": "ok"}))
                } else {
                    log::warn!(
                        "[SendToBackground] No pending sync execution for callId={} in session {}",
                        call_id,
                        session_id
                    );
                    Ok(json!({
                        "status": "error",
                        "message": "no pending sync execution for this callId"
                    }))
                }
            }
        })
        .await;

    let get_session_mcp_servers_method =
        session_local_method(&context.session_id, "get-session-mcp-servers");
    let get_session_mcp_servers_workdir = context.workdir.clone();
    let get_session_mcp_servers_db_path = context.db_path.clone();
    let get_session_mcp_servers_ids = context.mcp_session_ids.clone();
    registry
        .register(&get_session_mcp_servers_method, move |_params: Value| {
            let workdir = get_session_mcp_servers_workdir.clone();
            let db_path = get_session_mcp_servers_db_path.clone();
            let mcp_session_ids = get_session_mcp_servers_ids.clone();
            async move {
                let all_servers =
                    super::list_scoped_mcp_servers_for_workdir(&db_path, &workdir).await;
                let active_ids = mcp_session_ids.read().await.clone();
                Ok(json!({
                    "allServers": all_servers,
                    "activeServerIds": active_ids
                }))
            }
        })
        .await;

    let set_session_mcp_servers_method =
        session_local_method(&context.session_id, "set-session-mcp-servers");
    let set_session_mcp_servers_session_id = context.session_id.clone();
    let mcp_session_ids = context.mcp_session_ids.clone();
    registry
        .register(&set_session_mcp_servers_method, move |params: Value| {
            let session_id = set_session_mcp_servers_session_id.clone();
            let mcp_session_ids = mcp_session_ids.clone();
            async move {
                let server_ids: Vec<String> = params
                    .get("serverIds")
                    .and_then(|value| value.as_array())
                    .map(|server_ids| {
                        server_ids
                            .iter()
                            .filter_map(|value| value.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                log::info!(
                    "[MCP] Setting session MCP servers for {}: {:?}",
                    session_id,
                    server_ids
                );

                {
                    let mut ids = mcp_session_ids.write().await;
                    *ids = server_ids;
                }

                Ok(json!({"success": true}))
            }
        })
        .await;

    let bash_method = session_local_method(&context.session_id, "bash");
    let bash_workdir = context.workdir.clone();
    registry
        .register(&bash_method, move |params: Value| {
            let workdir = bash_workdir.clone();
            async move {
                let command = params
                    .get("command")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string();

                if command.is_empty() {
                    return Ok(json!({
                        "success": false,
                        "stdout": "",
                        "stderr": "Missing command",
                        "exitCode": -1
                    }));
                }

                let cwd = params
                    .get("cwd")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let resolved_cwd = resolve_path(&workdir, cwd);

                #[cfg(windows)]
                let (shell, shell_flag) = ("powershell".to_string(), "-Command");
                #[cfg(not(windows))]
                let (shell, shell_flag) = (
                    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()),
                    "-c",
                );

                let wrapped_command =
                    crate::tool_executors::shell::ShellExecutor::wrap_command_utf8(&command);
                let mut process = Command::new(&shell);
                process
                    .arg(shell_flag)
                    .arg(&wrapped_command)
                    .current_dir(&resolved_cwd)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true);

                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    const CREATE_NO_WINDOW: u32 = 0x08000000;
                    process.creation_flags(CREATE_NO_WINDOW);
                }

                match process.output().await {
                    Ok(output) => Ok(json!({
                        "success": output.status.success(),
                        "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                        "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                        "exitCode": output.status.code().unwrap_or(-1)
                    })),
                    Err(e) => Ok(json!({
                        "success": false,
                        "stdout": "",
                        "stderr": format!("Failed to execute command: {e}"),
                        "exitCode": -1
                    })),
                }
            }
        })
        .await;

    let read_file_method = session_local_method(&context.session_id, "readFile");
    let read_file_workdir = context.workdir.clone();
    registry
        .register(&read_file_method, move |params: Value| {
            let workdir = read_file_workdir.clone();
            async move {
                let path = params
                    .get("path")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| "Missing path".to_string())?;
                let resolved_path = resolve_path(&workdir, path);
                match tokio::fs::read(&resolved_path).await {
                    Ok(bytes) => Ok(json!({
                        "success": true,
                        "content": BASE64.encode(bytes)
                    })),
                    Err(e) => Ok(json!({
                        "success": false,
                        "error": format!("Failed to read {}: {e}", resolved_path.display())
                    })),
                }
            }
        })
        .await;

    let write_file_method = session_local_method(&context.session_id, "writeFile");
    let write_file_workdir = context.workdir.clone();
    registry
        .register(&write_file_method, move |params: Value| {
            let workdir = write_file_workdir.clone();
            async move {
                let path = params
                    .get("path")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| "Missing path".to_string())?;
                let content = params
                    .get("content")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| "Missing content".to_string())?;
                let resolved_path = resolve_path(&workdir, path);
                let bytes = match BASE64.decode(content) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        return Ok(json!({
                            "success": false,
                            "error": format!("Failed to decode base64 content: {e}")
                        }));
                    }
                };

                if let Some(parent) = resolved_path.parent() {
                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                        return Ok(json!({
                            "success": false,
                            "error": format!("Failed to create parent directory {}: {e}", parent.display())
                        }));
                    }
                }

                match tokio::fs::write(&resolved_path, &bytes).await {
                    Ok(()) => Ok(json!({
                        "success": true,
                        "hash": compute_sha1_hex(&bytes)
                    })),
                    Err(e) => Ok(json!({
                        "success": false,
                        "error": format!("Failed to write {}: {e}", resolved_path.display())
                    })),
                }
            }
        })
        .await;

    let list_directory_method = session_local_method(&context.session_id, "listDirectory");
    let list_directory_workdir = context.workdir.clone();
    registry
        .register(&list_directory_method, move |params: Value| {
            let workdir = list_directory_workdir.clone();
            async move {
                let path = params
                    .get("path")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let resolved_path = resolve_path(&workdir, path);
                match read_dir_entries(&resolved_path).await {
                    Ok(entries) => Ok(json!({
                        "success": true,
                        "entries": entries
                    })),
                    Err(error) => Ok(json!({
                        "success": false,
                        "error": error
                    })),
                }
            }
        })
        .await;

    let get_directory_tree_method = session_local_method(&context.session_id, "getDirectoryTree");
    let get_directory_tree_workdir = context.workdir.clone();
    registry
        .register(&get_directory_tree_method, move |params: Value| {
            let workdir = get_directory_tree_workdir.clone();
            async move {
                let path = params
                    .get("path")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let max_depth = params
                    .get("maxDepth")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0) as usize;
                let resolved_path = resolve_path(&workdir, path);
                match build_directory_tree(&resolved_path, max_depth) {
                    Ok(tree) => Ok(json!({
                        "success": true,
                        "tree": tree
                    })),
                    Err(error) => Ok(json!({
                        "success": false,
                        "error": error
                    })),
                }
            }
        })
        .await;

    let ripgrep_method = session_local_method(&context.session_id, "ripgrep");
    let ripgrep_workdir = context.workdir.clone();
    registry
        .register(&ripgrep_method, move |params: Value| {
            let workdir = ripgrep_workdir.clone();
            async move {
                let args: Vec<String> = params
                    .get("args")
                    .and_then(|value| value.as_array())
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let cwd = params
                    .get("cwd")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let resolved_cwd = resolve_path(&workdir, cwd);

                let mut process = Command::new("rg");
                process
                    .args(&args)
                    .current_dir(&resolved_cwd)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true);

                match process.output().await {
                    Ok(output) => Ok(json!({
                        "success": output.status.success(),
                        "exitCode": output.status.code(),
                        "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                        "stderr": String::from_utf8_lossy(&output.stderr).to_string()
                    })),
                    Err(e) => Ok(json!({
                        "success": false,
                        "error": format!("Failed to execute rg: {e}")
                    })),
                }
            }
        })
        .await;

    let switch_method = session_local_method(&context.session_id, "switch");
    registry
        .register(
            &switch_method,
            |_params: Value| async move { Ok(json!(true)) },
        )
        .await;
}

pub(crate) async fn unregister_session_local_rpcs(registry: &RpcRegistry, session_id: &str) {
    for method in SESSION_LOCAL_RPC_METHODS {
        registry
            .unregister(&session_local_method(session_id, method))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_path;
    use std::path::Path;

    #[test]
    fn resolve_path_preserves_absolute_paths() {
        let workdir = Path::new("/tmp/workdir");
        assert_eq!(
            resolve_path(workdir, "/tmp/file.txt"),
            Path::new("/tmp/file.txt")
        );
    }

    #[test]
    fn resolve_path_joins_relative_paths_to_workdir() {
        let workdir = Path::new("/tmp/workdir");
        assert_eq!(
            resolve_path(workdir, "nested/file.txt"),
            Path::new("/tmp/workdir/nested/file.txt")
        );
    }
}
