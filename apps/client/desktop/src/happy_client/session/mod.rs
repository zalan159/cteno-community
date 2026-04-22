//! Session-Scoped Socket.IO Connection Manager
//!
//! Each Happy session gets its own Socket.IO connection (session-scoped).
//! This connection receives user messages via `update` events, routes them to
//! the Agent, and sends ACP/event responses back through the session transport.

use super::permission::PermissionHandler;
use super::socket::HappySocket;
use crate::agent_queue::{AgentMessage, AgentMessageQueue};
use crate::agent_session::AgentSessionManager;
use crate::happy_client::RpcRegistry;
use crate::llm::StreamCallback;
use crate::llm_profile::{LlmProfile, ProfileStore};
use crate::service_init::SkillConfig;
use crate::session_message_codec::SessionMessageCodec;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

mod connection;
mod execution;
mod execution_state;
mod local;
mod local_rpc;
pub mod recovery;
pub mod registry;
mod remote;
pub mod spawn;
mod sync;
mod turn_preparation;
mod worker;
pub use connection::{SessionAgentConfig, SessionConnection, SessionConnectionHandle};
use execution::{handle_user_message, handle_user_message_with_stream};
pub(crate) use execution_state::ExecutionState;
use local_rpc::{register_session_local_rpcs, SessionLocalRpcContext};
pub(crate) use recovery::{
    build_session_agent_config_template, install_desktop_session_recovery,
    reconcile_default_profile_store, restore_active_sessions, DesktopSessionRecoveryHooks,
    DesktopSessionRecoveryRuntimeConfig,
};
pub use registry::{SessionConnectionsMap, SessionRegistry};
pub use spawn::{resume_session_connection, spawn_session_internal, SpawnSessionConfig};

pub(crate) async fn register_connection_local_rpcs(
    registry: &RpcRegistry,
    conn: &SessionConnection,
    workdir: PathBuf,
) {
    register_session_local_rpcs(
        registry,
        SessionLocalRpcContext {
            session_id: conn.session_id.clone(),
            workdir,
            db_path: conn.agent_config.db_path.clone(),
            server_url: conn.agent_config.server_url.clone(),
            auth_token: conn.agent_config.auth_token.clone(),
            execution_state: conn.execution_state.clone(),
            permission_handler: conn.permission_handler.clone(),
            session_connections: conn.session_connections.clone(),
            mcp_session_ids: conn.agent_config.session_mcp_server_ids.clone(),
            executor: conn.executor.clone(),
            session_ref: conn.session_ref.clone(),
        },
    )
    .await;
}

pub(crate) fn app_data_dir_from_db_path(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| db_path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn restored_workdir_for_session(db_path: &Path, session_id: &str) -> Option<PathBuf> {
    AgentSessionManager::new(db_path.to_path_buf())
        .get_session(session_id)
        .ok()
        .flatten()
        .and_then(|session| session.context_data)
        .and_then(|context| {
            context
                .get("workdir")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(crate) async fn list_scoped_mcp_servers_for_workdir(db_path: &Path, workdir: &Path) -> Value {
    let app_data_dir = app_data_dir_from_db_path(db_path);
    let global_config_path = app_data_dir.join("mcp_servers.yaml");
    let project_config_path = workdir.join(".cteno").join("mcp_servers.yaml");
    let mut registry = crate::mcp::MCPRegistry::new();

    match registry
        .load_from_scoped_configs(&global_config_path, Some(project_config_path.as_path()))
        .await
    {
        Ok(()) => serde_json::to_value(registry.list_servers()).unwrap_or_else(|e| {
            log::warn!("[MCP] Failed to serialize scoped MCP servers: {}", e);
            json!([])
        }),
        Err(e) => {
            log::warn!(
                "[MCP] Failed to load scoped MCP config for {}: {}",
                workdir.display(),
                e
            );
            json!([])
        }
    }
}

/// Number of consecutive heartbeat failures before considering the connection dead.
/// At 2s intervals, 15 failures = 30 seconds of no successful heartbeat.
const MAX_HEARTBEAT_FAILURES: u32 = 15;

pub(crate) fn encode_session_payload(
    payload: &[u8],
    message_codec: &SessionMessageCodec,
) -> Result<String, String> {
    message_codec.encode_payload(payload)
}

pub(crate) fn decode_session_payload(
    content_type: &str,
    content: &Value,
    message_codec: &SessionMessageCodec,
) -> Result<serde_json::Value, String> {
    message_codec.decode_message_content(content_type, content)
}

/// Send an intermediate ACP message (ACP envelope + encoded transport payload).
/// Zero-key sessions emit plaintext JSON instead of encrypted content.
async fn send_acp_message(
    socket: &HappySocket,
    session_id: &str,
    acp_data: serde_json::Value,
    message_codec: &SessionMessageCodec,
) -> Result<(), String> {
    let message = json!({
        "role": "agent",
        "content": {
            "type": "acp",
            "provider": "cteno",
            "data": acp_data
        },
        "meta": {
            "sentFrom": "cli"
        }
    });

    let message_json = serde_json::to_string(&message)
        .map_err(|e| format!("Failed to serialize ACP message: {}", e))?;

    let outbound_message = encode_session_payload(message_json.as_bytes(), message_codec)
        .map_err(|e| format!("Failed to encode ACP message: {}", e))?;

    socket
        .send_message(session_id, &outbound_message, None)
        .await?;

    Ok(())
}

/// Send a transient ACP message (forwarded to clients but NOT persisted to DB).
/// Used for streaming deltas (text-delta, thinking-delta, stream-start, etc.)
async fn send_transient_acp_message(
    socket: &HappySocket,
    session_id: &str,
    acp_data: serde_json::Value,
    message_codec: &SessionMessageCodec,
) -> Result<(), String> {
    let message = json!({
        "role": "agent",
        "content": {
            "type": "acp",
            "provider": "cteno",
            "data": acp_data
        },
        "meta": {
            "sentFrom": "cli"
        }
    });

    let message_json = serde_json::to_string(&message)
        .map_err(|e| format!("Failed to serialize transient ACP message: {}", e))?;

    let outbound_message = encode_session_payload(message_json.as_bytes(), message_codec)
        .map_err(|e| format!("Failed to encode transient ACP message: {}", e))?;

    socket
        .send_transient_message(session_id, &outbound_message)
        .await?;

    Ok(())
}

/// Send an explicit session event message (event envelope + encoded transport payload).
///
/// This is used for UI state updates that should not be inferred by the frontend from tool-call logs.
async fn send_event_message(
    socket: &HappySocket,
    session_id: &str,
    event_data: serde_json::Value,
    message_codec: &SessionMessageCodec,
) -> Result<(), String> {
    let message = json!({
        "role": "agent",
        "content": {
            "type": "event",
            "data": event_data
        },
        "meta": {
            "sentFrom": "cli"
        }
    });

    let message_json = serde_json::to_string(&message)
        .map_err(|e| format!("Failed to serialize event message: {}", e))?;
    let outbound_message = encode_session_payload(message_json.as_bytes(), message_codec)
        .map_err(|e| format!("Failed to encode event message: {}", e))?;
    socket
        .send_message(session_id, &outbound_message, None)
        .await?;
    Ok(())
}

/// Fetch per-session permission mode from server KV (used during session restore/reconnect).
pub(crate) async fn fetch_session_permission_mode_from_kv(
    server_url: &str,
    auth_token: &str,
    session_id: &str,
) -> Option<super::permission::PermissionMode> {
    let client = reqwest::Client::new();
    let base = server_url.trim_end_matches('/');
    let key = format!("session.{}.permissionMode", session_id);
    let get_url = format!("{}/v1/kv/{}", base, urlencoding::encode(&key));

    let resp = match client.get(&get_url).bearer_auth(auth_token).send().await {
        Ok(r) => r,
        Err(e) => {
            log::warn!(
                "KV fetch permissionMode for session {} failed: {}",
                session_id,
                e
            );
            return None;
        }
    };

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return None;
    }

    if !resp.status().is_success() {
        log::warn!(
            "KV fetch permissionMode for session {} returned HTTP {}",
            session_id,
            resp.status()
        );
        return None;
    }

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            log::warn!(
                "KV fetch permissionMode JSON parse failed for session {}: {}",
                session_id,
                e
            );
            return None;
        }
    };

    let value_str = body.get("value").and_then(|v| v.as_str())?;

    // Value is stored as a JSON string, e.g. "\"bypassPermissions\""
    let mode_str: String = match serde_json::from_str(value_str) {
        Ok(s) => s,
        Err(_) => value_str.to_string(), // fallback: raw string
    };

    let mode = super::permission::PermissionHandler::parse_mode(&mode_str);
    if let Some(m) = mode {
        log::info!(
            "Restored permissionMode {:?} for session {} from KV",
            m,
            session_id
        );
    }
    mode
}

/// Persist per-session permission mode to server KV so state survives reconnects.
pub(crate) async fn persist_session_permission_mode_to_kv(
    server_url: &str,
    auth_token: &str,
    session_id: &str,
    mode: super::permission::PermissionMode,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let base = server_url.trim_end_matches('/');
    let key = format!("session.{}.permissionMode", session_id);
    let value = serde_json::to_string(super::permission::PermissionHandler::mode_to_string(mode))
        .map_err(|e| format!("Failed to serialize permissionMode for KV: {}", e))?;

    // 1) Fetch current version (if any)
    let get_url = format!("{}/v1/kv/{}", base, urlencoding::encode(&key));
    let mut version: i64 = -1;
    let get_resp = client
        .get(get_url)
        .bearer_auth(auth_token)
        .send()
        .await
        .map_err(|e| format!("KV get failed: {}", e))?;
    if get_resp.status() != reqwest::StatusCode::NOT_FOUND {
        if !get_resp.status().is_success() {
            return Err(format!("KV get failed: HTTP {}", get_resp.status()));
        }
        let body: serde_json::Value = get_resp
            .json()
            .await
            .map_err(|e| format!("KV get JSON parse failed: {}", e))?;
        if let Some(v) = body.get("version").and_then(|v| v.as_i64()) {
            version = v;
        }
    }

    // 2) Mutate with OCC; on 409, retry once with server-provided version.
    let mutate_url = format!("{}/v1/kv", base);
    for attempt in 0..2 {
        let resp = client
            .post(&mutate_url)
            .bearer_auth(auth_token)
            .json(&json!({
                "mutations": [{
                    "key": key.clone(),
                    "value": value.clone(),
                    "version": version
                }]
            }))
            .send()
            .await
            .map_err(|e| format!("KV mutate failed: {}", e))?;

        let status = resp.status();

        if status.is_success() {
            return Ok(());
        }

        if status == reqwest::StatusCode::CONFLICT && attempt == 0 {
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| format!("KV conflict JSON parse failed: {}", e))?;
            if let Some(v) = body
                .get("errors")
                .and_then(|e| e.as_array())
                .and_then(|arr| arr.first())
                .and_then(|e| e.get("version"))
                .and_then(|v| v.as_i64())
            {
                version = v;
                continue;
            }
        }

        return Err(format!("KV mutate failed: HTTP {}", status));
    }

    Ok(())
}

/// Extract image sources from queued agent messages' metadata.
/// Resolves file_ref images by downloading from the server.
/// Returns None if no images found.
async fn extract_images_from_messages(
    messages: &[AgentMessage],
    server_url: &str,
    auth_token: &str,
) -> Option<Vec<crate::llm::ImageSource>> {
    let mut images = Vec::new();
    for msg in messages {
        if let Some(meta) = &msg.metadata {
            if let Some(img_arr) = meta.get("images").and_then(|v| v.as_array()) {
                for img in img_arr {
                    let source = img.get("source");
                    let source_type = source
                        .and_then(|s| s.get("type"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("base64");

                    if source_type == "file_ref" {
                        // Resolve file_ref: get signed URL then download as base64
                        if let Some(file_id) = source
                            .and_then(|s| s.get("file_id"))
                            .and_then(|v| v.as_str())
                        {
                            let media_type = source
                                .and_then(|s| s.get("media_type"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("image/jpeg");

                            match resolve_file_ref_url(server_url, auth_token, file_id).await {
                                Ok(signed_url) => {
                                    // Download image and convert to base64 for universal provider compatibility
                                    match download_image_to_base64(&signed_url).await {
                                        Ok(b64) => {
                                            log::info!("[Agent] Resolved file_ref image {} to base64 ({}KB)", file_id, b64.len() / 1024);
                                            images.push(crate::llm::ImageSource {
                                                source_type: "base64".to_string(),
                                                media_type: media_type.to_string(),
                                                data: b64,
                                            });
                                        }
                                        Err(e) => {
                                            log::error!(
                                                "[Agent] Failed to download file_ref image {}: {}",
                                                file_id,
                                                e
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::error!(
                                        "[Agent] Failed to resolve file_ref image {}: {}",
                                        file_id,
                                        e
                                    );
                                }
                            }
                        }
                    } else {
                        // Legacy base64 inline
                        if let (Some(media_type), Some(data)) = (
                            source
                                .and_then(|s| s.get("media_type"))
                                .and_then(|v| v.as_str()),
                            source.and_then(|s| s.get("data")).and_then(|v| v.as_str()),
                        ) {
                            images.push(crate::llm::ImageSource {
                                source_type: "base64".to_string(),
                                media_type: media_type.to_string(),
                                data: data.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }
    if images.is_empty() {
        None
    } else {
        Some(images)
    }
}

/// Get a signed download URL for an image referenced by file_id.
async fn resolve_file_ref_url(
    server_url: &str,
    auth_token: &str,
    file_id: &str,
) -> Result<String, String> {
    let client = reqwest::Client::new();

    let download_url = format!("{}/v1/files/{}/download", server_url, file_id);
    let resp = client
        .get(&download_url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .send()
        .await
        .map_err(|e| format!("Download URL request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Download URL request returned {}", resp.status()));
    }

    let download_info: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse download URL response: {}", e))?;

    download_info
        .get("url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "Missing url in download response".to_string())
}

/// Download an image from a URL and return base64-encoded data.
async fn download_image_to_base64(url: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Image download failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Image download returned {}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read image bytes: {}", e))?;

    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

/// Encrypt and send an agent response message in ACP format
async fn send_agent_response(
    socket: &HappySocket,
    session_id: &str,
    response_text: &str,
    message_codec: &SessionMessageCodec,
    _local_origin: bool,
) -> Result<(), String> {
    // Build ACP message and sync to server (for mobile access and history)
    let message = json!({
        "role": "agent",
        "content": {
            "type": "acp",
            "provider": "claude",
            "data": {
                "type": "message",
                "message": response_text
            }
        },
        "meta": {
            "sentFrom": "cli"
        }
    });

    let message_json = serde_json::to_string(&message)
        .map_err(|e| format!("Failed to serialize agent message: {}", e))?;

    let outbound_message = encode_session_payload(message_json.as_bytes(), message_codec)
        .map_err(|e| format!("Failed to encode agent message: {}", e))?;

    socket
        .send_message(session_id, &outbound_message, None)
        .await?;

    log::info!("Agent response sent for session: {}", session_id);

    Ok(())
}

fn upsert_agent_session_profile_id(
    db_path: &Path,
    session_id: &str,
    profile_id: &str,
) -> Result<(), String> {
    let manager = AgentSessionManager::new(db_path.to_path_buf());
    let existing = manager.get_session(session_id)?;

    if existing.is_none() {
        manager.create_session_with_id(
            session_id, "worker", None, None, // default timeout
        )?;
    }

    let mut context = existing
        .and_then(|s| s.context_data)
        .unwrap_or_else(|| json!({}));

    let context_obj = context
        .as_object_mut()
        .ok_or_else(|| "Session context_data is not an object".to_string())?;
    context_obj.insert(
        "profile_id".to_string(),
        Value::String(profile_id.to_string()),
    );

    manager.update_context_data(session_id, &context)
}

/// Pre-flight balance check: GET /v1/balance/status and return balanceYuan.
/// Returns Ok(balance) or Err if the check itself fails.
async fn check_proxy_balance(server_url: &str, auth_token: &str) -> Result<f64, String> {
    let url = format!("{}/v1/balance/status", server_url);
    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("Balance check request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Balance check returned status {}",
            response.status()
        ));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse balance response: {}", e))?;

    body.get("balanceYuan")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "Missing balanceYuan in response".to_string())
}

const CONTINUOUS_BROWSING_PROMPT_CTENO: &str = "继续";
const CONTINUOUS_BROWSING_PROMPT_OTHER: &str = "continue";

fn continuous_browsing_prompt_for_vendor(vendor: &str) -> &'static str {
    match vendor {
        "claude" | "codex" | "gemini" => CONTINUOUS_BROWSING_PROMPT_OTHER,
        _ => CONTINUOUS_BROWSING_PROMPT_CTENO,
    }
}

fn resolve_session_vendor(db_path: &Path, session_id: &str) -> Option<String> {
    AgentSessionManager::new(db_path.to_path_buf())
        .get_session(session_id)
        .ok()
        .flatten()
        .map(|session| session.vendor.trim().to_ascii_lowercase())
        .filter(|vendor| !vendor.is_empty())
}

pub(crate) fn continuous_browsing_prompt_for_session(db_path: &Path, session_id: &str) -> String {
    resolve_session_vendor(db_path, session_id)
        .as_deref()
        .map(continuous_browsing_prompt_for_vendor)
        .unwrap_or(CONTINUOUS_BROWSING_PROMPT_CTENO)
        .to_string()
}

pub(crate) fn is_continuous_browsing_control_message(content: &str) -> bool {
    let trimmed = content.trim();
    if trimmed == CONTINUOUS_BROWSING_PROMPT_CTENO {
        return true;
    }
    trimmed.eq_ignore_ascii_case(CONTINUOUS_BROWSING_PROMPT_OTHER)
}

pub(crate) fn spawn_queued_worker_for_session_if_idle(
    session_id: String,
    worker_label: &'static str,
    auto_rename_persona: bool,
) {
    tokio::spawn(async move {
        let spawn_config = match crate::local_services::spawn_config() {
            Ok(config) => config,
            Err(e) => {
                log::warn!(
                    "[Session {}] Failed to spawn queued worker ({worker_label}): {}",
                    session_id,
                    e
                );
                return;
            }
        };

        let Some(conn) = spawn_config.session_connections.get(&session_id).await else {
            log::warn!(
                "[Session {}] Failed to spawn queued worker ({worker_label}): session connection missing",
                session_id
            );
            return;
        };

        let handle = conn.message_handle();
        let started = worker::spawn_background_queue_worker_if_idle(
            worker::BackgroundWorkerState {
                session_id: session_id.clone(),
                execution_state: handle.execution_state.clone(),
                config: handle.agent_config.clone(),
                socket_for_response: handle.socket.clone(),
                message_codec: handle.message_codec,
                perm_handler: handle.permission_handler.clone(),
                context_tokens: handle.context_tokens.clone(),
                compression_threshold: handle.compression_threshold.clone(),
                executor: handle.executor.clone(),
                session_ref: handle.session_ref.clone(),
            },
            worker::BackgroundWorkerOptions {
                worker_label,
                auto_hibernate: true,
                auto_rename_persona,
            },
        );

        if !started {
            log::debug!(
                "[Session {}] Worker already running, queued message will be picked up ({worker_label})",
                session_id
            );
        }
    });
}

/// Check if a session belongs to a persona with continuous_browsing enabled,
/// and if so, schedule re-injection of a vendor-specific continue message.
/// This creates a perpetual browsing loop for autonomous persona exploration.
///
/// The injected message is picked up by the existing 2-second poll handler,
/// which starts a new worker loop automatically.
/// Check the server for the latest message in a session.
/// If it's an unprocessed user message, return it as an AgentMessage for queue injection.
/// This is a lightweight version of `catch_up_missed_messages` designed for periodic polling.
/// `last_seen_msg_id` is used to deduplicate: if the newest message has the same ID, skip it.
async fn check_for_missed_messages(
    session_id: &str,
    server_url: &str,
    auth_token: &str,
    message_codec: &SessionMessageCodec,
    last_seen_msg_id: &mut Option<String>,
) -> Option<crate::agent_queue::AgentMessage> {
    let url = format!("{}/v1/sessions/{}/messages?limit=1", server_url, session_id);
    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        return None;
    }

    let body: serde_json::Value = response.json().await.ok()?;
    let messages = body.get("messages")?.as_array()?;
    if messages.is_empty() {
        return None;
    }

    let newest = &messages[0];

    // Deduplicate: skip if this is the same message we already processed
    let msg_id = newest
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if let (Some(ref seen), Some(ref current)) = (last_seen_msg_id.as_ref(), msg_id.as_ref()) {
        if seen == current {
            return None;
        }
    }

    let content = newest.get("content")?;
    let content_type = content.get("t").and_then(|v| v.as_str())?;
    let payload = content.get("c")?;
    let message_json = decode_session_payload(content_type, payload, message_codec).ok()?;

    let role = message_json
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if role != "user" {
        return None;
    }

    // Extract text content
    let content_val = message_json.get("content")?;
    let mut user_text = String::new();
    let mut user_images: Vec<serde_json::Value> = Vec::new();

    if let Some(arr) = content_val.as_array() {
        for block in arr {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        if !user_text.is_empty() {
                            user_text.push('\n');
                        }
                        user_text.push_str(t);
                    }
                }
                Some("image") => {
                    user_images.push(block.clone());
                }
                _ => {}
            }
        }
    } else {
        user_text = content_val
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
    }

    if user_text.is_empty() && user_images.is_empty() {
        return None;
    }

    log::info!(
        "[Session {}] Periodic catch-up: found missed user message: '{}'",
        session_id,
        if user_text.len() > 80 {
            user_text
                .char_indices()
                .nth(80)
                .map_or(user_text.as_str(), |(i, _)| &user_text[..i])
        } else {
            &user_text
        }
    );

    // Record this message ID so we don't enqueue it again
    *last_seen_msg_id = msg_id;

    if user_images.is_empty() {
        Some(crate::agent_queue::AgentMessage::user(
            session_id.to_string(),
            user_text,
        ))
    } else {
        Some(crate::agent_queue::AgentMessage::user_with_images(
            session_id.to_string(),
            user_text,
            user_images,
        ))
    }
}

/// Release idle worker session connections after agent loop completes.
///
/// The executor session itself stays resumable through persisted metadata, so
/// only the in-memory local connection is dropped here.
/// Skips continuous_browsing and running hypothesis sessions (they self-schedule).
fn maybe_auto_hibernate_worker(session_id: &str, execution_state: &ExecutionState) {
    // Don't release Persona chat sessions — they're persistent orchestrators
    // that need to receive background run notifications via the poll loop.
    let is_persona_chat = crate::local_services::persona_manager()
        .ok()
        .and_then(|mgr| {
            mgr.store()
                .get_persona_for_session(session_id)
                .ok()
                .flatten()
        })
        .map(|link| link.session_type == crate::persona::models::PersonaSessionType::Chat)
        .unwrap_or(false);
    if is_persona_chat {
        return;
    }

    // Don't release continuous_browsing sessions — they schedule continue prompts.
    let is_continuous = crate::local_services::persona_manager()
        .ok()
        .and_then(|mgr| mgr.store().is_continuous_browsing_session(session_id).ok())
        .unwrap_or(false);
    if is_continuous {
        return;
    }

    let sid = session_id.to_string();
    let execution_state = execution_state.clone();
    tokio::spawn(async move {
        // Grace period: let in-flight operations settle
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

        // Double-check: if new messages arrived during the grace period, don't release
        if !execution_state.is_idle(&sid) {
            log::debug!(
                "[Session {}] Skipping idle release: queue not empty or still processing",
                sid
            );
            return;
        }

        // Don't release if the host run manager or vendor task registry still
        // has work for this session — the poll loop needs to stay alive to
        // consume their notifications.
        let host_has_runs = match crate::local_services::run_manager() {
            Ok(rm) => rm.has_running_tasks(&sid).await,
            Err(_) => false,
        };
        let vendor_has_tasks = crate::local_services::background_task_registry()
            .map(|registry| registry.has_running_for_session(&sid))
            .unwrap_or(false);
        if host_has_runs || vendor_has_tasks {
            log::debug!(
                "[Session {}] Skipping idle release: host_runs={} vendor_tasks={}",
                sid,
                host_has_runs,
                vendor_has_tasks
            );
            return;
        }

        let spawn_config = match crate::local_services::spawn_config() {
            Ok(c) => c,
            Err(_) => return,
        };

        let conn_opt = { spawn_config.session_connections.remove(&sid).await };
        if let Some(conn) = conn_opt {
            conn.disconnect().await;
            log::info!("[Session {}] Released idle worker session connection", sid);
        }
    });
}

///
/// `success` indicates whether the last agent execution succeeded.
/// If `false` (API error, panic, etc.), the loop is stopped to avoid infinite retries.
fn maybe_schedule_continuous_browsing(
    session_id: &str,
    execution_state: &ExecutionState,
    success: bool,
) {
    let persona_mgr = match crate::local_services::persona_manager().ok() {
        Some(mgr) => mgr,
        None => return,
    };

    let is_continuous = persona_mgr
        .store()
        .is_continuous_browsing_session(session_id)
        .unwrap_or(false);

    if !is_continuous {
        return;
    }

    if !success {
        log::warn!(
            "[ContinuousBrowsing] Session {} last execution failed, stopping continuous loop to avoid infinite retries",
            session_id
        );
        return;
    }

    // Don't schedule if the conversation has no real user messages
    // (only system-injected continue prompts).
    // This prevents an infinite loop in empty conversations.
    if !session_has_real_user_messages(persona_mgr.db_path(), session_id) {
        log::info!(
            "[ContinuousBrowsing] Session {} has no real user messages, skipping continuous loop",
            session_id
        );
        return;
    }

    log::info!(
        "[ContinuousBrowsing] Session {} is continuous_browsing, scheduling next round",
        session_id
    );

    let sid = session_id.to_string();
    let db_path = persona_mgr.db_path().to_path_buf();
    let execution_state = execution_state.clone();

    tokio::spawn(async move {
        // Delay before next round (5 seconds for natural pacing)
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        // Double-check the flag is still enabled (user may have turned it off)
        let still_enabled = crate::local_services::persona_manager()
            .ok()
            .and_then(|mgr| mgr.store().is_continuous_browsing_session(&sid).ok())
            .unwrap_or(false);

        if !still_enabled {
            log::info!(
                "[ContinuousBrowsing] Session {} continuous_browsing was disabled, stopping",
                sid
            );
            return;
        }

        // Don't inject if already processing (user sent a message manually)
        if execution_state.queue.is_processing(&sid) {
            log::info!(
                "[ContinuousBrowsing] Session {} already processing, skipping injection",
                sid
            );
            return;
        }

        let continue_prompt = continuous_browsing_prompt_for_session(&db_path, &sid);
        log::info!(
            "[ContinuousBrowsing] Injecting '{}' message into session {}",
            continue_prompt,
            sid
        );

        // Inject the vendor-aware continue message into the queue.
        // The existing 2-second poll handler will detect it and start a worker loop.
        let _ = execution_state
            .queue
            .push(AgentMessage::system(sid.clone(), continue_prompt));

        // Ensure injected control prompts are actively consumed even when the
        // remote keep-alive poll loop is not running.
        spawn_queued_worker_for_session_if_idle(sid.clone(), "continuous-browsing", true);
    });
}

/// Check if a session has any real user messages (not just system-injected continue prompts).
/// Returns false if the session doesn't exist, has no messages, or only has control messages.
pub(crate) fn session_has_real_user_messages(db_path: &std::path::Path, session_id: &str) -> bool {
    let manager = crate::agent_session::AgentSessionManager::new(db_path.to_path_buf());
    match manager.get_session(session_id) {
        Ok(Some(session)) => session
            .messages
            .iter()
            .any(|m| m.role == "user" && !is_continuous_browsing_control_message(&m.content)),
        _ => false,
    }
}
