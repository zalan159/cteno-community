//! Happy Client Manager
//!
//! Manages the lifecycle of Happy Server connections for Cteno Desktop.
//! Persists machine_key across restarts (server only stores dataEncryptionKey on first registration).

use super::profile_rpc::build_profile_rpc_hooks;
use super::session_helpers::ensure_spawn_directory;
use super::*;
use crate::agent_rpc_handler::AgentRpcConfig;
use crate::agent_session::AgentSessionManager;
use crate::auth_store_boot::load_persisted_machine_auth;
use crate::happy_client::runtime::{
    register_mcp_rpc_handlers, register_profile_rpc_handlers, register_session_bootstrap_handlers,
    register_session_reconnect_handler, register_skill_rpc_handlers, CliRunRequest,
    MachineRpcMethods, McpRpcHooks, SessionBootstrapHooks, SessionReconnectHooks, SkillRpcHooks,
    SpawnSessionRequest,
};
use crate::happy_client::session::{
    build_session_agent_config_template, continuous_browsing_prompt_for_session,
    reconcile_default_profile_store, session_has_real_user_messages,
    spawn_queued_worker_for_session_if_idle, SessionConnection,
};
use crate::host::sessions as host_sessions;
use crate::llm_profile::{self, LlmEndpoint, LlmProfile, ProfileStore};
use crate::system_prompt;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use cteno_host_session_registry::{
    BackgroundTaskCategory, BackgroundTaskFilter, BackgroundTaskStatus,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tokio::sync::{Mutex, RwLock};

pub use super::session::{resume_session_connection, spawn_session_internal, SpawnSessionConfig};

fn happy_server_url() -> String {
    crate::resolved_happy_server_url()
}

fn sanitize_skill_id(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_sep = false;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_sep = false;
        } else if (ch == '-' || ch == '_' || ch == ' ' || ch == '.') && !prev_sep {
            out.push('_');
            prev_sep = true;
        }
    }

    out.trim_matches('_').to_string()
}

fn resolve_workspace_agents_dir(params: &Value) -> Option<PathBuf> {
    params
        .get("workdir")
        .and_then(|v| v.as_str())
        .map(|workdir| {
            PathBuf::from(shellexpand::tilde(workdir).to_string())
                .join(".cteno")
                .join("agents")
        })
}

fn parse_background_task_category_param(
    params: &Value,
) -> Result<Option<BackgroundTaskCategory>, &'static str> {
    match params.get("category") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => match value.trim() {
            "execution" => Ok(Some(BackgroundTaskCategory::ExecutionTask)),
            "scheduled" | "scheduled_job" => Ok(Some(BackgroundTaskCategory::ScheduledJob)),
            "background_session" => Ok(Some(BackgroundTaskCategory::BackgroundSession)),
            _ => Err("Invalid category"),
        },
        Some(_) => Err("Invalid category"),
    }
}

fn parse_background_task_status_param(
    params: &Value,
) -> Result<Option<BackgroundTaskStatus>, &'static str> {
    match params.get("status") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => match value.trim() {
            "running" => Ok(Some(BackgroundTaskStatus::Running)),
            "completed" => Ok(Some(BackgroundTaskStatus::Completed)),
            "failed" => Ok(Some(BackgroundTaskStatus::Failed)),
            "cancelled" => Ok(Some(BackgroundTaskStatus::Cancelled)),
            "paused" => Ok(Some(BackgroundTaskStatus::Paused)),
            "unknown" => Ok(Some(BackgroundTaskStatus::Unknown)),
            _ => Err("Invalid status"),
        },
        Some(_) => Err("Invalid status"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MemoryRpcScope {
    Auto,
    Private,
    Global,
}

fn parse_memory_scope_param(params: &Value) -> Result<MemoryRpcScope, String> {
    match params.get("scope").and_then(|value| value.as_str()) {
        None => Ok(MemoryRpcScope::Auto),
        Some(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "" => Ok(MemoryRpcScope::Auto),
            "private" => Ok(MemoryRpcScope::Private),
            "global" => Ok(MemoryRpcScope::Global),
            other => Err(format!("Invalid scope '{other}', expected private/global")),
        },
    }
}

fn parse_memory_persona_id(params: &Value) -> Option<String> {
    params
        .get("persona_id")
        .or_else(|| params.get("personaId"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn resolve_memory_persona_workdir(
    params: &Value,
    required: bool,
) -> Result<Option<String>, String> {
    let Some(persona_id) = parse_memory_persona_id(params) else {
        if required {
            return Err("scope=private requires persona_id".to_string());
        }
        return Ok(None);
    };

    let pm = crate::local_services::persona_manager()
        .map_err(|e| format!("Failed to access persona manager: {e}"))?;
    match pm.store().get_persona(&persona_id) {
        Ok(Some(persona)) => {
            let trimmed = persona.workdir.trim();
            if trimmed.is_empty() {
                if required {
                    Err(format!("Persona {persona_id} has empty workdir"))
                } else {
                    Ok(None)
                }
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Ok(None) => {
            if required {
                Err(format!("Persona not found: {persona_id}"))
            } else {
                Ok(None)
            }
        }
        Err(e) => Err(format!("Failed to load persona {persona_id}: {e}")),
    }
}

fn normalize_memory_rpc_file_path(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("[global]") {
        return rest.trim().to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("[private]") {
        return rest.trim().to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("[private:") {
        if let Some(end_bracket) = rest.find(']') {
            return rest[end_bracket + 1..].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn build_agent_summary(agent: &crate::service_init::AgentConfig) -> Value {
    let agent_type = serde_json::to_value(&agent.agent_type)
        .ok()
        .and_then(|value| value.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "passthrough".to_string());

    json!({
        "id": agent.id,
        "name": agent.name,
        "description": agent.description,
        "version": agent.version,
        "agent_type": agent_type,
        "instructions": agent.instructions.as_deref().unwrap_or(""),
        "model": agent.model.as_deref().unwrap_or(""),
        "temperature": agent.temperature,
        "max_tokens": agent.max_tokens,
        "tools": agent.tools.clone().unwrap_or_default(),
        "skills": agent.skills.clone().unwrap_or_default(),
        "source": agent.source.as_deref().unwrap_or("builtin"),
        "allowed_tools": agent.allowed_tools.clone().unwrap_or_default(),
        "excluded_tools": agent.excluded_tools.clone().unwrap_or_default(),
        "expose_as_tool": agent.expose_as_tool.unwrap_or(false),
    })
}

fn build_skill_rpc_hooks(builtin_skills_dir: PathBuf, user_skills_dir: PathBuf) -> SkillRpcHooks {
    let list_skills_builtin_dir = builtin_skills_dir.clone();
    let list_skills_user_dir = user_skills_dir.clone();
    let create_skill_builtin_dir = builtin_skills_dir.clone();
    let create_skill_user_dir = user_skills_dir.clone();
    let delete_skill_builtin_dir = builtin_skills_dir.clone();
    let delete_skill_user_dir = user_skills_dir.clone();

    SkillRpcHooks {
        list_skills: Arc::new(move |_params: Value| {
            let all_skills = crate::service_init::load_all_skills(
                &list_skills_builtin_dir,
                &list_skills_user_dir,
                None,
            );
            let mut builtin_count = 0usize;
            let mut installed_count = 0usize;
            let skill_items: Vec<Value> = all_skills
                .iter()
                .map(|s| {
                    let source = if list_skills_builtin_dir.join(&s.id).exists() {
                        builtin_count += 1;
                        "builtin"
                    } else {
                        installed_count += 1;
                        "installed"
                    };
                    let display_name = if source != "builtin" {
                        s.path.as_ref().and_then(|p| {
                            let meta_path = p.join(".cteno-source.json");
                            let content = std::fs::read_to_string(&meta_path).ok()?;
                            let meta: serde_json::Value = serde_json::from_str(&content).ok()?;
                            meta.get("displayName")?.as_str().map(|s| s.to_string())
                        })
                    } else {
                        None
                    };
                    let skill_path = s.path.as_ref().map(|p| p.to_string_lossy().to_string());
                    let has_scripts = s
                        .path
                        .as_ref()
                        .map(|p| p.join("scripts").is_dir())
                        .unwrap_or(false);
                    json!({
                        "id": s.id,
                        "name": display_name.as_deref().unwrap_or(&s.name),
                        "description": s.description,
                        "version": s.version,
                        "source": source,
                        "instructions": s.instructions.as_deref().unwrap_or(""),
                        "path": skill_path,
                        "hasScripts": has_scripts,
                    })
                })
                .collect();
            log::info!(
                "list-skills result: total={}, builtin={}, installed={}",
                skill_items.len(),
                builtin_count,
                installed_count
            );
            Ok(json!({ "skills": skill_items }))
        }),
        create_skill: Arc::new(move |params: Value| {
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let description = params
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let instructions = params
                .get("instructions")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    format!("# {}\n\nDescribe how this skill should be used.", name)
                });

            if name.is_empty() || description.is_empty() {
                return Ok(json!({
                    "success": false,
                    "error": "Missing name or description"
                }));
            }

            let skill_id = sanitize_skill_id(&name);
            if skill_id.is_empty() {
                return Ok(json!({
                    "success": false,
                    "error": "Invalid skill name"
                }));
            }

            if create_skill_builtin_dir.join(&skill_id).exists() {
                return Ok(json!({
                    "success": false,
                    "error": "Cannot override built-in skill"
                }));
            }

            if create_skill_user_dir.join(&skill_id).exists() {
                return Ok(json!({
                    "success": false,
                    "error": format!("Skill '{}' already exists", skill_id)
                }));
            }

            let skill_dir = create_skill_user_dir.join(&skill_id);
            if let Err(e) = fs::create_dir_all(&skill_dir) {
                return Ok(json!({
                    "success": false,
                    "error": format!("Failed to create skill directory: {}", e)
                }));
            }

            let skill_md = format!(
                "---\nid: \"{}\"\nname: \"{}\"\ndescription: \"{}\"\nversion: \"1.0.0\"\n---\n\n{}\n",
                skill_id.replace('\"', "\\\""),
                name.replace('\"', "\\\""),
                description.replace('\"', "\\\""),
                instructions
            );

            if let Err(e) = fs::write(skill_dir.join("SKILL.md"), skill_md) {
                let _ = fs::remove_dir_all(&skill_dir);
                return Ok(json!({
                    "success": false,
                    "error": format!("Failed to write SKILL.md: {}", e)
                }));
            }

            tokio::spawn(async {
                crate::agent_sync_bridge::reconcile_global_skills_now().await;
            });
            Ok(json!({ "success": true, "skillId": skill_id }))
        }),
        delete_skill: Arc::new(move |params: Value| {
            let skill_id = params
                .get("skillId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if skill_id.is_empty() {
                return Ok(json!({ "success": false, "error": "Missing skillId" }));
            }

            if delete_skill_builtin_dir.join(&skill_id).exists() {
                return Ok(json!({
                    "success": false,
                    "error": "Cannot delete built-in skill"
                }));
            }

            let skill_dir = delete_skill_user_dir.join(&skill_id);
            if !skill_dir.exists() {
                return Ok(json!({
                    "success": false,
                    "error": "Skill not found"
                }));
            }

            match fs::remove_dir_all(&skill_dir) {
                Ok(()) => {
                    tokio::spawn(async {
                        crate::agent_sync_bridge::reconcile_global_skills_now().await;
                    });
                    Ok(json!({ "success": true }))
                }
                Err(e) => Ok(json!({
                    "success": false,
                    "error": format!("Failed to delete skill: {}", e)
                })),
            }
        }),
        skillhub_featured: Arc::new(move |_params: Value| {
            Box::pin(async move {
                let installed_ids = crate::skillhub::get_installed_skill_ids();
                match crate::skillhub::fetch_featured(&installed_ids).await {
                    Ok(skills) => Ok(json!({ "skills": skills })),
                    Err(e) => Ok(json!({ "skills": [], "error": e })),
                }
            })
        }),
        skillhub_search: Arc::new(move |params: Value| {
            Box::pin(async move {
                let query = params
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(30) as usize;

                let installed_ids = crate::skillhub::get_installed_skill_ids();
                match crate::skillhub::search_skills(&query, limit, &installed_ids).await {
                    Ok(skills) => Ok(json!({ "skills": skills })),
                    Err(e) => Ok(json!({ "skills": [], "error": e })),
                }
            })
        }),
        skillhub_install: Arc::new(move |params: Value| {
            Box::pin(async move {
                let slug = params
                    .get("slug")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let display_name = params
                    .get("displayName")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                if slug.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing slug" }));
                }

                match crate::skillhub::install_skill(&slug, display_name.as_deref()).await {
                    Ok(installed) => {
                        crate::agent_sync_bridge::reconcile_global_skills_now().await;
                        serde_json::to_value(installed)
                            .map_err(|e| format!("Failed to serialize install result: {}", e))
                    }
                    Err(err) => Ok(json!({ "success": false, "error": err })),
                }
            })
        }),
    }
}

fn build_mcp_rpc_hooks() -> McpRpcHooks {
    McpRpcHooks {
        list_mcp: Arc::new(move |_params: Value| {
            Box::pin(async move {
                let mcp_reg = crate::local_services::mcp_registry()
                    .map_err(|e| format!("MCP registry not available: {}", e))?;
                let reg = mcp_reg.read().await;
                let servers = reg.list_servers();
                Ok(json!({ "servers": servers }))
            })
        }),
        add_mcp: Arc::new(move |params: Value| {
            Box::pin(async move {
                let mcp_reg = crate::local_services::mcp_registry()
                    .map_err(|e| format!("MCP registry not available: {}", e))?;
                let config: crate::mcp::MCPServerConfig = serde_json::from_value(params.clone())
                    .map_err(|e| format!("Invalid MCP server config: {}", e))?;
                let server_id = config.id.clone();
                let mut reg = mcp_reg.write().await;
                match reg.add_server(config).await {
                    Ok(_) => Ok(json!({ "success": true, "serverId": server_id })),
                    Err(e) => Ok(json!({ "success": false, "error": e })),
                }
            })
        }),
        remove_mcp: Arc::new(move |params: Value| {
            Box::pin(async move {
                let server_id = params
                    .get("serverId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if server_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing serverId" }));
                }

                let mcp_reg = crate::local_services::mcp_registry()
                    .map_err(|e| format!("MCP registry not available: {}", e))?;
                let mut reg = mcp_reg.write().await;
                match reg.remove_server(&server_id).await {
                    Ok(_) => Ok(json!({ "success": true })),
                    Err(e) => Ok(json!({ "success": false, "error": e })),
                }
            })
        }),
        toggle_mcp: Arc::new(move |params: Value| {
            Box::pin(async move {
                let server_id = params
                    .get("serverId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let enabled = params
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                if server_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing serverId" }));
                }

                let mcp_reg = crate::local_services::mcp_registry()
                    .map_err(|e| format!("MCP registry not available: {}", e))?;
                let mut reg = mcp_reg.write().await;
                match reg.toggle_server(&server_id, enabled).await {
                    Ok(_) => Ok(json!({ "success": true })),
                    Err(e) => Ok(json!({ "success": false, "error": e })),
                }
            })
        }),
    }
}

#[derive(Clone)]
struct MachineUiRpcContext {
    machine_id: String,
    db_path: PathBuf,
    app_data_dir: PathBuf,
    api_key: String,
    system_prompt_text: String,
    builtin_skills_dir: PathBuf,
    user_skills_dir: PathBuf,
    builtin_agents_dir: PathBuf,
    user_agents_dir: PathBuf,
}

async fn spawn_local_persona_session(
    profile_store: Arc<RwLock<ProfileStore>>,
    persona: &crate::persona::Persona,
    agent_type: Option<&str>,
) -> Result<String, String> {
    let spawn_config = crate::local_services::spawn_config()?;
    // Prefer the RPC override when provided, otherwise restore the persona's
    // persisted vendor; legacy personas default to cteno during deserialization.
    let agent_flavor = agent_type.or(persona.agent.as_deref()).unwrap_or("cteno");
    let effective_profile_id =
        resolve_persona_model_selection(profile_store, persona.profile_id.as_deref(), agent_flavor)
            .await;
    let session_id = spawn_session_internal(
        spawn_config.as_ref(),
        &persona.workdir,
        agent_flavor,
        &effective_profile_id,
        None,
        None,
        None,
        None,
    )
    .await?;

    log::info!(
        "[Persona] Local session created: {} (persona: {}, workdir: {})",
        session_id,
        persona.id,
        persona.workdir
    );

    Ok(session_id)
}

async fn resolve_persona_model_selection(
    profile_store: Arc<RwLock<ProfileStore>>,
    requested_profile_id: Option<&str>,
    agent_flavor: &str,
) -> String {
    resolve_persona_model_selection_with_auth_state(
        profile_store,
        requested_profile_id,
        agent_flavor,
        crate::auth_store_boot::current_access_token().is_some(),
    )
    .await
}

async fn resolve_persona_model_selection_with_auth_state(
    profile_store: Arc<RwLock<ProfileStore>>,
    requested_profile_id: Option<&str>,
    agent_flavor: &str,
    has_happy_auth: bool,
) -> String {
    let trimmed_profile_id = requested_profile_id
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if agent_flavor == "cteno" {
        if !has_happy_auth {
            if let Some(profile_id) = trimmed_profile_id {
                if !llm_profile::is_proxy_profile(profile_id) {
                    return profile_id.to_owned();
                }
            }
            let store = profile_store.read().await;
            return llm_profile::direct_fallback_selection(&store).profile_id;
        }

        return trimmed_profile_id
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| llm_profile::DEFAULT_PROXY_PROFILE.to_string());
    }

    let store_default_profile_id = profile_store.read().await.default_profile_id.clone();
    let should_use_vendor_default = match trimmed_profile_id {
        None => true,
        Some(profile_id) => {
            profile_id == llm_profile::DEFAULT_PROXY_PROFILE
                || profile_id == store_default_profile_id
        }
    };

    if should_use_vendor_default {
        if let Ok(models) = crate::commands::collect_vendor_models(agent_flavor).await {
            if let Some(model_id) = models
                .iter()
                .find(|model| model.is_default)
                .map(|model| model.id.clone())
                .or_else(|| models.first().map(|model| model.id.clone()))
            {
                return model_id;
            }
        }

        return match agent_flavor {
            "claude" => "default".to_string(),
            "codex" => "gpt-5.4".to_string(),
            "gemini" => "gemini-2.5-pro".to_string(),
            _ => trimmed_profile_id
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| llm_profile::DEFAULT_PROXY_PROFILE.to_string()),
        };
    }

    trimmed_profile_id
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| llm_profile::DEFAULT_PROXY_PROFILE.to_string())
}

/// Happy Client Manager for Cteno Desktop
pub struct HappyClientManager {
    rpc_registry: Arc<RpcRegistry>,
    session_connections: SessionRegistry,
    profile_store: Arc<RwLock<ProfileStore>>,
    proxy_profiles: Arc<RwLock<Vec<llm_profile::LlmProfile>>>,
    app_data_dir: Arc<Mutex<Option<PathBuf>>>,
}

impl HappyClientManager {
    /// Create a new manager
    pub fn new() -> Self {
        let rpc_registry = Arc::new(RpcRegistry::new());
        crate::local_services::install_rpc_registry(rpc_registry.clone());

        // Placeholder ProfileStore; will be loaded in start_machine_connection
        let default_store = ProfileStore {
            profiles: vec![llm_profile::get_default_profile()],
            default_profile_id: llm_profile::DEFAULT_PROXY_PROFILE.to_string(),
        };

        Self {
            rpc_registry,
            session_connections: SessionRegistry::new(),
            profile_store: Arc::new(RwLock::new(default_store)),
            proxy_profiles: Arc::new(RwLock::new(Vec::new())),
            app_data_dir: Arc::new(Mutex::new(None)),
        }
    }

    /// Get the RPC registry (for local RPC server).
    pub fn rpc_registry(&self) -> Arc<RpcRegistry> {
        self.rpc_registry.clone()
    }

    pub async fn prime_machine_scoped_ui_rpc_handlers(
        &self,
        machine_id: String,
        db_path: PathBuf,
        app_data_dir: PathBuf,
        api_key: String,
        builtin_skills_dir: PathBuf,
        user_skills_dir: PathBuf,
        builtin_agents_dir: PathBuf,
        user_agents_dir: PathBuf,
    ) {
        let prompt_options = system_prompt::PromptOptions::default();
        let system_prompt_text = system_prompt::build_system_prompt(&prompt_options);

        self.register_machine_scoped_ui_rpc_handlers(MachineUiRpcContext {
            machine_id,
            db_path,
            app_data_dir,
            api_key,
            system_prompt_text,
            builtin_skills_dir,
            user_skills_dir,
            builtin_agents_dir,
            user_agents_dir,
        })
        .await;
    }

    /// Get the session connections map (for Tauri local IPC commands).
    pub fn session_connections(&self) -> SessionRegistry {
        self.session_connections.clone()
    }

    pub async fn start_local_machine_runtime(
        &self,
        db_path: String,
        api_key: String,
        app_data_dir: PathBuf,
        builtin_skills_dir: PathBuf,
        user_skills_dir: PathBuf,
        builtin_agents_dir: PathBuf,
        user_agents_dir: PathBuf,
        machine_id: String,
    ) -> Result<(), String> {
        log::info!("Starting local-first community machine runtime...");

        std::env::set_var("CTENO_APP_DATA_DIR", app_data_dir.as_os_str());

        let profiles = llm_profile::load_profiles(&app_data_dir);
        *self.profile_store.write().await = profiles;
        *self.app_data_dir.lock().await = Some(app_data_dir.clone());

        let persisted_machine_auth = match load_persisted_machine_auth(&app_data_dir) {
            Ok(auth) => auth,
            Err(e) => {
                log::warn!(
                    "Community runtime could not reuse cached machine auth: {}; skipping server-backed session upload (/v1/sessions), session-scoped remote sync transport, and cloud permission-mode persistence (/v1/kv) until cloud auth is available",
                    e
                );
                None
            }
        };
        let happy_server_url = happy_server_url();
        if persisted_machine_auth.is_some() {
            let fetched_proxy =
                llm_profile::fetch_proxy_profiles_from_server(&happy_server_url, &app_data_dir)
                    .await;
            *self.proxy_profiles.write().await = fetched_proxy;
            {
                let proxy_profiles = self.proxy_profiles.read().await;
                let mut store = self.profile_store.write().await;
                reconcile_default_profile_store(&mut store, &proxy_profiles, &app_data_dir, "");
            }
        } else {
            log::info!(
                "Community runtime is unauthenticated; skipping proxy profile fetch so local-only startup makes no server calls"
            );
        }

        let db_path_buf = PathBuf::from(&db_path);
        let prompt_options = system_prompt::PromptOptions::default();
        let system_prompt_text = system_prompt::build_system_prompt(&prompt_options);
        let rpc_methods = MachineRpcMethods::new(&machine_id);
        if persisted_machine_auth.is_some() {
            log::info!(
                "Community runtime found cached machine auth; new sessions can attempt best-effort cloud upload without blocking local startup"
            );
        } else {
            log::warn!(
                "Community runtime has no cached machine auth; new sessions will stay local-only and skip server-backed session upload (/v1/sessions), session-scoped remote sync transport, and cloud permission-mode persistence (/v1/kv) until cloud auth is available"
            );
        }
        let (sync_auth_token, sync_server_url) =
            if let Some((token, _, _, _)) = persisted_machine_auth {
                (token.clone(), happy_server_url.clone())
            } else {
                (String::new(), String::new())
            };

        let default_profile_id = self.profile_store.read().await.default_profile_id.clone();
        let default_agent_config = build_session_agent_config_template(
            db_path_buf.clone(),
            default_profile_id,
            self.profile_store.clone(),
            api_key.clone(),
            system_prompt_text.clone(),
            sync_server_url.clone(),
            sync_auth_token.clone(),
            builtin_skills_dir.clone(),
            user_skills_dir.clone(),
            self.proxy_profiles.clone(),
        );

        let spawn_config = Arc::new(SpawnSessionConfig {
            machine_id: machine_id.clone(),
            rpc_registry: self.rpc_registry.clone(),
            session_connections: self.session_connections.clone(),
            agent_config: default_agent_config,
            db_path: db_path_buf.clone(),
        });

        crate::local_services::install_spawn_config(spawn_config.clone());

        let bootstrap_hooks = SessionBootstrapHooks {
            default_profile_id: Arc::new({
                let profile_store = self.profile_store.clone();
                move || {
                    let profile_store = profile_store.clone();
                    Box::pin(async move { profile_store.read().await.default_profile_id.clone() })
                }
            }),
            spawn_session: Arc::new({
                let spawn_config = spawn_config.clone();
                move |request: SpawnSessionRequest| {
                    let spawn_config = spawn_config.clone();
                    Box::pin(async move {
                        crate::happy_client::session::spawn::spawn_session_internal(
                            &spawn_config,
                            &request.directory,
                            &request.agent_flavor,
                            &request.profile_id,
                            None,
                            None,
                            request.reasoning_effort.as_deref(),
                            None,
                        )
                        .await
                    })
                }
            }),
            cli_run: Arc::new(move |_request: CliRunRequest| {
                Box::pin(async move {
                    Err("cli-run-agent requires logged-in Happy Server auth".to_string())
                })
            }),
        };
        register_session_bootstrap_handlers(
            self.rpc_registry.clone(),
            &rpc_methods,
            bootstrap_hooks,
        )
        .await;

        let reconnect_hooks = SessionReconnectHooks {
            reconnect_session: Arc::new({
                let spawn_config = spawn_config.clone();
                move |request| {
                    let spawn_config = spawn_config.clone();
                    Box::pin(async move {
                        if let Some(existing) = spawn_config
                            .session_connections
                            .get(&request.session_id)
                            .await
                        {
                            if !existing.is_dead() {
                                return Ok(json!({ "status": "already_connected" }));
                            }
                        }

                        crate::happy_client::session::spawn::resume_session_connection(
                            &spawn_config,
                            &request.session_id,
                            request.requested_profile_id.as_deref(),
                        )
                        .await?;
                        Ok(json!({ "status": "reconnected" }))
                    })
                }
            }),
        };
        register_session_reconnect_handler(
            self.rpc_registry.clone(),
            &rpc_methods,
            reconnect_hooks,
        )
        .await;

        let profile_hooks = build_profile_rpc_hooks(
            self.session_connections.clone(),
            self.profile_store.clone(),
            self.proxy_profiles.clone(),
            app_data_dir.clone(),
            happy_server_url.clone(),
            api_key.clone(),
        );
        register_profile_rpc_handlers(self.rpc_registry.clone(), &rpc_methods, profile_hooks).await;

        let skill_hooks =
            build_skill_rpc_hooks(builtin_skills_dir.clone(), user_skills_dir.clone());
        register_skill_rpc_handlers(self.rpc_registry.clone(), &rpc_methods, skill_hooks).await;

        let mcp_hooks = build_mcp_rpc_hooks();
        register_mcp_rpc_handlers(self.rpc_registry.clone(), &rpc_methods, mcp_hooks).await;

        let registry = &self.rpc_registry;
        registry
            .register_sync(&rpc_methods.list_sessions, {
                let db_path = db_path_buf.clone();
                let machine_id = machine_id.clone();
                move |_params: Value| {
                    let sessions = host_sessions::list_host_sessions(&db_path, &machine_id)?;
                    Ok(json!({ "success": true, "sessions": sessions }))
                }
            })
            .await;

        registry
            .register_sync(&rpc_methods.get_session, {
                let db_path = db_path_buf.clone();
                let machine_id = machine_id.clone();
                move |params: Value| {
                    let session_id = params
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if session_id.is_empty() {
                        return Ok(json!({ "success": false, "error": "Missing id" }));
                    }
                    match host_sessions::get_host_session(&db_path, &machine_id, &session_id)? {
                        Some(session) => Ok(json!({ "success": true, "session": session })),
                        None => Ok(json!({ "success": false, "error": "Session not found" })),
                    }
                }
            })
            .await;

        registry
            .register_sync(&rpc_methods.get_session_messages, {
                let db_path = db_path_buf.clone();
                move |params: Value| {
                    let session_id = params
                        .get("sessionId")
                        .or_else(|| params.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if session_id.is_empty() {
                        return Ok(json!({ "success": false, "error": "Missing sessionId" }));
                    }
                    match host_sessions::get_host_session_messages(&db_path, &session_id)? {
                        Some(page) => Ok(json!({ "success": true, "messages": page.messages, "hasMore": page.has_more })),
                        None => Ok(json!({ "success": false, "error": "Session not found" })),
                    }
                }
            })
            .await;

        // list_available_vendors — expose ExecutorRegistry vendors, capabilities,
        // and progressive install/auth status to the RN frontend. Uses
        // underscore form to match
        // `apiSocket.machineRPC(machineId, 'list_available_vendors', ...)`.
        let list_available_vendors_method = format!("{}:list_available_vendors", machine_id);
        registry
            .register(
                &list_available_vendors_method,
                move |_params: Value| async move {
                    let vendors = crate::commands::collect_vendor_infos().await?;
                    Ok(json!({ "vendors": vendors }))
                },
            )
            .await;

        // probe_vendor_connection — force-refresh a vendor's live connection
        // probe so the UI can surface up-to-date health without waiting for
        // the next session spawn.
        let probe_vendor_connection_method = format!("{}:probe_vendor_connection", machine_id);
        registry
            .register(
                &probe_vendor_connection_method,
                move |params: Value| async move {
                    let vendor = params
                        .get("vendor")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if vendor.is_empty() {
                        return Ok(json!({
                            "success": false,
                            "error": "Missing vendor",
                        }));
                    }
                    match crate::commands::probe_vendor_connection(vendor.clone()).await {
                        Ok(connection) => Ok(json!({
                            "success": true,
                            "vendor": vendor,
                            "connection": connection,
                        })),
                        Err(err) => Ok(json!({
                            "success": false,
                            "vendor": vendor,
                            "error": err,
                        })),
                    }
                },
            )
            .await;

        let list_vendor_models_method = format!("{}:list-vendor-models", machine_id);
        registry
            .register(
                &list_vendor_models_method,
                move |params: Value| async move {
                    let vendor = params
                        .get("vendor")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if vendor.is_empty() {
                        return Ok(json!({
                            "success": false,
                            "error": "Missing vendor",
                        }));
                    }

                    let models = crate::commands::collect_vendor_models(&vendor).await?;
                    let default_model_id = models
                        .iter()
                        .find(|model| model.is_default)
                        .map(|model| model.id.clone())
                        .or_else(|| models.first().map(|model| model.id.clone()))
                        .unwrap_or_else(|| "default".to_string());

                    Ok(json!({
                        "success": true,
                        "vendor": vendor,
                        "models": models,
                        "defaultModelId": default_model_id,
                    }))
                },
            )
            .await;

        self.register_machine_scoped_ui_rpc_handlers(MachineUiRpcContext {
            machine_id: machine_id.clone(),
            db_path: db_path_buf.clone(),
            app_data_dir: app_data_dir.clone(),
            api_key: api_key.clone(),
            system_prompt_text: system_prompt_text.clone(),
            builtin_skills_dir: builtin_skills_dir.clone(),
            user_skills_dir: user_skills_dir.clone(),
            builtin_agents_dir: builtin_agents_dir.clone(),
            user_agents_dir: user_agents_dir.clone(),
        })
        .await;

        log::info!(
            "Local-first community machine runtime registered for machine {}",
            machine_id
        );

        // Keep this tokio runtime alive so that the local_rpc_server accept
        // loop (spawned earlier via tokio::spawn) stays up. Without this, this
        // function returns Ok(()), `start_machine_host` returns, the outer
        // `Runtime::new().block_on(...)` in HostMachineRuntime::spawn_machine_host
        // completes, the Runtime is dropped, and every spawned task (including
        // the UnixListener accept loop) is cancelled. The socket file is left
        // on disk but nothing is listening → `Connection refused` for every
        // `cteno*` CLI invocation.
        //
        // Block on SIGTERM / SIGINT on Unix (so `kill`/Ctrl-C shut down the
        // daemon gracefully); fall back to `pending` on non-Unix — the parent
        // process terminating will bring us down.
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate())
                .map_err(|e| format!("install SIGTERM handler: {}", e))?;
            let mut sigint = signal(SignalKind::interrupt())
                .map_err(|e| format!("install SIGINT handler: {}", e))?;
            tokio::select! {
                _ = sigterm.recv() => {
                    log::info!("community daemon received SIGTERM, shutting down");
                }
                _ = sigint.recv() => {
                    log::info!("community daemon received SIGINT, shutting down");
                }
            }
        }
        #[cfg(not(unix))]
        {
            std::future::pending::<()>().await;
        }

        Ok(())
    }

    async fn register_machine_scoped_ui_rpc_handlers(&self, ctx: MachineUiRpcContext) {
        let registry = &self.rpc_registry;
        let machine_id = ctx.machine_id.clone();
        let db_path_buf = ctx.db_path.clone();
        let app_data_dir = ctx.app_data_dir.clone();
        let api_key = ctx.api_key.clone();
        let system_prompt_text = ctx.system_prompt_text.clone();
        let builtin_skills_dir = ctx.builtin_skills_dir.clone();
        let user_skills_dir = ctx.user_skills_dir.clone();
        let builtin_agents_dir = ctx.builtin_agents_dir.clone();
        let user_agents_dir = ctx.user_agents_dir.clone();
        let rpc_methods = MachineRpcMethods::new(&machine_id);
        let rpc_method_toggle_scheduled_task = rpc_methods.toggle_scheduled_task.clone();
        let rpc_method_delete_scheduled_task = rpc_methods.delete_scheduled_task.clone();
        let rpc_method_update_scheduled_task = rpc_methods.update_scheduled_task.clone();
        let rpc_method_delete_scheduled_tasks_by_session =
            rpc_methods.delete_scheduled_tasks_by_session.clone();
        let rpc_method_list_background_tasks = rpc_methods.list_background_tasks.clone();
        let rpc_method_get_background_task = rpc_methods.get_background_task.clone();
        let rpc_method_list_runs = rpc_methods.list_runs.clone();
        let rpc_method_get_run = rpc_methods.get_run.clone();
        let rpc_method_stop_run = rpc_methods.stop_run.clone();
        let rpc_method_get_run_logs = rpc_methods.get_run_logs.clone();
        let rpc_method_get_session_trace = rpc_methods.get_session_trace.clone();
        let rpc_method_bash = rpc_methods.bash.clone();
        let rpc_method_stop_daemon = rpc_methods.stop_daemon.clone();
        let rpc_method_list_tools = rpc_methods.list_tools.clone();
        let rpc_method_exec_tool = rpc_methods.exec_tool.clone();
        let rpc_method_send_msg = rpc_methods.send_message.clone();
        let rpc_method_dispatch = rpc_methods.dispatch_task.clone();
        let rpc_method_list_personas = rpc_methods.list_personas.clone();
        let rpc_method_create_persona = rpc_methods.create_persona.clone();
        let rpc_method_update_persona = rpc_methods.update_persona.clone();
        let rpc_method_delete_persona = rpc_methods.delete_persona.clone();
        let rpc_method_get_persona_tasks = rpc_methods.get_persona_tasks.clone();
        let rpc_method_reset_persona_session = rpc_methods.reset_persona_session.clone();
        let rpc_method_get_agent_latest_text = format!("{machine_id}:get-agent-latest-text");
        let rpc_method_get_agent_artifacts = format!("{machine_id}:get-agent-artifacts");
        let rpc_method_get_target_page = format!("{machine_id}:get-target-page");
        let rpc_method_target_page_response = format!("{machine_id}:target-page-response");
        let rpc_method_get_goal_tree_page = format!("{machine_id}:get-goal-tree-page");
        let rpc_method_get_dashboard = format!("{machine_id}:get-dashboard");
        let rpc_method_list_notifications = format!("{machine_id}:list-notifications");
        let rpc_method_mark_notification_read = format!("{machine_id}:mark-notification-read");
        let rpc_method_get_notification_subscriptions =
            format!("{machine_id}:get-notification-subscriptions");
        let rpc_method_memory_list = rpc_methods.memory_list.clone();
        let rpc_method_memory_read = rpc_methods.memory_read.clone();
        let rpc_method_memory_write = rpc_methods.memory_write.clone();
        let rpc_method_memory_delete = rpc_methods.memory_delete.clone();
        let rpc_method_workspace_list = rpc_methods.workspace_list.clone();
        let rpc_method_workspace_read = rpc_methods.workspace_read.clone();
        let rpc_method_workspace_stat = rpc_methods.workspace_stat.clone();

        // kill-session — machine-scoped fallback for UI paths without session-specific encryption
        let kill_session_conns = self.session_connections.clone();
        registry
            .register(&rpc_methods.kill_session, move |params: Value| {
                let kill_session_conns = kill_session_conns.clone();
                async move {
                    let session_id = params
                        .get("sessionId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if session_id.is_empty() {
                        return Ok(json!({"success": false, "error": "Missing sessionId"}));
                    }

                    log::info!("[Machine RPC] kill-session: {}", session_id);

                    let session_conn = {
                        let mut conns_guard = kill_session_conns.lock().await;
                        conns_guard.remove(&session_id)
                    };
                    if let Some(session_conn) = session_conn {
                        session_conn.kill().await;
                        log::info!("[Machine RPC] kill-session: session {} archived", session_id);
                    } else {
                        log::info!(
                            "[Machine RPC] kill-session: session {} not found in active connections",
                            session_id
                        );
                    }

                    // Clean up persona task session link if this was a task session.
                    if let Ok(pm) = crate::local_services::persona_manager() {
                        pm.on_task_complete(&session_id).await;
                    }

                    Ok(json!({"success": true, "message": "Session archived"}))
                }
            })
            .await;

        // delete-session — permanent deletion in the relay-only architecture.
        // Replaces the legacy server-side `DELETE /v1/sessions/:id` endpoint
        // which no longer exists (happy-server is socket-transparent now).
        // Kills the live connection (if any) AND removes the session row from
        // the daemon's local SQLite `agent_sessions` table.
        let delete_session_conns = self.session_connections.clone();
        let delete_session_db_path = db_path_buf.clone();
        registry
            .register(&rpc_methods.delete_session, move |params: Value| {
                let delete_session_conns = delete_session_conns.clone();
                let db_path = delete_session_db_path.clone();
                async move {
                    let session_id = params
                        .get("sessionId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if session_id.is_empty() {
                        return Ok(json!({"success": false, "error": "Missing sessionId"}));
                    }

                    log::info!("[Machine RPC] delete-session: {}", session_id);

                    let session_conn = {
                        let mut conns_guard = delete_session_conns.lock().await;
                        conns_guard.remove(&session_id)
                    };
                    if let Some(session_conn) = session_conn {
                        session_conn.kill().await;
                    }

                    if let Ok(pm) = crate::local_services::persona_manager() {
                        pm.on_task_complete(&session_id).await;
                    }

                    let sid = session_id.clone();
                    let db = db_path.clone();
                    let deleted = tokio::task::spawn_blocking(move || {
                        cteno_agent_runtime::agent_session::AgentSessionManager::new(db)
                            .delete_session(&sid)
                    })
                    .await
                    .map_err(|e| format!("join delete_session: {e}"))?
                    .map_err(|e| format!("delete_session db: {e}"))?;

                    Ok(json!({
                        "success": true,
                        "message": if deleted { "Session deleted" } else { "Session not found in daemon DB" },
                        "rowDeleted": deleted,
                    }))
                }
            })
            .await;

        // get-agent-latest-text — return the latest persisted assistant text for an agent session
        registry
            .register_sync(&rpc_method_get_agent_latest_text, {
                let latest_text_db_path = db_path_buf.clone();
                move |params: Value| {
                    let agent_id = params
                        .get("agentId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();

                    if agent_id.is_empty() {
                        return Ok(json!({ "success": false, "error": "Missing agentId" }));
                    }

                    let manager = AgentSessionManager::new(latest_text_db_path.clone());
                    match manager.get_session(&agent_id) {
                        Ok(Some(session)) => Ok(json!({
                            "success": true,
                            "text": crate::agent_session::extract_last_assistant_text(&session.messages),
                        })),
                        Ok(None) => Ok(json!({ "success": true, "text": null })),
                        Err(e) => Ok(json!({ "success": false, "error": e })),
                    }
                }
            })
            .await;

        // Machine-scoped UI placeholders until the backing features land.
        registry
            .register_sync(&rpc_method_get_agent_artifacts, move |_params: Value| {
                Ok(json!({ "success": true, "artifacts": [] }))
            })
            .await;
        registry
            .register_sync(&rpc_method_get_target_page, move |_params: Value| {
                Ok(json!({ "success": true, "page": null }))
            })
            .await;
        registry
            .register_sync(&rpc_method_target_page_response, move |_params: Value| {
                Ok(json!({ "success": true }))
            })
            .await;
        registry
            .register_sync(&rpc_method_get_goal_tree_page, move |_params: Value| {
                Ok(json!({ "success": true, "page": null }))
            })
            .await;
        registry
            .register_sync(&rpc_method_get_dashboard, move |_params: Value| {
                Ok(json!({ "success": true, "page": null }))
            })
            .await;
        registry
            .register_sync(&rpc_method_list_notifications, move |_params: Value| {
                Ok(json!({ "success": true, "notifications": [] }))
            })
            .await;
        registry
            .register_sync(&rpc_method_mark_notification_read, move |_params: Value| {
                Ok(json!({ "success": true }))
            })
            .await;

        // memory-list-files — list markdown memory files for global/private scopes.
        registry
            .register_sync(&rpc_method_memory_list, {
                let memory_list_workspace = app_data_dir.join("workspace");
                move |params: Value| {
                    let scope = match parse_memory_scope_param(&params) {
                        Ok(value) => value,
                        Err(error) => {
                            return Ok(json!({ "success": false, "error": error }));
                        }
                    };
                    let persona_workdir = match scope {
                        MemoryRpcScope::Private => {
                            match resolve_memory_persona_workdir(&params, true) {
                                Ok(value) => value,
                                Err(error) => {
                                    return Ok(json!({ "success": false, "error": error }));
                                }
                            }
                        }
                        MemoryRpcScope::Global => None,
                        MemoryRpcScope::Auto => {
                            match resolve_memory_persona_workdir(&params, false) {
                                Ok(value) => value,
                                Err(error) => {
                                    return Ok(json!({ "success": false, "error": error }));
                                }
                            }
                        }
                    };

                    match cteno_community_core::memory::memory_list_core(
                        &memory_list_workspace,
                        persona_workdir.as_deref(),
                    ) {
                        Ok(files) => {
                            let data: Vec<String> = match scope {
                                MemoryRpcScope::Private => files
                                    .into_iter()
                                    .filter(|path| path.starts_with("[private"))
                                    .collect(),
                                MemoryRpcScope::Global => files
                                    .into_iter()
                                    .filter(|path| path.starts_with("[global] "))
                                    .collect(),
                                MemoryRpcScope::Auto => files,
                            };
                            Ok(json!({ "success": true, "data": data }))
                        }
                        Err(error) => Ok(json!({ "success": false, "error": error })),
                    }
                }
            })
            .await;

        // memory-read — read markdown memory file from selected scope.
        registry
            .register_sync(&rpc_method_memory_read, {
                let memory_read_workspace = app_data_dir.join("workspace");
                move |params: Value| {
                    let file_path = params
                        .get("file_path")
                        .or_else(|| params.get("key"))
                        .and_then(|value| value.as_str())
                        .map(normalize_memory_rpc_file_path)
                        .unwrap_or_default();
                    if file_path.is_empty() {
                        return Ok(json!({ "success": false, "error": "Missing file_path" }));
                    }

                    let scope = match parse_memory_scope_param(&params) {
                        Ok(value) => value,
                        Err(error) => {
                            return Ok(json!({ "success": false, "error": error }));
                        }
                    };
                    let persona_workdir = match scope {
                        MemoryRpcScope::Private => {
                            match resolve_memory_persona_workdir(&params, true) {
                                Ok(value) => value,
                                Err(error) => {
                                    return Ok(json!({ "success": false, "error": error }));
                                }
                            }
                        }
                        MemoryRpcScope::Global => None,
                        MemoryRpcScope::Auto => {
                            match resolve_memory_persona_workdir(&params, false) {
                                Ok(value) => value,
                                Err(error) => {
                                    return Ok(json!({ "success": false, "error": error }));
                                }
                            }
                        }
                    };

                    match cteno_community_core::memory::memory_read_core(
                        &memory_read_workspace,
                        &file_path,
                        persona_workdir.as_deref(),
                    ) {
                        Ok(content) => Ok(json!({ "success": true, "data": content })),
                        Err(error) => Ok(json!({ "success": false, "error": error })),
                    }
                }
            })
            .await;

        // memory-write — write markdown memory file to selected scope.
        registry
            .register_sync(&rpc_method_memory_write, {
                let memory_write_workspace = app_data_dir.join("workspace");
                move |params: Value| {
                    let file_path = params
                        .get("file_path")
                        .and_then(|value| value.as_str())
                        .map(normalize_memory_rpc_file_path)
                        .unwrap_or_default();
                    let content = params
                        .get("content")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string();

                    if file_path.is_empty() {
                        return Ok(json!({ "success": false, "error": "Missing file_path" }));
                    }

                    let scope = match parse_memory_scope_param(&params) {
                        Ok(value) => value,
                        Err(error) => {
                            return Ok(json!({ "success": false, "error": error }));
                        }
                    };
                    let persona_workdir = match scope {
                        MemoryRpcScope::Private => {
                            match resolve_memory_persona_workdir(&params, true) {
                                Ok(value) => value,
                                Err(error) => {
                                    return Ok(json!({ "success": false, "error": error }));
                                }
                            }
                        }
                        MemoryRpcScope::Global => None,
                        MemoryRpcScope::Auto => {
                            match resolve_memory_persona_workdir(&params, false) {
                                Ok(value) => value,
                                Err(error) => {
                                    return Ok(json!({ "success": false, "error": error }));
                                }
                            }
                        }
                    };

                    match cteno_community_core::memory::memory_write_core(
                        &memory_write_workspace,
                        &file_path,
                        &content,
                        persona_workdir.as_deref(),
                    ) {
                        Ok(()) => Ok(json!({ "success": true })),
                        Err(error) => Ok(json!({ "success": false, "error": error })),
                    }
                }
            })
            .await;

        // memory-delete — delete markdown memory file from selected scope.
        registry
            .register_sync(&rpc_method_memory_delete, {
                let memory_delete_workspace = app_data_dir.join("workspace");
                move |params: Value| {
                    let file_path = params
                        .get("file_path")
                        .and_then(|value| value.as_str())
                        .map(normalize_memory_rpc_file_path)
                        .unwrap_or_default();

                    if file_path.is_empty() {
                        return Ok(json!({ "success": false, "error": "Missing file_path" }));
                    }

                    let scope = match parse_memory_scope_param(&params) {
                        Ok(value) => value,
                        Err(error) => {
                            return Ok(json!({ "success": false, "error": error }));
                        }
                    };
                    let persona_workdir = match scope {
                        MemoryRpcScope::Private => {
                            match resolve_memory_persona_workdir(&params, true) {
                                Ok(value) => value,
                                Err(error) => {
                                    return Ok(json!({ "success": false, "error": error }));
                                }
                            }
                        }
                        MemoryRpcScope::Global => None,
                        MemoryRpcScope::Auto => {
                            match resolve_memory_persona_workdir(&params, false) {
                                Ok(value) => value,
                                Err(error) => {
                                    return Ok(json!({ "success": false, "error": error }));
                                }
                            }
                        }
                    };

                    match cteno_community_core::memory::memory_delete_core(
                        &memory_delete_workspace,
                        &file_path,
                        persona_workdir.as_deref(),
                    ) {
                        Ok(()) => Ok(json!({ "success": true })),
                        Err(error) => Ok(json!({ "success": false, "error": error })),
                    }
                }
            })
            .await;

        // workspace-list — allow browsing under a caller-selected absolute
        // workspace_root such as the machine home directory.
        registry
            .register(&rpc_method_workspace_list, {
                let workspace_list_app_data_dir = app_data_dir.clone();
                move |params: Value| {
                    let workspace_list_app_data_dir = workspace_list_app_data_dir.clone();
                    async move {
                        let (workspace_root, workspace_canon) =
                            match workspace_root_and_canonical_from_params(
                                &workspace_list_app_data_dir,
                                &params,
                            ) {
                                Ok(value) => value,
                                Err(error) => {
                                    return Ok(json!({ "success": false, "error": error }))
                                }
                            };
                        let requested_path =
                            params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                        let include_hidden = params
                            .get("include_hidden")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let limit = params
                            .get("limit")
                            .and_then(|v| v.as_u64())
                            .map(|value| value.clamp(1, 10_000) as usize);

                        let resolved_path = match resolve_workspace_existing_path(
                            &workspace_root,
                            &workspace_canon,
                            Some(requested_path),
                        ) {
                            Ok(value) => value,
                            Err(error) => {
                                return Ok(json!({ "success": false, "error": error }))
                            }
                        };

                        let dir_meta = match fs::metadata(&resolved_path) {
                            Ok(meta) => meta,
                            Err(error) => {
                                return Ok(json!({
                                    "success": false,
                                    "error": format!(
                                        "Failed to read metadata for '{}': {}",
                                        resolved_path.display(),
                                        error
                                    )
                                }))
                            }
                        };
                        if !dir_meta.is_dir() {
                            return Ok(json!({
                                "success": false,
                                "error": format!("Path is not a directory: {}", resolved_path.display())
                            }));
                        }

                        let current_relative =
                            workspace_relative_string(&workspace_canon, &resolved_path);
                        let read_dir = match fs::read_dir(&resolved_path) {
                            Ok(entries) => entries,
                            Err(error) => {
                                return Ok(json!({
                                    "success": false,
                                    "error": format!(
                                        "Failed to read directory '{}': {}",
                                        resolved_path.display(),
                                        error
                                    )
                                }))
                            }
                        };

                        let mut entries = Vec::new();
                        for child in read_dir {
                            let child = match child {
                                Ok(value) => value,
                                Err(error) => {
                                    return Ok(json!({
                                        "success": false,
                                        "error": format!(
                                            "Failed to read directory entry in '{}': {}",
                                            resolved_path.display(),
                                            error
                                        )
                                    }))
                                }
                            };
                            let name = child.file_name().to_string_lossy().into_owned();
                            if !include_hidden && name.starts_with('.') {
                                continue;
                            }
                            let metadata = match fs::symlink_metadata(child.path()) {
                                Ok(value) => value,
                                Err(error) => {
                                    return Ok(json!({
                                        "success": false,
                                        "error": format!(
                                            "Failed to read metadata for '{}': {}",
                                            child.path().display(),
                                            error
                                        )
                                    }))
                                }
                            };
                            entries.push(json!({
                                "name": name,
                                "path": workspace_join_relative_path(&current_relative, &name),
                                "type": workspace_entry_type(&metadata.file_type()),
                                "size": metadata.len(),
                                "modifiedAt": metadata_modified_at_ms(&metadata),
                            }));
                        }

                        entries.sort_by(|a, b| {
                            let a_type = a.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            let b_type = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            let a_name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let b_name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let a_dir = a_type == "directory";
                            let b_dir = b_type == "directory";
                            b_dir.cmp(&a_dir).then_with(|| a_name.cmp(b_name))
                        });

                        let total = entries.len();
                        let has_more = limit.is_some_and(|value| total > value);
                        if let Some(limit) = limit {
                            entries.truncate(limit);
                        }

                        Ok(json!({
                            "success": true,
                            "path": current_relative,
                            "entries": entries,
                            "hasMore": has_more,
                            "total": total,
                        }))
                    }
                }
            })
            .await;

        // workspace-read — read file content relative to the selected
        // workspace root.
        registry
            .register(&rpc_method_workspace_read, {
                let workspace_read_app_data_dir = app_data_dir.clone();
                move |params: Value| {
                    let workspace_read_app_data_dir = workspace_read_app_data_dir.clone();
                    async move {
                        let (workspace_root, workspace_canon) =
                            match workspace_root_and_canonical_from_params(
                                &workspace_read_app_data_dir,
                                &params,
                            ) {
                                Ok(value) => value,
                                Err(error) => {
                                    return Ok(json!({ "success": false, "error": error }))
                                }
                            };
                        let requested_path = match params.get("path").and_then(|v| v.as_str()) {
                            Some(value) if !value.trim().is_empty() => value,
                            _ => {
                                return Ok(json!({
                                    "success": false,
                                    "error": "Missing path"
                                }))
                            }
                        };
                        let offset = params.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
                        let max_length = params
                            .get("length")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(16 * 1024 * 1024)
                            .clamp(0, 16 * 1024 * 1024)
                            as usize;
                        let encoding = params
                            .get("encoding")
                            .and_then(|v| v.as_str())
                            .unwrap_or("utf8");

                        let resolved_path = match resolve_workspace_existing_path(
                            &workspace_root,
                            &workspace_canon,
                            Some(requested_path),
                        ) {
                            Ok(value) => value,
                            Err(error) => return Ok(json!({ "success": false, "error": error })),
                        };

                        let metadata = match fs::metadata(&resolved_path) {
                            Ok(meta) => meta,
                            Err(error) => {
                                return Ok(json!({
                                    "success": false,
                                    "error": format!(
                                        "Failed to read metadata for '{}': {}",
                                        resolved_path.display(),
                                        error
                                    )
                                }))
                            }
                        };
                        if !metadata.is_file() {
                            return Ok(json!({
                                "success": false,
                                "error": format!("Path is not a file: {}", resolved_path.display())
                            }));
                        }

                        let mut file = match fs::File::open(&resolved_path) {
                            Ok(value) => value,
                            Err(error) => {
                                return Ok(json!({
                                    "success": false,
                                    "error": format!(
                                        "Failed to open '{}': {}",
                                        resolved_path.display(),
                                        error
                                    )
                                }))
                            }
                        };
                        if let Err(error) = file.seek(SeekFrom::Start(offset)) {
                            return Ok(json!({
                                "success": false,
                                "error": format!(
                                    "Failed to seek '{}': {}",
                                    resolved_path.display(),
                                    error
                                )
                            }));
                        }

                        let mut buffer = vec![0u8; max_length];
                        let bytes_read = match file.read(&mut buffer) {
                            Ok(value) => value,
                            Err(error) => {
                                return Ok(json!({
                                    "success": false,
                                    "error": format!(
                                        "Failed to read '{}': {}",
                                        resolved_path.display(),
                                        error
                                    )
                                }))
                            }
                        };
                        buffer.truncate(bytes_read);

                        let data = match encoding {
                            "base64" => BASE64.encode(&buffer),
                            "utf8" => String::from_utf8_lossy(&buffer).into_owned(),
                            other => {
                                return Ok(json!({
                                    "success": false,
                                    "error": format!("Unsupported encoding: {}", other)
                                }))
                            }
                        };

                        let size = metadata.len();
                        let next_offset = offset.saturating_add(bytes_read as u64);
                        Ok(json!({
                            "success": true,
                            "path": workspace_relative_string(&workspace_canon, &resolved_path),
                            "encoding": encoding,
                            "data": data,
                            "bytesRead": bytes_read,
                            "offset": offset,
                            "nextOffset": next_offset,
                            "size": size,
                            "eof": next_offset >= size,
                            "modifiedAt": metadata_modified_at_ms(&metadata),
                        }))
                    }
                }
            })
            .await;

        // workspace-stat — inspect a batch of workspace-relative paths.
        registry
            .register(&rpc_method_workspace_stat, {
                let workspace_stat_app_data_dir = app_data_dir.clone();
                move |params: Value| {
                    let workspace_stat_app_data_dir = workspace_stat_app_data_dir.clone();
                    async move {
                        let (workspace_root, workspace_canon) =
                            match workspace_root_and_canonical_from_params(
                                &workspace_stat_app_data_dir,
                                &params,
                            ) {
                                Ok(value) => value,
                                Err(error) => {
                                    return Ok(json!({ "success": false, "error": error }))
                                }
                            };
                        let Some(raw_paths) = params.get("paths").and_then(|v| v.as_array()) else {
                            return Ok(json!({
                                "success": false,
                                "error": "Missing paths"
                            }));
                        };

                        let mut items = Vec::with_capacity(raw_paths.len());
                        for raw_path in raw_paths {
                            let path = match raw_path.as_str() {
                                Some(value) => value,
                                None => {
                                    items.push(json!({
                                        "path": "",
                                        "exists": false,
                                        "error": "Path must be a string"
                                    }));
                                    continue;
                                }
                            };

                            let relative = match normalize_workspace_relative(path) {
                                Ok(value) => value,
                                Err(error) => {
                                    items.push(json!({
                                        "path": path,
                                        "exists": false,
                                        "error": error
                                    }));
                                    continue;
                                }
                            };
                            let relative_str = if relative.as_os_str().is_empty() {
                                ".".to_string()
                            } else {
                                relative.to_string_lossy().replace('\\', "/")
                            };
                            let target = workspace_root.join(&relative);
                            if !target.exists() {
                                items.push(json!({
                                    "path": relative_str,
                                    "exists": false,
                                }));
                                continue;
                            }

                            let canonical = match fs::canonicalize(&target) {
                                Ok(value) => value,
                                Err(error) => {
                                    items.push(json!({
                                        "path": relative_str,
                                        "exists": false,
                                        "error": format!(
                                            "Failed to resolve path '{}': {}",
                                            target.display(),
                                            error
                                        )
                                    }));
                                    continue;
                                }
                            };
                            if !canonical.starts_with(&workspace_canon) {
                                items.push(json!({
                                    "path": relative_str,
                                    "exists": false,
                                    "error": "Path escapes workspace root"
                                }));
                                continue;
                            }

                            let metadata = match fs::symlink_metadata(&target) {
                                Ok(value) => value,
                                Err(error) => {
                                    items.push(json!({
                                        "path": relative_str,
                                        "exists": false,
                                        "error": format!(
                                            "Failed to read metadata for '{}': {}",
                                            target.display(),
                                            error
                                        )
                                    }));
                                    continue;
                                }
                            };
                            items.push(json!({
                                "path": relative_str,
                                "exists": true,
                                "type": workspace_entry_type(&metadata.file_type()),
                                "size": metadata.len(),
                                "modifiedAt": metadata_modified_at_ms(&metadata),
                            }));
                        }

                        Ok(json!({
                            "success": true,
                            "items": items,
                        }))
                    }
                }
            })
            .await;

        // list-agents — merge builtin + global + workspace agent definitions for UI consumers
        registry
            .register_sync(&rpc_methods.list_agents, {
                let list_agents_builtin_dir = builtin_agents_dir.clone();
                let list_agents_user_dir = user_agents_dir.clone();
                move |params: Value| {
                    let workspace_agents_dir = resolve_workspace_agents_dir(&params);
                    let agents = crate::service_init::load_all_agents(
                        &list_agents_builtin_dir,
                        &list_agents_user_dir,
                        workspace_agents_dir.as_deref(),
                    );
                    let summaries: Vec<Value> = agents.iter().map(build_agent_summary).collect();
                    Ok(json!({ "success": true, "agents": summaries }))
                }
            })
            .await;

        // get-notification-subscriptions — best-effort empty result until watcher-backed state exists
        registry
            .register_sync(
                &rpc_method_get_notification_subscriptions,
                move |_params: Value| Ok(json!({ "success": true, "subscriptions": [] })),
            )
            .await;

        // list-subagents — direct SubAgentManager call
        registry
            .register(&rpc_methods.list_subagents, {
                let list_subagents_db_path = db_path_buf.clone();
                move |params: Value| {
                    let list_subagents_db_path = list_subagents_db_path.clone();
                    async move {
                        let result = {
                            let manager = crate::subagent::manager::global();

                            let status_str = params.get("status").and_then(|v| v.as_str());
                            let status = status_str.and_then(|s| match s {
                                "pending" => Some(crate::subagent::SubAgentStatus::Pending),
                                "running" => Some(crate::subagent::SubAgentStatus::Running),
                                "completed" => Some(crate::subagent::SubAgentStatus::Completed),
                                "failed" => Some(crate::subagent::SubAgentStatus::Failed),
                                "stopped" => Some(crate::subagent::SubAgentStatus::Stopped),
                                "timed_out" => Some(crate::subagent::SubAgentStatus::TimedOut),
                                _ => None,
                            });
                            let parent_session_id = params
                                .get("parentSessionId")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let active_only = params
                                .get("activeOnly")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);

                            let filter = crate::subagent::SubAgentFilter {
                                parent_session_id,
                                status,
                                active_only,
                            };
                            let mut subagents = manager.list(filter).await;
                            let local_subagents = list_local_agent_sessions_as_subagents(
                                &list_subagents_db_path,
                                status_str,
                                params.get("parentSessionId").and_then(|v| v.as_str()),
                                active_only,
                            );
                            if !local_subagents.is_empty() {
                                let existing_ids: std::collections::HashSet<String> =
                                    subagents.iter().map(|item| item.id.clone()).collect();
                                for local in local_subagents {
                                    let local_id = local.id.clone();
                                    if !existing_ids.contains(&local_id) {
                                        subagents.push(local);
                                    }
                                }
                            }
                            Ok::<Value, String>(serde_json::json!({ "subagents": subagents }))
                        };
                        result
                    }
                }
            })
            .await;

        // get-subagent — direct SubAgentManager call
        registry
            .register(&rpc_methods.get_subagent, {
                let get_subagent_db_path = db_path_buf.clone();
                move |params: Value| {
                    let get_subagent_db_path = get_subagent_db_path.clone();
                    async move {
                        let subagent_id = params
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        if subagent_id.is_empty() {
                            return Ok(json!({ "success": false, "error": "Missing id" }));
                        }

                        let result = {
                            let manager = crate::subagent::manager::global();
                            match manager.get(&subagent_id).await {
                                Some(sa) => {
                                    Ok::<Value, String>(serde_json::to_value(&sa).unwrap_or(
                                        json!({ "success": false, "error": "serialize error" }),
                                    ))
                                }
                                None => {
                                    let local_manager =
                                        AgentSessionManager::new(get_subagent_db_path.clone());
                                    match local_manager.get_session(&subagent_id) {
                                        Ok(Some(session)) if session.agent_id != "persona" => {
                                            Ok(serde_json::to_value(
                                                local_agent_session_to_subagent_value(&session),
                                            )
                                            .unwrap_or(json!({
                                                "success": false,
                                                "error": "serialize error"
                                            })))
                                        }
                                        _ => Ok(json!({
                                            "success": false,
                                            "error": "SubAgent not found"
                                        })),
                                    }
                                }
                            }
                        };
                        result
                    }
                }
            })
            .await;

        // stop-subagent — direct SubAgentManager call
        registry
            .register(
                &rpc_methods.stop_subagent,
                move |params: Value| async move {
                    let subagent_id = params
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if subagent_id.is_empty() {
                        return Ok(json!({ "success": false, "error": "Missing id" }));
                    }

                    let result = {
                        let manager = crate::subagent::manager::global();
                        match manager.stop(&subagent_id).await {
                            Ok(_) => Ok::<Value, String>(json!({ "success": true })),
                            Err(e) => Ok(json!({ "success": false, "error": e })),
                        }
                    };
                    result
                },
            )
            .await;

        // list-scheduled-tasks — use in-process scheduler
        registry
            .register_sync(&rpc_methods.list_scheduled_tasks, move |params: Value| {
                let enabled_only = params
                    .get("enabledOnly")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let scheduler = match crate::local_services::scheduler() {
                    Ok(scheduler) => scheduler,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };
                match scheduler.list_tasks(enabled_only) {
                    Ok(tasks) => Ok(json!({ "success": true, "tasks": tasks })),
                    Err(e) => Ok(json!({ "success": false, "error": e })),
                }
            })
            .await;

        // toggle-scheduled-task — use in-process scheduler
        registry
            .register_sync(&rpc_method_toggle_scheduled_task, move |params: Value| {
                let task_id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let enabled = params
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                if task_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing id" }));
                }
                let scheduler = match crate::local_services::scheduler() {
                    Ok(scheduler) => scheduler,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };
                let mut task = match scheduler.get_task(&task_id) {
                    Ok(Some(task)) => task,
                    Ok(None) => {
                        return Ok(json!({ "success": false, "error": "Task not found" }));
                    }
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };
                task.enabled = enabled;
                task.updated_at = chrono::Utc::now().timestamp_millis();
                match scheduler.update_task(&task) {
                    Ok(()) => Ok(json!({ "success": true })),
                    Err(e) => Ok(json!({ "success": false, "error": e })),
                }
            })
            .await;

        // delete-scheduled-task — use in-process scheduler
        registry
            .register_sync(&rpc_method_delete_scheduled_task, move |params: Value| {
                let task_id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if task_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing id" }));
                }
                let scheduler = match crate::local_services::scheduler() {
                    Ok(scheduler) => scheduler,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };
                match scheduler.delete_task(&task_id) {
                    Ok(true) => Ok(json!({ "success": true })),
                    Ok(false) => Ok(json!({ "success": false, "error": "Task not found" })),
                    Err(e) => Ok(json!({ "success": false, "error": e })),
                }
            })
            .await;

        // update-scheduled-task — use in-process scheduler
        registry
            .register_sync(&rpc_method_update_scheduled_task, move |params: Value| {
                let task_id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if task_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing id" }));
                }

                let scheduler = match crate::local_services::scheduler() {
                    Ok(scheduler) => scheduler,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };
                let mut task = match scheduler.get_task(&task_id) {
                    Ok(Some(task)) => task,
                    Ok(None) => {
                        return Ok(json!({ "success": false, "error": "Task not found" }));
                    }
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };

                if let Some(name) = params.get("name").and_then(|v| v.as_str()) {
                    task.name = name.to_string();
                }
                if let Some(prompt) = params.get("task_prompt").and_then(|v| v.as_str()) {
                    task.task_prompt = prompt.to_string();
                }
                if let Some(enabled) = params.get("enabled").and_then(|v| v.as_bool()) {
                    task.enabled = enabled;
                }
                if let Some(delete_after_run) =
                    params.get("delete_after_run").and_then(|v| v.as_bool())
                {
                    task.delete_after_run = delete_after_run;
                }

                let mut schedule_changed = false;
                if let Some(schedule_value) = params.get("schedule") {
                    match serde_json::from_value::<crate::scheduler::ScheduleType>(
                        schedule_value.clone(),
                    ) {
                        Ok(schedule) => {
                            task.schedule = schedule;
                            schedule_changed = true;
                        }
                        Err(e) => {
                            return Ok(json!({
                                "success": false,
                                "error": format!("Invalid schedule: {}", e)
                            }))
                        }
                    }
                }
                if let Some(timezone) = params.get("timezone").and_then(|v| v.as_str()) {
                    task.timezone = timezone.to_string();
                    schedule_changed = true;
                }
                if schedule_changed {
                    let now = chrono::Utc::now().timestamp_millis();
                    task.state.next_run_at = crate::scheduler::timer::compute_next_run(
                        &task.schedule,
                        &task.timezone,
                        now,
                    );
                }
                task.updated_at = chrono::Utc::now().timestamp_millis();

                match scheduler.update_task(&task) {
                    Ok(()) => Ok(json!({ "success": true, "task": task })),
                    Err(e) => Ok(json!({ "success": false, "error": e })),
                }
            })
            .await;

        // delete-scheduled-tasks-by-session — use in-process scheduler
        registry
            .register_sync(
                &rpc_method_delete_scheduled_tasks_by_session,
                move |params: Value| {
                    let session_id = params
                        .get("sessionId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if session_id.is_empty() {
                        return Ok(json!({ "success": false, "error": "Missing sessionId" }));
                    }
                    let scheduler = match crate::local_services::scheduler() {
                        Ok(scheduler) => scheduler,
                        Err(e) => return Ok(json!({ "success": false, "error": e })),
                    };
                    match scheduler.delete_tasks_by_session(&session_id) {
                        Ok(deleted_count) => {
                            Ok(json!({ "success": true, "deleted_count": deleted_count }))
                        }
                        Err(e) => Ok(json!({ "success": false, "error": e })),
                    }
                },
            )
            .await;

        // list-background-tasks — use shared in-process background task registry
        registry
            .register_sync(&rpc_method_list_background_tasks, move |params: Value| {
                let background_task_registry =
                    match crate::local_services::background_task_registry() {
                        Ok(registry) => registry,
                        Err(e) => return Ok(json!({ "success": false, "error": e })),
                    };
                let category = match parse_background_task_category_param(&params) {
                    Ok(category) => category,
                    Err(error) => return Ok(json!({ "success": false, "error": error })),
                };
                let status = match parse_background_task_status_param(&params) {
                    Ok(status) => status,
                    Err(error) => return Ok(json!({ "success": false, "error": error })),
                };
                let session_id = params
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                let tasks = background_task_registry.list(BackgroundTaskFilter {
                    session_id,
                    category,
                    status,
                });
                Ok(json!({ "success": true, "data": tasks }))
            })
            .await;

        // get-background-task — fetch one background task from the shared registry
        registry
            .register_sync(&rpc_method_get_background_task, move |params: Value| {
                let task_id = params
                    .get("taskId")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .unwrap_or("")
                    .to_string();
                if task_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing taskId" }));
                }
                let background_task_registry =
                    match crate::local_services::background_task_registry() {
                        Ok(registry) => registry,
                        Err(e) => return Ok(json!({ "success": false, "error": e })),
                    };
                match background_task_registry.get(&task_id) {
                    Some(task) => Ok(json!({ "success": true, "data": task })),
                    None => Ok(json!({ "success": false, "error": "Task not found" })),
                }
            })
            .await;

        // list-runs — use in-process run manager
        registry
            .register(&rpc_method_list_runs, move |params: Value| async move {
                let run_manager = match crate::local_services::run_manager() {
                    Ok(run_manager) => run_manager,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };
                let session_id = params
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let runs = run_manager.list_runs(session_id.as_deref()).await;
                Ok(json!({ "success": true, "data": runs }))
            })
            .await;

        // get-run — use in-process run manager
        registry
            .register(&rpc_method_get_run, move |params: Value| async move {
                let run_id = params
                    .get("runId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if run_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing runId" }));
                }
                let run_manager = match crate::local_services::run_manager() {
                    Ok(run_manager) => run_manager,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };
                let run = run_manager.get_run(&run_id).await;
                match run {
                    Some(run) => Ok(json!({ "success": true, "data": run })),
                    None => Ok(json!({ "success": false, "error": "Run not found" })),
                }
            })
            .await;

        // stop-run — use in-process run manager
        registry
            .register(&rpc_method_stop_run, move |params: Value| async move {
                let run_id = params
                    .get("runId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if run_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing runId" }));
                }
                let run_manager = match crate::local_services::run_manager() {
                    Ok(run_manager) => run_manager,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };
                let killed = run_manager.kill_run(&run_id, "stopped by user").await;
                Ok(json!({ "success": true, "data": { "killed": killed } }))
            })
            .await;

        // get-run-logs — use in-process run manager
        registry
            .register(&rpc_method_get_run_logs, move |params: Value| async move {
                let run_id = params
                    .get("runId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let lines = params.get("lines").and_then(|v| v.as_u64()).unwrap_or(100);

                if run_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing runId" }));
                }
                let run_manager = match crate::local_services::run_manager() {
                    Ok(run_manager) => run_manager,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };
                let max_bytes = (lines as usize).saturating_mul(160).clamp(256, 256_000);
                let logs = run_manager.tail_log(&run_id, max_bytes).await;
                match logs {
                    Ok(content) => Ok(json!({ "success": true, "data": content })),
                    Err(e) => Ok(json!({ "success": false, "error": e })),
                }
            })
            .await;

        // get-session-trace — parse persisted session messages into structured trace events
        let trace_db_path = db_path_buf.clone();
        registry
            .register_sync(&rpc_method_get_session_trace, move |params: Value| {
                let session_id = params
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let limit = params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(300)
                    .clamp(1, 2000) as usize;

                if session_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing sessionId" }));
                }

                let manager = AgentSessionManager::new(trace_db_path.clone());
                let session = match manager.get_session(&session_id) {
                    Ok(Some(s)) => s,
                    Ok(None) => {
                        return Ok(json!({
                            "success": false,
                            "error": "Session not found"
                        }));
                    }
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };

                let events = build_session_trace_events(&session.messages, limit);
                let total = events.len();
                let tool_calls = events
                    .iter()
                    .filter(|e| e.event_type == "tool_call")
                    .count();
                let tool_results = events
                    .iter()
                    .filter(|e| e.event_type == "tool_result")
                    .count();

                Ok(json!({
                    "success": true,
                    "sessionId": session_id,
                    "events": events,
                    "summary": {
                        "total": total,
                        "toolCalls": tool_calls,
                        "toolResults": tool_results,
                    }
                }))
            })
            .await;

        // bash — execute shell command on this machine
        registry
            .register(&rpc_method_bash, move |params: Value| async move {
                let command = params
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let cwd = params
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .unwrap_or("/")
                    .to_string();

                if command.is_empty() {
                    return Ok(json!({
                        "success": false,
                        "stdout": "",
                        "stderr": "Missing command",
                        "exitCode": -1
                    }));
                }

                #[cfg(windows)]
                let (shell, shell_flag) = ("powershell".to_string(), "-Command");
                #[cfg(not(windows))]
                let (shell, shell_flag) = (
                    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()),
                    "-c",
                );

                let resolved_cwd = if cwd == "~" || cwd.starts_with("~/") {
                    if let Some(home) = dirs::home_dir() {
                        if cwd == "~" {
                            home.to_string_lossy().to_string()
                        } else {
                            home.join(&cwd[2..]).to_string_lossy().to_string()
                        }
                    } else {
                        cwd.clone()
                    }
                } else {
                    cwd.clone()
                };

                let wrapped_command =
                    crate::tool_executors::shell::ShellExecutor::wrap_command_utf8(&command);
                let mut cmd = tokio::process::Command::new(&shell);
                cmd.arg(shell_flag)
                    .arg(&wrapped_command)
                    .current_dir(&resolved_cwd)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true);

                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    const CREATE_NO_WINDOW: u32 = 0x08000000;
                    cmd.creation_flags(CREATE_NO_WINDOW);
                }

                match cmd.output().await {
                    Ok(output) => {
                        let exit_code = output.status.code().unwrap_or(-1);
                        Ok(json!({
                            "success": output.status.success(),
                            "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                            "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                            "exitCode": exit_code
                        }))
                    }
                    Err(e) => Ok(json!({
                        "success": false,
                        "stdout": "",
                        "stderr": format!("Failed to execute command: {}", e),
                        "exitCode": -1
                    })),
                }
            })
            .await;

        // stop-daemon — graceful shutdown placeholder
        registry
            .register_sync(&rpc_method_stop_daemon, move |_params: Value| {
                log::info!("Received stop-daemon RPC request");
                Ok(json!({ "message": "Daemon stop requested" }))
            })
            .await;

        // list-tools — list all registered tools
        registry
            .register(&rpc_method_list_tools, move |_params: Value| async move {
                let tool_reg = match crate::local_services::tool_registry() {
                    Ok(r) => r,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };
                let tools: Vec<Value> = {
                    let reg = tool_reg.read().await;
                    reg.get_all_configs()
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.id,
                                "name": c.name,
                                "description": c.description,
                                "category": format!("{:?}", c.category),
                            })
                        })
                        .collect()
                };
                Ok(json!({ "success": true, "tools": tools }))
            })
            .await;

        // exec-tool — execute a tool by ID
        registry
            .register(&rpc_method_exec_tool, move |params: Value| async move {
                let tool_id = params
                    .get("tool_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input = params.get("input").cloned().unwrap_or(json!({}));

                if tool_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing tool_id" }));
                }

                let tool_reg = match crate::local_services::tool_registry() {
                    Ok(r) => r,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };

                let result = {
                    let reg = tool_reg.read().await;
                    reg.execute(&tool_id, input).await
                };
                match result {
                    Ok(output) => Ok(json!({ "success": true, "output": output })),
                    Err(e) => Ok(json!({ "success": false, "error": e })),
                }
            })
            .await;

        // send-message — send a user message to a persona's chat session
        let send_msg_session_connections = self.session_connections.clone();
        registry
            .register(&rpc_method_send_msg, move |params: Value| {
                let send_msg_session_connections = send_msg_session_connections.clone();
                async move {
                    let persona_id = params
                        .get("persona_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let message = params
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if persona_id.is_empty() || message.is_empty() {
                        return Ok(json!({
                            "success": false,
                            "error": "Missing persona_id or message"
                        }));
                    }

                    let owner = match crate::agent_owner::resolve_owner(&persona_id) {
                        Ok(owner) => owner,
                        Err(e) => {
                            return Ok(json!({
                                "success": false,
                                "error": format!("Failed to find persona: {}", e)
                            }))
                        }
                    };
                    let chat_sid = owner.chat_session_id.clone();

                    let handle_opt = {
                        send_msg_session_connections
                            .get(&chat_sid)
                            .await
                            .map(|conn| conn.message_handle())
                    };
                    if let Some(handle) = handle_opt {
                        match handle.send_initial_user_message(&message).await {
                            Ok(()) => Ok(json!({
                                "success": true,
                                "sessionId": chat_sid,
                            })),
                            Err(e) => Ok(json!({
                                "success": false,
                                "error": format!("Failed to send message: {}", e)
                            })),
                        }
                    } else {
                        Ok(json!({
                            "success": false,
                            "error": format!(
                                "Chat session {} not connected (persona may need to be activated first)",
                                chat_sid
                            )
                        }))
                    }
                }
            })
            .await;

        // dispatch-task — dispatch a task to a persona's worker
        registry
            .register(&rpc_method_dispatch, move |params: Value| async move {
                let persona_id = params
                    .get("personaId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let task = params
                    .get("task")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if persona_id.is_empty() || task.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing personaId or task" }));
                }
                let workdir = params
                    .get("workdir")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let model_id = params
                    .get("modelId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let agent_type = params
                    .get("agentType")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let wait = params
                    .get("wait")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let timeout_secs = params
                    .get("timeout")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(300);

                let pm = match crate::local_services::persona_manager() {
                    Ok(pm) => pm,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };

                let perm_override = if wait {
                    Some(super::permission::PermissionMode::BypassPermissions)
                } else {
                    None
                };

                let skill_ids: Option<Vec<String>> = params
                    .get("skillIds")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    });
                let label = params.get("label").and_then(|v| v.as_str());
                let agent_flavor = params.get("agentFlavor").and_then(|v| v.as_str());

                let session_id = match pm
                    .dispatch_task_async(
                        &persona_id,
                        &task,
                        workdir.as_deref(),
                        model_id.as_deref(),
                        skill_ids.as_deref(),
                        agent_type.as_deref(),
                        agent_flavor,
                        label,
                        perm_override,
                    )
                    .await
                {
                    Ok(sid) => sid,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };

                if !wait {
                    return Ok(json!({
                        "success": true,
                        "sessionId": session_id,
                    }));
                }

                let rx = cteno_host_bridge_localrpc::register_completion(session_id.clone()).await;

                log::info!(
                    "[CLI] dispatch --wait: waiting for session {} (timeout {}s)...",
                    session_id,
                    timeout_secs
                );

                let result = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), rx)
                    .await
                    .map_err(|_| {
                        format!("Session {} timed out after {}s", session_id, timeout_secs)
                    })
                    .and_then(|result| {
                        result.map_err(|_| "Completion channel dropped".to_string())
                    });

                match result {
                    Ok(final_text) => Ok(json!({
                        "success": true,
                        "sessionId": session_id,
                        "response": final_text,
                    })),
                    Err(e) => Ok(json!({ "success": false, "error": e })),
                }
            })
            .await;

        // list-personas
        registry
            .register_sync(&rpc_method_list_personas, move |_params: Value| {
                let pm = match crate::local_services::persona_manager() {
                    Ok(pm) => pm,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };
                match pm.store().list_personas() {
                    Ok(personas) => Ok(json!({ "success": true, "personas": personas })),
                    Err(e) => Ok(json!({ "success": false, "error": e })),
                }
            })
            .await;

        // create-persona — creates persona and starts the local session runtime
        // Uses async `register` so we can await session creation before returning
        // the persona with a real (non-pending) chat_session_id.
        let cp_profile_store = self.profile_store.clone();
        registry
            .register(&rpc_method_create_persona, move |params: Value| {
                let profile_store = cp_profile_store.clone();
                async move {
                    let name = params
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let description = params
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let model = params
                        .get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or("deepseek-chat")
                        .to_string();
                    let avatar_id = params
                        .get("avatarId")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let model_id = params
                        .get("modelId")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let workdir = params
                        .get("workdir")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let agent_type = params
                        .get("agent")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    let name = if name.is_empty() {
                        "新对话".to_string()
                    } else {
                        name
                    };

                    let pm = match crate::local_services::persona_manager() {
                        Ok(pm) => pm,
                        Err(e) => return Ok(json!({ "success": false, "error": e })),
                    };
                    let resolved_profile_id = resolve_persona_model_selection(
                        profile_store.clone(),
                        model_id.as_deref(),
                        agent_type.as_deref().unwrap_or("cteno"),
                    )
                    .await;
                    let mut persona = match pm.create_persona(
                        &name,
                        &description,
                        &model,
                        avatar_id.as_deref(),
                        Some(resolved_profile_id.as_str()),
                        agent_type.as_deref(),
                        workdir.as_deref(),
                    ) {
                        Ok(p) => p,
                        Err(e) => return Ok(json!({ "success": false, "error": e })),
                    };

                    // Await session creation so the persona returned to the frontend
                    // has a real chat_session_id (not "pending-...").
                    match spawn_local_persona_session(
                        profile_store,
                        &persona,
                        agent_type.as_deref(),
                    )
                    .await
                    {
                        Ok(session_id) => {
                            // Update the DB and our local copy
                            if let Ok(pm) = crate::local_services::persona_manager() {
                                let _ = pm.store().update_chat_session_id(&persona.id, &session_id);
                            }
                            persona.chat_session_id = session_id.clone();

                            // Notify frontend (fire-and-forget)
                            let persona_id = persona.id.clone();
                            tokio::spawn(async move {
                                if let Ok(socket) = crate::local_services::machine_socket() {
                                    let payload = serde_json::json!({
                                        "type": "hypothesis-push",
                                        "agentId": persona_id,
                                        "event": "persona_session_ready",
                                    });
                                    let _ = socket.emit("hypothesis-push", payload).await;
                                }
                            });
                        }
                        Err(e) => {
                            log::error!(
                                "[Persona] Failed to create local session for {}: {}",
                                persona.id,
                                e
                            );
                            // Return persona anyway — frontend will see pending- ID
                            // and can retry or show an error
                        }
                    }

                    Ok(json!({ "success": true, "persona": persona }))
                }
            })
            .await;

        // update-persona
        let update_session_conns = self.session_connections.clone();
        registry
            .register_sync(&rpc_method_update_persona, move |params: Value| {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing id" }));
                }

                let pm = match crate::local_services::persona_manager() {
                    Ok(pm) => pm,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };

                let mut persona = match pm.store().get_persona(&id) {
                    Ok(Some(p)) => p,
                    Ok(None) => {
                        return Ok(json!({ "success": false, "error": "Persona not found" }));
                    }
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };

                let was_continuous = persona.continuous_browsing;

                if let Some(name) = params.get("name").and_then(|v| v.as_str()) {
                    persona.name = name.to_string();
                }
                if let Some(desc) = params.get("description").and_then(|v| v.as_str()) {
                    persona.description = desc.to_string();
                }
                if let Some(model) = params.get("model").and_then(|v| v.as_str()) {
                    persona.model = model.to_string();
                }
                if let Some(avatar) = params.get("avatarId").and_then(|v| v.as_str()) {
                    persona.avatar_id = avatar.to_string();
                }
                if let Some(model_id) = params.get("modelId").and_then(|v| v.as_str()) {
                    persona.profile_id = Some(model_id.to_string());
                }
                if let Some(cb) = params.get("continuousBrowsing").and_then(|v| v.as_bool()) {
                    persona.continuous_browsing = cb;
                }

                match pm.store().update_persona(&persona) {
                    Ok(()) => {
                        if let Some(model_id) = params.get("modelId").and_then(|v| v.as_str()) {
                            let chat_sid = persona.chat_session_id.clone();
                            let new_profile = model_id.to_string();
                            let conns = update_session_conns.clone();
                            tokio::spawn(async move {
                                let conn_opt = {
                                    let conns = conns.lock().await;
                                    conns.get(&chat_sid).cloned()
                                };
                                if let Some(conn) = conn_opt {
                                    let _ = conn.switch_profile(new_profile, None).await;
                                }
                            });
                        }

                        if persona.continuous_browsing && !was_continuous {
                            let chat_session_id = persona.chat_session_id.clone();
                            let conns = update_session_conns.clone();
                            let db_path = pm.db_path().clone();
                            tokio::spawn(async move {
                                let has_messages =
                                    session_has_real_user_messages(&db_path, &chat_session_id);

                                if !has_messages {
                                    log::info!(
                                        "[ContinuousBrowsing] Toggled ON for persona {} but session {} has no real user messages, skipping injection",
                                        id,
                                        chat_session_id
                                    );
                                    return;
                                }

                                let conns = conns.lock().await;
                                if let Some(conn) = conns.get(&chat_session_id) {
                                    let queue = conn.queue();
                                    if !queue.is_processing(&chat_session_id) {
                                        let continue_prompt = continuous_browsing_prompt_for_session(
                                            &db_path,
                                            &chat_session_id,
                                        );
                                        log::info!(
                                            "[ContinuousBrowsing] Toggled ON for persona {}, injecting '{}' into session {}",
                                            id,
                                            continue_prompt,
                                            chat_session_id
                                        );
                                        let _ = queue.push(crate::agent_queue::AgentMessage::system(
                                            chat_session_id.clone(),
                                            continue_prompt,
                                        ));
                                        spawn_queued_worker_for_session_if_idle(
                                            chat_session_id.clone(),
                                            "continuous-browsing-toggle",
                                            true,
                                        );
                                    }
                                }
                            });
                        }
                        Ok(json!({ "success": true, "persona": persona }))
                    }
                    Err(e) => Ok(json!({ "success": false, "error": e })),
                }
            })
            .await;

        // delete-persona
        let app_data_dir_for_delete = app_data_dir.clone();
        registry
            .register(&rpc_method_delete_persona, move |params: Value| {
                let app_data_dir_for_delete = app_data_dir_for_delete.clone();
                async move {
                    let id = params
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if id.is_empty() {
                        return Ok(json!({ "success": false, "error": "Missing id" }));
                    }

                    let pm = match crate::local_services::persona_manager() {
                        Ok(pm) => pm,
                        Err(e) => return Ok(json!({ "success": false, "error": e })),
                    };

                    let mut sessions_to_delete: Vec<String> = Vec::new();
                    let mut persona_workdir: Option<String> = None;

                    if let Ok(Some(persona)) = pm.store().get_persona(&id) {
                        if !persona.chat_session_id.starts_with("pending-") {
                            sessions_to_delete.push(persona.chat_session_id.clone());
                        }
                        persona_workdir = Some(persona.workdir.clone());
                    }

                    if let Ok(links) = pm.store().list_task_sessions(&id) {
                        for link in links {
                            sessions_to_delete.push(link.session_id);
                        }
                    }

                    if !sessions_to_delete.is_empty() {
                        if let Ok(spawn_config) = crate::local_services::spawn_config() {
                            let removed = {
                                let mut removed = Vec::new();
                                for sid in &sessions_to_delete {
                                    if let Some(conn) =
                                        spawn_config.session_connections.remove(sid).await
                                    {
                                        removed.push((sid.clone(), conn));
                                    }
                                }
                                removed
                            };
                            for (sid, conn) in removed {
                                conn.kill().await;
                                log::info!("[Persona] Killed session {} (persona {} deleted)", sid, id);
                            }

                            let app_data_dir = spawn_config
                                .db_path
                                .parent()
                                .and_then(|db_dir| db_dir.parent())
                                .unwrap_or_else(|| std::path::Path::new("."));
                            if let Some((auth_token, _, _, _)) =
                                load_persisted_machine_auth(app_data_dir)?
                            {
                                let http_client = reqwest::Client::new();
                                let server_url = crate::resolved_happy_server_url();
                                for sid in &sessions_to_delete {
                                    let url = format!("{}/v1/sessions/{}", server_url, sid);
                                    match http_client
                                        .delete(&url)
                                        .header(
                                            "Authorization",
                                            format!("Bearer {}", auth_token),
                                        )
                                        .send()
                                        .await
                                    {
                                        Ok(resp) if resp.status().is_success() => {
                                            log::info!("[Persona] Deleted session {} from server", sid);
                                        }
                                        Ok(resp) => {
                                            log::warn!(
                                                "[Persona] Server delete session {} failed: {}",
                                                sid,
                                                resp.status()
                                            );
                                        }
                                        Err(e) => {
                                            log::warn!(
                                                "[Persona] Server delete session {} error: {}",
                                                sid,
                                                e
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }

                    #[cfg(target_os = "macos")]
                    {
                        if let Ok(watcher) = crate::local_services::notification_watcher() {
                            if let Err(e) = watcher.store().delete_by_persona(&id) {
                                log::warn!("[Persona] Notification cleanup failed for {}: {}", id, e);
                            }
                        }
                    }

                    match pm.store().delete_persona(&id) {
                        Ok(true) => {
                            if let Some(workdir) = persona_workdir {
                                let _ = workdir;
                            }
                            let _ = app_data_dir_for_delete;
                            log::info!(
                                "[Persona] Deleted persona {} ({} task session(s) deleted, memory cleaned)",
                                id,
                                sessions_to_delete.len()
                            );
                            Ok(json!({ "success": true }))
                        }
                        Ok(false) => Ok(json!({ "success": false, "error": "Persona not found" })),
                        Err(e) => Ok(json!({ "success": false, "error": e })),
                    }
                }
            })
            .await;

        // get-persona-tasks
        registry
            .register_sync(&rpc_method_get_persona_tasks, move |params: Value| {
                let persona_id = params
                    .get("personaId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if persona_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing personaId" }));
                }

                let pm = match crate::local_services::persona_manager() {
                    Ok(pm) => pm,
                    Err(e) => return Ok(json!({ "success": false, "error": e })),
                };
                match pm.list_active_tasks(&persona_id) {
                    Ok(tasks) => Ok(json!({ "success": true, "tasks": tasks })),
                    Err(e) => Ok(json!({ "success": false, "error": e })),
                }
            })
            .await;

        // reset-persona-session — archive old chat session and create a fresh one
        let rps_profile_store = self.profile_store.clone();
        registry
            .register(&rpc_method_reset_persona_session, move |params: Value| {
                let rps_profile_store = rps_profile_store.clone();
                async move {
                    let persona_id = params
                        .get("personaId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if persona_id.is_empty() {
                        return Ok(json!({ "success": false, "error": "Missing personaId" }));
                    }

                    let pm = match crate::local_services::persona_manager() {
                        Ok(pm) => pm,
                        Err(e) => return Ok(json!({ "success": false, "error": e })),
                    };

                    let persona = match pm.store().get_persona(&persona_id) {
                        Ok(Some(p)) => p,
                        Ok(None) => {
                            return Ok(json!({ "success": false, "error": "Persona not found" }))
                        }
                        Err(e) => return Ok(json!({ "success": false, "error": e })),
                    };

                    let old_session_id = persona.chat_session_id.clone();
                    let result = spawn_local_persona_session(
                        rps_profile_store.clone(),
                        &persona,
                        persona.agent.as_deref(),
                    )
                    .await
                    .map(|new_session_id| {
                        if !old_session_id.starts_with("pending-") {
                            let old_sid = old_session_id.clone();
                            tokio::spawn(async move {
                                if let Ok(spawn_config) = crate::local_services::spawn_config() {
                                    if let Some(old_conn) =
                                        spawn_config.session_connections.remove(&old_sid).await
                                    {
                                        old_conn.kill().await;
                                        log::info!(
                                            "[Persona] Background: killed old session {}",
                                            old_sid
                                        );
                                    }
                                }
                            });
                        }
                        new_session_id
                    });

                    match result {
                        Ok(new_session_id) => {
                            if let Err(e) = pm
                                .store()
                                .update_chat_session_id(&persona_id, &new_session_id)
                            {
                                log::warn!("[Persona] Failed to update chat_session_id: {}", e);
                            }
                            log::info!(
                                "[Persona] Reset persona {} session: {} -> {}",
                                persona_id,
                                old_session_id,
                                new_session_id
                            );
                            Ok(json!({
                                "success": true,
                                "newSessionId": new_session_id,
                                "oldSessionId": old_session_id,
                            }))
                        }
                        Err(e) => {
                            log::error!("[Persona] Failed to reset session: {}", e);
                            Ok(json!({ "success": false, "error": e }))
                        }
                    }
                }
            })
            .await;
    }
}

/// Extract the last assistant text content from agent_sessions.messages JSON.
fn extract_last_agent_text(messages_json: &str) -> Option<String> {
    let messages: Vec<serde_json::Value> = serde_json::from_str(messages_json).ok()?;

    // Walk backwards to find the last assistant message with text content
    for msg in messages.iter().rev() {
        if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");

        // Content format: "BLOCKS:[{...},{...}]" or plain text
        if let Some(blocks_str) = content.strip_prefix("BLOCKS:") {
            if let Ok(blocks) = serde_json::from_str::<Vec<serde_json::Value>>(blocks_str) {
                // Find the last text block
                for block in blocks.iter().rev() {
                    if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                        if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() && !trimmed.starts_with('{') {
                                return Some(trimmed.to_string());
                            }
                        }
                    }
                }
            }
        } else if !content.is_empty() && !content.starts_with('{') {
            return Some(content.trim().to_string());
        }
    }
    None
}

fn local_agent_session_status_to_subagent_status(
    status: &crate::agent_session::SessionStatus,
) -> &'static str {
    match status {
        crate::agent_session::SessionStatus::Active => "running",
        crate::agent_session::SessionStatus::Closed => "completed",
        crate::agent_session::SessionStatus::Expired => "stopped",
    }
}

fn local_agent_session_timestamp(input: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(input)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn local_agent_session_task(session: &crate::agent_session::AgentSession) -> String {
    session
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.trim().to_string())
        .filter(|text| !text.is_empty())
        .or_else(|| {
            session
                .context_data
                .as_ref()
                .and_then(|ctx| ctx.get("task_description"))
                .and_then(|value| value.as_str())
                .map(|value| value.trim().to_string())
                .filter(|text| !text.is_empty())
        })
        .unwrap_or_else(|| "Local agent session".to_string())
}

fn local_agent_session_to_subagent_value(
    session: &crate::agent_session::AgentSession,
) -> crate::subagent::SubAgent {
    let result = session
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|message| message.content.trim().to_string())
        .filter(|text| !text.is_empty());

    crate::subagent::SubAgent {
        id: session.id.clone(),
        parent_session_id: session.owner_session_id.clone().unwrap_or_default(),
        agent_id: session.agent_id.clone(),
        task: local_agent_session_task(session),
        label: session
            .context_data
            .as_ref()
            .and_then(|ctx| ctx.get("label"))
            .and_then(|value| value.as_str())
            .or_else(|| {
                session
                    .context_data
                    .as_ref()
                    .and_then(|ctx| ctx.get("agent_id"))
                    .and_then(|value| value.as_str())
            })
            .map(|value| value.to_string()),
        status: match session.status {
            crate::agent_session::SessionStatus::Active => crate::subagent::SubAgentStatus::Running,
            crate::agent_session::SessionStatus::Closed => {
                crate::subagent::SubAgentStatus::Completed
            }
            crate::agent_session::SessionStatus::Expired => {
                crate::subagent::SubAgentStatus::Stopped
            }
        },
        created_at: local_agent_session_timestamp(&session.created_at).unwrap_or_default(),
        started_at: local_agent_session_timestamp(&session.created_at),
        completed_at: if session.status == crate::agent_session::SessionStatus::Active {
            None
        } else {
            local_agent_session_timestamp(&session.updated_at)
        },
        result,
        error: None,
        iteration_count: 0,
        cleanup: crate::subagent::CleanupPolicy::Keep,
    }
}

fn list_local_agent_sessions_as_subagents(
    db_path: &std::path::Path,
    status: Option<&str>,
    parent_session_id: Option<&str>,
    active_only: bool,
) -> Vec<crate::subagent::SubAgent> {
    let manager = AgentSessionManager::new(db_path.to_path_buf());
    let status_filter = if active_only {
        Some(crate::agent_session::SessionStatus::Active)
    } else {
        match status {
            Some("running") | Some("pending") => Some(crate::agent_session::SessionStatus::Active),
            Some("completed") => Some(crate::agent_session::SessionStatus::Closed),
            Some("stopped") | Some("timed_out") | Some("failed") => {
                Some(crate::agent_session::SessionStatus::Expired)
            }
            _ => None,
        }
    };

    manager
        .list_sessions(status_filter)
        .unwrap_or_default()
        .into_iter()
        .filter(|session| session.agent_id != "persona")
        .filter(|session| {
            if let Some(parent) = parent_session_id {
                session.owner_session_id.as_deref() == Some(parent)
            } else {
                true
            }
        })
        .map(|session| local_agent_session_to_subagent_value(&session))
        .collect()
}

#[derive(Debug, Clone, Serialize)]
struct SessionTraceEvent {
    seq: usize,
    timestamp: String,
    role: String,
    event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_error: Option<bool>,
}

fn truncate_trace_text(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut out = String::new();
    for (idx, ch) in input.chars().enumerate() {
        if idx >= max_chars {
            break;
        }
        out.push(ch);
    }
    out.push_str("...");
    out
}

fn build_session_trace_events(
    messages: &[crate::agent_session::SessionMessage],
    limit: usize,
) -> Vec<SessionTraceEvent> {
    let mut events: Vec<SessionTraceEvent> = Vec::new();

    for msg in messages {
        let timestamp = msg.timestamp.clone();
        let role = msg.role.clone();
        let message_event = if role == "assistant" {
            "assistant_message"
        } else {
            "user_message"
        };

        match crate::llm::parse_session_content(&msg.content) {
            crate::llm::MessageContent::Text(text) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    events.push(SessionTraceEvent {
                        seq: 0,
                        timestamp,
                        role,
                        event_type: message_event.to_string(),
                        text: Some(truncate_trace_text(trimmed, 1200)),
                        tool_name: None,
                        call_id: None,
                        is_error: None,
                    });
                }
            }
            crate::llm::MessageContent::Blocks(blocks) => {
                for block in blocks {
                    match block {
                        crate::llm::ContentBlock::Text { text } => {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                events.push(SessionTraceEvent {
                                    seq: 0,
                                    timestamp: timestamp.clone(),
                                    role: role.clone(),
                                    event_type: message_event.to_string(),
                                    text: Some(truncate_trace_text(trimmed, 1200)),
                                    tool_name: None,
                                    call_id: None,
                                    is_error: None,
                                });
                            }
                        }
                        crate::llm::ContentBlock::ToolUse {
                            id, name, input, ..
                        } => {
                            events.push(SessionTraceEvent {
                                seq: 0,
                                timestamp: timestamp.clone(),
                                role: role.clone(),
                                event_type: "tool_call".to_string(),
                                text: Some(truncate_trace_text(&input.to_string(), 800)),
                                tool_name: Some(name),
                                call_id: Some(id),
                                is_error: None,
                            });
                        }
                        crate::llm::ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            events.push(SessionTraceEvent {
                                seq: 0,
                                timestamp: timestamp.clone(),
                                role: role.clone(),
                                event_type: "tool_result".to_string(),
                                text: Some(truncate_trace_text(content.trim(), 1200)),
                                tool_name: None,
                                call_id: Some(tool_use_id),
                                is_error: Some(is_error),
                            });
                        }
                        crate::llm::ContentBlock::Thinking { .. }
                        | crate::llm::ContentBlock::Image { .. } => {}
                    }
                }
            }
        }
    }

    let mut events = if events.len() > limit {
        events.split_off(events.len() - limit)
    } else {
        events
    };
    for (idx, event) in events.iter_mut().enumerate() {
        event.seq = idx + 1;
    }
    events
}

fn workspace_root_and_canonical(app_data_dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    let workspace_root = app_data_dir.join("workspace");
    fs::create_dir_all(&workspace_root).map_err(|e| {
        format!(
            "Failed to create workspace root '{}': {}",
            workspace_root.display(),
            e
        )
    })?;
    let workspace_canon = fs::canonicalize(&workspace_root).map_err(|e| {
        format!(
            "Failed to canonicalize workspace root '{}': {}",
            workspace_root.display(),
            e
        )
    })?;
    Ok((workspace_root, workspace_canon))
}

fn workspace_root_and_canonical_from_params(
    app_data_dir: &Path,
    params: &Value,
) -> Result<(PathBuf, PathBuf), String> {
    let custom_root = params
        .get("workspace_root")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    if let Some(root_raw) = custom_root {
        let expanded = shellexpand::tilde(root_raw).to_string();
        let root = PathBuf::from(expanded);
        if !root.is_absolute() {
            return Err("workspace_root must be an absolute path".to_string());
        }
        if !root.exists() {
            return Err(format!("workspace_root not found: {}", root.display()));
        }
        if !root.is_dir() {
            return Err(format!(
                "workspace_root is not a directory: {}",
                root.display()
            ));
        }
        let root_canon = fs::canonicalize(&root).map_err(|e| {
            format!(
                "Failed to canonicalize workspace_root '{}': {}",
                root.display(),
                e
            )
        })?;
        return Ok((root, root_canon));
    }

    workspace_root_and_canonical(app_data_dir)
}

fn normalize_workspace_relative(raw_path: &str) -> Result<PathBuf, String> {
    let raw = raw_path.trim();
    if raw.is_empty() || raw == "." || raw == "/" {
        return Ok(PathBuf::new());
    }

    let path = Path::new(raw);
    if path.is_absolute() {
        return Err("Path must be workspace-relative".to_string());
    }

    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(seg) => out.push(seg),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Path cannot contain '..' or absolute prefixes".to_string());
            }
        }
    }
    Ok(out)
}

fn resolve_workspace_existing_path(
    workspace_root: &Path,
    workspace_canon: &Path,
    raw_path: Option<&str>,
) -> Result<PathBuf, String> {
    let relative = normalize_workspace_relative(raw_path.unwrap_or("."))?;
    let target = workspace_root.join(relative);
    if !target.exists() {
        return Err(format!("Path not found: {}", target.display()));
    }
    let canonical = fs::canonicalize(&target)
        .map_err(|e| format!("Failed to resolve path '{}': {}", target.display(), e))?;
    if !canonical.starts_with(workspace_canon) {
        return Err("Path escapes workspace root".to_string());
    }
    Ok(canonical)
}

fn workspace_relative_string(workspace_canon: &Path, absolute: &Path) -> String {
    if let Ok(rel) = absolute.strip_prefix(workspace_canon) {
        let v = rel.to_string_lossy().replace('\\', "/");
        if v.is_empty() {
            ".".to_string()
        } else {
            v
        }
    } else {
        absolute.to_string_lossy().to_string()
    }
}

fn metadata_modified_at_ms(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

fn workspace_entry_type(file_type: &std::fs::FileType) -> &'static str {
    if file_type.is_symlink() {
        "symlink"
    } else if file_type.is_dir() {
        "directory"
    } else if file_type.is_file() {
        "file"
    } else {
        "other"
    }
}

fn workspace_join_relative_path(parent: &str, child_name: &str) -> String {
    if parent.is_empty() || parent == "." {
        child_name.to_string()
    } else {
        format!("{parent}/{child_name}")
    }
}

// ============================================================================
// CLI Eval Persona Helper
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_session::SessionMessage;
    use crate::llm_profile;
    use crate::scheduler::{
        ScheduleType, ScheduledTask, TaskExecutionType, TaskRunStatus, TaskState,
    };
    use cteno_host_session_registry::{
        BackgroundTaskCategory, BackgroundTaskRecord, BackgroundTaskRegistry, BackgroundTaskStatus,
    };
    use tempfile::tempdir;

    fn ensure_background_task_registry_for_tests() -> Arc<BackgroundTaskRegistry> {
        if let Ok(registry) = crate::local_services::background_task_registry() {
            registry
        } else {
            let registry = Arc::new(BackgroundTaskRegistry::new());
            crate::local_services::install_background_task_registry(registry.clone());
            registry
        }
    }

    fn sample_background_task_record(
        task_id: &str,
        session_id: &str,
        category: BackgroundTaskCategory,
        status: BackgroundTaskStatus,
    ) -> BackgroundTaskRecord {
        BackgroundTaskRecord {
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            vendor: "codex".to_string(),
            category,
            task_type: "background_session".to_string(),
            description: Some(format!("task {task_id}")),
            summary: None,
            status,
            started_at: 1_700_000_000_000,
            completed_at: None,
            tool_use_id: None,
            output_file: None,
            vendor_extra: json!({}),
        }
    }

    fn sample_scheduled_task(task_id: &str, session_id: &str) -> ScheduledTask {
        ScheduledTask {
            id: task_id.to_string(),
            name: format!("scheduled {task_id}"),
            task_prompt: "echo scheduled".to_string(),
            enabled: true,
            delete_after_run: false,
            schedule: ScheduleType::At {
                at: "2026-04-20T00:00:00Z".to_string(),
            },
            timezone: "UTC".to_string(),
            session_id: session_id.to_string(),
            persona_id: Some("persona-1".to_string()),
            task_type: TaskExecutionType::Dispatch,
            state: TaskState {
                next_run_at: Some(1_800_000_000_000),
                running_since: None,
                last_run_at: Some(1_700_000_123_456),
                last_status: Some(TaskRunStatus::Success),
                last_result_summary: Some("scheduled ok".to_string()),
                consecutive_errors: 0,
                total_runs: 1,
            },
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_123_456,
        }
    }

    #[tokio::test]
    async fn local_first_runtime_registers_shared_machine_ui_methods() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        let app_data_dir = temp.path().join("app-data");
        let builtin_skills_dir = temp.path().join("builtin-skills");
        let user_skills_dir = temp.path().join("user-skills");
        std::fs::create_dir_all(&app_data_dir).unwrap();
        std::fs::create_dir_all(&builtin_skills_dir).unwrap();
        std::fs::create_dir_all(&user_skills_dir).unwrap();

        let manager = HappyClientManager::new();
        manager
            .register_machine_scoped_ui_rpc_handlers(MachineUiRpcContext {
                machine_id: "machine-local".to_string(),
                db_path: db_path.clone(),
                app_data_dir: app_data_dir.clone(),
                api_key: String::new(),
                system_prompt_text: "test system prompt".to_string(),
                builtin_skills_dir: builtin_skills_dir.clone(),
                user_skills_dir: user_skills_dir.clone(),
                builtin_agents_dir: temp.path().join("builtin-agents"),
                user_agents_dir: temp.path().join("user-agents"),
            })
            .await;

        let rpc_methods = MachineRpcMethods::new("machine-local");
        let registry = manager.rpc_registry();

        assert!(registry.has_method(&rpc_methods.list_personas).await);
        assert!(registry.has_method(&rpc_methods.list_scheduled_tasks).await);
        assert!(
            registry
                .has_method(&rpc_methods.list_background_tasks)
                .await
        );
        assert!(registry.has_method(&rpc_methods.get_background_task).await);
        assert!(registry.has_method(&rpc_methods.list_runs).await);
        assert!(registry.has_method(&rpc_methods.list_subagents).await);
        assert!(registry.has_method(&rpc_methods.bash).await);
        assert!(registry.has_method(&rpc_methods.kill_session).await);
        assert!(registry.has_method(&rpc_methods.memory_list).await);
        assert!(registry.has_method(&rpc_methods.memory_read).await);
        assert!(registry.has_method(&rpc_methods.memory_write).await);
        assert!(registry.has_method(&rpc_methods.memory_delete).await);
        assert!(registry.has_method(&rpc_methods.workspace_list).await);
        assert!(registry.has_method(&rpc_methods.workspace_read).await);
        assert!(registry.has_method(&rpc_methods.workspace_stat).await);
        assert!(
            registry
                .has_method("machine-local:get-agent-latest-text")
                .await
        );
        assert!(
            registry
                .has_method("machine-local:get-agent-artifacts")
                .await
        );
        assert!(registry.has_method("machine-local:get-target-page").await);
        assert!(
            registry
                .has_method("machine-local:target-page-response")
                .await
        );
        assert!(
            registry
                .has_method("machine-local:get-goal-tree-page")
                .await
        );
        assert!(registry.has_method("machine-local:get-dashboard").await);
        assert!(
            registry
                .has_method("machine-local:list-notifications")
                .await
        );
        assert!(
            registry
                .has_method("machine-local:mark-notification-read")
                .await
        );
        assert!(registry.has_method(&rpc_methods.list_agents).await);
        assert!(
            registry
                .has_method("machine-local:get-notification-subscriptions")
                .await
        );
    }

    #[tokio::test]
    async fn shared_machine_ui_background_task_rpcs_validate_filters_and_fetch_records() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        let app_data_dir = temp.path().join("app-data");
        let builtin_skills_dir = temp.path().join("builtin-skills");
        let user_skills_dir = temp.path().join("user-skills");
        let builtin_agents_dir = temp.path().join("builtin-agents");
        let user_agents_dir = temp.path().join("user-agents");
        std::fs::create_dir_all(&app_data_dir).unwrap();
        std::fs::create_dir_all(&builtin_skills_dir).unwrap();
        std::fs::create_dir_all(&user_skills_dir).unwrap();
        std::fs::create_dir_all(&builtin_agents_dir).unwrap();
        std::fs::create_dir_all(&user_agents_dir).unwrap();

        let background_registry = ensure_background_task_registry_for_tests();
        let running_task_id = "bg05-running-task";
        let completed_task_id = "bg05-completed-task";
        let scheduled_task_id = "bg05-scheduled-task";
        let merged_session_id = "bg05-session-merged";
        background_registry.remove(running_task_id);
        background_registry.remove(completed_task_id);
        let scheduler = Arc::new(crate::scheduler::TaskScheduler::new(db_path.clone()));
        scheduler
            .create_task(&sample_scheduled_task(scheduled_task_id, merged_session_id))
            .unwrap();
        background_registry.set_scheduled_job_source(Arc::new(
            crate::local_services::LocalScheduledJobSource::new(scheduler),
        ));
        background_registry.upsert(sample_background_task_record(
            running_task_id,
            merged_session_id,
            BackgroundTaskCategory::ExecutionTask,
            BackgroundTaskStatus::Running,
        ));
        background_registry.upsert(sample_background_task_record(
            completed_task_id,
            merged_session_id,
            BackgroundTaskCategory::ExecutionTask,
            BackgroundTaskStatus::Completed,
        ));

        let manager = HappyClientManager::new();
        manager
            .register_machine_scoped_ui_rpc_handlers(MachineUiRpcContext {
                machine_id: "machine-local".to_string(),
                db_path: db_path.clone(),
                app_data_dir: app_data_dir.clone(),
                api_key: String::new(),
                system_prompt_text: "test system prompt".to_string(),
                builtin_skills_dir: builtin_skills_dir.clone(),
                user_skills_dir: user_skills_dir.clone(),
                builtin_agents_dir: builtin_agents_dir.clone(),
                user_agents_dir: user_agents_dir.clone(),
            })
            .await;

        let registry = manager.rpc_registry();
        let rpc_methods = MachineRpcMethods::new("machine-local");

        let list_all_result = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "list-background-tasks-all".to_string(),
                method: rpc_methods.list_background_tasks.clone(),
                params: json!({ "sessionId": merged_session_id }),
            })
            .await
            .result
            .unwrap();
        assert_eq!(
            list_all_result.get("success").and_then(|v| v.as_bool()),
            Some(true)
        );
        let all_tasks = list_all_result
            .get("data")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap();
        assert_eq!(all_tasks.len(), 3);
        assert!(all_tasks
            .iter()
            .any(|task| { task.get("taskId").and_then(|v| v.as_str()) == Some(running_task_id) }));
        assert!(all_tasks.iter().any(|task| {
            task.get("taskId").and_then(|v| v.as_str()) == Some(completed_task_id)
        }));
        let scheduled_task = all_tasks
            .iter()
            .find(|task| task.get("taskId").and_then(|v| v.as_str()) == Some(scheduled_task_id))
            .cloned()
            .expect("scheduled task projection should be present");
        assert_eq!(scheduled_task.get("category"), Some(&json!("scheduledJob")));
        assert_eq!(
            scheduled_task.get("taskType"),
            Some(&json!("scheduled_job"))
        );
        assert_eq!(scheduled_task.get("status"), Some(&json!("completed")));

        let execution_only_result = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "list-background-tasks-execution-only".to_string(),
                method: rpc_methods.list_background_tasks.clone(),
                params: json!({
                    "sessionId": merged_session_id,
                    "category": "execution",
                }),
            })
            .await
            .result
            .unwrap();
        assert_eq!(
            execution_only_result,
            json!({
                "success": true,
                "data": [
                    sample_background_task_record(
                        completed_task_id,
                        merged_session_id,
                        BackgroundTaskCategory::ExecutionTask,
                        BackgroundTaskStatus::Completed,
                    ),
                    sample_background_task_record(
                        running_task_id,
                        merged_session_id,
                        BackgroundTaskCategory::ExecutionTask,
                        BackgroundTaskStatus::Running,
                    )
                ],
            })
        );

        let scheduled_only_result = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "list-background-tasks-scheduled-only".to_string(),
                method: rpc_methods.list_background_tasks.clone(),
                params: json!({
                    "sessionId": merged_session_id,
                    "category": "scheduled_job",
                }),
            })
            .await
            .result
            .unwrap();
        let scheduled_only_tasks = scheduled_only_result
            .get("data")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap();
        assert_eq!(scheduled_only_result.get("success"), Some(&json!(true)));
        assert_eq!(scheduled_only_tasks.len(), 1);
        assert_eq!(
            scheduled_only_tasks[0].get("taskId"),
            Some(&json!(scheduled_task_id))
        );
        assert_eq!(
            scheduled_only_tasks[0].get("category"),
            Some(&json!("scheduledJob"))
        );

        let invalid_category_result = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "list-background-tasks-invalid-category".to_string(),
                method: rpc_methods.list_background_tasks.clone(),
                params: json!({ "category": "bogus" }),
            })
            .await
            .result
            .unwrap();
        assert_eq!(
            invalid_category_result,
            json!({ "success": false, "error": "Invalid category" })
        );

        let invalid_status_result = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "list-background-tasks-invalid-status".to_string(),
                method: rpc_methods.list_background_tasks.clone(),
                params: json!({ "status": "bogus" }),
            })
            .await
            .result
            .unwrap();
        assert_eq!(
            invalid_status_result,
            json!({ "success": false, "error": "Invalid status" })
        );

        let get_missing_task_id_result = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "get-background-task-missing-id".to_string(),
                method: rpc_methods.get_background_task.clone(),
                params: json!({}),
            })
            .await
            .result
            .unwrap();
        assert_eq!(
            get_missing_task_id_result,
            json!({ "success": false, "error": "Missing taskId" })
        );

        let get_unknown_result = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "get-background-task-unknown".to_string(),
                method: rpc_methods.get_background_task.clone(),
                params: json!({ "taskId": "bg05-missing-task" }),
            })
            .await
            .result
            .unwrap();
        assert_eq!(
            get_unknown_result,
            json!({ "success": false, "error": "Task not found" })
        );

        let get_known_result = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "get-background-task-known".to_string(),
                method: rpc_methods.get_background_task.clone(),
                params: json!({ "taskId": completed_task_id }),
            })
            .await
            .result
            .unwrap();
        assert_eq!(
            get_known_result,
            json!({
                "success": true,
                "data": sample_background_task_record(
                    completed_task_id,
                    merged_session_id,
                    BackgroundTaskCategory::ExecutionTask,
                    BackgroundTaskStatus::Completed,
                ),
            })
        );

        background_registry.remove(running_task_id);
        background_registry.remove(completed_task_id);
    }

    #[tokio::test]
    async fn shared_machine_ui_workspace_rpc_supports_custom_workspace_root() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        let app_data_dir = temp.path().join("app-data");
        let builtin_skills_dir = temp.path().join("builtin-skills");
        let user_skills_dir = temp.path().join("user-skills");
        let builtin_agents_dir = temp.path().join("builtin-agents");
        let user_agents_dir = temp.path().join("user-agents");
        let custom_root = temp.path().join("custom-root");
        let nested_dir = custom_root.join("Projects");
        let nested_file = nested_dir.join("note.txt");

        std::fs::create_dir_all(&app_data_dir).unwrap();
        std::fs::create_dir_all(&builtin_skills_dir).unwrap();
        std::fs::create_dir_all(&user_skills_dir).unwrap();
        std::fs::create_dir_all(&builtin_agents_dir).unwrap();
        std::fs::create_dir_all(&user_agents_dir).unwrap();
        std::fs::create_dir_all(&nested_dir).unwrap();
        std::fs::write(&nested_file, "hello workspace root").unwrap();

        let manager = HappyClientManager::new();
        manager
            .register_machine_scoped_ui_rpc_handlers(MachineUiRpcContext {
                machine_id: "machine-local".to_string(),
                db_path: db_path.clone(),
                app_data_dir: app_data_dir.clone(),
                api_key: String::new(),
                system_prompt_text: "test system prompt".to_string(),
                builtin_skills_dir: builtin_skills_dir.clone(),
                user_skills_dir: user_skills_dir.clone(),
                builtin_agents_dir: builtin_agents_dir.clone(),
                user_agents_dir: user_agents_dir.clone(),
            })
            .await;

        let registry = manager.rpc_registry();
        let rpc_methods = MachineRpcMethods::new("machine-local");
        let workspace_root = custom_root.to_string_lossy().to_string();

        let list_response = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "workspace-list".to_string(),
                method: rpc_methods.workspace_list.clone(),
                params: json!({
                    "path": ".",
                    "workspace_root": workspace_root,
                }),
            })
            .await;
        let list_result = list_response.result.unwrap();
        assert_eq!(
            list_result.get("success").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(list_result.get("path").and_then(|v| v.as_str()), Some("."));
        let entries = list_result
            .get("entries")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap();
        assert!(
            entries.iter().any(|entry| {
                entry.get("name").and_then(|v| v.as_str()) == Some("Projects")
                    && entry.get("path").and_then(|v| v.as_str()) == Some("Projects")
                    && entry.get("type").and_then(|v| v.as_str()) == Some("directory")
            }),
            "expected Projects directory in workspace listing: {entries:?}"
        );

        let stat_response = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "workspace-stat".to_string(),
                method: rpc_methods.workspace_stat.clone(),
                params: json!({
                    "paths": ["Projects/note.txt", "missing.txt"],
                    "workspace_root": custom_root.to_string_lossy().to_string(),
                }),
            })
            .await;
        let stat_result = stat_response.result.unwrap();
        assert_eq!(
            stat_result.get("success").and_then(|v| v.as_bool()),
            Some(true)
        );
        let items = stat_result
            .get("items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap();
        assert!(
            items.iter().any(|item| {
                item.get("path").and_then(|v| v.as_str()) == Some("Projects/note.txt")
                    && item.get("exists").and_then(|v| v.as_bool()) == Some(true)
                    && item.get("type").and_then(|v| v.as_str()) == Some("file")
            }),
            "expected Projects/note.txt stat item: {items:?}"
        );
        assert!(
            items.iter().any(|item| {
                item.get("path").and_then(|v| v.as_str()) == Some("missing.txt")
                    && item.get("exists").and_then(|v| v.as_bool()) == Some(false)
            }),
            "expected missing.txt stat item: {items:?}"
        );

        let read_response = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "workspace-read".to_string(),
                method: rpc_methods.workspace_read.clone(),
                params: json!({
                    "path": "Projects/note.txt",
                    "workspace_root": custom_root.to_string_lossy().to_string(),
                    "encoding": "utf8",
                }),
            })
            .await;
        let read_result = read_response.result.unwrap();
        let modified_at = read_result
            .get("modifiedAt")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        assert_eq!(
            read_result,
            json!({
                "success": true,
                "path": "Projects/note.txt",
                "encoding": "utf8",
                "data": "hello workspace root",
                "bytesRead": 20,
                "offset": 0,
                "nextOffset": 20,
                "size": 20,
                "eof": true,
                "modifiedAt": modified_at,
            })
        );
    }

    #[tokio::test]
    async fn shared_machine_ui_memory_rpcs_support_global_files() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        let app_data_dir = temp.path().join("app-data");
        let builtin_skills_dir = temp.path().join("builtin-skills");
        let user_skills_dir = temp.path().join("user-skills");
        let builtin_agents_dir = temp.path().join("builtin-agents");
        let user_agents_dir = temp.path().join("user-agents");
        std::fs::create_dir_all(&app_data_dir).unwrap();
        std::fs::create_dir_all(&builtin_skills_dir).unwrap();
        std::fs::create_dir_all(&user_skills_dir).unwrap();
        std::fs::create_dir_all(&builtin_agents_dir).unwrap();
        std::fs::create_dir_all(&user_agents_dir).unwrap();

        let manager = HappyClientManager::new();
        manager
            .register_machine_scoped_ui_rpc_handlers(MachineUiRpcContext {
                machine_id: "machine-local".to_string(),
                db_path: db_path.clone(),
                app_data_dir: app_data_dir.clone(),
                api_key: String::new(),
                system_prompt_text: "test system prompt".to_string(),
                builtin_skills_dir: builtin_skills_dir.clone(),
                user_skills_dir: user_skills_dir.clone(),
                builtin_agents_dir: builtin_agents_dir.clone(),
                user_agents_dir: user_agents_dir.clone(),
            })
            .await;

        let registry = manager.rpc_registry();
        let rpc_methods = MachineRpcMethods::new("machine-local");

        let write_result = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "memory-write".to_string(),
                method: rpc_methods.memory_write.clone(),
                params: json!({
                    "file_path": "notes/sample.md",
                    "content": "hello memory",
                }),
            })
            .await
            .result
            .unwrap();
        assert_eq!(write_result, json!({ "success": true }));

        let list_result = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "memory-list".to_string(),
                method: rpc_methods.memory_list.clone(),
                params: json!({}),
            })
            .await
            .result
            .unwrap();
        let files = list_result
            .get("data")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        assert_eq!(list_result.get("success"), Some(&json!(true)));
        assert!(
            files
                .iter()
                .any(|value| value.as_str() == Some("[global] notes/sample.md")),
            "expected [global] notes/sample.md in list: {files:?}"
        );

        let read_result = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "memory-read".to_string(),
                method: rpc_methods.memory_read.clone(),
                params: json!({ "file_path": "notes/sample.md" }),
            })
            .await
            .result
            .unwrap();
        assert_eq!(
            read_result,
            json!({ "success": true, "data": "hello memory" })
        );

        let delete_result = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "memory-delete".to_string(),
                method: rpc_methods.memory_delete.clone(),
                params: json!({ "file_path": "notes/sample.md" }),
            })
            .await
            .result
            .unwrap();
        assert_eq!(delete_result, json!({ "success": true }));

        let list_after_delete = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "memory-list-after-delete".to_string(),
                method: rpc_methods.memory_list.clone(),
                params: json!({}),
            })
            .await
            .result
            .unwrap();
        let files_after_delete = list_after_delete
            .get("data")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        assert_eq!(list_after_delete.get("success"), Some(&json!(true)));
        assert!(
            !files_after_delete
                .iter()
                .any(|value| value.as_str() == Some("[global] notes/sample.md")),
            "file should be removed: {files_after_delete:?}"
        );
    }

    #[tokio::test]
    async fn shared_machine_ui_agent_rpc_defaults_use_local_session_store() {
        let temp = tempdir().unwrap();
        crate::db::init_at_data_dir(temp.path()).unwrap();
        let db_path = temp.path().join("db").join("cteno.db");
        let app_data_dir = temp.path().join("app-data");
        let builtin_skills_dir = temp.path().join("builtin-skills");
        let user_skills_dir = temp.path().join("user-skills");
        let builtin_agents_dir = temp.path().join("builtin-agents");
        let user_agents_dir = temp.path().join("user-agents");
        std::fs::create_dir_all(&app_data_dir).unwrap();
        std::fs::create_dir_all(&builtin_skills_dir).unwrap();
        std::fs::create_dir_all(&user_skills_dir).unwrap();
        std::fs::create_dir_all(&builtin_agents_dir).unwrap();
        std::fs::create_dir_all(&user_agents_dir).unwrap();

        std::fs::create_dir_all(builtin_agents_dir.join("worker")).unwrap();
        std::fs::write(
            builtin_agents_dir.join("worker").join("AGENT.md"),
            "---\nname: Builtin Worker\ndescription: Handles built-in work.\nversion: 1.0.0\ntype: autonomous\n---\n\nBuiltin worker instructions.\n",
        )
        .unwrap();

        std::fs::create_dir_all(user_agents_dir.join("global-helper")).unwrap();
        std::fs::write(
            user_agents_dir.join("global-helper").join("AGENT.md"),
            "---\nname: Global Helper\ndescription: Handles global work.\nversion: 1.0.0\nallowed_tools:\n  - read\nexpose_as_tool: true\n---\n\nGlobal helper instructions.\n",
        )
        .unwrap();

        let workspace_agents_dir = temp.path().join("workspace").join(".cteno").join("agents");
        std::fs::create_dir_all(workspace_agents_dir.join("workspace-helper")).unwrap();
        std::fs::write(
            workspace_agents_dir.join("workspace-helper").join("AGENT.md"),
            "---\nname: Workspace Helper\ndescription: Handles workspace work.\nversion: 1.0.0\nmodel: gpt-5.4\nexcluded_tools:\n  - bash\n---\n\nWorkspace helper instructions.\n",
        )
        .unwrap();

        let session_manager = AgentSessionManager::new(db_path.clone());
        session_manager
            .create_session_with_id("agent-session-1", "worker", None, None)
            .unwrap();
        session_manager
            .update_messages(
                "agent-session-1",
                &[
                    SessionMessage {
                        role: "user".to_string(),
                        content: "hello".to_string(),
                        timestamp: "2026-01-01T00:00:00Z".to_string(),
                        local_id: None,
                    },
                    SessionMessage {
                        role: "assistant".to_string(),
                        content: "latest answer".to_string(),
                        timestamp: "2026-01-01T00:00:01Z".to_string(),
                        local_id: None,
                    },
                ],
            )
            .unwrap();

        let manager = HappyClientManager::new();
        manager
            .register_machine_scoped_ui_rpc_handlers(MachineUiRpcContext {
                machine_id: "machine-local".to_string(),
                db_path: db_path.clone(),
                app_data_dir: app_data_dir.clone(),
                api_key: String::new(),
                system_prompt_text: "test system prompt".to_string(),
                builtin_skills_dir: builtin_skills_dir.clone(),
                user_skills_dir: user_skills_dir.clone(),
                builtin_agents_dir: builtin_agents_dir.clone(),
                user_agents_dir: user_agents_dir.clone(),
            })
            .await;

        let registry = manager.rpc_registry();

        let latest_text = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "latest-text".to_string(),
                method: "machine-local:get-agent-latest-text".to_string(),
                params: json!({ "agentId": "agent-session-1" }),
            })
            .await;
        assert_eq!(
            latest_text.result.unwrap(),
            json!({ "success": true, "text": "latest answer" })
        );

        let artifacts = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "artifacts".to_string(),
                method: "machine-local:get-agent-artifacts".to_string(),
                params: json!({ "agentId": "agent-session-1" }),
            })
            .await;
        assert_eq!(
            artifacts.result.unwrap(),
            json!({ "success": true, "artifacts": [] })
        );

        let dashboard = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "dashboard".to_string(),
                method: "machine-local:get-dashboard".to_string(),
                params: json!({ "agentId": "agent-session-1" }),
            })
            .await;
        assert_eq!(
            dashboard.result.unwrap(),
            json!({ "success": true, "page": null })
        );

        let listed_agents = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "list-agents".to_string(),
                method: "machine-local:list-agents".to_string(),
                params: json!({ "workdir": temp.path().join("workspace") }),
            })
            .await;
        assert_eq!(
            listed_agents.result.unwrap(),
            json!({
                "success": true,
                "agents": [
                    {
                        "id": "global-helper",
                        "name": "Global Helper",
                        "description": "Handles global work.",
                        "version": "1.0.0",
                        "agent_type": "passthrough",
                        "instructions": "Global helper instructions.",
                        "model": "",
                        "temperature": null,
                        "max_tokens": null,
                        "tools": [],
                        "skills": [],
                        "source": "global",
                        "allowed_tools": ["read"],
                        "excluded_tools": [],
                        "expose_as_tool": true
                    },
                    {
                        "id": "worker",
                        "name": "Builtin Worker",
                        "description": "Handles built-in work.",
                        "version": "1.0.0",
                        "agent_type": "autonomous",
                        "instructions": "Builtin worker instructions.",
                        "model": "",
                        "temperature": null,
                        "max_tokens": null,
                        "tools": [],
                        "skills": [],
                        "source": "builtin",
                        "allowed_tools": [],
                        "excluded_tools": [],
                        "expose_as_tool": false
                    },
                    {
                        "id": "workspace-helper",
                        "name": "Workspace Helper",
                        "description": "Handles workspace work.",
                        "version": "1.0.0",
                        "agent_type": "passthrough",
                        "instructions": "Workspace helper instructions.",
                        "model": "gpt-5.4",
                        "temperature": null,
                        "max_tokens": null,
                        "tools": [],
                        "skills": [],
                        "source": "workspace",
                        "allowed_tools": [],
                        "excluded_tools": ["bash"],
                        "expose_as_tool": false
                    }
                ]
            })
        );

        let notifications = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "notifications".to_string(),
                method: "machine-local:list-notifications".to_string(),
                params: json!({ "agentId": "agent-session-1", "limit": 50 }),
            })
            .await;
        assert_eq!(
            notifications.result.unwrap(),
            json!({ "success": true, "notifications": [] })
        );

        let notification_subscriptions = registry
            .handle(cteno_host_rpc_core::RpcRequest {
                request_id: "notification-subscriptions".to_string(),
                method: "machine-local:get-notification-subscriptions".to_string(),
                params: json!({ "personaId": "persona-1" }),
            })
            .await;
        assert_eq!(
            notification_subscriptions.result.unwrap(),
            json!({ "success": true, "subscriptions": [] })
        );
    }

    #[tokio::test]
    async fn resolve_persona_model_selection_uses_direct_default_without_login_for_cteno() {
        let profile_store = Arc::new(RwLock::new(ProfileStore {
            profiles: vec![llm_profile::get_default_profile()],
            default_profile_id: llm_profile::DEFAULT_PROXY_PROFILE.to_string(),
        }));

        let resolved =
            resolve_persona_model_selection_with_auth_state(profile_store, None, "cteno", false)
                .await;

        assert_eq!(resolved, llm_profile::DEFAULT_DIRECT_PROFILE);
    }

    #[tokio::test]
    async fn resolve_persona_model_selection_keeps_proxy_default_with_login_for_cteno() {
        let profile_store = Arc::new(RwLock::new(ProfileStore {
            profiles: vec![llm_profile::get_default_profile()],
            default_profile_id: llm_profile::DEFAULT_PROXY_PROFILE.to_string(),
        }));

        let resolved =
            resolve_persona_model_selection_with_auth_state(profile_store, None, "cteno", true)
                .await;

        assert_eq!(resolved, llm_profile::DEFAULT_PROXY_PROFILE);
    }

    #[tokio::test]
    async fn resolve_persona_model_selection_rewrites_proxy_request_without_login_for_cteno() {
        let profile_store = Arc::new(RwLock::new(ProfileStore {
            profiles: vec![llm_profile::get_default_profile()],
            default_profile_id: llm_profile::DEFAULT_PROXY_PROFILE.to_string(),
        }));

        let resolved = resolve_persona_model_selection_with_auth_state(
            profile_store,
            Some(llm_profile::DEFAULT_PROXY_PROFILE),
            "cteno",
            false,
        )
        .await;

        assert_eq!(resolved, llm_profile::DEFAULT_DIRECT_PROFILE);
    }

    #[tokio::test]
    async fn resolve_persona_model_selection_prefers_store_direct_default_without_login() {
        let profile_store = Arc::new(RwLock::new(ProfileStore {
            profiles: vec![
                llm_profile::LlmProfile {
                    id: "user-direct".to_string(),
                    name: "User Direct".to_string(),
                    chat: llm_profile::LlmEndpoint {
                        api_key: "user-key".to_string(),
                        base_url: "https://example.com".to_string(),
                        model: "gpt-5.1".to_string(),
                        temperature: 0.2,
                        max_tokens: 4096,
                        context_window_tokens: None,
                    },
                    compress: llm_profile::LlmEndpoint {
                        api_key: String::new(),
                        base_url: "https://example.com".to_string(),
                        model: "gpt-5.1-mini".to_string(),
                        temperature: 0.1,
                        max_tokens: 1024,
                        context_window_tokens: None,
                    },
                    supports_vision: false,
                    supports_computer_use: false,
                    api_format: llm_profile::ApiFormat::Anthropic,
                    thinking: false,
                    is_free: false,
                    supports_function_calling: true,
                    supports_image_output: false,
                },
                llm_profile::get_default_profile(),
            ],
            default_profile_id: "user-direct".to_string(),
        }));

        let resolved =
            resolve_persona_model_selection_with_auth_state(profile_store, None, "cteno", false)
                .await;

        assert_eq!(resolved, "user-direct");
    }
}
