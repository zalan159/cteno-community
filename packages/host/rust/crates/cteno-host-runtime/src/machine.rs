//! Host machine runtime trait seam.
//!
//! `HostMachineRuntime` no longer depends on any concrete
//! `HappyClientManager` / workspace-registrar / service-init symbol. The app
//! crate provides implementations of the traits below via `host/hooks.rs` and
//! wires them during bootstrap (see `host/shells.rs`).
//!
//! The commercial feature gate stays on the app side: host crates compile the
//! same code for both flavours.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cteno_host_rpc_core::RpcRegistry;

use crate::HostPaths;

/// Resolves the stable machine id for this host. Implemented by the app crate
/// on top of `MachineManager::get_or_create_machine_id`.
pub trait MachineIdProvider: Send + Sync {
    fn get_or_create(&self, app_data_dir: &Path) -> Option<String>;
}

/// Registers the per-machine local workspace RPC handlers onto a registry.
/// Implemented by the app crate on top of
/// `multi_agent::register_local_workspace_rpc_handlers`.
#[async_trait]
pub trait LocalWorkspaceRpcRegistrar: Send + Sync {
    async fn register(&self, registry: Arc<RpcRegistry>, machine_id: &str);
}

/// Runs `service_init::initialize_services`-equivalent bootstrap work.
#[async_trait]
pub trait ServiceInitializer: Send + Sync {
    async fn initialize(&self, config: ServiceInitConfig);
}

/// Inputs to `ServiceInitializer::initialize` (mirrors the existing
/// `service_init::initialize_services` signature with `PathBuf`s).
pub struct ServiceInitConfig {
    pub db_path: PathBuf,
    pub app_data_dir: PathBuf,
    pub data_dir: PathBuf,
    pub builtin_tools_dir: PathBuf,
    pub builtin_skills_dir: PathBuf,
    pub user_skills_dir: PathBuf,
    pub builtin_agents_dir: PathBuf,
    pub config_path: PathBuf,
}

/// Drives the machine host lifecycle (local RPC server + optional cloud
/// services). The host crate remains oblivious to login-gated behavior; the
/// app-side impl decides how and when to enable those services.
#[async_trait]
pub trait HostMachineSpawner: Send + Sync {
    /// Exposes the shared RPC registry consumed by both the local RPC server
    /// and Tauri bridge.
    fn rpc_registry(&self) -> Arc<RpcRegistry>;

    /// Kick off the machine host: register workspace RPC handlers, start the
    /// local RPC server, and run the desktop machine loop.
    async fn start_machine_host(&self, paths: HostPaths, config: HostMachineSpawnConfig);
}

/// Config passed to `HostMachineSpawner::start_machine_host`.
///
/// `machine_auth_state` is an opaque `Any` so the host crate does not leak any
/// commercial types; the concrete impl downcasts as needed.
pub struct HostMachineSpawnConfig {
    pub machine_auth_state: Box<dyn Any + Send + Sync>,
    pub api_key: String,
    pub workspace_rpc: Arc<dyn LocalWorkspaceRpcRegistrar>,
}

/// Thin facade used by entry points. It owns a `HostMachineSpawner` and
/// exposes the registry/machine-id metadata bootstrap code needs.
pub struct HostMachineRuntime {
    rpc_registry: Arc<RpcRegistry>,
    machine_id: String,
    env_tag: String,
    spawner: Arc<dyn HostMachineSpawner>,
}

impl HostMachineRuntime {
    pub fn new(
        paths: &HostPaths,
        spawner: Arc<dyn HostMachineSpawner>,
        machine_id_provider: &dyn MachineIdProvider,
    ) -> Result<Self, String> {
        let rpc_registry = spawner.rpc_registry();
        let machine_id = machine_id_provider
            .get_or_create(&paths.identity.app_data_dir)
            .unwrap_or_default();
        let env_tag = paths.identity.local_rpc_env_tag.clone();
        Ok(Self {
            rpc_registry,
            machine_id,
            env_tag,
            spawner,
        })
    }

    pub fn rpc_registry(&self) -> Arc<RpcRegistry> {
        self.rpc_registry.clone()
    }

    pub fn machine_id(&self) -> &str {
        &self.machine_id
    }

    pub fn env_tag(&self) -> &str {
        &self.env_tag
    }

    pub fn spawner(&self) -> Arc<dyn HostMachineSpawner> {
        self.spawner.clone()
    }

    /// Spawn the machine host on a dedicated OS thread with its own Tokio
    /// runtime (mirrors the previous behaviour of `core.rs::spawn_machine_host`).
    pub fn spawn_machine_host(&self, paths: HostPaths, config: HostMachineSpawnConfig) {
        let spawner = self.spawner.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
            rt.block_on(async move {
                spawner.start_machine_host(paths, config).await;
            });
        });
    }
}

/// Drive `ServiceInitializer::initialize` from a `HostPaths` — replaces the old
/// app-side `core::spawn_service_initializer`. Kept host-crate-side so Tauri
/// and headless entrypoints share a single implementation.
pub fn spawn_service_initializer(paths: &HostPaths, initializer: Arc<dyn ServiceInitializer>) {
    let paths = paths.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
        rt.block_on(async move {
            initializer
                .initialize(ServiceInitConfig {
                    db_path: paths.db_path.clone(),
                    app_data_dir: paths.app_data_dir.clone(),
                    data_dir: paths.data_dir.clone(),
                    builtin_tools_dir: paths.builtin_tools_dir.clone(),
                    builtin_skills_dir: paths.builtin_skills_dir.clone(),
                    user_skills_dir: paths.user_skills_dir.clone(),
                    builtin_agents_dir: paths.builtin_agents_dir.clone(),
                    config_path: paths.config_path.clone(),
                })
                .await;
        });
    });
}
