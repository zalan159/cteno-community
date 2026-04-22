//! Local RPC Server — Unix Domain Socket IPC for ctenoctl.
//!
//! Generic socket routing / completion registry / kind registry live in
//! `cteno-host-bridge-localrpc`. The app-specific auth gate now lives in
//! `crate::host::hooks::AppLocalRpcAuthGate`.

use std::sync::Arc;

use crate::host::hooks::AppLocalRpcAuthGate;

pub use cteno_host_bridge_localrpc::{
    cleanup, env_tag_from_data_dir, get_session_kind_label, register_completion,
    remove_session_kind_label, set_session_kind_label, socket_path, socket_path_for_env,
    try_complete_cli_session,
};

pub async fn start(
    registry: Arc<cteno_host_rpc_core::RpcRegistry>,
    machine_id: String,
    env_tag: String,
) {
    let gate = Arc::new(AppLocalRpcAuthGate);
    cteno_host_bridge_localrpc::start_with_gate(registry, machine_id, env_tag, gate).await;
}
