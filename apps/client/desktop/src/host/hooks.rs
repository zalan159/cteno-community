//! App-side implementations of the `cteno-host-runtime` trait seam.
//!
//! These hooks own every dependency on concrete `HappyClientManager` /
//! `service_init` / `multi_agent` / `commands` / `headless_auth` symbols, so
//! the host crate stays feature-gate free.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use cteno_host_bridge_localrpc::LocalRpcAuthGate;
use cteno_host_rpc_core::RpcRegistry;
use cteno_host_runtime::machine::{
    HostMachineSpawnConfig, HostMachineSpawner, LocalWorkspaceRpcRegistrar, MachineIdProvider,
    ServiceInitConfig, ServiceInitializer,
};
use cteno_host_runtime::HostPaths;
use serde_json::{json, Value};

use crate::commands;
use crate::happy_client::manager::HappyClientManager;
use crate::headless_auth;
use crate::host::daemon_runtime;
use crate::host::local_rpc_server;
use crate::multi_agent;
use crate::service_init;

// ---------------------------------------------------------------------------
// Machine id
// ---------------------------------------------------------------------------

pub struct HappyMachineIdProvider;

impl MachineIdProvider for HappyMachineIdProvider {
    fn get_or_create(&self, app_data_dir: &Path) -> Option<String> {
        crate::auth_anonymous::ensure_local_machine_id(app_data_dir).ok()
    }
}

// ---------------------------------------------------------------------------
// Workspace RPC handlers
// ---------------------------------------------------------------------------

pub struct MultiAgentRpcRegistrar;

#[async_trait]
impl LocalWorkspaceRpcRegistrar for MultiAgentRpcRegistrar {
    async fn register(&self, registry: Arc<RpcRegistry>, machine_id: &str) {
        multi_agent::register_local_workspace_rpc_handlers(registry, machine_id).await;
    }
}

// ---------------------------------------------------------------------------
// Service init
// ---------------------------------------------------------------------------

pub struct DefaultServiceInitializer;

#[async_trait]
impl ServiceInitializer for DefaultServiceInitializer {
    async fn initialize(&self, cfg: ServiceInitConfig) {
        service_init::initialize_services(
            cfg.db_path,
            cfg.data_dir,
            cfg.builtin_tools_dir,
            cfg.builtin_skills_dir,
            cfg.user_skills_dir,
            cfg.builtin_agents_dir,
            cfg.config_path,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Happy client spawner (drives the machine host loop)
// ---------------------------------------------------------------------------

/// Wraps the shared `HappyClientManager`. Entry points clone the `Arc` so that
/// Tauri state (`SessionConnectionsState`) can still reach the same manager
/// instance used by the host spawner.
#[derive(Clone)]
pub struct HappyClientSpawner {
    manager: Arc<HappyClientManager>,
}

impl HappyClientSpawner {
    pub fn new(manager: Arc<HappyClientManager>) -> Self {
        Self { manager }
    }

    pub fn manager(&self) -> Arc<HappyClientManager> {
        self.manager.clone()
    }
}

fn headless_community_runtime_enabled(paths: &HostPaths) -> bool {
    paths.identity.shell_kind == crate::host::core::HostShellKind::Agentd
}

fn log_headless_community_cloud_state(paths: &HostPaths) {
    let machine_authenticated = paths.identity.machine_auth_cache_path.exists();
    let account_authenticated = paths.identity.account_auth_store_path.exists();

    if machine_authenticated && account_authenticated {
        log::info!(
            "Headless community runtime starting offline-first with cached cloud auth available; local persona/session RPC stays local-first"
        );
    } else {
        log::warn!(
            "Headless community runtime starting in degraded local-only mode (machine_auth_cached={}, account_auth_cached={}); local persona/session RPC will stay available without cloud auth",
            machine_authenticated,
            account_authenticated
        );
    }
}

#[async_trait]
impl HostMachineSpawner for HappyClientSpawner {
    fn rpc_registry(&self) -> Arc<RpcRegistry> {
        self.manager.rpc_registry()
    }

    async fn start_machine_host(&self, paths: HostPaths, config: HostMachineSpawnConfig) {
        let manager = self.manager.clone();
        let rpc_reg = manager.rpc_registry();
        let env_tag = paths.identity.local_rpc_env_tag.clone();
        let machine_id =
            crate::auth_anonymous::ensure_local_machine_id(&paths.identity.app_data_dir)
                .unwrap_or_default();
        let db_path = paths.db_path.to_string_lossy().to_string();
        let app_data_dir = paths.app_data_dir.clone();
        let builtin_skills_dir = paths.builtin_skills_dir.clone();
        let user_skills_dir = paths.user_skills_dir.clone();
        let builtin_agents_dir = paths.builtin_agents_dir.clone();
        let user_agents_dir = paths.user_agents_dir.clone();
        let api_key = config.api_key;
        let workspace_rpc = config.workspace_rpc;

        let _ = config.machine_auth_state;

        // 1. Register the per-machine workspace RPC handlers.
        workspace_rpc.register(rpc_reg.clone(), &machine_id).await;

        // 2. Bring up the local RPC server (Unix socket).
        {
            let rpc_reg_for_server = rpc_reg.clone();
            let env_tag_for_server = env_tag.clone();
            let machine_id_for_server = machine_id.clone();
            tokio::spawn(async move {
                local_rpc_server::start(
                    rpc_reg_for_server,
                    machine_id_for_server,
                    env_tag_for_server,
                )
                .await;
            });
        }

        // 3. Start the shared local machine runtime. The sync sidecar is
        // installed separately during service init and self-disables without
        // persisted Happy Server auth.
        if headless_community_runtime_enabled(&paths) {
            log_headless_community_cloud_state(&paths);
        }

        if let Err(e) = manager
            .start_local_machine_runtime(
                db_path,
                api_key,
                app_data_dir,
                builtin_skills_dir,
                user_skills_dir,
                builtin_agents_dir,
                user_agents_dir,
                machine_id.clone(),
            )
            .await
        {
            log::error!("Local community machine runtime exited with error: {}", e);
        }
    }
}

// ---------------------------------------------------------------------------
// Local RPC auth gate
// ---------------------------------------------------------------------------

pub struct AppLocalRpcAuthGate;

#[async_trait]
impl LocalRpcAuthGate for AppLocalRpcAuthGate {
    async fn handle(&self, method: &str, machine_id: &str) -> Result<Option<Value>, String> {
        let app_data_dir = daemon_runtime::ensure_app_data_dir()?;
        let identity =
            crate::host::core::resolve_headless_identity_paths(Some(app_data_dir.clone()))?;
        let account_auth = headless_auth::load_account_auth(&app_data_dir)?;
        let machine_auth_cache = daemon_runtime::machine_auth_cache_path(&app_data_dir);
        let machine_authenticated = machine_auth_cache.exists();

        match method {
            "auth.status" => Ok(Some(json!({
                "daemonRunning": true,
                "shellKind": identity.shell_kind.as_str(),
                "appDataDir": identity.app_data_dir.display().to_string(),
                "configPath": identity.config_path.display().to_string(),
                "profilesPath": identity.profiles_path.display().to_string(),
                "machineIdPath": identity.machine_id_path.display().to_string(),
                "localRpcEnvTag": identity.local_rpc_env_tag,
                "managedMode": false,
                "machineId": if machine_id.is_empty() { Value::Null } else { json!(machine_id) },
                "machineAuthenticated": machine_authenticated,
                "machinePending": false,
                "pendingMachinePublicKey": Value::Null,
                "pendingMachineUri": Value::Null,
                "machineAuthStorePath": machine_auth_cache.display().to_string(),
                "accountAuthenticated": account_auth.is_some(),
                "accountAuthStorePath": headless_auth::account_auth_store_path(&app_data_dir).display().to_string(),
            }))),
            "auth.pending-machine" => Ok(Some(json!({
                "managedMode": false,
                "machineId": if machine_id.is_empty() { Value::Null } else { json!(machine_id) },
                "publicKey": Value::Null,
                "uri": Value::Null,
                "pending": false,
                "machineAuthenticated": machine_authenticated,
            }))),
            "auth.machine-connection-status" => {
                let status = if machine_authenticated {
                    "connected"
                } else {
                    "disconnected"
                };
                Ok(Some(json!({
                    "managedMode": false,
                    "machineId": if machine_id.is_empty() { Value::Null } else { json!(machine_id) },
                    "status": status,
                    "machineAuthenticated": machine_authenticated,
                    "machinePending": false,
                })))
            }
            "auth.trigger-reauth" => {
                if commands::signal_machine_reauth() {
                    Ok(Some(json!({ "success": true })))
                } else {
                    Err("Machine reauth signal is not available in this process".to_string())
                }
            }
            _ => Ok(None),
        }
    }
}
