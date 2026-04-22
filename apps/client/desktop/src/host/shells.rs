use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;

use crate::commands::MachineAuthState;
use crate::happy_client::manager::HappyClientManager;
use crate::host::hooks::{
    DefaultServiceInitializer, HappyClientSpawner, HappyMachineIdProvider, MultiAgentRpcRegistrar,
};
use crate::{db, LocalHostInfoState, RpcRegistryState, SessionConnectionsState, APP_HANDLE};
use cteno_host_runtime::machine::{
    spawn_service_initializer, HostMachineRuntime, HostMachineSpawnConfig,
    LocalWorkspaceRpcRegistrar,
};

use super::core::{self, HostPaths};
use super::daemon_runtime;

pub struct TauriHostBootstrap {
    pub session_connections: SessionConnectionsState,
    pub rpc_registry: RpcRegistryState,
    pub local_host_info: LocalHostInfoState,
    pub paths: HostPaths,
}

pub struct HeadlessDaemonBootstrap {
    pub daemon_mode: String,
    pub app_data_dir: std::path::PathBuf,
    pub paths: HostPaths,
    _lock_guard: daemon_runtime::DaemonLockGuard,
}

pub fn prepare_tauri_runtime_env() -> Result<(), String> {
    crate::load_runtime_env()
}

pub fn prepare_headless_runtime_env() -> Result<(), String> {
    crate::load_headless_runtime_env()
}

fn build_machine_runtime_and_spawner(
    host_paths: &HostPaths,
) -> Result<(HostMachineRuntime, HappyClientSpawner), String> {
    let manager = Arc::new(HappyClientManager::new());
    let spawner = HappyClientSpawner::new(manager);
    let runtime = HostMachineRuntime::new(
        host_paths,
        Arc::new(spawner.clone()),
        &HappyMachineIdProvider,
    )?;
    Ok((runtime, spawner))
}

fn launch_machine_host(
    runtime: &HostMachineRuntime,
    host_paths: &HostPaths,
    machine_auth_state: MachineAuthState,
    api_key: String,
) {
    let config = HostMachineSpawnConfig {
        machine_auth_state: Box::new(machine_auth_state),
        api_key,
        workspace_rpc: Arc::new(MultiAgentRpcRegistrar),
    };
    runtime.spawn_machine_host(host_paths.clone(), config);
}

fn prime_tauri_machine_rpc_registry(
    manager: Arc<HappyClientManager>,
    machine_id: &str,
    db_path: std::path::PathBuf,
    app_data_dir: std::path::PathBuf,
    api_key: String,
    builtin_skills_dir: std::path::PathBuf,
    user_skills_dir: std::path::PathBuf,
    builtin_agents_dir: std::path::PathBuf,
    user_agents_dir: std::path::PathBuf,
) {
    tauri::async_runtime::block_on(async move {
        MultiAgentRpcRegistrar
            .register(manager.rpc_registry(), machine_id)
            .await;
        manager
            .prime_machine_scoped_ui_rpc_handlers(
                machine_id.to_string(),
                db_path,
                app_data_dir,
                api_key,
                builtin_skills_dir,
                user_skills_dir,
                builtin_agents_dir,
                user_agents_dir,
            )
            .await;
    });
}

pub fn setup_tauri_host(
    app: &tauri::App,
    machine_auth_state: MachineAuthState,
    api_key: String,
) -> Result<TauriHostBootstrap, String> {
    cteno_community_host::permissions::install_ctenoctl_symlink_if_needed();
    APP_HANDLE.set(app.handle().clone()).ok();
    db::init(&app.handle().clone()).map_err(|e| format!("Failed to init db: {}", e))?;

    let host_paths = core::resolve_tauri_paths(&app.handle())?;
    std::env::set_var("CTENO_ENV", &host_paths.identity.local_rpc_env_tag);

    // Install the global LocalEventSink now that AppHandle + db_path are
    // available. All `HappySocket::local(...)` constructions after this point
    // will pick it up via `attach_to_socket` and fan out broadcast emits to
    // the frontend through Tauri events instead of a dead Socket.IO.
    crate::happy_client::local_sink::install_global_sink(Arc::new(
        crate::happy_client::local_sink::DesktopLocalSink::new(
            host_paths.db_path.clone(),
            app.handle().clone(),
        ),
    ));

    // Install the unified AuthStore before any other service so the agent
    // runtime's CredentialsProvider hook is ready on the first tool call.
    if let Err(e) = crate::auth_store_boot::install_auth_store(&host_paths.app_data_dir) {
        log::error!("Failed to install AuthStore: {e}");
    }

    spawn_service_initializer(&host_paths, Arc::new(DefaultServiceInitializer));

    let (machine_runtime, spawner) = build_machine_runtime_and_spawner(&host_paths)?;
    prime_tauri_machine_rpc_registry(
        spawner.manager(),
        machine_runtime.machine_id(),
        host_paths.db_path.clone(),
        host_paths.app_data_dir.clone(),
        api_key.clone(),
        host_paths.builtin_skills_dir.clone(),
        host_paths.user_skills_dir.clone(),
        host_paths.builtin_agents_dir.clone(),
        host_paths.user_agents_dir.clone(),
    );
    let session_connections = SessionConnectionsState(spawner.manager().session_connections());
    let rpc_registry = RpcRegistryState(machine_runtime.rpc_registry());
    let host = hostname::get()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|_| "localhost".to_string());
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let home_dir = dirs::home_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "".to_string());
    let local_host_info = LocalHostInfoState {
        machine_id: machine_runtime.machine_id().to_string(),
        shell_kind: host_paths.identity.shell_kind.as_str().to_string(),
        local_rpc_env_tag: host_paths.identity.local_rpc_env_tag.clone(),
        app_data_dir: host_paths.identity.app_data_dir.display().to_string(),
        host,
        platform,
        happy_cli_version: env!("CARGO_PKG_VERSION").to_string(),
        happy_home_dir: host_paths.identity.app_data_dir.display().to_string(),
        home_dir,
    };
    launch_machine_host(&machine_runtime, &host_paths, machine_auth_state, api_key);

    Ok(TauriHostBootstrap {
        session_connections,
        rpc_registry,
        local_host_info,
        paths: host_paths,
    })
}

fn start_headless_host(
    app_data_dir: std::path::PathBuf,
    machine_auth_state: MachineAuthState,
    api_key: String,
) -> Result<HostPaths, String> {
    cteno_community_host::permissions::install_ctenoctl_symlink_if_needed();

    let host_paths = core::resolve_headless_paths(Some(app_data_dir))?;
    std::env::set_var("CTENO_ENV", &host_paths.identity.local_rpc_env_tag);
    crate::db::init_at_data_dir(&host_paths.app_data_dir)
        .map_err(|e| format!("Failed to init db: {}", e))?;

    if let Err(e) = crate::auth_store_boot::install_auth_store(&host_paths.app_data_dir) {
        log::error!("Failed to install AuthStore: {e}");
    }

    spawn_service_initializer(&host_paths, Arc::new(DefaultServiceInitializer));
    let (machine_runtime, _spawner) = build_machine_runtime_and_spawner(&host_paths)?;
    launch_machine_host(&machine_runtime, &host_paths, machine_auth_state, api_key);
    Ok(host_paths)
}

fn configure_headless_daemon_root(app_data_dir: &std::path::Path, isolated_startup: bool) {
    if !isolated_startup || std::env::var_os(daemon_runtime::DAEMON_ROOT_ENV).is_some() {
        return;
    }

    let daemon_root = daemon_runtime::local_daemon_root(app_data_dir);
    std::env::set_var(daemon_runtime::DAEMON_ROOT_ENV, &daemon_root);
    log::info!(
        "Headless daemon using isolated daemon root {}",
        daemon_root.display()
    );
}

fn configure_headless_proxy_env(isolated_startup: bool) {
    if !isolated_startup {
        return;
    }

    std::env::set_var("NO_PROXY", "*");
    std::env::set_var("no_proxy", "*");
    log::info!("Headless daemon disabled proxy autodiscovery for isolated startup");
}

pub fn setup_headless_daemon(
    machine_auth_state: MachineAuthState,
) -> Result<HeadlessDaemonBootstrap, String> {
    prepare_headless_runtime_env()?;

    let daemon_mode = "agentd".to_string();
    let isolated_startup = std::env::var_os("CTENO_APP_DATA_DIR").is_some();
    let app_data_dir = daemon_runtime::ensure_app_data_dir()?;
    configure_headless_daemon_root(&app_data_dir, isolated_startup);
    configure_headless_proxy_env(isolated_startup);
    let lock_guard = daemon_runtime::acquire_daemon_lock(&daemon_mode)?;
    core::seed_headless_identity_from_tauri(&app_data_dir)?;
    let config_path = app_data_dir.join("config.json");
    let api_key = daemon_runtime::load_llm_api_key_from_config(&config_path).unwrap_or_else(|e| {
        log::warn!("Failed to load API key from config: {}", e);
        String::new()
    });

    let paths = start_headless_host(app_data_dir.clone(), machine_auth_state, api_key)?;

    Ok(HeadlessDaemonBootstrap {
        daemon_mode,
        app_data_dir,
        paths,
        _lock_guard: lock_guard,
    })
}

pub fn run_headless_dev_frontend_watch_loop() {
    let dev_frontend_addr = std::env::var("CTENO_DEV_FRONTEND_PORT")
        .ok()
        .and_then(|raw| raw.parse::<u16>().ok())
        .map(|port| SocketAddr::from(([127, 0, 0, 1], port)));
    let mut frontend_seen_up = false;
    let mut frontend_missing_count: u32 = 0;
    let mut frontend_startup_grace_checks: u32 = 15;

    if let Some(addr) = dev_frontend_addr {
        log::info!("cteno-agentd dev frontend watcher enabled on {}", addr);
    }

    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));

        if let Some(addr) = dev_frontend_addr {
            let alive =
                TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(350)).is_ok();
            if alive {
                frontend_seen_up = true;
                frontend_missing_count = 0;
                continue;
            }

            if !frontend_seen_up && frontend_startup_grace_checks > 0 {
                frontend_startup_grace_checks -= 1;
                continue;
            }

            frontend_missing_count += 1;
            if frontend_missing_count >= 3 {
                log::warn!(
                    "Dev frontend {} is down; stopping cteno-agentd for dev restart",
                    addr
                );
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::happy_client::runtime::MachineRpcMethods;
    use cteno_host_rpc_core::RpcRequest;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn primed_tauri_registry_handles_community_persona_task_and_workspace_methods() {
        let temp = tempdir().unwrap();
        let manager = Arc::new(HappyClientManager::new());
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

        prime_tauri_machine_rpc_registry(
            manager.clone(),
            "machine-local",
            db_path,
            app_data_dir,
            String::new(),
            builtin_skills_dir,
            user_skills_dir,
            builtin_agents_dir,
            user_agents_dir,
        );

        let registry = manager.rpc_registry();
        let rpc_methods = MachineRpcMethods::new("machine-local");
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let list_personas = runtime.block_on(registry.handle(RpcRequest {
            request_id: "list-personas".to_string(),
            method: rpc_methods.list_personas.clone(),
            params: json!({}),
        }));
        assert!(list_personas.error.is_none());
        assert_eq!(
            list_personas
                .result
                .as_ref()
                .and_then(|value| value.get("success"))
                .and_then(|value| value.as_bool()),
            Some(false)
        );

        let create_persona = runtime.block_on(registry.handle(RpcRequest {
            request_id: "create-persona".to_string(),
            method: rpc_methods.create_persona.clone(),
            params: json!({ "name": "Smoke Persona" }),
        }));
        assert!(create_persona.error.is_none());
        assert_eq!(
            create_persona
                .result
                .as_ref()
                .and_then(|value| value.get("success"))
                .and_then(|value| value.as_bool()),
            Some(false)
        );
        let create_error = create_persona
            .result
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        assert!(!create_error.contains("Unknown method"));
        assert!(!create_error.contains("No handler registered"));

        let list_scheduled_tasks = runtime.block_on(registry.handle(RpcRequest {
            request_id: "list-scheduled-tasks".to_string(),
            method: rpc_methods.list_scheduled_tasks.clone(),
            params: json!({}),
        }));
        assert!(list_scheduled_tasks.error.is_none());
        assert_eq!(
            list_scheduled_tasks
                .result
                .as_ref()
                .and_then(|value| value.get("success"))
                .and_then(|value| value.as_bool()),
            Some(false)
        );

        let list_agent_workspaces = runtime.block_on(registry.handle(RpcRequest {
            request_id: "list-agent-workspaces".to_string(),
            method: rpc_methods.list_agent_workspaces.clone(),
            params: json!({}),
        }));
        assert!(list_agent_workspaces.error.is_none());

        for (request_id, method, params) in [
            (
                "bootstrap-workspace",
                rpc_methods.bootstrap_workspace.clone(),
                json!({}),
            ),
            (
                "get-agent-workspace",
                rpc_methods.get_agent_workspace.clone(),
                json!({}),
            ),
            (
                "delete-agent-workspace",
                rpc_methods.delete_agent_workspace.clone(),
                json!({}),
            ),
            (
                "workspace-send-message",
                rpc_methods.workspace_send.clone(),
                json!({}),
            ),
        ] {
            let response = runtime.block_on(registry.handle(RpcRequest {
                request_id: request_id.to_string(),
                method,
                params,
            }));
            assert!(response.error.is_none(), "{request_id} should be handled");
            let error = response
                .result
                .as_ref()
                .and_then(|value| value.get("error"))
                .and_then(|value| value.as_str())
                .unwrap_or("");
            assert!(
                !error.contains("Unknown method") && !error.contains("No handler registered"),
                "{request_id} should fail with a domain error, not a missing handler: {error}"
            );
        }
    }
}
