use crate::{LocalHostInfoState, RpcRegistryState, SessionConnectionsState};

/// Stream events sent from Rust to frontend via Tauri Channel during local IPC.
#[derive(Clone, serde::Serialize)]
#[serde(tag = "event", content = "data")]
pub enum AgentStreamEvent {
    #[serde(rename = "stream-start")]
    StreamStart,
    #[serde(rename = "text-delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking-delta")]
    ThinkingDelta { text: String },
    #[serde(rename = "response")]
    Response { text: String },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "stream-end")]
    StreamEnd,
    #[serde(rename = "finished")]
    Finished,
}

/// Send a message directly to a session via Tauri IPC, bypassing Happy Server.
/// Streaming deltas are pushed through the Tauri Channel (on_event).
/// The command blocks until the agent finishes processing, then returns.
/// User message and agent response are asynchronously synced to the server.
#[tauri::command]
pub async fn send_message_local(
    session_id: String,
    text: String,
    images: Option<Vec<serde_json::Value>>,
    permission_mode: Option<String>,
    model: Option<String>,
    system_prompt: Option<String>,
    local_id: Option<String>,
    on_event: tauri::ipc::Channel<AgentStreamEvent>,
    state: tauri::State<'_, SessionConnectionsState>,
) -> Result<(), String> {
    let _ = model;
    let _ = system_prompt;

    log::info!("[LocalIPC] send_message_local for session {}", session_id);
    let handle = state
        .0
        .get(&session_id)
        .await
        .ok_or_else(|| format!("Session {} not connected", session_id))?
        .message_handle();
    handle
        .inject_local_message(
            text,
            images.unwrap_or_default(),
            permission_mode,
            local_id,
            on_event,
        )
        .await
}

/// Generic local RPC gateway — dispatches any RPC method to the in-process
/// RpcRegistry, bypassing Happy Server and encryption.
/// `scope_id` is the machineId (for machine RPCs) or sessionId (for session RPCs).
#[tauri::command]
pub async fn local_rpc(
    method: String,
    scope_id: String,
    params: serde_json::Value,
    state: tauri::State<'_, RpcRegistryState>,
    local_host: tauri::State<'_, LocalHostInfoState>,
) -> Result<serde_json::Value, String> {
    let full_method = format!("{}:{}", scope_id, method);
    let request = cteno_host_rpc_core::RpcRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        method: full_method,
        params: params.clone(),
    };
    let response = state.0.handle(request).await;
    if let Some(error) = response.error {
        let local_machine_id = local_host.machine_id.trim();
        if is_unknown_method_error(&error)
            && is_machine_scope_id(&scope_id)
            && !local_machine_id.is_empty()
            && scope_id != local_machine_id
        {
            let local_method = format!("{}:{}", local_machine_id, method);
            if state.0.has_method(&local_method).await {
                log::warn!(
                    "[LocalIPC] Retrying machine RPC '{}' from stale scope '{}' on local machine '{}'",
                    method,
                    scope_id,
                    local_machine_id
                );
                let retry = cteno_host_rpc_core::RpcRequest {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    method: local_method,
                    params,
                };
                let retry_response = state.0.handle(retry).await;
                if let Some(retry_error) = retry_response.error {
                    return Err(retry_error);
                }
                return Ok(retry_response.result.unwrap_or(serde_json::Value::Null));
            }
        }
        Err(error)
    } else {
        Ok(response.result.unwrap_or(serde_json::Value::Null))
    }
}

fn is_unknown_method_error(error: &str) -> bool {
    error.contains("Unknown method") || error.contains("No handler registered")
}

fn is_machine_scope_id(scope_id: &str) -> bool {
    scope_id.starts_with("cteno-")
}

#[cfg(test)]
mod tests {
    use super::{is_machine_scope_id, is_unknown_method_error};

    #[test]
    fn stale_machine_retry_only_targets_cteno_machine_scopes() {
        assert!(is_machine_scope_id(
            "cteno-199fba5c-9535-4220-8fb0-8db64c5368b2"
        ));
        assert!(!is_machine_scope_id(
            "session-199fba5c-9535-4220-8fb0-8db64c5368b2"
        ));
        assert!(!is_machine_scope_id("199fba5c-9535-4220-8fb0-8db64c5368b2"));
    }

    #[test]
    fn stale_machine_retry_only_handles_missing_handler_errors() {
        assert!(is_unknown_method_error(
            "Unknown method: cteno-old:create-persona"
        ));
        assert!(is_unknown_method_error(
            "No handler registered for method: cteno-old:create-persona"
        ));
        assert!(!is_unknown_method_error("Persona not found"));
    }
}

#[tauri::command]
pub fn get_local_host_info(
    state: tauri::State<'_, LocalHostInfoState>,
) -> Result<serde_json::Value, String> {
    serde_json::to_value(state.inner().clone()).map_err(|e| e.to_string())
}
