//! Shared host session-registry state containers.
//!
//! P1 scope: RpcRegistryState + LocalHostInfoState.
//! SessionConnectionsState 仍保留在 app crate，待 P2 随 cteno-agent-runtime 迁出。

pub mod background_tasks;

use std::sync::Arc;

pub use background_tasks::{
    BackgroundSessionSource, BackgroundTaskCategory, BackgroundTaskFilter, BackgroundTaskRecord,
    BackgroundTaskRegistry, BackgroundTaskStatus, ScheduledJobSource,
};

/// Shared RPC registry handle, accessible from Tauri commands for local IPC.
#[derive(Clone)]
pub struct RpcRegistryState(pub Arc<cteno_host_rpc_core::RpcRegistry>);

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalHostInfoState {
    pub machine_id: String,
    pub shell_kind: String,
    pub local_rpc_env_tag: String,
    pub app_data_dir: String,
    pub host: String,
    pub platform: String,
    pub happy_cli_version: String,
    pub happy_home_dir: String,
    pub home_dir: String,
}
