use super::{
    register_connection_local_rpcs, SessionAgentConfig, SessionConnection, SessionRegistry,
};
use crate::agent_session::{AgentSession, AgentSessionManager};
use crate::executor_normalizer::user_visible_executor_error;
use crate::happy_client::permission::PermissionMode;
use cteno_host_rpc_core::RpcRegistry;
use cteno_host_runtime::session_sync_service;
use multi_agent_runtime_core::{
    AgentExecutor, ModelSpec, NativeSessionId, PermissionMode as CorePermissionMode, ResumeHints,
    SessionRef, SpawnSessionSpec,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Default vendor used when none is specified in agent_flavor or
/// `SpawnSessionConfig`. `cteno` always resolves because the `cteno-agent`
/// sidecar binary ships with every desktop build.
const DEFAULT_EXECUTOR_VENDOR: &str = "cteno";

/// Map host-side [`PermissionMode`] (4-variant, matches what the Happy Server
/// permission handler uses) into runtime-core's 7-variant
/// [`CorePermissionMode`] expected by `SpawnSessionSpec`.
fn host_to_core_permission_mode(mode: PermissionMode) -> CorePermissionMode {
    match mode {
        PermissionMode::Default => CorePermissionMode::Default,
        PermissionMode::AcceptEdits => CorePermissionMode::AcceptEdits,
        PermissionMode::BypassPermissions => CorePermissionMode::BypassPermissions,
        PermissionMode::Plan => CorePermissionMode::Plan,
    }
}

fn executor_vendor(agent_flavor: &str) -> &str {
    let flavor = agent_flavor.trim();
    if flavor.is_empty() || flavor == "persona" {
        DEFAULT_EXECUTOR_VENDOR
    } else {
        flavor
    }
}

async fn is_vendor_native_model_id(vendor: &str, model_id: &str) -> bool {
    crate::commands::collect_vendor_models(vendor)
        .await
        .map(|models| models.into_iter().any(|model| model.id == model_id))
        .unwrap_or(false)
}

async fn default_vendor_model_id(vendor: &str) -> Option<String> {
    crate::commands::collect_vendor_models(vendor)
        .await
        .ok()
        .and_then(|models| {
            models
                .iter()
                .find(|model| model.is_default)
                .map(|model| model.id.clone())
                .or_else(|| models.first().map(|model| model.id.clone()))
        })
}

/// Resolve an `AgentExecutor` and spawn a new vendor session for this Happy
/// session.
async fn try_spawn_executor_session(
    config: &SpawnSessionConfig,
    session_id: &str,
    workdir: &str,
    system_prompt: Option<String>,
    profile_id: &str,
    reasoning_effort: Option<&str>,
    permission_mode: Option<PermissionMode>,
    vendor: &str,
) -> Result<(Arc<dyn AgentExecutor>, SessionRef), String> {
    let registry = crate::local_services::executor_registry()
        .map_err(|e| format!("Executor registry unavailable: {e}"))?;

    let executor = registry
        .resolve(vendor)
        .map_err(|e| format!("Executor vendor '{vendor}' unavailable: {e}"))?;

    // Canonicalize the vendor name into the registry's &'static key so we can
    // reach the connection cache. Unknown vendors fall through to the legacy
    // spawn_session path without registry interception.
    let vendor_key: Option<&'static str> = match vendor {
        "cteno" => Some("cteno"),
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "gemini" => Some("gemini"),
        _ => None,
    };

    let core_mode = permission_mode
        .map(host_to_core_permission_mode)
        .unwrap_or(CorePermissionMode::Default);

    let agent_config = build_executor_agent_config(profile_id);
    let model = resolve_spawn_model(config, vendor, profile_id, reasoning_effort).await;
    let spec = SpawnSessionSpec {
        workdir: PathBuf::from(workdir),
        system_prompt,
        model,
        permission_mode: core_mode,
        allowed_tools: None,
        additional_directories: Vec::new(),
        env: Default::default(),
        agent_config,
        resume_hint: None,
    };

    // Prefer routing through the registry's cached connection so
    // multi-session vendors (cteno / codex / gemini) attach to a pre-warmed
    // handle. Claude (1:1 conn:session) falls through `start_session_on`'s
    // default impl, which calls `spawn_session` — behavior unchanged.
    //
    // `start_session_with_autoreopen` already health-checks the cached handle
    // and, on a "connection is closed"-class error from `start_session_on`,
    // drops the slot and retries once with a freshly dialed subprocess. This
    // is the armor against stale preheat handles whose child died during a
    // long idle window.
    let spawn_result = if let Some(vendor_key) = vendor_key {
        match registry
            .start_session_with_autoreopen(vendor_key, spec.clone())
            .await
        {
            Ok(session) => Ok(session),
            Err(err) => {
                log::warn!(
                    "[Session {}] start_session_with_autoreopen({}) failed: {} — falling back to spawn_session",
                    session_id,
                    vendor,
                    err
                );
                executor.spawn_session(spec).await
            }
        }
    } else {
        executor.spawn_session(spec).await
    };

    match spawn_result {
        Ok(session_ref) => {
            log::info!(
                "[Session {}] Executor session spawned (vendor={}, native_id={})",
                session_id,
                session_ref.vendor,
                session_ref.id
            );
            Ok((executor, session_ref))
        }
        Err(error) => Err(format!(
            "executor.spawn_session({vendor}) failed: {}",
            user_visible_executor_error(&error)
        )),
    }
}

fn build_executor_agent_config(profile_id: &str) -> serde_json::Value {
    let mut agent_config = serde_json::json!({});
    if let Some(profile_id) = Some(profile_id.trim()).filter(|value| !value.is_empty()) {
        agent_config["profile_id"] = serde_json::Value::String(profile_id.to_string());
    }
    crate::executor_session::merge_auth_into(&mut agent_config);
    agent_config
}

async fn resolve_spawn_model(
    config: &SpawnSessionConfig,
    vendor: &str,
    profile_id: &str,
    reasoning_effort: Option<&str>,
) -> Option<ModelSpec> {
    let reasoning_effort = reasoning_effort
        .map(str::trim)
        .filter(|value| matches!(*value, "low" | "medium" | "high" | "xhigh" | "max"))
        .map(ToOwned::to_owned);

    let provider = match vendor {
        "claude" => Some("anthropic"),
        "codex" => Some("openai"),
        "gemini" => Some("gemini"),
        _ => None,
    };

    if let Some(provider) = provider {
        if is_vendor_native_model_id(vendor, profile_id).await {
            log::info!(
                "[spawn] Preserving vendor-native model '{}' for vendor={} ahead of profile-store resolution",
                profile_id,
                vendor
            );
            return Some(ModelSpec {
                provider: provider.to_string(),
                model_id: profile_id.to_string(),
                reasoning_effort,
                temperature: None,
            });
        }
    }

    let profile_store = config.agent_config.profile_store.read().await;
    let proxy_profiles = config.agent_config.proxy_profiles.read().await;
    if let Some(profile) = profile_store.get_profile_or_proxy(profile_id, &proxy_profiles) {
        let provider = match profile.api_format {
            crate::llm_profile::ApiFormat::Anthropic => "anthropic",
            crate::llm_profile::ApiFormat::OpenAI => "openai",
            crate::llm_profile::ApiFormat::Gemini => "gemini",
        };

        return Some(ModelSpec {
            provider: provider.to_string(),
            model_id: profile.chat.model,
            reasoning_effort,
            temperature: None,
        });
    }

    let provider = provider?;

    if let Some(model_id) = default_vendor_model_id(vendor).await {
        log::warn!(
            "[spawn] Requested model/profile '{}' is not available for vendor={}; falling back to '{}'",
            profile_id,
            vendor,
            model_id
        );
        return Some(ModelSpec {
            provider: provider.to_string(),
            model_id,
            reasoning_effort,
            temperature: None,
        });
    }

    Some(ModelSpec {
        provider: provider.to_string(),
        model_id: profile_id.to_string(),
        reasoning_effort,
        temperature: None,
    })
}

pub(crate) fn persist_executor_session_metadata(
    db_path: &std::path::Path,
    happy_session_id: &str,
    workdir: &str,
    profile_id: &str,
    session_ref: &SessionRef,
) -> Result<(), String> {
    crate::happy_client::session_helpers::upsert_agent_session_workdir_profile_and_vendor(
        db_path,
        happy_session_id,
        workdir,
        Some(profile_id),
        &session_ref.vendor,
    )?;
    crate::happy_client::session_helpers::upsert_agent_session_native_session_id(
        db_path,
        happy_session_id,
        &session_ref.vendor,
        session_ref.id.as_str(),
    )?;
    Ok(())
}

/// Configuration needed to spawn a Happy Session.
///
/// Stored in `local_services` so that `PersonaManager::dispatch_task()` and the
/// Scheduler can create sessions without going through the RPC handler.
#[derive(Clone)]
pub struct SpawnSessionConfig {
    pub machine_id: String,
    pub rpc_registry: Arc<RpcRegistry>,
    pub session_connections: SessionRegistry,
    pub agent_config: SessionAgentConfig,
    pub db_path: PathBuf,
}

fn permission_mode_context_value(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => "default",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::BypassPermissions => "bypassPermissions",
        PermissionMode::Plan => "plan",
    }
}

fn persist_spawned_session_context(
    config: &SpawnSessionConfig,
    session_id: &str,
    directory: &str,
    agent_flavor: &str,
    profile_id: &str,
    inherit_permission_mode: Option<PermissionMode>,
) -> Result<(), String> {
    let manager = AgentSessionManager::new(config.db_path.clone());
    if let Err(e) = manager.create_session_with_id(session_id, agent_flavor, None, None) {
        if !e.contains("UNIQUE constraint failed") {
            return Err(e);
        }
    }

    let mut context = json!({
        "workdir": directory,
        "profile_id": profile_id,
        "flavor": agent_flavor,
    });
    if let Some(mode) = inherit_permission_mode {
        context["permissionMode"] =
            serde_json::Value::String(permission_mode_context_value(mode).to_string());
    }
    manager.update_context_data(session_id, &context)
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

fn restored_workdir(session: &AgentSession) -> Option<PathBuf> {
    session
        .context_data
        .as_ref()
        .and_then(|context| context.get("workdir"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn is_missing_claude_conversation_error(vendor: &str, error: &str) -> bool {
    vendor == "claude" && error.contains("No conversation found with session ID")
}

async fn try_resume_executor_session(
    config: &SpawnSessionConfig,
    happy_session_id: &str,
    agent_session: &AgentSession,
    profile_id: &str,
    system_prompt: Option<String>,
    permission_mode: Option<PermissionMode>,
) -> Result<(Arc<dyn AgentExecutor>, SessionRef, PathBuf), String> {
    let native_session_id = restored_native_session_id(agent_session).ok_or_else(|| {
        format!("Missing persisted native_session_id for session {happy_session_id}")
    })?;
    let session_store = crate::session_store_impl::build_session_store(config.db_path.clone());
    let session_info = session_store
        .get_session_info(
            &agent_session.vendor,
            &NativeSessionId::new(native_session_id.clone()),
        )
        .await
        .map_err(|e| {
            format!(
                "Persisted executor session {} unavailable for session {}: {}",
                native_session_id, happy_session_id, e
            )
        })?;
    let workdir = session_info.meta.workdir;
    let registry = crate::local_services::executor_registry()
        .map_err(|e| format!("Executor registry unavailable: {e}"))?;
    let executor = registry.resolve(&agent_session.vendor).map_err(|e| {
        format!(
            "Executor vendor '{}' unavailable for session {}: {}",
            agent_session.vendor, happy_session_id, e
        )
    })?;

    let mut metadata = BTreeMap::new();
    metadata.insert("happy_session_id".to_string(), happy_session_id.to_string());
    metadata.insert("agent_id".to_string(), agent_session.agent_id.clone());
    let hints = ResumeHints {
        vendor_cursor: Some(native_session_id.clone()),
        workdir: Some(workdir.clone()),
        metadata,
    };

    let session_ref = if agent_session.vendor == "cteno" {
        let core_mode = permission_mode
            .map(host_to_core_permission_mode)
            .unwrap_or(CorePermissionMode::Default);
        let spec = SpawnSessionSpec {
            workdir: workdir.clone(),
            system_prompt,
            model: resolve_spawn_model(config, &agent_session.vendor, profile_id, None).await,
            permission_mode: core_mode,
            allowed_tools: None,
            additional_directories: Vec::new(),
            env: Default::default(),
            agent_config: build_executor_agent_config(profile_id),
            resume_hint: Some(hints),
        };

        log::info!(
            "[Session {}] Resuming Cteno executor with profile_id='{}' via resume SpawnSessionSpec",
            happy_session_id,
            profile_id
        );

        match registry
            .start_session_with_autoreopen("cteno", spec.clone())
            .await
        {
            Ok(session_ref) => session_ref,
            Err(err) => {
                log::warn!(
                    "[Session {}] start_session_with_autoreopen(cteno resume) failed: {} — falling back to spawn_session",
                    happy_session_id,
                    err
                );
                executor.spawn_session(spec).await.map_err(|e| {
                    format!(
                        "executor.spawn_session({}) resume failed for session {}: {}",
                        agent_session.vendor,
                        happy_session_id,
                        user_visible_executor_error(&e)
                    )
                })?
            }
        }
    } else {
        executor
            .resume_session(NativeSessionId::new(native_session_id), hints)
            .await
            .map_err(|e| {
                format!(
                    "executor.resume_session({}) failed for session {}: {}",
                    agent_session.vendor, happy_session_id, e
                )
            })?
    };

    log::info!(
        "[Session {}] Executor session resumed (vendor={}, native_id={})",
        happy_session_id,
        session_ref.vendor,
        session_ref.id
    );

    Ok((executor, session_ref, workdir))
}

/// Create a Happy Session, establish a local session connection, and
/// optionally send an initial user message (for dispatch_task).
///
/// This is the shared core logic used by both the `spawn-happy-session` RPC
/// handler and `PersonaManager::dispatch_task()`.
///
/// Pipeline:
/// 1. Generate the local Happy session id and resolve the workdir.
/// 2. Prepare the local session agent config.
/// 3. Spawn the executor-backed vendor session.
/// 4. Persist local session metadata and establish the local connection.
pub async fn spawn_session_internal(
    config: &SpawnSessionConfig,
    directory: &str,
    agent_flavor: &str,
    profile_id: &str,
    initial_message: Option<&str>,
    inherit_permission_mode: Option<PermissionMode>,
    reasoning_effort: Option<&str>,
    skill_ids: Option<Vec<String>>,
) -> Result<String, String> {
    let directory = ensure_spawn_directory(directory)?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let agent_config = prepare_spawn_agent_config(config, profile_id, skill_ids).await;
    let (executor, session_ref) = try_spawn_executor_session(
        config,
        &session_id,
        &directory,
        Some(agent_config.system_prompt.clone()),
        profile_id,
        reasoning_effort,
        inherit_permission_mode,
        executor_vendor(agent_flavor),
    )
    .await?;

    persist_spawned_session_context(
        config,
        &session_id,
        &directory,
        agent_flavor,
        profile_id,
        inherit_permission_mode,
    )?;
    persist_executor_session_metadata(
        &config.db_path,
        &session_id,
        &directory,
        profile_id,
        &session_ref,
    )?;

    let mut conn = SessionConnection::establish_local_connection(
        session_id.clone(),
        agent_config.clone(),
        config.session_connections.clone(),
    )
    .await?;
    conn.executor = Some(executor);
    conn.session_ref = Some(session_ref.clone());

    register_connection_local_rpcs(&config.rpc_registry, &conn, PathBuf::from(&directory)).await;

    if let Some(mode) = inherit_permission_mode {
        conn.set_permission_mode(mode);
    }

    config
        .session_connections
        .insert(session_id.clone(), conn)
        .await;
    // Cloud sync is optional best-effort work. Do not let a slow or
    // unavailable server delay local session creation.
    let sync_service = session_sync_service();
    let sync_session_id = session_id.clone();
    let sync_workdir = PathBuf::from(&directory);
    let sync_vendor = session_ref.vendor.to_string();
    tokio::spawn(async move {
        let sync_result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            sync_service.on_session_created(&sync_session_id, &sync_workdir, &sync_vendor),
        )
        .await;
        if sync_result.is_err() {
            log::warn!(
                "Best-effort session sync create timed out for {} after 10s",
                sync_session_id
            );
        }
    });

    if let Some(msg) = initial_message {
        if let Some(conn) = config.session_connections.get(&session_id).await {
            conn.send_initial_user_message(msg).await?;
        }
    }

    log::info!(
        "spawn_session_internal: Session fully initialized: {}",
        session_id
    );

    Ok(session_id)
}

/// Clone the base agent config and apply spawn-specific profile/skill state.
pub(super) async fn prepare_spawn_agent_config(
    config: &SpawnSessionConfig,
    profile_id: &str,
    skill_ids: Option<Vec<String>>,
) -> SessionAgentConfig {
    let mut agent_config = config.agent_config.clone();
    *agent_config.profile_id.write().await = profile_id.to_string();
    if let Some(sids) = skill_ids.filter(|sids| !sids.is_empty()) {
        agent_config.pre_activated_skill_ids = Some(sids);
    }
    agent_config
}

fn restored_profile_id(session: &AgentSession) -> Option<String> {
    session
        .context_data
        .as_ref()
        .and_then(|context| context.get("profile_id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn restored_permission_mode(session: &AgentSession) -> Option<PermissionMode> {
    session
        .context_data
        .as_ref()
        .and_then(|context| context.get("permissionMode"))
        .and_then(|value| value.as_str())
        .and_then(crate::happy_client::permission::PermissionHandler::parse_mode)
}

/// Recreate a local session connection from persisted session metadata and
/// resume the executor-backed vendor session.
pub async fn resume_session_connection(
    config: &SpawnSessionConfig,
    session_id: &str,
    requested_profile_id: Option<&str>,
) -> Result<(), String> {
    log::info!(
        "[Session {}] Resuming executor-backed session...",
        session_id
    );
    if let Some(existing) = config.session_connections.remove(session_id).await {
        existing.disconnect().await;
    }

    let agent_session = AgentSessionManager::new(config.db_path.clone())
        .get_session(session_id)?
        .ok_or_else(|| format!("Missing persisted session row for {}", session_id))?;
    let default_profile_id = config.agent_config.profile_id.read().await.clone();
    let profile_id = match requested_profile_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(profile_id) => profile_id.to_string(),
        None => restored_profile_id(&agent_session).unwrap_or(default_profile_id),
    };
    let agent_config = prepare_spawn_agent_config(config, &profile_id, None).await;
    let permission_mode = restored_permission_mode(&agent_session);
    let (executor, session_ref, workdir) = match try_resume_executor_session(
        config,
        session_id,
        &agent_session,
        &profile_id,
        Some(agent_config.system_prompt.clone()),
        permission_mode,
    )
    .await
    {
        Ok(resumed) => resumed,
        Err(error)
            if is_missing_claude_conversation_error(&agent_session.vendor, &error)
                && !super::session_has_real_user_messages(&config.db_path, session_id) =>
        {
            let workdir =
                restored_workdir(&agent_session).unwrap_or_else(|| default_user_home_dir());
            let workdir_str = ensure_spawn_directory(&workdir.to_string_lossy())?;
            log::warn!(
                    "[Session {}] Claude native session missing and local session has no user messages; spawning a fresh native session (old error: {})",
                    session_id,
                    error
                );
            let (executor, session_ref) = try_spawn_executor_session(
                config,
                session_id,
                &workdir_str,
                Some(agent_config.system_prompt.clone()),
                &profile_id,
                None,
                permission_mode,
                &agent_session.vendor,
            )
            .await?;
            persist_executor_session_metadata(
                &config.db_path,
                session_id,
                &workdir_str,
                &profile_id,
                &session_ref,
            )?;
            (executor, session_ref, PathBuf::from(workdir_str))
        }
        Err(error) => return Err(error),
    };

    let mut conn = SessionConnection::establish_local_connection(
        session_id.to_string(),
        agent_config,
        config.session_connections.clone(),
    )
    .await?;
    conn.executor = Some(executor);
    conn.session_ref = Some(session_ref);

    register_connection_local_rpcs(&config.rpc_registry, &conn, workdir).await;

    if let Some(mode) = permission_mode {
        conn.set_permission_mode(mode);
    }

    config
        .session_connections
        .insert(session_id.to_string(), conn)
        .await;

    log::info!("[Session {}] Executor session resumed", session_id);
    Ok(())
}

fn default_user_home_dir() -> PathBuf {
    dirs::home_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn ensure_spawn_directory(raw_directory: &str) -> Result<String, String> {
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
        fs::create_dir_all(&resolved)
            .map_err(|e| format!("Failed to create directory '{}': {}", resolved.display(), e))?;
        log::info!(
            "spawn-happy-session: created missing directory {}",
            resolved.display()
        );
    }

    Ok(resolved.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::happy_client::session::build_session_agent_config_template;
    use crate::llm_profile::{self, ApiFormat, LlmEndpoint, LlmProfile, ProfileStore};
    use std::sync::{Arc, Once, OnceLock};
    use tempfile::tempdir;
    use tokio::sync::RwLock;

    static TEST_NO_PROXY: Once = Once::new();
    static TEST_RUNTIME_ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();
    static TEST_EXECUTOR_REGISTRY: Once = Once::new();

    fn install_test_no_proxy() {
        TEST_NO_PROXY.call_once(|| {
            std::env::set_var("NO_PROXY", "*");
            std::env::set_var("no_proxy", "*");
        });
    }

    fn shared_test_data_dir() -> std::path::PathBuf {
        TEST_RUNTIME_ROOT
            .get_or_init(|| {
                let path =
                    std::env::temp_dir().join(format!("cteno-spawn-tests-{}", std::process::id()));
                std::fs::create_dir_all(&path).expect("shared test data dir");
                crate::db::init_at_data_dir(&path).expect("shared test db init");
                path
            })
            .clone()
    }

    fn shared_test_db_path() -> std::path::PathBuf {
        shared_test_data_dir().join("db").join("cteno.db")
    }

    fn install_test_executor_registry() {
        TEST_EXECUTOR_REGISTRY.call_once(|| {
            let agent_binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("debug")
                .join(if cfg!(windows) {
                    "cteno-agent.exe"
                } else {
                    "cteno-agent"
                });
            assert!(
                agent_binary.is_file(),
                "cteno-agent binary missing at {}",
                agent_binary.display()
            );
            std::env::set_var("CTENO_AGENT_PATH", &agent_binary);

            // ExecutorRegistry::build is now async (registers cteno
            // autonomous-turn handler). This test fixture runs inside
            // `Once::call_once` (sync) — spin up a temporary
            // single-threaded Tokio runtime to drive the future without
            // re-entering whatever runtime might host the test caller.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("transient runtime for executor registry build");
            let registry = rt
                .block_on(crate::executor_registry::ExecutorRegistry::build(
                    crate::session_store_impl::build_session_store(shared_test_db_path()),
                ))
                .expect("build executor registry");
            crate::local_services::install_executor_registry(Arc::new(registry));
        });
    }

    fn build_mock_openai_profile(base_url: &str) -> LlmProfile {
        LlmProfile {
            id: "mock-openai".to_string(),
            name: "Mock OpenAI".to_string(),
            chat: LlmEndpoint {
                api_key: "test-key".to_string(),
                base_url: base_url.to_string(),
                model: "mock-model".to_string(),
                temperature: 0.0,
                max_tokens: 128,
                context_window_tokens: Some(4096),
            },
            compress: LlmEndpoint {
                api_key: "test-key".to_string(),
                base_url: base_url.to_string(),
                model: "mock-model".to_string(),
                temperature: 0.0,
                max_tokens: 128,
                context_window_tokens: Some(4096),
            },
            supports_vision: false,
            supports_computer_use: false,
            api_format: ApiFormat::OpenAI,
            thinking: false,
            is_free: true,
            supports_function_calling: false,
            supports_image_output: false,
        }
    }

    fn build_mock_spawn_config(
        temp_dir: &tempfile::TempDir,
        profile: LlmProfile,
        server_url: String,
        auth_token: String,
    ) -> (SpawnSessionConfig, SessionRegistry, std::path::PathBuf) {
        install_test_no_proxy();
        install_test_executor_registry();

        let db_path = shared_test_db_path();
        let builtin_root = temp_dir.path().join("builtin");
        let user_root = temp_dir.path().join("user");
        let builtin_skills_dir = builtin_root.join("skills");
        let user_skills_dir = user_root.join("skills");
        std::fs::create_dir_all(&builtin_skills_dir).expect("builtin skills dir");
        std::fs::create_dir_all(&user_skills_dir).expect("user skills dir");

        let profile_store = Arc::new(RwLock::new(ProfileStore {
            profiles: vec![profile.clone()],
            default_profile_id: profile.id.clone(),
        }));
        let proxy_profiles = Arc::new(RwLock::new(Vec::new()));
        let agent_config = build_session_agent_config_template(
            db_path.clone(),
            profile.id.clone(),
            profile_store,
            String::new(),
            "offline-first broader regression".to_string(),
            server_url.clone(),
            auth_token.clone(),
            builtin_skills_dir,
            user_skills_dir,
            proxy_profiles,
        );
        let session_connections = SessionRegistry::new();
        let spawn_config = SpawnSessionConfig {
            machine_id: "test-machine".to_string(),
            rpc_registry: Arc::new(RpcRegistry::new()),
            session_connections: session_connections.clone(),
            agent_config,
            db_path: db_path.clone(),
        };

        (spawn_config, session_connections, db_path)
    }

    #[tokio::test]
    async fn spawn_session_internal_registers_local_session_without_cloud_auth() {
        let temp_dir = tempdir().expect("temp dir");
        install_test_no_proxy();
        install_test_executor_registry();

        let db_path = shared_test_db_path();
        let builtin_root = temp_dir.path().join("builtin");
        let user_root = temp_dir.path().join("user");
        let builtin_skills_dir = builtin_root.join("skills");
        let user_skills_dir = user_root.join("skills");
        std::fs::create_dir_all(&builtin_skills_dir).expect("builtin skills dir");
        std::fs::create_dir_all(&user_skills_dir).expect("user skills dir");

        let profile_store = Arc::new(RwLock::new(ProfileStore {
            profiles: vec![llm_profile::get_default_profile()],
            default_profile_id: llm_profile::DEFAULT_PROXY_PROFILE.to_string(),
        }));
        let proxy_profiles = Arc::new(RwLock::new(Vec::new()));
        let agent_config = build_session_agent_config_template(
            db_path.clone(),
            llm_profile::DEFAULT_PROXY_PROFILE.to_string(),
            profile_store,
            String::new(),
            "offline-first test".to_string(),
            String::new(),
            String::new(),
            builtin_skills_dir,
            user_skills_dir,
            proxy_profiles,
        );
        let session_connections = SessionRegistry::new();
        let spawn_config = SpawnSessionConfig {
            machine_id: "test-machine".to_string(),
            rpc_registry: Arc::new(RpcRegistry::new()),
            session_connections: session_connections.clone(),
            agent_config,
            db_path: db_path.clone(),
        };

        let workdir = temp_dir.path().join("workspace").join("offline-first");
        std::fs::create_dir_all(&workdir).expect("workdir");

        let session_id = spawn_session_internal(
            &spawn_config,
            workdir.to_str().expect("workdir utf8"),
            "cteno",
            llm_profile::DEFAULT_PROXY_PROFILE,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("local session spawn");

        assert!(session_connections.contains_key(&session_id).await);

        let stored = AgentSessionManager::new(db_path)
            .get_session(&session_id)
            .expect("load session")
            .expect("session exists");
        let context = stored.context_data.expect("session context");
        assert_eq!(stored.agent_id, "cteno");
        assert_eq!(context["profile_id"], llm_profile::DEFAULT_PROXY_PROFILE);
        assert_eq!(context["workdir"], workdir.to_string_lossy().to_string());
    }

    #[tokio::test]
    async fn spawn_session_internal_persists_executor_metadata() {
        let temp_dir = tempdir().expect("temp dir");
        let profile = build_mock_openai_profile("mock://offline_local_response");
        let (spawn_config, session_connections, db_path) =
            build_mock_spawn_config(&temp_dir, profile.clone(), String::new(), String::new());

        let workdir = temp_dir.path().join("workspace").join("metadata-persist");
        std::fs::create_dir_all(&workdir).expect("workdir");

        let session_id = spawn_session_internal(
            &spawn_config,
            workdir.to_str().expect("workdir utf8"),
            "cteno",
            &profile.id,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("spawn");

        let conn = session_connections
            .get(&session_id)
            .await
            .expect("session connection");
        assert!(conn.executor.is_some());
        let session_ref = conn.session_ref.as_ref().expect("executor session ref");
        assert_eq!(session_ref.vendor, "cteno");
        assert!(!session_ref.id.as_str().is_empty());

        let stored = AgentSessionManager::new(db_path)
            .get_session(&session_id)
            .expect("load session")
            .expect("session exists");
        let context = stored.context_data.expect("session context");
        assert_eq!(stored.vendor, "cteno");
        assert_eq!(
            context["native_session_id"].as_str(),
            Some(session_ref.id.as_str())
        );
    }

    #[test]
    fn build_executor_agent_config_includes_profile_id() {
        let config = build_executor_agent_config("user-direct");
        assert_eq!(
            config.get("profile_id").and_then(serde_json::Value::as_str),
            Some("user-direct")
        );
    }

    #[test]
    fn build_executor_agent_config_omits_blank_profile_id() {
        let config = build_executor_agent_config("   ");
        assert!(config.get("profile_id").is_none());
    }

    #[test]
    fn detects_missing_claude_conversation_resume_errors() {
        assert!(is_missing_claude_conversation_error(
            "claude",
            "executor.resume_session(claude) failed: vendor error (claude): No conversation found with session ID: native-1"
        ));
        assert!(!is_missing_claude_conversation_error(
            "codex",
            "No conversation found with session ID: native-1"
        ));
        assert!(!is_missing_claude_conversation_error(
            "claude",
            "some unrelated resume failure"
        ));
    }

    #[tokio::test]
    async fn spawn_session_internal_ignores_cloud_sync_credentials_during_local_spawn() {
        let temp_dir = tempdir().expect("temp dir");
        let profile = build_mock_openai_profile("mock://best_effort_local_response");
        let (spawn_config, session_connections, db_path) = build_mock_spawn_config(
            &temp_dir,
            profile.clone(),
            "http://127.0.0.1:1".to_string(),
            "best-effort-token".to_string(),
        );

        let workdir = temp_dir
            .path()
            .join("workspace")
            .join("best-effort-broader");
        std::fs::create_dir_all(&workdir).expect("workdir");

        let session_id = spawn_session_internal(
            &spawn_config,
            workdir.to_str().expect("workdir utf8"),
            "cteno",
            &profile.id,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("local spawn with cloud credentials present");

        assert!(session_connections.contains_key(&session_id).await);
        let stored = AgentSessionManager::new(db_path)
            .get_session(&session_id)
            .expect("load session")
            .expect("session exists");
        assert_eq!(stored.vendor, "cteno");
    }
}
