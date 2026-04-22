//! Agent runtime hook implementations.
//!
//! The runtime crate (`cteno-agent-runtime`) defines a set of `trait` seams in
//! `cteno_agent_runtime::hooks` for capabilities it needs from the host.
//! This module provides the app-side implementations and installs them during
//! boot via [`install_all`].
//!
//! Wave 2.3a: skill / a2ui_render / start_subagent executors migrated into the
//! runtime, driven through four new hooks (SkillRegistryProvider,
//! PersonaDispatchProvider, A2uiStoreProvider, SubagentBootstrapProvider).
//! Community builds install stub-capable impls where possible; paths that
//! require PersonaManager / SpawnSessionConfig fail loudly when called.

#![allow(dead_code)]

use async_trait::async_trait;
use std::sync::Arc;

use cteno_agent_runtime::hooks as rt;
#[cfg(all(
    not(any(target_os = "android", target_os = "ios")),
    not(target_os = "macos")
))]
use tauri_plugin_notification::NotificationExt;

// ---------------------------------------------------------------------------
// 2.1 ToolRegistryProvider — thin façade over the concrete `ToolRegistry`
// handle (already runtime-native).  Only 3 methods are trait-visible; executors
// that need richer access grab `tool_registry_handle()` directly.
// ---------------------------------------------------------------------------

struct AppToolRegistryProvider {
    handle: Arc<tokio::sync::RwLock<crate::tool::registry::ToolRegistry>>,
}

#[async_trait]
impl rt::ToolRegistryProvider for AppToolRegistryProvider {
    async fn execute(&self, tool_name: &str, input: serde_json::Value) -> Result<String, String> {
        let reg = self.handle.read().await;
        reg.execute(tool_name, input).await
    }

    async fn list_tools(&self) -> Vec<String> {
        let reg = self.handle.read().await;
        reg.get_tools_for_llm()
            .into_iter()
            .map(|t| t.name)
            .collect()
    }

    async fn describe(&self, tool_name: &str) -> Option<serde_json::Value> {
        let reg = self.handle.read().await;
        reg.get_tools_for_llm()
            .into_iter()
            .find(|t| t.name == tool_name)
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
    }
}

// ---------------------------------------------------------------------------
// 2.2 CommandInterceptor — wraps `command_interceptor::CommandHandler`.
// Used by the ReAct queue path to short-circuit slash commands before they
// reach the LLM.
// ---------------------------------------------------------------------------

struct AppCommandInterceptor {
    db_path: std::path::PathBuf,
}

#[async_trait]
impl rt::CommandInterceptor for AppCommandInterceptor {
    async fn intercept(
        &self,
        session_id: &str,
        user_message: &str,
    ) -> Option<rt::InterceptedOutcome> {
        let cmd = crate::command_interceptor::SlashCommand::parse(user_message)?;
        let handler = crate::command_interceptor::CommandHandler::new(self.db_path.clone());
        let response = handler
            .execute(cmd, session_id)
            .await
            .unwrap_or_else(|e| format!("命令执行失败: {}", e));
        Some(rt::InterceptedOutcome {
            message: response,
            stop: true,
        })
    }
}

// ---------------------------------------------------------------------------
// 2.4 ResolvedUrlProvider
// ---------------------------------------------------------------------------

struct AppUrlProvider;

impl rt::ResolvedUrlProvider for AppUrlProvider {
    fn happy_server_url(&self) -> String {
        crate::resolved_happy_server_url()
    }
}

// ---------------------------------------------------------------------------
// 2.5 SpawnConfigProvider — exposes `peek_session_message` so the `wait`
// executor can poll the session queue without depending on the full
// SpawnSessionConfig / SessionRegistry types.
// ---------------------------------------------------------------------------

struct AppSpawnConfigProvider;

#[async_trait]
impl rt::SpawnConfigProvider for AppSpawnConfigProvider {
    async fn peek_session_message(&self, session_id: &str) -> Option<String> {
        let spawn_config = crate::local_services::spawn_config().ok()?;
        let conn = spawn_config.session_connections.get(session_id).await?;
        let queue = conn.queue();
        queue.peek(session_id).map(|msg| msg.content)
    }
}

// ---------------------------------------------------------------------------
// 2.6 AgentOwnerProvider — wraps `agent_owner::resolve_owner_name` and related
// host helpers so runtime executors (e.g. `memory`) can look up display
// labels without depending on the app crate.
// ---------------------------------------------------------------------------

struct AppAgentOwnerProvider;

impl rt::AgentOwnerProvider for AppAgentOwnerProvider {
    fn session_owner(&self, session_id: &str) -> Option<rt::SessionOwner> {
        // Best-effort: resolve_owner() works on owner_id, not session_id, but
        // the session→owner mapping lives in persona_sessions DB.  Runtime
        // call sites currently only need `resolve_owner_name`, so we return
        // None here and revisit when a session-keyed call site appears.
        let _ = session_id;
        None
    }

    fn resolve_owner_name(&self, owner_id: &str) -> Option<String> {
        crate::agent_owner::resolve_owner_name(owner_id)
    }

    fn record_agent_reply(&self, session_id: &str, message: &str) -> Result<(), String> {
        // Currently only used by orchestration paths (ask_persona /
        // dispatch_task) that still live in the app crate — runtime has no
        // call site yet, so this is a no-op until one appears.
        let _ = (session_id, message);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 2.11 SessionWaker — used by crate::runs to recreate released session
// connections so their notification poll loops consume background-run events.
// ---------------------------------------------------------------------------

struct AppSessionWaker;

#[async_trait]
impl rt::SessionWaker for AppSessionWaker {
    async fn wake_session(&self, session_id: &str, label: &str) -> bool {
        if let Ok(spawn_config) = crate::local_services::spawn_config() {
            return crate::session_delivery::ensure_session_connected(
                &spawn_config,
                session_id,
                label,
            )
            .await;
        }
        false
    }
}

// ---------------------------------------------------------------------------
// 2.7 MachineSocketProvider (Wave 2.3a)
// Used by `a2ui_render` executor to push `hypothesis-push` events to the
// frontend after each render batch.  Community build has no machine socket;
// the impl returns an Err that a2ui_render swallows (fire-and-forget).
// ---------------------------------------------------------------------------

struct AppMachineSocketProvider;

#[async_trait]
impl rt::MachineSocketProvider for AppMachineSocketProvider {
    async fn push_to_frontend(
        &self,
        channel: &str,
        payload: serde_json::Value,
    ) -> Result<(), String> {
        let sock = crate::local_services::machine_socket()?;
        sock.emit(channel, payload)
            .await
            .map_err(|e| format!("machine socket emit failed: {}", e))
    }
}

// ---------------------------------------------------------------------------
// 2.18 LocalNotificationProvider (Wave 3.4a)
// Runtime-side push notifications delegate to this provider so desktop
// notifications are emitted by the Cteno app process (correct icon / sender).
// ---------------------------------------------------------------------------

struct AppLocalNotificationProvider;

impl rt::LocalNotificationProvider for AppLocalNotificationProvider {
    fn send_local_notification(&self, title: &str, body: &str) -> Result<(), String> {
        let handle = crate::APP_HANDLE
            .get()
            .cloned()
            .ok_or_else(|| "AppHandle not initialized".to_string())?;

        #[cfg(target_os = "macos")]
        {
            let mut notification = notify_rust::Notification::new();
            notification.summary(title);
            notification.body(body);
            notification.auto_icon();
            let _ = notify_rust::set_application(&handle.config().identifier);
            notification
                .show()
                .map_err(|e| format!("Failed to show macOS local notification: {}", e))?;
            Ok(())
        }

        #[cfg(all(
            not(any(target_os = "android", target_os = "ios")),
            not(target_os = "macos")
        ))]
        {
            handle
                .notification()
                .builder()
                .title(title)
                .body(body)
                .show()
                .map_err(|e| format!("Failed to show local notification: {}", e))?;
            Ok(())
        }

        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let _ = (handle, title, body);
            Err("Local desktop notification unsupported on mobile targets".to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// 2.9 SkillRegistryProvider (Wave 2.3a)
// Delegates FS loading to `service_init::load_all_skills` (host owns the
// builtin/global dir layout) and SkillHub ops to `crate::skillhub::*`.
// ---------------------------------------------------------------------------

struct AppSkillRegistryProvider {
    builtin_skills_dir: std::path::PathBuf,
    user_skills_dir: std::path::PathBuf,
}

#[async_trait]
impl rt::SkillRegistryProvider for AppSkillRegistryProvider {
    fn load_all_skills(
        &self,
        workspace_dir: Option<&std::path::Path>,
    ) -> Vec<cteno_agent_runtime::agent_config::SkillConfig> {
        crate::service_init::load_all_skills(
            &self.builtin_skills_dir,
            &self.user_skills_dir,
            workspace_dir,
        )
    }

    fn installed_skill_ids(&self) -> Vec<String> {
        crate::skillhub::get_installed_skill_ids()
    }

    async fn search_skills(&self, query: &str, limit: usize) -> Result<serde_json::Value, String> {
        let installed = crate::skillhub::get_installed_skill_ids();
        let items = crate::skillhub::search_skills(query, limit, &installed)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_value(items).map_err(|e| e.to_string())
    }

    async fn fetch_featured(&self) -> Result<serde_json::Value, String> {
        let installed = crate::skillhub::get_installed_skill_ids();
        let items = crate::skillhub::fetch_featured(&installed)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_value(items).map_err(|e| e.to_string())
    }

    async fn install_skill(&self, slug: &str) -> Result<serde_json::Value, String> {
        let result = crate::skillhub::install_skill(slug, None).await?;
        serde_json::to_value(result).map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// 2.10 PersonaDispatchProvider (Wave 2.3a)
// Narrow seam for fork-context skill activation.
// ---------------------------------------------------------------------------

struct AppPersonaDispatchProvider;

#[async_trait]
impl rt::PersonaDispatchProvider for AppPersonaDispatchProvider {
    async fn dispatch_task(
        &self,
        persona_id: &str,
        task_description: &str,
        workdir: Option<&str>,
        profile_id: Option<&str>,
        skill_ids: Option<Vec<String>>,
        agent_type: Option<&str>,
        label: Option<&str>,
    ) -> Result<String, String> {
        let pm = crate::local_services::persona_manager()?;
        let skill_slice = skill_ids.as_deref();
        // PersonaManager::dispatch_task is a block_in_place sync wrapper; call
        // the async variant to stay on the current runtime thread.
        pm.dispatch_task_async(
            persona_id,
            task_description,
            workdir,
            profile_id,
            skill_slice,
            agent_type,
            None,
            label,
            None,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// 2.12 A2uiStoreProvider (Wave 2.3a)
// Thin wrapper around the shared `A2uiStore` in local_services.
// ---------------------------------------------------------------------------

struct AppA2uiStoreProvider;

impl rt::A2uiStoreProvider for AppA2uiStoreProvider {
    fn create_surface(&self, agent_id: &str, surface_id: &str, catalog_id: &str) -> u64 {
        match crate::local_services::a2ui_store() {
            Ok(s) => s.create_surface(agent_id, surface_id, catalog_id),
            Err(e) => {
                log::error!("A2uiStoreProvider::create_surface failed: {}", e);
                0
            }
        }
    }

    fn update_components(
        &self,
        agent_id: &str,
        surface_id: &str,
        components: Vec<serde_json::Value>,
    ) -> Result<u64, String> {
        crate::local_services::a2ui_store()?.update_components(agent_id, surface_id, components)
    }

    fn update_data_model(
        &self,
        agent_id: &str,
        surface_id: &str,
        data: serde_json::Value,
    ) -> Result<u64, String> {
        crate::local_services::a2ui_store()?.update_data_model(agent_id, surface_id, data)
    }

    fn delete_surface(&self, agent_id: &str, surface_id: &str) -> bool {
        match crate::local_services::a2ui_store() {
            Ok(s) => s.delete_surface(agent_id, surface_id),
            Err(_) => false,
        }
    }
}

// ---------------------------------------------------------------------------
// 2.14 NotificationDeliveryProvider (Wave 3.2b)
// Used by the macOS notification_watcher background loop to route incoming
// system notifications into a Persona chat session.  The watcher itself
// lives in the runtime crate and only reads rusqlite/plist; the actual
// PersonaManager + SpawnSessionConfig glue stays here in the app crate.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
struct AppNotificationDeliveryProvider;

#[cfg(target_os = "macos")]
#[async_trait]
impl rt::NotificationDeliveryProvider for AppNotificationDeliveryProvider {
    async fn deliver_to_persona(
        &self,
        persona_id: &str,
        app_display_name: &str,
        title: &str,
        body: &str,
    ) {
        let persona_mgr = match crate::local_services::persona_manager() {
            Ok(pm) => pm,
            Err(e) => {
                log::debug!("[NotifWatcher] PersonaManager not ready: {}", e);
                return;
            }
        };

        let persona = match persona_mgr.store().get_persona(persona_id) {
            Ok(Some(p)) => p,
            _ => {
                log::debug!("[NotifWatcher] Persona {} not found, skipping", persona_id);
                return;
            }
        };

        let msg = format!(
            "[Notification from {}]\nFrom: {}\nMessage: {}",
            app_display_name, title, body,
        );

        let chat_session_id = persona.chat_session_id.clone();
        let persona_name = persona.name.clone();

        let spawn_config = match crate::local_services::spawn_config() {
            Ok(c) => c,
            Err(e) => {
                log::debug!("[NotifWatcher] Spawn config not ready: {}", e);
                return;
            }
        };

        if let Some(handle) = spawn_config
            .session_connections
            .get(&chat_session_id)
            .await
            .map(|conn| conn.message_handle())
        {
            if let Err(e) = handle.send_initial_user_message(&msg).await {
                log::error!(
                    "[NotifWatcher] Failed to deliver {} notification to persona '{}': {}",
                    app_display_name,
                    persona_name,
                    e
                );
            } else {
                log::info!(
                    "[NotifWatcher] Delivered {} notification to persona '{}' (from: {})",
                    app_display_name,
                    persona_name,
                    title
                );
            }
        } else {
            log::debug!(
                "[NotifWatcher] Chat session {} not connected, cannot deliver notification",
                chat_session_id
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2.13 SubagentBootstrapProvider (Wave 2.3a)
// Mirrors the old in-executor logic for resolving (AgentConfig, SubAgentContext).
// ---------------------------------------------------------------------------

struct AppSubagentBootstrapProvider;

#[async_trait]
impl rt::SubagentBootstrapProvider for AppSubagentBootstrapProvider {
    async fn build_subagent_context(
        &self,
        agent_id: &str,
        parent_session_id: &str,
        override_profile_id: Option<&str>,
    ) -> Result<
        (
            cteno_agent_runtime::agent_config::AgentConfig,
            cteno_agent_runtime::agent::executor::SubAgentContext,
        ),
        String,
    > {
        let spawn_cfg = crate::local_services::spawn_config()
            .map_err(|e| format!("Cannot start SubAgent: {}", e))?;
        let agent_cfg = &spawn_cfg.agent_config;

        let agent_config = {
            let all_agents = crate::service_init::load_all_agents(
                &agent_cfg.builtin_agents_dir,
                &agent_cfg.user_agents_dir,
                None,
            );
            all_agents
                .into_iter()
                .find(|a| a.id == agent_id)
                .ok_or_else(|| format!("Agent '{}' not found", agent_id))?
        };

        let resolved_profile_id = override_profile_id
            .map(|s| s.to_string())
            .or_else(|| resolve_session_profile_id(&spawn_cfg.db_path, parent_session_id));

        let (api_key, base_url, final_profile_id, profile_model, use_proxy) = {
            let global_key = agent_cfg.global_api_key.clone();
            let store = agent_cfg.profile_store.read().await;
            let proxy_profiles = agent_cfg.proxy_profiles.read().await;

            let profile = resolved_profile_id
                .as_deref()
                .and_then(|pid| store.get_profile_or_proxy(pid, &proxy_profiles))
                .unwrap_or_else(|| store.get_default().clone());

            let pid = resolved_profile_id
                .clone()
                .unwrap_or_else(|| profile.id.clone());
            let has_explicit = resolved_profile_id.is_some();

            let mut key = if !profile.chat.api_key.is_empty() {
                profile.chat.api_key.clone()
            } else {
                global_key
            };

            let mut base = profile.chat.base_url.clone();
            let use_proxy =
                cteno_agent_runtime::llm_profile::is_proxy_profile(&pid) || key.is_empty();
            if use_proxy {
                key = load_machine_auth_token(&spawn_cfg.db_path)
                    .map_err(|e| format!("Failed to resolve proxy auth token: {}", e))?;
                base = crate::resolved_happy_server_url();
            }

            (
                key,
                base,
                pid,
                if has_explicit {
                    Some(profile.chat.model.clone())
                } else {
                    None
                },
                use_proxy,
            )
        };

        let exec_ctx = cteno_agent_runtime::agent::executor::SubAgentContext {
            db_path: spawn_cfg.db_path.clone(),
            builtin_skills_dir: agent_cfg.builtin_skills_dir.clone(),
            user_skills_dir: agent_cfg.user_skills_dir.clone(),
            global_api_key: api_key,
            default_base_url: base_url,
            profile_id: Some(final_profile_id),
            use_proxy,
            profile_model,
            acp_sender: None,
            permission_checker: None,
            abort_flag: None,
            thinking_flag: None,
            api_format: cteno_agent_runtime::llm_profile::ApiFormat::Anthropic,
        };

        Ok((agent_config, exec_ctx))
    }
}

fn resolve_session_profile_id(db_path: &std::path::Path, session_id: &str) -> Option<String> {
    let manager =
        cteno_agent_runtime::agent_session::AgentSessionManager::new(db_path.to_path_buf());
    manager
        .get_session(session_id)
        .ok()
        .flatten()
        .and_then(|s| s.context_data)
        .and_then(|ctx| {
            ctx.get("profile_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

fn load_machine_auth_token(data_dir: &std::path::Path) -> Result<String, String> {
    let app_data_dir = if data_dir.is_dir() {
        data_dir.to_path_buf()
    } else {
        data_dir
            .parent()
            .and_then(|db_dir| db_dir.parent())
            .or_else(|| data_dir.parent())
            .unwrap_or(data_dir)
            .to_path_buf()
    };
    let (token, _, _, _) = crate::auth_store_boot::load_persisted_machine_auth(&app_data_dir)?
        .ok_or_else(|| "No auth token found in machine_auth.json".to_string())?;
    Ok(token)
}

// ---------------------------------------------------------------------------
// 2.15 AgentKindResolver (Wave 3.3a)
// Exposes the app-side `resolve_agent_kind` (which queries `persona_sessions`)
// to runtime callers that only depend on the runtime's `AgentKind` taxonomy.
// Persona fields are returned as opaque JSON so the runtime doesn't need to
// depend on `crate::persona::models::*`.
// ---------------------------------------------------------------------------

struct AppAgentKindResolver;

#[async_trait]
impl rt::AgentKindResolver for AppAgentKindResolver {
    async fn resolve(&self, session_id: &str) -> Result<rt::AgentKindResolution, String> {
        let res = crate::agent_kind::resolve_agent_kind(session_id);
        let persona_link = res
            .persona_link
            .as_ref()
            .and_then(|link| serde_json::to_value(link).ok());
        let persona = res
            .persona
            .as_ref()
            .and_then(|p| serde_json::to_value(p).ok());
        Ok(rt::AgentKindResolution {
            kind: res.kind,
            persona_link,
            persona,
        })
    }
}

// ---------------------------------------------------------------------------
// 2.16 HeadlessAuthPathProvider (Wave 3.3c)
// Narrow seam for runtime callers that need the headless app data dir path.
// Delegates to the host crate's platform-specific resolver for any runtime
// path that needs the shared headless auth directory.
// ---------------------------------------------------------------------------

struct AppHeadlessAuthPathProvider;

impl rt::HeadlessAuthPathProvider for AppHeadlessAuthPathProvider {
    fn headless_auth_dir(&self) -> Result<std::path::PathBuf, String> {
        Ok(crate::headless_auth::resolve_app_data_dir())
    }
}

/// Install all runtime hooks.  Call once during `service_init::initialize_services`
/// — specifically before the `ToolRegistry` is populated with the runtime's own
/// executor types.
pub fn install_all(
    tool_registry: Arc<tokio::sync::RwLock<crate::tool::registry::ToolRegistry>>,
    db_path: std::path::PathBuf,
    builtin_skills_dir: std::path::PathBuf,
    user_skills_dir: std::path::PathBuf,
) {
    // 2.1 ToolRegistry — both the provider façade and the concrete handle.
    rt::install_tool_registry(Arc::new(AppToolRegistryProvider {
        handle: tool_registry.clone(),
    }));
    rt::install_tool_registry_handle(tool_registry);

    // 2.2 CommandInterceptor — slash command short-circuit.
    rt::install_command_interceptor(Arc::new(AppCommandInterceptor { db_path }));

    // 2.4 URL provider (used by oss_upload / websearch / fetch executors).
    rt::install_url_provider(Arc::new(AppUrlProvider));

    // 2.5 SpawnConfig — used by `wait` executor to poll session queues.
    rt::install_spawn_config(Arc::new(AppSpawnConfigProvider));

    // 2.6 AgentOwner — used by `memory` executor for display labels.
    rt::install_agent_owner(Arc::new(AppAgentOwnerProvider));

    // 2.7 MachineSocket — used by `a2ui_render` executor to push update events.
    rt::install_machine_socket(Arc::new(AppMachineSocketProvider));

    // 2.18 LocalNotification — use host app identity for desktop notifications.
    rt::install_local_notification(Arc::new(AppLocalNotificationProvider));

    // 2.9 SkillRegistry — skill loading + SkillHub ops.
    rt::install_skill_registry(Arc::new(AppSkillRegistryProvider {
        builtin_skills_dir,
        user_skills_dir,
    }));

    // 2.10 PersonaDispatch — fork-context skill activation.
    rt::install_persona_dispatch(Arc::new(AppPersonaDispatchProvider));

    // 2.12 A2uiStore — `a2ui_render` executor state.
    rt::install_a2ui_store(Arc::new(AppA2uiStoreProvider));

    // 2.13 SubagentBootstrap — `start_subagent` executor bootstrap.
    rt::install_subagent_bootstrap(Arc::new(AppSubagentBootstrapProvider));

    // 2.11 SessionWaker — used by crate::runs::RunManager background-run
    // completion / timeout notification delivery.
    rt::install_session_waker(Arc::new(AppSessionWaker));

    // 2.14 NotificationDeliveryProvider (Wave 3.2b) — used by the runtime's
    // macOS notification_watcher background loop.
    #[cfg(target_os = "macos")]
    rt::install_notification_delivery(Arc::new(AppNotificationDeliveryProvider));

    // 2.15 AgentKindResolver (Wave 3.3a) — session-id → AgentKind lookup
    // delegated back to the app crate's persona DB.
    rt::install_agent_kind_resolver(Arc::new(AppAgentKindResolver));

    // 2.16 HeadlessAuthPathProvider (Wave 3.3c) — headless app data dir
    // location for runtime callers.
    rt::install_headless_auth_path(Arc::new(AppHeadlessAuthPathProvider));
}
