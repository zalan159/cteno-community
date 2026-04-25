use super::{
    fetch_session_permission_mode_from_kv, HappySocket, SessionAgentConfig, SessionConnection,
    SessionRegistry,
};
use crate::agent_session::{AgentSession, AgentSessionManager};
use crate::happy_client::runtime::{
    install_session_recovery_runtime, reconcile_default_profile_id, RestoreSessionsHook,
    SessionRecoveryRuntimeConfig,
};
use crate::happy_client::socket::{
    ConnectionWatchdog, HeartbeatManager, SessionRecoveryHooks, WatchdogState,
};
use crate::llm_profile::{LlmProfile, ProfileStore};
use crate::session_message_codec::SessionMessageCodec;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use cteno_host_rpc_core::RpcRegistry;
use cteno_host_session_codec::{encrypt_box_for_public_key, EncryptionVariant};
use multi_agent_runtime_core::{AgentExecutor, NativeSessionId, ResumeHints, SessionRef};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

pub(crate) struct SessionMetadata {
    pub workdir: Option<String>,
    pub profile_id: Option<String>,
    pub flavor: Option<String>,
}

pub(crate) struct DesktopSessionRecoveryHooks {
    pub server_url: String,
    pub auth_token: String,
    pub encryption_key: [u8; 32],
    pub encryption_variant: EncryptionVariant,
    pub session_connections: SessionRegistry,
    pub session_agent_config_template: SessionAgentConfig,
}

pub(crate) struct DesktopSessionRecoveryRuntimeConfig {
    pub machine_id: String,
    pub server_url: String,
    pub auth_token: String,
    pub encryption_key: [u8; 32],
    pub encryption_variant: EncryptionVariant,
    pub data_key_public: Option<[u8; 32]>,
    pub db_path: PathBuf,
    pub api_key: String,
    pub system_prompt: String,
    pub builtin_skills_dir: PathBuf,
    pub user_skills_dir: PathBuf,
    pub profile_store: Arc<RwLock<ProfileStore>>,
    pub proxy_profiles: Arc<RwLock<Vec<LlmProfile>>>,
    pub session_connections: SessionRegistry,
    pub machine_socket: Arc<Mutex<Option<Arc<HappySocket>>>>,
    pub rpc_registry: Arc<RpcRegistry>,
    pub heartbeat_manager: Arc<Mutex<Option<HeartbeatManager>>>,
    pub heartbeat_failures: Arc<AtomicU32>,
    pub watchdog_slot: Arc<Mutex<Option<ConnectionWatchdog>>>,
    pub rpc_methods: Vec<String>,
    pub started_at: i64,
    pub daemon_state_version: Arc<AtomicU32>,
}

pub(crate) fn build_session_agent_config_template(
    db_path: PathBuf,
    profile_id: String,
    profile_store: Arc<RwLock<ProfileStore>>,
    global_api_key: String,
    system_prompt: String,
    server_url: String,
    auth_token: String,
    builtin_skills_dir: PathBuf,
    user_skills_dir: PathBuf,
    proxy_profiles: Arc<RwLock<Vec<LlmProfile>>>,
) -> SessionAgentConfig {
    let builtin_agents_dir = builtin_skills_dir
        .parent()
        .map(|p| p.join("agents"))
        .unwrap_or_else(|| builtin_skills_dir.clone());
    let user_agents_dir = user_skills_dir
        .parent()
        .map(|p| p.join("agents"))
        .unwrap_or_else(|| user_skills_dir.clone());

    SessionAgentConfig {
        db_path,
        profile_id: Arc::new(RwLock::new(profile_id)),
        profile_store,
        global_api_key,
        system_prompt,
        skills: vec![],
        server_url,
        auth_token,
        builtin_skills_dir,
        user_skills_dir,
        session_mcp_server_ids: Arc::new(RwLock::new(vec![])),
        builtin_agents_dir,
        user_agents_dir,
        proxy_profiles,
        allowed_tool_ids: None,
        pre_activated_skill_ids: None,
    }
}

pub(crate) fn reconcile_default_profile_store(
    store: &mut ProfileStore,
    proxy_profiles: &[LlmProfile],
    app_data_dir: &Path,
    log_prefix: &str,
) {
    if let Some((old, new)) = reconcile_default_profile_id(
        &store.default_profile_id,
        store.profiles.iter().map(|profile| profile.id.as_str()),
        proxy_profiles.iter().map(|profile| profile.id.as_str()),
        crate::llm_profile::DEFAULT_PROXY_PROFILE,
    ) {
        store.default_profile_id = new.clone();
        log::warn!(
            "{}Default profile '{}' is no longer available; switched to '{}'",
            log_prefix,
            old,
            new
        );
        if let Err(e) = crate::llm_profile::save_profiles(app_data_dir, store) {
            log::warn!(
                "{}Failed to persist reconciled default profile: {}",
                log_prefix,
                e
            );
        }
    }
}

pub(crate) async fn install_desktop_session_recovery(config: DesktopSessionRecoveryRuntimeConfig) {
    let DesktopSessionRecoveryRuntimeConfig {
        machine_id,
        server_url,
        auth_token,
        encryption_key,
        encryption_variant,
        data_key_public,
        db_path,
        api_key,
        system_prompt,
        builtin_skills_dir,
        user_skills_dir,
        profile_store,
        proxy_profiles,
        session_connections,
        machine_socket,
        rpc_registry,
        heartbeat_manager,
        heartbeat_failures,
        watchdog_slot,
        rpc_methods,
        started_at,
        daemon_state_version,
    } = config;

    let default_profile_id = profile_store.read().await.default_profile_id.clone();

    let restore_agent_config = build_session_agent_config_template(
        db_path.clone(),
        default_profile_id.clone(),
        profile_store.clone(),
        api_key.clone(),
        system_prompt.clone(),
        server_url.clone(),
        auth_token.clone(),
        builtin_skills_dir.clone(),
        user_skills_dir.clone(),
        proxy_profiles.clone(),
    );

    let watchdog_agent_config_template = build_session_agent_config_template(
        db_path.clone(),
        default_profile_id,
        profile_store.clone(),
        api_key.clone(),
        system_prompt.clone(),
        server_url.clone(),
        auth_token.clone(),
        builtin_skills_dir.clone(),
        user_skills_dir.clone(),
        proxy_profiles.clone(),
    );

    let restore_server_url = server_url.clone();
    let restore_auth_token = auth_token.clone();
    let restore_session_conns = session_connections.clone();
    let restore_sessions: RestoreSessionsHook = Arc::new(move || {
        let restore_server_url = restore_server_url.clone();
        let restore_auth_token = restore_auth_token.clone();
        let restore_session_conns = restore_session_conns.clone();
        let restore_agent_config = restore_agent_config.clone();
        Box::pin(async move {
            restore_active_sessions(
                &restore_server_url,
                &restore_auth_token,
                encryption_key,
                encryption_variant,
                data_key_public,
                restore_agent_config,
                restore_session_conns,
            )
            .await
        })
    });

    let watchdog_state = Arc::new(WatchdogState {
        machine_id,
        server_url: server_url.clone(),
        auth_token: auth_token.clone(),
        encryption_key,
        encryption_variant,
        rpc_methods,
        started_at,
        daemon_state_version,
        machine_socket,
        rpc_registry,
        heartbeat_manager,
        heartbeat_failures,
        session_hooks: Arc::new(DesktopSessionRecoveryHooks {
            server_url,
            auth_token,
            encryption_key,
            encryption_variant,
            session_connections,
            session_agent_config_template: watchdog_agent_config_template,
        }),
    });

    install_session_recovery_runtime(SessionRecoveryRuntimeConfig {
        restore_sessions,
        watchdog_state,
        watchdog_slot,
    })
    .await;
}

pub(crate) fn decrypt_session_metadata(
    session_val: &Value,
    encryption_key: &[u8; 32],
    encryption_variant: EncryptionVariant,
) -> Option<SessionMetadata> {
    let encrypted_b64 = session_val.get("metadata")?.as_str()?;
    let message_codec = SessionMessageCodec::encrypted(*encryption_key, encryption_variant);
    let metadata = message_codec.decode_metadata_blob(encrypted_b64).ok()?;
    Some(SessionMetadata {
        workdir: metadata
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty()),
        profile_id: metadata
            .get("modelId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty()),
        flavor: metadata
            .get("flavor")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

pub(crate) fn schedule_session_catch_up(
    session_connections: SessionRegistry,
    session_id: String,
    server_url: String,
    auth_token: String,
) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        if let Some(conn) = session_connections.get(&session_id).await {
            conn.catch_up_missed_messages(&server_url, &auth_token)
                .await;
        }
    });
}

const CTENO_VENDOR: &str = "cteno";

fn load_restored_agent_session(db_path: &Path, session_id: &str) -> Option<AgentSession> {
    let manager = AgentSessionManager::new(db_path.to_path_buf());
    match manager.get_session(session_id) {
        Ok(session) => session,
        Err(e) => {
            log::warn!(
                "[Session {}] Failed to load local AgentSession for recovery: {}",
                session_id,
                e
            );
            None
        }
    }
}

fn restored_session_workdir(session: &AgentSession) -> Option<PathBuf> {
    session
        .context_data
        .as_ref()
        .and_then(|ctx| ctx.get("workdir"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
}

fn restored_native_session_id(session: &AgentSession) -> Option<String> {
    session
        .context_data
        .as_ref()
        .and_then(|context| context.get("native_session_id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn build_cteno_resume_request(
    happy_session_id: &str,
    agent_session: &AgentSession,
) -> Option<(String, ResumeHints)> {
    let native_session_id = restored_native_session_id(agent_session)?;

    let mut metadata = BTreeMap::new();
    metadata.insert("happy_session_id".to_string(), happy_session_id.to_string());
    metadata.insert("agent_id".to_string(), agent_session.agent_id.clone());

    Some((
        native_session_id.clone(),
        ResumeHints {
            vendor_cursor: Some(native_session_id),
            workdir: restored_session_workdir(agent_session),
            metadata,
        },
    ))
}

async fn resume_cteno_executor_session(
    executor: Arc<dyn AgentExecutor>,
    happy_session_id: &str,
    agent_session: &AgentSession,
) -> Option<(Arc<dyn AgentExecutor>, SessionRef)> {
    let (native_session_id, hints) = match build_cteno_resume_request(
        happy_session_id,
        agent_session,
    ) {
        Some(request) => request,
        None => {
            log::info!(
                "[Session {}] No persisted native_session_id for Cteno recovery; executor resume unavailable",
                happy_session_id
            );
            return None;
        }
    };

    match executor
        .resume_session(NativeSessionId::new(native_session_id), hints)
        .await
    {
        Ok(session_ref) => {
            log::info!(
                "[Session {}] Executor session resumed (vendor={}, native_id={})",
                happy_session_id,
                session_ref.vendor,
                session_ref.id
            );
            Some((executor, session_ref))
        }
        Err(e) => {
            log::warn!(
                "[Session {}] executor.resume_session({}) failed: {} — executor resume unavailable",
                happy_session_id,
                CTENO_VENDOR,
                e
            );
            None
        }
    }
}

async fn try_resume_executor_session(
    happy_session_id: &str,
    agent_session: &AgentSession,
) -> Option<(Arc<dyn AgentExecutor>, SessionRef)> {
    if !agent_session.vendor.eq_ignore_ascii_case(CTENO_VENDOR) {
        return None;
    }

    let registry = match crate::local_services::executor_registry() {
        Ok(registry) => registry,
        Err(e) => {
            log::info!(
                "[Session {}] Executor registry unavailable ({}), executor resume unavailable",
                happy_session_id,
                e
            );
            return None;
        }
    };

    let executor = match registry.resolve(CTENO_VENDOR) {
        Ok(executor) => executor,
        Err(e) => {
            log::info!(
                "[Session {}] Executor vendor '{}' unavailable ({}), executor resume unavailable",
                happy_session_id,
                CTENO_VENDOR,
                e
            );
            return None;
        }
    };

    resume_cteno_executor_session(executor, happy_session_id, agent_session).await
}

pub(crate) async fn connect_restored_session(
    server_url: &str,
    auth_token: &str,
    session_id: String,
    encryption_key: [u8; 32],
    encryption_variant: EncryptionVariant,
    agent_config: SessionAgentConfig,
    session_connections: SessionRegistry,
) -> Result<(), String> {
    let restored_permission_mode =
        fetch_session_permission_mode_from_kv(server_url, auth_token, &session_id).await;
    let restored_agent_session = load_restored_agent_session(&agent_config.db_path, &session_id);
    let message_codec =
        SessionMessageCodec::for_session_messages(encryption_key, encryption_variant);

    let mut conn = SessionConnection::establish_remote_connection(
        server_url,
        auth_token,
        session_id.clone(),
        message_codec,
        agent_config.clone(),
        session_connections.clone(),
    )
    .await?;

    if let Some(agent_session) = restored_agent_session.as_ref() {
        if let Some((executor, session_ref)) =
            try_resume_executor_session(&session_id, agent_session).await
        {
            conn.executor = Some(executor);
            conn.session_ref = Some(session_ref);
        }
    }

    conn.start_remote_runtime(agent_config).await;

    if let Some(mode) = restored_permission_mode {
        conn.set_permission_mode(mode);
    }

    session_connections.insert(session_id.clone(), conn).await;
    schedule_session_catch_up(
        session_connections,
        session_id,
        server_url.to_string(),
        auth_token.to_string(),
    );

    Ok(())
}

pub(crate) async fn restore_active_sessions(
    server_url: &str,
    auth_token: &str,
    encryption_key: [u8; 32],
    encryption_variant: EncryptionVariant,
    data_key_public: Option<[u8; 32]>,
    agent_config: SessionAgentConfig,
    session_connections: SessionRegistry,
) -> Result<(), String> {
    log::info!("Restoring active sessions from server...");

    let client = reqwest::Client::new();
    let url = format!("{}/v2/sessions/active", server_url);

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch active sessions: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Failed to fetch active sessions: {}",
            response.status()
        ));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse sessions response: {}", e))?;

    let sessions = body
        .get("sessions")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();

    log::info!("Found {} active sessions to restore", sessions.len());

    for session_val in &sessions {
        let session_id = match session_val.get("id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };

        let metadata = decrypt_session_metadata(session_val, &encryption_key, encryption_variant);

        if let Some(ref meta) = metadata {
            if let Some(ref workdir) = meta.workdir {
                if let Err(e) = crate::happy_client::session_helpers::upsert_agent_session_workdir(
                    &agent_config.db_path,
                    &session_id,
                    workdir,
                ) {
                    log::warn!(
                        "Failed to restore local workdir for session {}: {}",
                        session_id,
                        e
                    );
                }
            }
        }

        if session_connections.contains_key(&session_id).await {
            log::debug!("Session {} already connected, skipping", session_id);
            continue;
        }

        let mut session_config = agent_config.clone();

        let local_profile_id = {
            let manager =
                crate::agent_session::AgentSessionManager::new(agent_config.db_path.to_path_buf());
            manager
                .get_session(&session_id)
                .ok()
                .flatten()
                .and_then(|s| s.context_data)
                .and_then(|ctx| {
                    ctx.get("profile_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
                .filter(|s| !s.trim().is_empty())
        };

        if let Some(ref local_pid) = local_profile_id {
            log::info!(
                "Restoring session {} with profile '{}' (from local DB)",
                session_id,
                local_pid
            );
            session_config.profile_id = Arc::new(RwLock::new(local_pid.clone()));
        } else if let Some(ref meta) = metadata {
            if let Some(ref pid) = meta.profile_id {
                log::info!(
                    "Restoring session {} with profile '{}' (from server metadata)",
                    session_id,
                    pid
                );
                session_config.profile_id = Arc::new(RwLock::new(pid.clone()));
            }
        }

        if let Some(ref meta) = metadata {
            if meta.flavor.as_deref() == Some("persona") {
                log::info!(
                    "Restoring persona session {} with full tool access",
                    session_id
                );
            }
        }

        log::info!("Restoring session connection: {}", session_id);

        match connect_restored_session(
            server_url,
            auth_token,
            session_id.clone(),
            encryption_key,
            encryption_variant,
            session_config,
            session_connections.clone(),
        )
        .await
        {
            Ok(()) => {
                log::info!("Session restored: {}", session_id);

                if encryption_variant == EncryptionVariant::DataKey {
                    if let Some(pub_key) = data_key_public {
                        if let Ok(bundle) = encrypt_box_for_public_key(&encryption_key, &pub_key) {
                            let mut versioned = Vec::with_capacity(1 + bundle.len());
                            versioned.push(0u8);
                            versioned.extend_from_slice(&bundle);
                            let bundle_b64 = BASE64.encode(&versioned);
                            let patch_url = format!("{}/v1/sessions/{}", server_url, session_id);
                            let patch_token = auth_token.to_string();
                            tokio::spawn(async move {
                                let client = reqwest::Client::new();
                                if let Err(e) = client
                                    .patch(&patch_url)
                                    .header("Authorization", format!("Bearer {}", patch_token))
                                    .json(&serde_json::json!({
                                        "dataEncryptionKey": bundle_b64,
                                    }))
                                    .send()
                                    .await
                                {
                                    log::warn!(
                                        "Failed to update dataEncryptionKey for session {}: {}",
                                        session_id,
                                        e
                                    );
                                }
                            });
                        }
                    }
                }
            }
            Err(e) => {
                log::warn!("Failed to restore session {}: {}", session_id, e);
            }
        }
    }

    log::info!(
        "Session restoration complete ({} sessions processed)",
        sessions.len()
    );
    Ok(())
}

#[async_trait]
impl SessionRecoveryHooks for DesktopSessionRecoveryHooks {
    async fn reconnect_dead_sessions(&self) -> Result<(), String> {
        let registry_snapshot = self.session_connections.snapshot().await;
        let dead_sessions: Vec<(String, String)> = registry_snapshot
            .into_iter()
            .filter(|(_, conn)| conn.is_dead())
            .map(|(sid, conn)| {
                let profile_id = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(conn.get_profile_id())
                });
                (sid, profile_id)
            })
            .collect();

        if dead_sessions.is_empty() {
            log::info!("🐕 No dead sessions to reconnect");
            return Ok(());
        }

        log::info!(
            "🐕 Found {} dead sessions to reconnect",
            dead_sessions.len()
        );

        for (session_id, profile_id) in dead_sessions {
            log::info!(
                "🐕 Reconnecting session {} (profile={})",
                session_id,
                profile_id
            );

            if let Some(old_conn) = self.session_connections.remove(&session_id).await {
                old_conn.disconnect().await;
            }

            let mut agent_config = self.session_agent_config_template.clone();
            agent_config.profile_id = Arc::new(RwLock::new(profile_id));

            match connect_restored_session(
                &self.server_url,
                &self.auth_token,
                session_id.clone(),
                self.encryption_key,
                self.encryption_variant,
                agent_config,
                self.session_connections.clone(),
            )
            .await
            {
                Ok(()) => {
                    log::info!("🐕 Session {} reconnected", session_id);
                }
                Err(e) => {
                    log::warn!("🐕 Failed to reconnect session {}: {}", session_id, e);
                }
            }
        }

        Ok(())
    }

    async fn discover_new_sessions(&self) -> Result<(), String> {
        log::info!("🐕 Checking for new active sessions on server...");

        let client = reqwest::Client::new();
        let url = format!("{}/v2/sessions/active", self.server_url);

        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .send()
            .await
            .map_err(|e| format!("Failed to fetch active sessions: {}", e))?;

        if !response.status().is_success() {
            return Err(format!(
                "Failed to fetch active sessions: {}",
                response.status()
            ));
        }

        let body: Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse sessions response: {}", e))?;

        let sessions = body
            .get("sessions")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();

        let mut new_count = 0u32;

        for session_val in &sessions {
            let session_id = match session_val.get("id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };

            if self.session_connections.contains_key(&session_id).await {
                continue;
            }

            log::info!("🐕 Discovered new session: {}", session_id);

            let mut agent_config = self.session_agent_config_template.clone();
            let metadata = decrypt_session_metadata(
                session_val,
                &self.encryption_key,
                self.encryption_variant,
            );

            if let Some(ref meta) = metadata {
                if let Some(ref pid) = meta.profile_id {
                    log::info!(
                        "🐕 Restoring discovered session {} with profile '{}'",
                        session_id,
                        pid
                    );
                    agent_config.profile_id = Arc::new(RwLock::new(pid.clone()));
                }
                if meta.flavor.as_deref() == Some("persona") {
                    log::info!(
                        "🐕 Discovered persona session {} — persona gets all tools",
                        session_id
                    );
                }
            }

            match connect_restored_session(
                &self.server_url,
                &self.auth_token,
                session_id.clone(),
                self.encryption_key,
                self.encryption_variant,
                agent_config,
                self.session_connections.clone(),
            )
            .await
            {
                Ok(()) => {
                    new_count += 1;
                    log::info!("🐕 New session {} connected", session_id);
                }
                Err(e) => {
                    log::warn!("🐕 Failed to connect new session {}: {}", session_id, e);
                }
            }
        }

        if new_count > 0 {
            log::info!("🐕 Connected {} new sessions from server", new_count);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_cteno_resume_request, restored_native_session_id, resume_cteno_executor_session,
    };
    use crate::agent_session::{AgentSession, SessionStatus};
    use crate::happy_client::session::spawn::persist_executor_session_metadata;
    use crate::session_store_impl::build_session_store;
    use multi_agent_runtime_core::{AgentExecutor, PermissionMode, SpawnSessionSpec};
    use multi_agent_runtime_cteno::CtenoAgentExecutor;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn test_session(context_data: serde_json::Value) -> AgentSession {
        AgentSession {
            id: "happy-session-1".to_string(),
            agent_id: "worker".to_string(),
            user_id: None,
            messages: vec![],
            context_data: Some(context_data),
            agent_state: None,
            agent_state_version: 0,
            status: SessionStatus::Active,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            expires_at: None,
            owner_session_id: None,
            vendor: "cteno".to_string(),
        }
    }

    #[test]
    fn restored_native_session_id_reads_stable_key_only() {
        assert_eq!(
            restored_native_session_id(&test_session(json!({
                "native_session_id": "native-123",
                "resume_session_id": "legacy-ignored",
            }))),
            Some("native-123".to_string())
        );
        assert_eq!(
            restored_native_session_id(&test_session(json!({
                "resume_session_id": "legacy-only",
            }))),
            None
        );
    }

    #[test]
    fn build_cteno_resume_request_carries_native_id_and_resume_hints() {
        let session = test_session(json!({
            "native_session_id": "native-123",
            "workdir": "/tmp/workspace-a",
        }));

        let (native_session_id, hints) =
            build_cteno_resume_request("happy-session-1", &session).unwrap();

        assert_eq!(native_session_id, "native-123");
        assert_eq!(hints.vendor_cursor.as_deref(), Some("native-123"));
        assert_eq!(hints.workdir, Some(PathBuf::from("/tmp/workspace-a")));
        assert_eq!(
            hints.metadata.get("happy_session_id").map(String::as_str),
            Some("happy-session-1")
        );
        assert_eq!(
            hints.metadata.get("agent_id").map(String::as_str),
            Some("worker")
        );
    }

    #[tokio::test]
    #[ignore = "manual smoke: requires a built target/debug/cteno-agent sidecar"]
    async fn cteno_restart_smoke_persists_native_id_and_resumes() {
        let temp = tempdir().unwrap();
        let app_data_dir = temp.path().join("app-data");
        std::fs::create_dir_all(&app_data_dir).unwrap();
        crate::db::init_at_data_dir(&app_data_dir).unwrap();

        let db_path = app_data_dir.join("db").join("cteno.db");
        let workdir = temp.path().join("workspace");
        std::fs::create_dir_all(&workdir).unwrap();
        let agent_data_dir = temp.path().join("cteno-agent-data");
        std::fs::create_dir_all(&agent_data_dir).unwrap();

        let cteno_agent_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug")
            .join("cteno-agent");
        assert!(
            cteno_agent_path.is_file(),
            "missing built cteno-agent sidecar at {}",
            cteno_agent_path.display()
        );

        let happy_session_id = "happy-session-smoke";
        let session_store = build_session_store(db_path.clone());
        let spawn_executor: Arc<dyn AgentExecutor> = Arc::new(CtenoAgentExecutor::new(
            cteno_agent_path.clone(),
            session_store.clone(),
        ));
        let spawned = spawn_executor
            .spawn_session(SpawnSessionSpec {
                workdir: workdir.clone(),
                system_prompt: Some("manual restart smoke".to_string()),
                model: None,
                permission_mode: PermissionMode::Default,
                allowed_tools: None,
                additional_directories: Vec::new(),
                env: BTreeMap::from([(
                    "CTENO_AGENT_DATA_DIR".to_string(),
                    agent_data_dir.to_string_lossy().to_string(),
                )]),
                agent_config: json!({}),
                resume_hint: None,
            })
            .await
            .unwrap();

        persist_executor_session_metadata(
            &db_path,
            happy_session_id,
            workdir.to_str().unwrap(),
            "smoke-profile",
            &spawned,
        )
        .unwrap();

        let persisted = crate::agent_session::AgentSessionManager::new(db_path.clone())
            .get_session(happy_session_id)
            .unwrap()
            .unwrap();
        let persisted_native_id = persisted
            .context_data
            .as_ref()
            .and_then(|ctx| ctx.get("native_session_id"))
            .and_then(|value| value.as_str())
            .unwrap()
            .to_string();

        spawn_executor.close_session(&spawned).await.unwrap();

        let resume_executor: Arc<dyn AgentExecutor> =
            Arc::new(CtenoAgentExecutor::new(cteno_agent_path, session_store));
        let (_executor, resumed) =
            resume_cteno_executor_session(resume_executor.clone(), happy_session_id, &persisted)
                .await
                .expect("resume helper should use executor path");

        println!("spawned_native_session_id={}", spawned.id.as_str());
        println!("persisted_native_session_id={}", persisted_native_id);
        println!("resumed_native_session_id={}", resumed.id.as_str());

        assert_eq!(persisted_native_id, spawned.id.as_str());
        assert_eq!(resumed.id.as_str(), persisted_native_id);

        resume_executor.close_session(&resumed).await.unwrap();
    }
}
