//! Shared helper functions for session creation, workdir management, and directory resolution.
//!
//! Extracted from `manager.rs` to reduce its size. These helpers are used by
//! `manager.rs`, `session/spawn.rs`, and `session/recovery.rs`.

use crate::agent_session::AgentSessionManager;
use cteno_host_session_codec::EncryptionVariant;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Directory helpers
// ---------------------------------------------------------------------------

fn default_user_home_dir() -> PathBuf {
    dirs::home_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn ensure_spawn_directory(raw_directory: &str) -> Result<String, String> {
    let raw = raw_directory.trim();
    if raw.is_empty() {
        return Err("Missing directory".to_string());
    }

    let home = default_user_home_dir();
    let mut resolved = if raw == "~" {
        home.clone()
    } else if raw.starts_with("~/") || raw.starts_with("~\\") {
        home.join(&raw[2..])
    } else {
        PathBuf::from(raw)
    };

    if resolved.is_relative() {
        resolved = home.join(resolved);
    }

    if resolved.exists() {
        if !resolved.is_dir() {
            return Err(format!(
                "Path exists but is not a directory: {}",
                resolved.display()
            ));
        }
    } else {
        std::fs::create_dir_all(&resolved)
            .map_err(|e| format!("Failed to create directory '{}': {}", resolved.display(), e))?;
        log::info!(
            "spawn-happy-session: created missing directory {}",
            resolved.display()
        );
    }

    Ok(resolved.to_string_lossy().to_string())
}

// ---------------------------------------------------------------------------
// Local DB upsert helpers
// ---------------------------------------------------------------------------

pub(crate) fn upsert_agent_session_workdir(
    db_path: &std::path::Path,
    session_id: &str,
    workdir: &str,
) -> Result<(), String> {
    upsert_agent_session_metadata(db_path, session_id, workdir, None, None)
}

pub(crate) fn upsert_agent_session_workdir_and_profile(
    db_path: &std::path::Path,
    session_id: &str,
    workdir: &str,
    profile_id: Option<&str>,
) -> Result<(), String> {
    upsert_agent_session_metadata(db_path, session_id, workdir, profile_id, None)
}

pub(crate) fn upsert_agent_session_workdir_profile_and_vendor(
    db_path: &std::path::Path,
    session_id: &str,
    workdir: &str,
    profile_id: Option<&str>,
    vendor: &str,
) -> Result<(), String> {
    upsert_agent_session_metadata(db_path, session_id, workdir, profile_id, Some(vendor))
}

pub(crate) fn upsert_agent_session_native_session_id(
    db_path: &std::path::Path,
    session_id: &str,
    vendor: &str,
    native_session_id: &str,
) -> Result<(), String> {
    let manager = AgentSessionManager::new(db_path.to_path_buf());
    let existing = manager.get_session(session_id)?;

    if existing.is_none() {
        if let Err(err) =
            manager.create_session_with_id_and_vendor(session_id, "worker", None, None, vendor)
        {
            if !err.contains("UNIQUE constraint failed") {
                return Err(err);
            }
        }
    }

    if existing.as_ref().map(|session| session.vendor.as_str()) != Some(vendor) {
        manager.set_vendor(session_id, vendor)?;
    }

    manager.update_context_field(
        session_id,
        "native_session_id",
        Value::String(native_session_id.to_string()),
    )
}

pub(crate) fn upsert_agent_session_permission_mode(
    db_path: &std::path::Path,
    session_id: &str,
    permission_mode: super::permission::PermissionMode,
) -> Result<(), String> {
    let manager = AgentSessionManager::new(db_path.to_path_buf());
    let mode = match permission_mode {
        super::permission::PermissionMode::Default => "default",
        super::permission::PermissionMode::AcceptEdits => "acceptEdits",
        super::permission::PermissionMode::BypassPermissions => "bypassPermissions",
        super::permission::PermissionMode::Plan => "plan",
    };

    upsert_agent_session_permission_mode_value_with_manager(&manager, session_id, mode)
}

pub(crate) fn upsert_agent_session_permission_mode_value(
    db_path: &std::path::Path,
    session_id: &str,
    permission_mode: &str,
) -> Result<(), String> {
    let manager = AgentSessionManager::new(db_path.to_path_buf());
    upsert_agent_session_permission_mode_value_with_manager(&manager, session_id, permission_mode)
}

fn upsert_agent_session_permission_mode_value_with_manager(
    manager: &AgentSessionManager,
    session_id: &str,
    permission_mode: &str,
) -> Result<(), String> {
    manager.update_context_field(
        session_id,
        "permissionMode",
        Value::String(permission_mode.to_string()),
    )
}

pub(crate) fn load_agent_session_workdir(
    db_path: &std::path::Path,
    session_id: &str,
) -> Result<Option<String>, String> {
    let manager = AgentSessionManager::new(db_path.to_path_buf());
    Ok(manager
        .get_session(session_id)?
        .and_then(|session| session.context_data)
        .and_then(|context| {
            context
                .get("workdir")
                .and_then(|value| value.as_str().map(str::to_owned))
        }))
}

fn upsert_agent_session_metadata(
    db_path: &std::path::Path,
    session_id: &str,
    workdir: &str,
    profile_id: Option<&str>,
    vendor: Option<&str>,
) -> Result<(), String> {
    let manager = AgentSessionManager::new(db_path.to_path_buf());
    let mut existing = manager.get_session(session_id)?;

    if existing.is_none() {
        let create_result = match vendor {
            Some(vendor) => {
                manager.create_session_with_id_and_vendor(
                    session_id, "worker", None, None, // default timeout
                    vendor,
                )
            }
            None => manager.create_session_with_id(
                session_id, "worker", None, None, // default timeout
            ),
        };

        if let Err(err) = create_result {
            if !err.contains("UNIQUE constraint failed") {
                return Err(err);
            }
        }

        existing = manager.get_session(session_id)?;
    }

    if let Some(vendor) = vendor {
        if existing.as_ref().map(|session| session.vendor.as_str()) != Some(vendor) {
            manager.set_vendor(session_id, vendor)?;
        }
    }

    let mut context = existing
        .and_then(|s| s.context_data)
        .unwrap_or_else(|| json!({}));

    let context_obj = context
        .as_object_mut()
        .ok_or_else(|| "Session context_data is not an object".to_string())?;
    context_obj.insert("workdir".to_string(), Value::String(workdir.to_string()));
    if let Some(pid) = profile_id {
        context_obj.insert("profile_id".to_string(), Value::String(pid.to_string()));
    }

    manager.update_context_data(session_id, &context)
}

#[cfg(test)]
mod tests {
    use super::{
        build_create_session_request_body, build_session_index_metadata,
        upsert_agent_session_native_session_id, upsert_agent_session_permission_mode,
        upsert_agent_session_workdir_profile_and_vendor,
    };
    use crate::agent_session::AgentSessionManager;
    use crate::happy_client::permission::PermissionMode;
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use tempfile::tempdir;

    fn init_agent_sessions_table(db_path: &std::path::Path) {
        let conn = Connection::open(db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE agent_sessions (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                user_id TEXT,
                messages TEXT NOT NULL DEFAULT '[]',
                context_data TEXT,
                status TEXT DEFAULT 'active',
                created_at TEXT,
                updated_at TEXT,
                expires_at TEXT,
                owner_session_id TEXT
            );
            "#,
        )
        .unwrap();
    }

    #[test]
    fn upsert_agent_session_metadata_sets_vendor_and_merges_context_idempotently() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        init_agent_sessions_table(&db_path);

        upsert_agent_session_workdir_profile_and_vendor(
            &db_path,
            "happy-session-1",
            "/tmp/workspace-a",
            Some("profile-a"),
            "claude",
        )
        .unwrap();
        upsert_agent_session_workdir_profile_and_vendor(
            &db_path,
            "happy-session-1",
            "/tmp/workspace-b",
            Some("profile-b"),
            "claude",
        )
        .unwrap();

        let manager = AgentSessionManager::new(db_path);
        let session = manager.get_session("happy-session-1").unwrap().unwrap();

        assert_eq!(session.vendor, "claude");
        assert_eq!(
            session
                .context_data
                .as_ref()
                .and_then(|ctx| ctx.get("workdir"))
                .and_then(|value| value.as_str()),
            Some("/tmp/workspace-b")
        );
        assert_eq!(
            session
                .context_data
                .as_ref()
                .and_then(|ctx| ctx.get("profile_id"))
                .and_then(|value| value.as_str()),
            Some("profile-b")
        );

        let claude_rows = manager.list_sessions_by_vendor("claude", None).unwrap();
        assert_eq!(claude_rows.len(), 1);
        assert_eq!(claude_rows[0].id, "happy-session-1");
    }

    #[test]
    fn upsert_agent_session_native_session_id_writes_stable_key() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        init_agent_sessions_table(&db_path);

        upsert_agent_session_native_session_id(&db_path, "happy-session-2", "claude", "native-123")
            .unwrap();
        upsert_agent_session_native_session_id(&db_path, "happy-session-2", "claude", "native-123")
            .unwrap();

        let manager = AgentSessionManager::new(db_path);
        let session = manager.get_session("happy-session-2").unwrap().unwrap();

        assert_eq!(session.vendor, "claude");
        assert_eq!(
            session
                .context_data
                .as_ref()
                .and_then(|ctx| ctx.get("native_session_id"))
                .and_then(|value| value.as_str()),
            Some("native-123")
        );
    }

    #[test]
    fn upsert_agent_session_permission_mode_updates_context_without_clobbering() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        init_agent_sessions_table(&db_path);

        let manager = AgentSessionManager::new(db_path.clone());
        manager
            .create_session_with_id("happy-session-3", "worker", None, None)
            .unwrap();
        manager
            .update_context_data(
                "happy-session-3",
                &json!({
                    "workdir": "/tmp/workspace-c",
                    "permissionMode": "default",
                }),
            )
            .unwrap();

        upsert_agent_session_permission_mode(
            &db_path,
            "happy-session-3",
            PermissionMode::BypassPermissions,
        )
        .unwrap();

        let session = manager.get_session("happy-session-3").unwrap().unwrap();
        assert_eq!(
            session
                .context_data
                .as_ref()
                .and_then(|ctx| ctx.get("permissionMode"))
                .and_then(|value| value.as_str()),
            Some("bypassPermissions")
        );
        assert_eq!(
            session
                .context_data
                .as_ref()
                .and_then(|ctx| ctx.get("workdir"))
                .and_then(|value| value.as_str()),
            Some("/tmp/workspace-c")
        );
    }

    #[test]
    fn create_session_request_body_only_contains_session_index_fields() {
        let body =
            build_create_session_request_body("/tmp/workspace-a", "codex", "cteno-test").unwrap();

        assert_eq!(
            body.get("tag").and_then(|value| value.as_str()),
            Some("cteno-test")
        );
        assert_eq!(body.as_object().map(|object| object.len()), Some(2));
        assert!(body.get("agentState").is_none());

        let metadata: Value = serde_json::from_str(
            body.get("metadata")
                .and_then(|value| value.as_str())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(metadata.as_object().map(|object| object.len()), Some(3));
        assert_eq!(
            metadata,
            json!({
                "title": "workspace-a",
                "flavor": "codex",
                "workdir": "/tmp/workspace-a",
            })
        );
    }

    #[test]
    fn session_index_metadata_uses_directory_when_no_filename_exists() {
        let metadata = build_session_index_metadata("/", "cteno");

        assert_eq!(
            metadata,
            json!({
                "title": "/",
                "flavor": "cteno",
                "workdir": "/",
            })
        );
    }
}

// ---------------------------------------------------------------------------
// Server session creation
// ---------------------------------------------------------------------------

fn session_title_from_directory(directory: &str) -> String {
    Path::new(directory)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(directory)
        .to_string()
}

fn build_session_index_metadata(directory: &str, agent_flavor: &str) -> Value {
    json!({
        "title": session_title_from_directory(directory),
        "flavor": agent_flavor,
        "workdir": directory,
    })
}

fn build_create_session_request_body(
    directory: &str,
    agent_flavor: &str,
    tag: &str,
) -> Result<Value, String> {
    let metadata = build_session_index_metadata(directory, agent_flavor);
    let metadata_string = serde_json::to_string(&metadata)
        .map_err(|e| format!("Failed to serialize metadata: {}", e))?;

    Ok(json!({
        "tag": tag,
        "metadata": metadata_string,
    }))
}

/// Create a session on Happy Server via POST /v1/sessions
///
/// This is called by the spawn-happy-session RPC handler.
/// It creates encrypted session metadata and sends it to the server.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_session_on_server(
    server_url: &str,
    auth_token: &str,
    encryption_key: &[u8; 32],
    encryption_variant: EncryptionVariant,
    data_key_public: Option<[u8; 32]>,
    machine_id: &str,
    directory: &str,
    agent_flavor: &str,
    profile_id: &str,
    permission_mode: Option<super::permission::PermissionMode>,
) -> Result<String, String> {
    // Session creation is relay-index only: the daemon sends a lightweight
    // metadata blob for list/reconnect purposes and never uploads agent state
    // or messages through this HTTP path. Legacy crypto params stay in the
    // signature for call-site compatibility and are ignored here.
    let _ = (
        encryption_key,
        encryption_variant,
        data_key_public,
        machine_id,
        profile_id,
        permission_mode,
    );

    let tag = format!("cteno-{}", uuid::Uuid::new_v4());
    let body = build_create_session_request_body(directory, agent_flavor, &tag)?;

    let url = format!("{}/v1/sessions", server_url);
    let http_client = if server_url.contains("127.0.0.1") || server_url.contains("localhost") {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|e| format!("Failed to build loopback HTTP client: {}", e))?
    } else {
        reqwest::Client::new()
    };

    log::info!("Creating session: POST {} tag={}", url, tag);

    let response = http_client
        .post(&url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Session creation failed: {} - {}", status, body));
    }

    // Parse response to get session ID
    let response_json: Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    let session_id = response_json
        .get("session")
        .and_then(|s| s.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| "No session ID in response".to_string())?
        .to_string();

    log::info!("Session created on server: {}", session_id);

    Ok(session_id)
}
