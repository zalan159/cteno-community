//! Boot + wire the global `UsageMonitor` into the daemon RPC surface.
//!
//! One monitor per daemon process. Spawn lazily on first access so tests
//! that instantiate the rpc layer without a live Claude/Codex/Gemini CLI
//! don't trip the probes. Probes poll on their own schedule; the RPC
//! handlers only ever read the cached snapshot.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use cteno_host_rpc_core::RpcRegistry;
use cteno_host_usage_monitor::{
    probes::{claude::ClaudeProbe, codex::CodexProbe, gemini::GeminiProbe},
    UsageMonitor,
};
use serde_json::Value;

static USAGE_MONITOR: OnceLock<Arc<UsageMonitor>> = OnceLock::new();
const POLL_INTERVAL: Duration = Duration::from_secs(60);

pub fn usage_monitor() -> Arc<UsageMonitor> {
    USAGE_MONITOR
        .get_or_init(|| {
            let monitor = UsageMonitor::new();
            monitor.spawn_probe(ClaudeProbe::new(), POLL_INTERVAL);
            monitor.spawn_probe(CodexProbe::new(), POLL_INTERVAL);
            monitor.spawn_probe(GeminiProbe::new(), POLL_INTERVAL);
            log::info!("[usage-monitor] probes spawned for claude/codex/gemini at 60s interval");
            monitor
        })
        .clone()
}

/// Register per-machine RPC endpoints. Called from
/// `multi_agent::register_local_workspace_rpc_handlers`.
pub async fn register_rpc(registry: Arc<RpcRegistry>, machine_id: &str) {
    let read_method = format!("{}:usage-read", machine_id);

    let monitor = usage_monitor();
    registry
        .register_persistent(&read_method, move |_params: Value| {
            let monitor = monitor.clone();
            async move {
                let snap = monitor.snapshot().await;
                serde_json::to_value(&snap)
                    .map_err(|e| format!("usage snapshot serialization failed: {}", e))
            }
        })
        .await;

    log::info!("Registered usage monitor RPC: {}", read_method);
}
