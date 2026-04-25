//! Process-local mirror of cteno-agent's SubAgent registry.
//!
//! cteno-agent owns the canonical SubAgent state inside its child process
//! (`cteno_agent_runtime::subagent::manager::SubAgentManager`). The host
//! cannot reach into that process to inspect it directly, so we mirror
//! lifecycle events here as they arrive over stdio (`Outbound::SubAgentLifecycle`,
//! routed by the cteno adapter's dispatcher into `SessionEventSink::on_subagent_lifecycle`).
//!
//! The mirror is the data source for:
//!
//! - `list_subagents` machine RPC (replaces the legacy desktop-side
//!   `crate::subagent::manager::global()` lookup that always returned
//!   empty for cteno-agent-spawned subagents).
//! - The `local-session:subagents-updated` Tauri event the
//!   `BackgroundRunsModal` subscribes to so it can re-render in real
//!   time without polling.
//!
//! Stays purely in-memory. The agent-side persisted SubAgent metadata
//! (in cteno-agent's process — and any session row in `agent_sessions`
//! for the subagent's transcript) remains the durable source; the
//! mirror only exists so the host UI can show live state.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use cteno_agent_runtime::subagent::{SubAgent, SubAgentFilter, SubAgentStatus};
use multi_agent_runtime_cteno::SubAgentLifecycleEvent;
use serde_json::json;
use tauri::{AppHandle, Emitter};

static GLOBAL_MIRROR: OnceLock<Arc<SubAgentMirror>> = OnceLock::new();

pub fn install(app_handle: AppHandle) -> Arc<SubAgentMirror> {
    let mirror = Arc::new(SubAgentMirror::new(app_handle));
    if GLOBAL_MIRROR.set(mirror.clone()).is_err() {
        log::warn!("[SubAgentMirror] global mirror already installed; ignoring");
    }
    mirror
}

pub fn instance() -> Option<Arc<SubAgentMirror>> {
    GLOBAL_MIRROR.get().cloned()
}

pub struct SubAgentMirror {
    app_handle: AppHandle,
    /// id → SubAgent
    by_id: RwLock<HashMap<String, SubAgent>>,
    /// parent_session_id → ordered list of subagent ids spawned under it
    by_parent: RwLock<HashMap<String, Vec<String>>>,
}

impl SubAgentMirror {
    pub fn new(app_handle: AppHandle) -> Self {
        Self {
            app_handle,
            by_id: RwLock::new(HashMap::new()),
            by_parent: RwLock::new(HashMap::new()),
        }
    }

    /// Apply one lifecycle transition. Updates the in-memory mirror and
    /// emits `local-session:subagents-updated` so the frontend can
    /// re-fetch.
    pub fn apply_lifecycle(&self, parent_session_id: &str, event: SubAgentLifecycleEvent) {
        let kind = match &event {
            SubAgentLifecycleEvent::Spawned { .. } => "spawned",
            SubAgentLifecycleEvent::Started { .. } => "started",
            SubAgentLifecycleEvent::Updated { .. } => "updated",
            SubAgentLifecycleEvent::Completed { .. } => "completed",
            SubAgentLifecycleEvent::Failed { .. } => "failed",
            SubAgentLifecycleEvent::Stopped { .. } => "stopped",
        };
        log::info!(
            "[SubAgentMirror] apply {kind} for parent={parent_session_id}"
        );
        match event {
            SubAgentLifecycleEvent::Spawned {
                subagent_id,
                agent_id,
                task,
                label,
                created_at_ms,
            } => {
                let mut sa = SubAgent::new(
                    subagent_id.clone(),
                    parent_session_id.to_string(),
                    agent_id,
                    task,
                    label,
                    cteno_agent_runtime::subagent::CleanupPolicy::Keep,
                );
                sa.created_at = created_at_ms;
                self.by_id.write().unwrap().insert(subagent_id.clone(), sa);
                self.by_parent
                    .write()
                    .unwrap()
                    .entry(parent_session_id.to_string())
                    .or_default()
                    .push(subagent_id);
            }
            SubAgentLifecycleEvent::Started {
                subagent_id,
                started_at_ms,
            } => {
                if let Some(sa) = self.by_id.write().unwrap().get_mut(&subagent_id) {
                    sa.status = SubAgentStatus::Running;
                    sa.started_at = Some(started_at_ms);
                }
            }
            SubAgentLifecycleEvent::Updated {
                subagent_id,
                iteration_count,
            } => {
                if let Some(sa) = self.by_id.write().unwrap().get_mut(&subagent_id) {
                    sa.iteration_count = iteration_count;
                }
            }
            SubAgentLifecycleEvent::Completed {
                subagent_id,
                result,
                completed_at_ms,
            } => {
                if let Some(sa) = self.by_id.write().unwrap().get_mut(&subagent_id) {
                    sa.status = SubAgentStatus::Completed;
                    sa.result = result;
                    sa.completed_at = Some(completed_at_ms);
                }
            }
            SubAgentLifecycleEvent::Failed {
                subagent_id,
                error,
                completed_at_ms,
            } => {
                if let Some(sa) = self.by_id.write().unwrap().get_mut(&subagent_id) {
                    sa.status = SubAgentStatus::Failed;
                    sa.error = Some(error);
                    sa.completed_at = Some(completed_at_ms);
                }
            }
            SubAgentLifecycleEvent::Stopped {
                subagent_id,
                completed_at_ms,
            } => {
                if let Some(sa) = self.by_id.write().unwrap().get_mut(&subagent_id) {
                    sa.status = SubAgentStatus::Stopped;
                    sa.completed_at = Some(completed_at_ms);
                }
            }
        }

        if let Err(e) = self.app_handle.emit(
            "local-session:subagents-updated",
            json!({ "sessionId": parent_session_id }),
        ) {
            log::warn!(
                "[SubAgentMirror] failed to emit subagents-updated for {parent_session_id}: {e}"
            );
        }
    }

    /// Snapshot all subagents matching the filter. Mirrors the API of
    /// `cteno_agent_runtime::subagent::SubAgentManager::list`.
    pub fn list(&self, filter: SubAgentFilter) -> Vec<SubAgent> {
        let by_id = self.by_id.read().unwrap();
        let mut out: Vec<SubAgent> = by_id
            .values()
            .filter(|sa| {
                if let Some(parent) = &filter.parent_session_id {
                    if &sa.parent_session_id != parent {
                        return false;
                    }
                }
                if let Some(status) = &filter.status {
                    if &sa.status != status {
                        return false;
                    }
                }
                if filter.active_only && !sa.is_active() {
                    return false;
                }
                true
            })
            .cloned()
            .collect();
        // Stable order: by created_at ascending (matches spawn order).
        out.sort_by_key(|sa| sa.created_at);
        out
    }

    pub fn get(&self, id: &str) -> Option<SubAgent> {
        self.by_id.read().unwrap().get(id).cloned()
    }
}
