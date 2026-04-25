//! Feature-gated machine RPC runtime façade.

#[cfg(feature = "commercial-cloud")]
pub use cteno_happy_client_runtime::*;

#[cfg(not(feature = "commercial-cloud"))]
mod community {
    use serde_json::{json, Value};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    use crate::happy_client::socket::{ConnectionWatchdog, WatchdogState};
    use cteno_host_rpc_core::RpcRegistry;

    pub type RuntimeFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
    pub type DefaultProfileIdHook = Arc<dyn Fn() -> RuntimeFuture<String> + Send + Sync>;
    pub type SpawnSessionHook =
        Arc<dyn Fn(SpawnSessionRequest) -> RuntimeFuture<Result<String, String>> + Send + Sync>;
    pub type CliRunHook =
        Arc<dyn Fn(CliRunRequest) -> RuntimeFuture<Result<Value, String>> + Send + Sync>;
    pub type ReconnectSessionHook =
        Arc<dyn Fn(ReconnectSessionRequest) -> RuntimeFuture<Result<Value, String>> + Send + Sync>;
    pub type RestoreSessionsHook = Arc<dyn Fn() -> RuntimeFuture<Result<(), String>> + Send + Sync>;

    #[derive(Debug, Clone)]
    pub struct SpawnSessionRequest {
        pub directory: String,
        pub agent_flavor: String,
        pub profile_id: String,
        pub reasoning_effort: Option<String>,
    }

    #[derive(Debug, Clone)]
    pub struct CliRunRequest {
        pub message: String,
        pub workdir: String,
        pub profile_id: String,
        pub timeout_secs: u64,
        pub kind: String,
    }

    #[derive(Debug, Clone)]
    pub struct ReconnectSessionRequest {
        pub session_id: String,
        pub requested_profile_id: Option<String>,
    }

    #[derive(Clone)]
    pub struct SessionBootstrapHooks {
        pub default_profile_id: DefaultProfileIdHook,
        pub spawn_session: SpawnSessionHook,
        pub cli_run: CliRunHook,
    }

    #[derive(Clone)]
    pub struct SessionReconnectHooks {
        pub reconnect_session: ReconnectSessionHook,
    }

    #[derive(Clone)]
    pub struct ProfileRpcHooks {
        pub list_profiles: Arc<dyn Fn() -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
        pub refresh_proxy_profiles:
            Arc<dyn Fn() -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
        pub export_profiles: Arc<dyn Fn() -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
        pub save_profile: Arc<dyn Fn(Value) -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
        pub save_coding_plan_profiles:
            Arc<dyn Fn(Value) -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
        pub delete_profile:
            Arc<dyn Fn(String) -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
        pub switch_session_model: Arc<
            dyn Fn(String, String, Option<String>) -> RuntimeFuture<Result<Value, String>>
                + Send
                + Sync,
        >,
    }

    #[derive(Clone)]
    pub struct SkillRpcHooks {
        pub list_skills: Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>,
        pub create_skill: Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>,
        pub delete_skill: Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>,
        pub skillhub_featured:
            Arc<dyn Fn(Value) -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
        pub skillhub_search:
            Arc<dyn Fn(Value) -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
        pub skillhub_install:
            Arc<dyn Fn(Value) -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
    }

    #[derive(Clone)]
    pub struct McpRpcHooks {
        pub list_mcp: Arc<dyn Fn(Value) -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
        pub add_mcp: Arc<dyn Fn(Value) -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
        pub remove_mcp: Arc<dyn Fn(Value) -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
        pub toggle_mcp: Arc<dyn Fn(Value) -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
    }

    #[derive(Clone)]
    pub struct SessionRecoveryRuntimeConfig {
        pub restore_sessions: RestoreSessionsHook,
        pub watchdog_state: Arc<WatchdogState>,
        pub watchdog_slot: Arc<Mutex<Option<ConnectionWatchdog>>>,
    }

    pub async fn install_session_recovery_runtime(config: SessionRecoveryRuntimeConfig) {
        let restore_sessions = config.restore_sessions.clone();
        tokio::spawn(async move {
            let _ = restore_sessions().await;
        });
        let watchdog = ConnectionWatchdog::start(config.watchdog_state);
        *config.watchdog_slot.lock().await = Some(watchdog);
    }

    pub fn profile_id_exists<I, J, S, T>(
        profile_id: &str,
        local_profile_ids: I,
        proxy_profile_ids: J,
    ) -> bool
    where
        I: IntoIterator<Item = S>,
        J: IntoIterator<Item = T>,
        S: AsRef<str>,
        T: AsRef<str>,
    {
        local_profile_ids
            .into_iter()
            .any(|id| id.as_ref() == profile_id)
            || proxy_profile_ids
                .into_iter()
                .any(|id| id.as_ref() == profile_id)
    }

    pub fn pick_fallback_default_profile_id<I, J, S, T>(
        local_profile_ids: I,
        proxy_profile_ids: J,
        preferred_proxy_profile_id: &str,
    ) -> String
    where
        I: IntoIterator<Item = S>,
        J: IntoIterator<Item = T>,
        S: AsRef<str>,
        T: AsRef<str>,
    {
        let local_profile_ids = local_profile_ids
            .into_iter()
            .map(|id| id.as_ref().to_string())
            .collect::<Vec<_>>();
        let proxy_profile_ids = proxy_profile_ids
            .into_iter()
            .map(|id| id.as_ref().to_string())
            .collect::<Vec<_>>();
        proxy_profile_ids
            .iter()
            .find(|id| id.as_str() == preferred_proxy_profile_id)
            .cloned()
            .or_else(|| proxy_profile_ids.first().cloned())
            .or_else(|| {
                local_profile_ids
                    .iter()
                    .find(|id| id.as_str() == "default")
                    .cloned()
            })
            .or_else(|| local_profile_ids.first().cloned())
            .unwrap_or_else(|| "default".to_string())
    }

    pub fn reconcile_default_profile_id<I, J, S, T>(
        current_default_profile_id: &str,
        local_profile_ids: I,
        proxy_profile_ids: J,
        preferred_proxy_profile_id: &str,
    ) -> Option<(String, String)>
    where
        I: IntoIterator<Item = S>,
        J: IntoIterator<Item = T>,
        S: AsRef<str>,
        T: AsRef<str>,
    {
        let local_profile_ids = local_profile_ids
            .into_iter()
            .map(|id| id.as_ref().to_string())
            .collect::<Vec<_>>();
        let proxy_profile_ids = proxy_profile_ids
            .into_iter()
            .map(|id| id.as_ref().to_string())
            .collect::<Vec<_>>();
        if profile_id_exists(
            current_default_profile_id,
            local_profile_ids.iter().map(|id| id.as_str()),
            proxy_profile_ids.iter().map(|id| id.as_str()),
        ) {
            return None;
        }
        let new = pick_fallback_default_profile_id(
            local_profile_ids.iter().map(|id| id.as_str()),
            proxy_profile_ids.iter().map(|id| id.as_str()),
            preferred_proxy_profile_id,
        );
        (current_default_profile_id != new).then(|| (current_default_profile_id.to_string(), new))
    }

    pub async fn register_session_bootstrap_handlers(
        registry: Arc<RpcRegistry>,
        methods: &MachineRpcMethods,
        hooks: SessionBootstrapHooks,
    ) {
        let spawn_method = methods.spawn.clone();
        let default_profile = hooks.default_profile_id.clone();
        let spawn = hooks.spawn_session.clone();
        registry
            .register(&spawn_method, move |params: Value| {
                let default_profile = default_profile.clone();
                let spawn = spawn.clone();
                async move {
                    let directory = params
                        .get("directory")
                        .and_then(|v| v.as_str())
                        .unwrap_or("/")
                        .to_string();
                    let agent_flavor = params
                        .get("agent")
                        .and_then(|v| v.as_str())
                        .unwrap_or("cteno")
                        .to_string();
                    let profile_id =
                        if let Some(id) = params.get("modelId").and_then(|v| v.as_str()) {
                            id.to_string()
                        } else {
                            default_profile().await
                        };
                    match spawn(SpawnSessionRequest {
                        directory,
                        agent_flavor,
                        profile_id,
                        reasoning_effort: params
                            .get("reasoningEffort")
                            .and_then(|v| v.as_str())
                            .map(ToString::to_string),
                    })
                    .await
                    {
                        Ok(session_id) => Ok(json!({ "type": "success", "sessionId": session_id })),
                        Err(e) => Ok(json!({ "type": "error", "errorMessage": e })),
                    }
                }
            })
            .await;

        let cli_method = methods.cli_run.clone();
        let default_profile = hooks.default_profile_id.clone();
        let cli_run = hooks.cli_run.clone();
        registry
            .register(&cli_method, move |params: Value| {
                let default_profile = default_profile.clone();
                let cli_run = cli_run.clone();
                async move {
                    let profile_id =
                        if let Some(id) = params.get("modelId").and_then(|v| v.as_str()) {
                            id.to_string()
                        } else {
                            default_profile().await
                        };
                    cli_run(CliRunRequest {
                        message: params
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        workdir: params
                            .get("workdir")
                            .and_then(|v| v.as_str())
                            .unwrap_or("~")
                            .to_string(),
                        profile_id,
                        timeout_secs: params
                            .get("timeout")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(300),
                        kind: params
                            .get("kind")
                            .and_then(|v| v.as_str())
                            .unwrap_or("worker")
                            .to_string(),
                    })
                    .await
                    .or_else(|e| Ok(json!({ "success": false, "error": e })))
                }
            })
            .await;
    }

    pub async fn register_session_reconnect_handler(
        registry: Arc<RpcRegistry>,
        methods: &MachineRpcMethods,
        hooks: SessionReconnectHooks,
    ) {
        let method = methods.reconnect.clone();
        let reconnect = hooks.reconnect_session.clone();
        registry
            .register(&method, move |params: Value| {
                let reconnect = reconnect.clone();
                async move {
                    reconnect(ReconnectSessionRequest {
                        session_id: params
                            .get("sessionId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        requested_profile_id: params
                            .get("modelId")
                            .and_then(|v| v.as_str())
                            .map(ToString::to_string),
                    })
                    .await
                    .or_else(|e| Ok(json!({ "status": "error", "message": e })))
                }
            })
            .await;
    }

    pub async fn register_profile_rpc_handlers(
        registry: Arc<RpcRegistry>,
        methods: &MachineRpcMethods,
        hooks: ProfileRpcHooks,
    ) {
        let list = hooks.list_profiles.clone();
        registry
            .register(&methods.list_profiles, move |_| {
                let list = list.clone();
                async move {
                    list()
                        .await
                        .or_else(|e| Ok(json!({ "success": false, "error": e })))
                }
            })
            .await;
        let refresh = hooks.refresh_proxy_profiles.clone();
        registry
            .register(&methods.refresh_proxy_profiles, move |_| {
                let refresh = refresh.clone();
                async move {
                    refresh()
                        .await
                        .or_else(|e| Ok(json!({ "success": false, "error": e })))
                }
            })
            .await;
        let export = hooks.export_profiles.clone();
        registry
            .register(&methods.export_profiles, move |_| {
                let export = export.clone();
                async move {
                    export()
                        .await
                        .or_else(|e| Ok(json!({ "success": false, "error": e })))
                }
            })
            .await;
        let save = hooks.save_profile.clone();
        registry
            .register(&methods.save_profile, move |params| {
                let save = save.clone();
                async move {
                    let profile_val = params.get("profile").cloned().unwrap_or(params);
                    save(profile_val)
                        .await
                        .or_else(|e| Ok(json!({ "success": false, "error": e })))
                }
            })
            .await;
        let save_coding_plan = hooks.save_coding_plan_profiles.clone();
        registry
            .register(&methods.save_coding_plan_profiles, move |params| {
                let save_coding_plan = save_coding_plan.clone();
                async move {
                    save_coding_plan(params)
                        .await
                        .or_else(|e| Ok(json!({ "success": false, "error": e })))
                }
            })
            .await;
        let delete = hooks.delete_profile.clone();
        registry
            .register(&methods.delete_profile, move |params| {
                let delete = delete.clone();
                async move {
                    let id = params
                        .get("id")
                        .or_else(|| params.get("profileId"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    delete(id)
                        .await
                        .or_else(|e| Ok(json!({ "success": false, "error": e })))
                }
            })
            .await;
        let switch = hooks.switch_session_model.clone();
        registry
            .register(&methods.switch_profile, move |params| {
                let switch = switch.clone();
                async move {
                    switch(
                        params
                            .get("sessionId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        params
                            .get("profileId")
                            .or_else(|| params.get("modelId"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        params
                            .get("reasoningEffort")
                            .and_then(|v| v.as_str())
                            .map(ToString::to_string),
                    )
                    .await
                    .or_else(|e| Ok(json!({ "success": false, "error": e })))
                }
            })
            .await;
    }

    pub async fn register_skill_rpc_handlers(
        registry: Arc<RpcRegistry>,
        methods: &MachineRpcMethods,
        hooks: SkillRpcHooks,
    ) {
        let list = hooks.list_skills.clone();
        registry
            .register_sync(&methods.list_skills, move |p| list(p))
            .await;
        let create = hooks.create_skill.clone();
        registry
            .register_sync(&methods.create_skill, move |p| create(p))
            .await;
        let delete = hooks.delete_skill.clone();
        registry
            .register_sync(&methods.delete_skill, move |p| delete(p))
            .await;
        register_async(
            registry.clone(),
            &methods.skillhub_featured,
            hooks.skillhub_featured,
        )
        .await;
        register_async(
            registry.clone(),
            &methods.skillhub_search,
            hooks.skillhub_search,
        )
        .await;
        register_async(registry, &methods.skillhub_install, hooks.skillhub_install).await;
    }

    pub async fn register_mcp_rpc_handlers(
        registry: Arc<RpcRegistry>,
        methods: &MachineRpcMethods,
        hooks: McpRpcHooks,
    ) {
        register_async(registry.clone(), &methods.list_mcp, hooks.list_mcp).await;
        register_async(registry.clone(), &methods.add_mcp, hooks.add_mcp).await;
        register_async(registry.clone(), &methods.remove_mcp, hooks.remove_mcp).await;
        register_async(registry, &methods.toggle_mcp, hooks.toggle_mcp).await;
    }

    async fn register_async(
        registry: Arc<RpcRegistry>,
        method: &str,
        handler: Arc<dyn Fn(Value) -> RuntimeFuture<Result<Value, String>> + Send + Sync>,
    ) {
        registry
            .register(method, move |params| {
                let handler = handler.clone();
                async move {
                    handler(params)
                        .await
                        .or_else(|e| Ok(json!({ "success": false, "error": e })))
                }
            })
            .await;
    }

    #[derive(Debug, Clone)]
    pub struct MachineRpcMethods {
        pub execute: String,
        pub spawn: String,
        pub reconnect: String,
        pub list_profiles: String,
        pub export_profiles: String,
        pub save_profile: String,
        pub save_coding_plan_profiles: String,
        pub delete_profile: String,
        pub switch_profile: String,
        pub refresh_proxy_profiles: String,
        pub list_skills: String,
        pub create_skill: String,
        pub delete_skill: String,
        pub skillhub_featured: String,
        pub skillhub_search: String,
        pub skillhub_install: String,
        pub list_mcp: String,
        pub add_mcp: String,
        pub remove_mcp: String,
        pub toggle_mcp: String,
        pub list_sessions: String,
        pub get_session: String,
        pub get_session_messages: String,
        pub list_personas: String,
        pub create_persona: String,
        pub update_persona: String,
        pub delete_persona: String,
        pub get_persona_tasks: String,
        pub list_scheduled_tasks: String,
        pub toggle_scheduled_task: String,
        pub delete_scheduled_task: String,
        pub update_scheduled_task: String,
        pub delete_scheduled_tasks_by_session: String,
        pub list_background_tasks: String,
        pub get_background_task: String,
        pub list_runs: String,
        pub get_run: String,
        pub stop_run: String,
        pub get_run_logs: String,
        pub get_session_trace: String,
        pub bash: String,
        pub stop_daemon: String,
        pub kill_session: String,
        pub delete_session: String,
        pub list_subagents: String,
        pub get_subagent: String,
        pub stop_subagent: String,
        pub memory_list: String,
        pub memory_read: String,
        pub memory_write: String,
        pub memory_delete: String,
        pub workspace_list: String,
        pub workspace_read: String,
        pub workspace_stat: String,
        pub get_local_usage: String,
        pub webview_eval: String,
        pub webview_screenshot: String,
        pub cli_run: String,
        pub list_tools: String,
        pub exec_tool: String,
        pub send_message: String,
        pub dispatch_task: String,
        pub reset_persona_session: String,
        pub create_orch_flow: String,
        pub get_orch_flow: String,
        pub delete_orch_flow: String,
        pub get_a2ui_state: String,
        pub a2ui_action: String,
        pub list_agents: String,
        pub get_agent: String,
        pub create_agent: String,
        pub delete_agent: String,
        pub bootstrap_workspace: String,
        pub list_agent_workspace_templates: String,
        pub list_agent_workspaces: String,
        pub get_agent_workspace: String,
        pub delete_agent_workspace: String,
        pub workspace_send: String,
        #[cfg(target_os = "macos")]
        pub list_notification_apps: String,
        #[cfg(target_os = "macos")]
        pub get_notification_subs: String,
        #[cfg(target_os = "macos")]
        pub update_notification_sub: String,
    }

    impl MachineRpcMethods {
        pub fn new(machine_id: &str) -> Self {
            let m = |name: &str| format!("{machine_id}:{name}");
            Self {
                execute: m("agent.execute"),
                spawn: m("spawn-happy-session"),
                reconnect: m("reconnect-session"),
                list_profiles: m("list-profiles"),
                export_profiles: m("export-profiles"),
                save_profile: m("save-profile"),
                save_coding_plan_profiles: m("save-coding-plan-profiles"),
                delete_profile: m("delete-profile"),
                switch_profile: m("switch-session-model"),
                refresh_proxy_profiles: m("refresh-proxy-profiles"),
                list_skills: m("list-skills"),
                create_skill: m("create-skill"),
                delete_skill: m("delete-skill"),
                skillhub_featured: m("skillhub-featured"),
                skillhub_search: m("skillhub-search"),
                skillhub_install: m("skillhub-install"),
                list_mcp: m("list-mcp-servers"),
                add_mcp: m("add-mcp-server"),
                remove_mcp: m("remove-mcp-server"),
                toggle_mcp: m("toggle-mcp-server"),
                list_sessions: m("list-sessions"),
                get_session: m("get-session"),
                get_session_messages: m("get-session-messages"),
                list_personas: m("list-personas"),
                create_persona: m("create-persona"),
                update_persona: m("update-persona"),
                delete_persona: m("delete-persona"),
                get_persona_tasks: m("get-persona-tasks"),
                list_scheduled_tasks: m("list-scheduled-tasks"),
                toggle_scheduled_task: m("toggle-scheduled-task"),
                delete_scheduled_task: m("delete-scheduled-task"),
                update_scheduled_task: m("update-scheduled-task"),
                delete_scheduled_tasks_by_session: m("delete-scheduled-tasks-by-session"),
                list_background_tasks: m("list-background-tasks"),
                get_background_task: m("get-background-task"),
                list_runs: m("list-runs"),
                get_run: m("get-run"),
                stop_run: m("stop-run"),
                get_run_logs: m("get-run-logs"),
                get_session_trace: m("get-session-trace"),
                bash: m("bash"),
                stop_daemon: m("stop-daemon"),
                kill_session: m("kill-session"),
                delete_session: m("delete-session"),
                list_subagents: m("list-subagents"),
                get_subagent: m("get-subagent"),
                stop_subagent: m("stop-subagent"),
                memory_list: m("memory-list"),
                memory_read: m("memory-read"),
                memory_write: m("memory-write"),
                memory_delete: m("memory-delete"),
                workspace_list: m("workspace-list"),
                workspace_read: m("workspace-read"),
                workspace_stat: m("workspace-stat"),
                get_local_usage: m("get-local-usage"),
                webview_eval: m("webview-eval"),
                webview_screenshot: m("webview-screenshot"),
                cli_run: m("cli-run-agent"),
                list_tools: m("list-tools"),
                exec_tool: m("exec-tool"),
                send_message: m("send-message"),
                dispatch_task: m("dispatch-task"),
                reset_persona_session: m("reset-persona-session"),
                create_orch_flow: m("create-orchestration-flow"),
                get_orch_flow: m("get-orchestration-flow"),
                delete_orch_flow: m("delete-orchestration-flow"),
                get_a2ui_state: m("get-a2ui-state"),
                a2ui_action: m("a2ui-action"),
                list_agents: m("list-agents"),
                get_agent: m("get-agent"),
                create_agent: m("create-agent"),
                delete_agent: m("delete-agent"),
                bootstrap_workspace: m("bootstrap-workspace"),
                list_agent_workspace_templates: m("list-agent-workspace-templates"),
                list_agent_workspaces: m("list-agent-workspaces"),
                get_agent_workspace: m("get-agent-workspace"),
                delete_agent_workspace: m("delete-agent-workspace"),
                workspace_send: m("workspace-send"),
                #[cfg(target_os = "macos")]
                list_notification_apps: m("list-notification-apps"),
                #[cfg(target_os = "macos")]
                get_notification_subs: m("get-notification-subs"),
                #[cfg(target_os = "macos")]
                update_notification_sub: m("update-notification-sub"),
            }
        }

        pub fn all_methods(&self) -> Vec<String> {
            vec![
                self.execute.clone(),
                self.spawn.clone(),
                self.reconnect.clone(),
                self.list_profiles.clone(),
                self.save_coding_plan_profiles.clone(),
                self.get_session.clone(),
                self.get_session_messages.clone(),
            ]
        }
    }
}

#[cfg(not(feature = "commercial-cloud"))]
pub use community::*;
