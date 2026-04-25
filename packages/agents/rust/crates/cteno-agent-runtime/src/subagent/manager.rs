//! SubAgent Manager
//!
//! Manages the lifecycle of background SubAgents:
//! - Spawning new tasks
//! - Tracking status
//! - Notification delivery
//! - Cleanup

use super::{CleanupPolicy, SubAgent, SubAgentFilter, SubAgentNotification, SubAgentStatus};
use crate::agent::executor::SubAgentContext;
use crate::agent_config::AgentConfig;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

/// Global singleton SubAgentManager instance.
/// Shared across extension server and Happy client runtimes.
static GLOBAL_MANAGER: OnceLock<Arc<SubAgentManager>> = OnceLock::new();

/// Get the global SubAgentManager singleton (Arc for cross-runtime sharing).
pub fn global() -> Arc<SubAgentManager> {
    GLOBAL_MANAGER
        .get_or_init(|| Arc::new(SubAgentManager::new()))
        .clone()
}

/// Global SubAgent manager
#[derive(Clone)]
pub struct SubAgentManager {
    /// Active SubAgents (id -> SubAgent)
    subagents: Arc<RwLock<HashMap<String, SubAgent>>>,
    /// Notification queue (parent_session_id -> Vec<notification>)
    notifications: Arc<Mutex<HashMap<String, Vec<SubAgentNotification>>>>,
    /// Registered session channels for push-based notification delivery.
    /// When a SubAgent completes, its result is sent through the channel
    /// to the session that spawned it, triggering agent processing.
    session_senders: Arc<RwLock<HashMap<String, UnboundedSender<String>>>>,
}

impl SubAgentManager {
    /// Create a new SubAgent manager
    pub fn new() -> Self {
        Self {
            subagents: Arc::new(RwLock::new(HashMap::new())),
            notifications: Arc::new(Mutex::new(HashMap::new())),
            session_senders: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a session's message sender for push-based SubAgent notifications.
    /// Returns an `UnboundedReceiver` that the session should listen on.
    pub async fn register_session(
        &self,
        session_id: String,
    ) -> tokio::sync::mpsc::UnboundedReceiver<String> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.session_senders
            .write()
            .await
            .insert(session_id.clone(), tx);
        log::info!(
            "[SubAgentManager] Registered session '{}' for push notifications",
            session_id
        );
        rx
    }

    /// Unregister a session (e.g. when archived or disconnected).
    pub async fn unregister_session(&self, session_id: &str) {
        self.session_senders.write().await.remove(session_id);
        log::info!("[SubAgentManager] Unregistered session '{}'", session_id);
    }

    /// Spawn a new SubAgent (non-blocking)
    ///
    /// Returns the SubAgent ID immediately. The SubAgent will run in the background
    /// and send a notification to the parent session when complete.
    #[allow(clippy::too_many_arguments)]
    pub async fn spawn(
        &self,
        parent_session_id: String,
        agent_id: String,
        task: String,
        label: Option<String>,
        cleanup: CleanupPolicy,
        agent_config: AgentConfig,
        exec_ctx: SubAgentContext,
    ) -> Result<String, String> {
        let id = Uuid::new_v4().to_string();

        let subagent = SubAgent::new(
            id.clone(),
            parent_session_id.clone(),
            agent_id.clone(),
            task.clone(),
            label.clone(),
            cleanup.clone(),
        );
        let created_at_ms = subagent.created_at;

        // Store in memory
        self.subagents.write().await.insert(id.clone(), subagent);

        log::info!(
            "[SubAgentManager] Spawned SubAgent '{}' for parent '{}', agent '{}'",
            id,
            parent_session_id,
            agent_id
        );

        // Best-effort lifecycle emission to the host (mirror sink).
        // Skipped silently if no emitter is installed (tests / library use).
        if let Some(emitter) = crate::hooks::subagent_lifecycle_emitter() {
            emitter.emit(
                &parent_session_id,
                crate::hooks::SubAgentLifecycleEventDto::Spawned {
                    subagent_id: id.clone(),
                    agent_id: agent_id.clone(),
                    task: task.clone(),
                    label: label.clone(),
                    created_at_ms,
                },
            );
        }

        // Execute in background
        let manager = self.clone();
        let id_for_spawn = id.clone();
        let parent_for_lifecycle = parent_session_id.clone();
        tokio::spawn(async move {
            manager
                .execute_in_background(
                    id_for_spawn,
                    parent_for_lifecycle,
                    agent_config,
                    task,
                    exec_ctx,
                )
                .await;
        });

        Ok(id)
    }

    /// Execute SubAgent in background
    async fn execute_in_background(
        &self,
        id: String,
        parent_session_id: String,
        agent_config: AgentConfig,
        task: String,
        exec_ctx: SubAgentContext,
    ) {
        // Update status to Running
        self.update_status(&id, SubAgentStatus::Running).await;
        let started_at = Utc::now().timestamp_millis();
        self.update_field(&id, |sa| sa.started_at = Some(started_at))
            .await;

        if let Some(emitter) = crate::hooks::subagent_lifecycle_emitter() {
            emitter.emit(
                &parent_session_id,
                crate::hooks::SubAgentLifecycleEventDto::Started {
                    subagent_id: id.clone(),
                    started_at_ms: started_at,
                },
            );
        }

        log::info!("[SubAgentManager] Starting execution of SubAgent '{}'", id);

        // Execute the SubAgent (reuse existing executor). Pass the
        // SubAgent record id as the session id override so the agent
        // session row in `agent_sessions` shares the SubAgent's id —
        // clicking the entry in BackgroundRunsModal then navigates to
        // `/session/{subagent.id}` and finds its transcript.
        let result = crate::agent::executor::execute_sub_agent(
            &agent_config,
            &task,
            None, // context
            &exec_ctx,
            0, // depth = 0 (top-level, SubAgent is not a recursive call)
            Some(id.clone()),
        )
        .await;

        let completed_at = Utc::now().timestamp_millis();

        // Update result
        match result {
            Ok(response) => {
                if is_agent_abort_response(&response) {
                    log::warn!(
                        "[SubAgentManager] SubAgent '{}' aborted by user; treating as failed",
                        id
                    );

                    self.update_field(&id, |sa| {
                        sa.status = SubAgentStatus::Failed;
                        sa.error = Some(response.clone());
                        sa.completed_at = Some(completed_at);
                    })
                    .await;

                    if let Some(emitter) = crate::hooks::subagent_lifecycle_emitter() {
                        emitter.emit(
                            &parent_session_id,
                            crate::hooks::SubAgentLifecycleEventDto::Failed {
                                subagent_id: id.clone(),
                                error: response.clone(),
                                completed_at_ms: completed_at,
                            },
                        );
                    }

                    self.notify_parent(&id, SubAgentStatus::Failed, None, Some(response))
                        .await;
                    return;
                }

                log::info!(
                    "[SubAgentManager] SubAgent '{}' completed successfully, response length: {}",
                    id,
                    response.len()
                );

                self.update_field(&id, |sa| {
                    sa.status = SubAgentStatus::Completed;
                    sa.result = Some(response.clone());
                    sa.completed_at = Some(completed_at);
                })
                .await;

                if let Some(emitter) = crate::hooks::subagent_lifecycle_emitter() {
                    emitter.emit(
                        &parent_session_id,
                        crate::hooks::SubAgentLifecycleEventDto::Completed {
                            subagent_id: id.clone(),
                            result: Some(response.clone()),
                            completed_at_ms: completed_at,
                        },
                    );
                }

                // Send notification
                self.notify_parent(&id, SubAgentStatus::Completed, Some(response), None)
                    .await;
            }
            Err(e) => {
                log::error!("[SubAgentManager] SubAgent '{}' failed: {}", id, e);

                self.update_field(&id, |sa| {
                    sa.status = SubAgentStatus::Failed;
                    sa.error = Some(e.clone());
                    sa.completed_at = Some(completed_at);
                })
                .await;

                if let Some(emitter) = crate::hooks::subagent_lifecycle_emitter() {
                    emitter.emit(
                        &parent_session_id,
                        crate::hooks::SubAgentLifecycleEventDto::Failed {
                            subagent_id: id.clone(),
                            error: e.clone(),
                            completed_at_ms: completed_at,
                        },
                    );
                }

                self.notify_parent(&id, SubAgentStatus::Failed, None, Some(e))
                    .await;
            }
        }

        // Handle cleanup policy
        if let Some(sa) = self.get(&id).await {
            if sa.cleanup == CleanupPolicy::Delete {
                log::info!(
                    "[SubAgentManager] SubAgent '{}' has cleanup=delete, will remove in 60s",
                    id
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                self.remove(&id).await;
            }
        }
    }

    /// Send notification to parent session
    async fn notify_parent(
        &self,
        subagent_id: &str,
        status: SubAgentStatus,
        result: Option<String>,
        error: Option<String>,
    ) {
        let subagent = match self.get(subagent_id).await {
            Some(sa) => sa,
            None => {
                log::warn!(
                    "[SubAgentManager] Cannot notify: SubAgent '{}' not found",
                    subagent_id
                );
                return;
            }
        };

        let mut notification = SubAgentNotification::from_subagent(&subagent);
        notification.status = status;
        notification.result = result;
        notification.error = error;

        let parent_sid = subagent.parent_session_id.clone();
        log::info!(
            "[SubAgentManager] Notifying parent '{}' about SubAgent '{}'",
            parent_sid,
            subagent_id
        );

        let graph_owned = crate::task_graph::manager::is_task_graph_subagent(subagent_id).await;
        if graph_owned {
            log::info!(
                "[SubAgentManager] SubAgent '{}' is owned by a TaskGraph; suppressing generic parent notification",
                subagent_id
            );
        } else {
            // Push-based delivery: send through registered session channel.
            let message = notification.to_message();
            let senders = self.session_senders.read().await;
            if let Some(sender) = senders.get(&parent_sid) {
                match sender.send(message) {
                    Ok(_) => {
                        log::info!(
                            "[SubAgentManager] Pushed notification to session '{}'",
                            parent_sid
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "[SubAgentManager] Failed to push to session '{}': {}",
                            parent_sid,
                            e
                        );
                    }
                }
            } else {
                log::info!(
                    "[SubAgentManager] No registered session for '{}', notification queued only",
                    parent_sid
                );
            }
        }

        // Also store in notification queue (for HTTP polling fallback)
        let mut queue = self.notifications.lock().await;
        queue
            .entry(parent_sid)
            .or_insert_with(Vec::new)
            .push(notification.clone());

        // Runtime-owned DAGs advance from SubAgent completion here. This is
        // queued instead of awaited to avoid tying the SubAgent executor's
        // Send-ness to downstream graph scheduling.
        crate::task_graph::manager::observe_subagent_complete(notification);
    }

    /// Send a message to a registered session's notification channel.
    /// Used by persona task completion to push results to the parent session.
    pub async fn send_to_session(&self, session_id: &str, message: String) -> bool {
        let senders = self.session_senders.read().await;
        if let Some(sender) = senders.get(session_id) {
            match sender.send(message) {
                Ok(_) => {
                    log::info!(
                        "[SubAgentManager] Pushed message to session '{}'",
                        session_id
                    );
                    true
                }
                Err(e) => {
                    log::warn!(
                        "[SubAgentManager] Failed to push to session '{}': {}",
                        session_id,
                        e
                    );
                    false
                }
            }
        } else {
            log::warn!(
                "[SubAgentManager] No registered sender for session '{}', message dropped",
                session_id
            );
            false
        }
    }

    /// Get SubAgent by ID
    pub async fn get(&self, id: &str) -> Option<SubAgent> {
        self.subagents.read().await.get(id).cloned()
    }

    /// List SubAgents with optional filter
    pub async fn list(&self, filter: SubAgentFilter) -> Vec<SubAgent> {
        let subagents = self.subagents.read().await;
        subagents
            .values()
            .filter(|sa| filter.matches(sa))
            .cloned()
            .collect()
    }

    /// Stop a running SubAgent
    pub async fn stop(&self, id: &str) -> Result<(), String> {
        let subagent = self
            .get(id)
            .await
            .ok_or_else(|| format!("SubAgent '{}' not found", id))?;

        if !subagent.is_active() {
            return Err(format!(
                "SubAgent '{}' is not active (status: {})",
                id, subagent.status
            ));
        }

        // TODO: Implement abort mechanism
        // For now, just update status
        log::info!("[SubAgentManager] Stopping SubAgent '{}'", id);

        let completed_at = Utc::now().timestamp_millis();
        self.update_status(id, SubAgentStatus::Stopped).await;
        self.update_field(id, |sa| {
            sa.completed_at = Some(completed_at);
        })
        .await;

        if let Some(emitter) = crate::hooks::subagent_lifecycle_emitter() {
            emitter.emit(
                &subagent.parent_session_id,
                crate::hooks::SubAgentLifecycleEventDto::Stopped {
                    subagent_id: id.to_string(),
                    completed_at_ms: completed_at,
                },
            );
        }

        Ok(())
    }

    /// Remove a SubAgent from the registry
    pub async fn remove(&self, id: &str) -> bool {
        log::info!("[SubAgentManager] Removing SubAgent '{}'", id);
        self.subagents.write().await.remove(id).is_some()
    }

    /// Pop notifications for a parent session
    pub async fn pop_notifications(&self, parent_session_id: &str) -> Vec<SubAgentNotification> {
        let mut queue = self.notifications.lock().await;
        queue.remove(parent_session_id).unwrap_or_default()
    }

    /// Update SubAgent status
    async fn update_status(&self, id: &str, status: SubAgentStatus) {
        self.update_field(id, |sa| sa.status = status).await;
    }

    /// Update SubAgent with a mutation function
    async fn update_field<F>(&self, id: &str, f: F)
    where
        F: FnOnce(&mut SubAgent),
    {
        let mut subagents = self.subagents.write().await;
        if let Some(sa) = subagents.get_mut(id) {
            f(sa);
        }
    }

    /// Get statistics
    pub async fn stats(&self) -> SubAgentStats {
        let subagents = self.subagents.read().await;
        let mut stats = SubAgentStats::default();

        for sa in subagents.values() {
            stats.total += 1;
            match sa.status {
                SubAgentStatus::Pending => stats.pending += 1,
                SubAgentStatus::Running => stats.running += 1,
                SubAgentStatus::Completed => stats.completed += 1,
                SubAgentStatus::Failed => stats.failed += 1,
                SubAgentStatus::Stopped => stats.stopped += 1,
                SubAgentStatus::TimedOut => stats.timed_out += 1,
            }
        }

        stats
    }
}

impl Default for SubAgentManager {
    fn default() -> Self {
        Self::new()
    }
}

fn is_agent_abort_response(response: &str) -> bool {
    matches!(
        response.trim(),
        "Agent execution was aborted by user." | "Agent execution aborted by user"
    )
}

/// SubAgent statistics
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SubAgentStats {
    pub total: usize,
    pub pending: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub stopped: usize,
    pub timed_out: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_abort_response_is_failed_subagent_signal() {
        assert!(is_agent_abort_response(
            "Agent execution was aborted by user."
        ));
        assert!(is_agent_abort_response("Agent execution aborted by user"));
        assert!(!is_agent_abort_response(
            "The child task reported that another agent execution was aborted by user."
        ));
    }

    #[tokio::test]
    async fn test_manager_create() {
        let manager = SubAgentManager::new();
        let stats = manager.stats().await;
        assert_eq!(stats.total, 0);
    }

    #[tokio::test]
    async fn test_list_with_filter() {
        let manager = SubAgentManager::new();

        // Manually insert test SubAgents
        let sa1 = SubAgent::new(
            "id1".to_string(),
            "parent-123".to_string(),
            "agent1".to_string(),
            "task1".to_string(),
            None,
            CleanupPolicy::Keep,
        );

        let mut sa2 = sa1.clone();
        sa2.id = "id2".to_string();
        sa2.status = SubAgentStatus::Running;

        let mut sa3 = sa1.clone();
        sa3.id = "id3".to_string();
        sa3.parent_session_id = "parent-456".to_string();

        manager
            .subagents
            .write()
            .await
            .insert("id1".to_string(), sa1);
        manager
            .subagents
            .write()
            .await
            .insert("id2".to_string(), sa2);
        manager
            .subagents
            .write()
            .await
            .insert("id3".to_string(), sa3);

        // Filter by parent_session_id
        let filter = SubAgentFilter {
            parent_session_id: Some("parent-123".to_string()),
            status: None,
            active_only: false,
        };
        let results = manager.list(filter).await;
        assert_eq!(results.len(), 2);

        // Filter by status
        let filter = SubAgentFilter {
            parent_session_id: None,
            status: Some(SubAgentStatus::Running),
            active_only: false,
        };
        let results = manager.list(filter).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "id2");

        // Active only
        let filter = SubAgentFilter {
            parent_session_id: None,
            status: None,
            active_only: true,
        };
        let results = manager.list(filter).await;
        assert_eq!(results.len(), 3); // id1 (Pending) + id2 (Running) + id3 (Pending)
    }

    #[tokio::test]
    async fn test_notifications() {
        let manager = SubAgentManager::new();

        let sa = SubAgent::new(
            "id1".to_string(),
            "parent-123".to_string(),
            "agent1".to_string(),
            "task1".to_string(),
            Some("Test Task".to_string()),
            CleanupPolicy::Keep,
        );

        manager
            .subagents
            .write()
            .await
            .insert("id1".to_string(), sa);

        // Notify
        manager
            .notify_parent(
                "id1",
                SubAgentStatus::Completed,
                Some("Success!".to_string()),
                None,
            )
            .await;

        // Pop notifications
        let notifs = manager.pop_notifications("parent-123").await;
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].subagent_id, "id1");
        assert_eq!(notifs[0].status, SubAgentStatus::Completed);
        assert_eq!(notifs[0].result, Some("Success!".to_string()));

        // Pop again - should be empty
        let notifs = manager.pop_notifications("parent-123").await;
        assert_eq!(notifs.len(), 0);
    }
}
