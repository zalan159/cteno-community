//! Persona Manager
//!
//! Orchestrates persona lifecycle: creation, task dispatch, notifications.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::models::{
    Persona, PersonaSessionLink, PersonaSessionType, TaskNodeInput, TaskNodeStatus,
};
use super::store::PersonaStore;
use crate::task_graph::{TaskGraphDelegate, TaskGraphEngine, TaskNodeState, build_group_summary};

/// Summary of an active task session.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub session_id: String,
    pub task_description: String,
    pub created_at: String,
}

/// Manages persona lifecycle and task dispatch.
pub struct PersonaManager {
    store: PersonaStore,
    db_path: PathBuf,
    /// Active task sessions tracked per persona: persona_id -> Set<session_id>
    active_tasks: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    /// Shared DAG execution engine.
    graph_engine: Arc<TaskGraphEngine>,
}

impl PersonaManager {
    fn resolve_dispatch_agent_flavor(
        owner: &crate::agent_owner::AgentOwnerInfo,
        agent_flavor_override: Option<&str>,
    ) -> String {
        agent_flavor_override
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(owner.agent_flavor.as_str())
            .to_string()
    }

    fn ensure_dispatch_vendor_available(agent_flavor: &str) -> Result<(), String> {
        let registry = crate::local_services::executor_registry()?;
        Self::validate_dispatch_vendor_availability(
            agent_flavor,
            registry.is_vendor_installed(agent_flavor),
        )
    }

    fn validate_dispatch_vendor_availability(
        agent_flavor: &str,
        availability: Result<bool, String>,
    ) -> Result<(), String> {
        match availability {
            Ok(true) => Ok(()),
            Ok(false) | Err(_) => Err(format!(
                "vendor {} not available on this host",
                agent_flavor
            )),
        }
    }

    pub fn new(db_path: PathBuf, graph_engine: Arc<TaskGraphEngine>) -> Self {
        let store = PersonaStore::new(db_path.clone());
        Self {
            store,
            db_path,
            active_tasks: Arc::new(RwLock::new(HashMap::new())),
            graph_engine,
        }
    }

    /// Access the underlying store for direct CRUD.
    pub fn store(&self) -> &PersonaStore {
        &self.store
    }

    pub fn db_path(&self) -> &PathBuf {
        &self.db_path
    }

    /// Create a new persona.
    ///
    /// The `chat_session_id` is set to a placeholder initially.
    /// The RPC handler will create the real session on Happy Server
    /// and call `store().update_chat_session_id()` with the server-assigned ID.
    pub fn create_persona(
        &self,
        name: &str,
        description: &str,
        model: &str,
        avatar_id: Option<&str>,
        profile_id: Option<&str>,
        agent: Option<&str>,
        workdir: Option<&str>,
    ) -> Result<Persona, String> {
        let persona_id = uuid::Uuid::new_v4().to_string();
        // Placeholder — will be replaced by the Happy Server session ID
        let chat_session_id = format!("pending-{}", persona_id);
        let now = chrono::Utc::now().to_rfc3339();

        let persona = Persona {
            id: persona_id.clone(),
            name: name.to_string(),
            avatar_id: avatar_id.unwrap_or("default").to_string(),
            description: description.to_string(),
            personality_notes: String::new(),
            model: model.to_string(),
            profile_id: profile_id.map(|s| s.to_string()),
            agent: Some(agent.unwrap_or("cteno").to_string()),
            workdir: workdir.unwrap_or("~").to_string(),
            chat_session_id: chat_session_id.clone(),
            is_default: false,
            continuous_browsing: false,
            created_at: now.clone(),
            updated_at: now.clone(),
        };

        self.store.create_persona(&persona)?;

        // Link placeholder chat session to persona
        let link = PersonaSessionLink {
            persona_id: persona_id.clone(),
            session_id: chat_session_id,
            session_type: PersonaSessionType::Chat,
            task_description: None,
            agent_type: None,
            owner_kind: "persona".to_string(),
            label: None,
            created_at: now,
        };
        self.store.link_session(&link)?;

        // Create persona's private memory directory at {workdir}/.cteno/memory/
        let workdir_expanded = shellexpand::tilde(workdir.unwrap_or("~")).to_string();
        let memory_dir = std::path::PathBuf::from(&workdir_expanded)
            .join(".cteno")
            .join("memory");
        if let Err(e) = std::fs::create_dir_all(&memory_dir) {
            log::warn!(
                "[Persona] Failed to create private memory dir at {}: {}",
                memory_dir.display(),
                e
            );
        } else {
            let memory_file = memory_dir.join("MEMORY.md");
            if !memory_file.exists() {
                let default_content = format!(
                    "# {} 的记忆\n\n\
                    ## 偏好\n\n\
                    ## 经验教训\n\n\
                    ## 站点知识\n\
                    <!-- Browser Agent 发现的 API 端点、认证方式、提取 Schema -->\n\n\
                    ## 任务模板\n\
                    <!-- 成功的 dispatch 模式：什么任务用什么 agent_type、profile、参数 -->\n\n\
                    ## 失败记录\n\
                    <!-- 踩过的坑和解决方案，避免重蹈覆辙 -->\n",
                    name
                );
                if let Err(e) = std::fs::write(&memory_file, default_content) {
                    log::warn!(
                        "[Persona] Failed to create default MEMORY.md at {}: {}",
                        memory_file.display(),
                        e
                    );
                }
            }
        }

        log::info!("[Persona] Created persona '{}' ({})", name, persona_id);
        Ok(persona)
    }

    /// Dispatch a task to a new worker session (async version).
    ///
    /// Creates a real Happy Session on the server, establishes a session-scoped
    /// Socket.IO connection, and sends the task prompt as the first user message
    /// to trigger agent execution.
    ///
    /// Returns the new Happy Session ID.
    pub async fn dispatch_task_async(
        &self,
        persona_id: &str,
        task_description: &str,
        workdir: Option<&str>,
        profile_id: Option<&str>,
        skill_ids: Option<&[String]>,
        agent_type: Option<&str>,
        agent_flavor_override: Option<&str>,
        label: Option<&str>,
        permission_mode_override: Option<super::super::happy_client::permission::PermissionMode>,
    ) -> Result<String, String> {
        let spawn_config = crate::local_services::spawn_config()?;
        let owner = crate::agent_owner::resolve_owner(persona_id)?;
        let agent_flavor = Self::resolve_dispatch_agent_flavor(&owner, agent_flavor_override);
        Self::ensure_dispatch_vendor_available(&agent_flavor)?;

        let fallback_profile = spawn_config.agent_config.profile_id.read().await.clone();
        let resolved_profile_id =
            crate::agent_owner::resolve_profile_id(&owner, profile_id, &fallback_profile);
        let owner_workdir = owner.workdir.clone();
        let owner_chat_session_id = Some(owner.chat_session_id.clone());

        let directory = workdir.unwrap_or_else(|| {
            if owner_workdir.is_empty() {
                "~"
            } else {
                &owner_workdir
            }
        });

        let inherit_mode = if let Some(mode) = permission_mode_override {
            Some(mode)
        } else if let Some(ref chat_sid) = owner_chat_session_id {
            spawn_config
                .session_connections
                .get(chat_sid)
                .await
                .map(|conn| conn.get_permission_mode())
        } else {
            None
        };
        if let Some(mode) = inherit_mode {
            log::info!(
                "[Persona] Task session will inherit permission mode {:?} from owner {} (session {:?})",
                mode,
                persona_id,
                owner_chat_session_id
            );
        }

        let skill_ids_vec = skill_ids.map(|s| s.to_vec());
        let task_msg = task_description.to_string();
        let session_id = crate::happy_client::manager::spawn_session_internal(
            &spawn_config,
            directory,
            &agent_flavor,
            &resolved_profile_id,
            None,
            inherit_mode,
            None,
            skill_ids_vec,
        )
        .await?;

        let now = chrono::Utc::now().to_rfc3339();
        let owner_kind = crate::agent_owner::resolve_owner(persona_id)
            .map(|info| info.owner_kind.as_str().to_string())
            .unwrap_or_else(|_| "persona".to_string());
        let link = PersonaSessionLink {
            persona_id: persona_id.to_string(),
            session_id: session_id.clone(),
            session_type: PersonaSessionType::Task,
            task_description: Some(task_description.to_string()),
            agent_type: agent_type.map(|s| s.to_string()),
            owner_kind,
            label: label.map(|s| s.to_string()),
            created_at: now,
        };
        self.store.link_session(&link)?;

        if let Some(lbl) = label {
            if let Ok(orch_store) = crate::local_services::orchestration_store() {
                orch_store.link_session_to_node_by_label(persona_id, lbl, &session_id);
            }
        }

        {
            if let Some(handle) = spawn_config
                .session_connections
                .get(&session_id)
                .await
                .map(|conn| conn.message_handle())
            {
                if let Err(e) = handle.send_initial_user_message(&task_msg).await {
                    log::error!(
                        "[Persona] Failed to send initial message to task session {}: {}",
                        session_id,
                        e
                    );
                }
            }
        }

        {
            let mut tasks = self.active_tasks.write().await;
            tasks
                .entry(persona_id.to_string())
                .or_default()
                .insert(session_id.clone());
        }

        log::info!(
            "[Persona] Dispatched task '{}' for persona {} -> Happy Session {}",
            task_description,
            persona_id,
            session_id
        );
        Ok(session_id)
    }

    /// Dispatch a task (sync wrapper). Delegates to dispatch_task_async via block_in_place.
    pub fn dispatch_task(
        &self,
        persona_id: &str,
        task_description: &str,
        workdir: Option<&str>,
        profile_id: Option<&str>,
        skill_ids: Option<&[String]>,
        agent_type: Option<&str>,
        agent_flavor_override: Option<&str>,
        label: Option<&str>,
        permission_mode_override: Option<super::super::happy_client::permission::PermissionMode>,
    ) -> Result<String, String> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.dispatch_task_async(
                persona_id,
                task_description,
                workdir,
                profile_id,
                skill_ids,
                agent_type,
                agent_flavor_override,
                label,
                permission_mode_override,
            ))
        })
    }

    /// Notify persona that a task session's worker loop finished.
    /// Only sends the result back — does NOT unlink or clean up the session.
    /// The task session stays alive waiting for persona to either
    /// `send_to_session` (continue) or `close_task_session` (terminate).
    ///
    /// Also advances the task graph if this session belongs to one.
    pub fn notify_task_result(&self, session_id: &str) {
        if let Ok(Some(link)) = self.store.get_persona_for_session(session_id) {
            if link.session_type == PersonaSessionType::Task {
                self.send_task_result_to_persona(session_id, &link);
                log::info!(
                    "[Persona] Task result from session {} pushed to persona {}",
                    session_id,
                    link.persona_id
                );
            }
        }

        // Update orchestration flow node status if this session is linked to one
        if let Ok(orch_store) = crate::local_services::orchestration_store() {
            orch_store.on_session_complete(session_id);
        }

        // Advance task graph if this session is part of one
        let delegate = PersonaGraphDelegate {
            db_path: self.db_path.clone(),
            store: PersonaStore::new(self.db_path.clone()),
        };
        self.graph_engine.on_session_complete(session_id, &delegate);
    }

    /// Called when a task session is explicitly closed (via close_task_session tool).
    /// Unlinks from DB and removes from active tracking.
    pub async fn on_task_complete(&self, session_id: &str) {
        if let Ok(Some(link)) = self.store.get_persona_for_session(session_id) {
            if link.session_type == PersonaSessionType::Task {
                // Remove from in-memory active tracking
                let mut tasks = self.active_tasks.write().await;
                if let Some(set) = tasks.get_mut(&link.persona_id) {
                    set.remove(session_id);
                }

                // Remove from DB so list_active_tasks no longer returns it
                if let Err(e) = self.store.unlink_session(&link.persona_id, session_id) {
                    log::warn!(
                        "[Persona] Failed to unlink task session {} from persona {}: {}",
                        session_id,
                        link.persona_id,
                        e
                    );
                }

                log::info!(
                    "[Persona] Task session {} closed and unlinked from persona {}",
                    session_id,
                    link.persona_id
                );
            }
        }
    }

    /// Send the task session's final result back to the persona's chat session
    /// as a simulated user message via the session's Socket.IO connection.
    /// This ensures the message appears in the frontend chat history and
    /// triggers the persona agent to process the result.
    fn send_task_result_to_persona(&self, session_id: &str, link: &PersonaSessionLink) {
        // Resolve the owner via unified lookup
        let (chat_session_id, persona_name) =
            match crate::agent_owner::resolve_owner(&link.persona_id) {
                Ok(info) => (info.chat_session_id, info.name),
                Err(e) => {
                    log::warn!("[Persona] Cannot send task result: {}", e);
                    return;
                }
            };

        // Extract the final text output from the task session.
        let session_mgr = crate::agent_session::AgentSessionManager::new(self.db_path.clone());
        let final_output = session_mgr.extract_final_output(session_id);

        let task_desc = link.task_description.as_deref().unwrap_or("unknown task");
        let callback_msg = if !final_output.is_empty() {
            format!(
                "[Task Complete - session {}]\nTask: {}\nResult:\n{}",
                session_id, task_desc, final_output
            )
        } else {
            format!(
                "[Task Complete - session {}]\nTask: {}\nResult: (no output captured)",
                session_id, task_desc
            )
        };

        // Send to persona's chat session via its SessionConnection.
        // This sends through Socket.IO (visible in frontend) + injects into
        // local agent queue (triggers persona agent to process the result).
        tokio::spawn(async move {
            let spawn_config = match crate::local_services::spawn_config() {
                Ok(c) => c,
                Err(e) => {
                    log::error!("[Persona] Cannot get spawn config: {}", e);
                    return;
                }
            };

            match crate::session_delivery::deliver_message_to_session(
                &spawn_config,
                &chat_session_id,
                &callback_msg,
                "Persona",
            )
            .await
            {
                Ok(()) => {
                    log::info!(
                        "[Persona] Task result pushed to persona '{}' (chat session {})",
                        persona_name,
                        chat_session_id
                    );
                }
                Err(e) => {
                    log::error!(
                        "[Persona] Failed to deliver task result to persona '{}': {}",
                        persona_name,
                        e
                    );
                }
            }
        });
    }

    /// Auto-rename a persona based on the first user message.
    /// Calls DeepSeek API to generate a short title (max 10 chars).
    pub async fn auto_rename_persona(
        &self,
        persona_id: &str,
        first_message: &str,
        api_key: &str,
    ) -> Result<(), String> {
        let prompt = format!(
            "用5个字以内总结这条消息的主题，只返回标题，不要标点：{}",
            first_message
        );

        let body = serde_json::json!({
            "model": "deepseek-chat",
            "messages": [{ "role": "user", "content": prompt }],
            "stream": false,
            "max_tokens": 20
        });

        let client = reqwest::Client::new();
        let resp = client
            .post("https://api.deepseek.com/chat/completions")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("DeepSeek API request failed: {}", e))?;

        let status = resp.status();
        let resp_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {}", e))?;

        if !status.is_success() {
            return Err(format!("DeepSeek API error ({}): {}", status, resp_text));
        }

        let resp_json: serde_json::Value = serde_json::from_str(&resp_text)
            .map_err(|e| format!("Failed to parse response JSON: {}", e))?;

        let new_name = resp_json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| "No content in DeepSeek response".to_string())?;

        // Trim whitespace and limit to 10 chars
        let new_name = new_name.trim();
        let new_name: String = new_name.chars().take(10).collect();

        if new_name.is_empty() {
            return Err("Generated name is empty".to_string());
        }

        self.store.update_name(persona_id, &new_name)?;
        log::info!(
            "[Persona] Auto-renamed persona {} to '{}'",
            persona_id,
            new_name
        );
        Ok(())
    }

    /// List active task sessions for a persona.
    pub fn list_active_tasks(&self, persona_id: &str) -> Result<Vec<TaskSummary>, String> {
        let links = self.store.list_task_sessions(persona_id)?;
        Ok(links
            .into_iter()
            .map(|link| TaskSummary {
                session_id: link.session_id,
                task_description: link.task_description.unwrap_or_default(),
                created_at: link.created_at,
            })
            .collect())
    }

    // ========================================================================
    // Task Graph (delegated to shared engine)
    // ========================================================================

    /// Dispatch a task graph (DAG of tasks with dependencies).
    ///
    /// Validates the DAG, stores state in memory, and starts all root tasks
    /// (those with no dependencies). Returns the group_id.
    pub fn dispatch_task_graph(
        &self,
        persona_id: &str,
        tasks: &[TaskNodeInput],
    ) -> Result<String, String> {
        let delegate = PersonaGraphDelegate {
            db_path: self.db_path.clone(),
            store: PersonaStore::new(self.db_path.clone()),
        };
        self.graph_engine
            .dispatch_graph(persona_id, tasks, &delegate)
    }

    /// Send a message to a persona's chat session.
    #[allow(dead_code)]
    fn send_message_to_persona(&self, persona_id: &str, message: &str) {
        let persona = match self.store.get_persona(persona_id) {
            Ok(Some(p)) => p,
            _ => return,
        };

        let chat_session_id = persona.chat_session_id.clone();
        let persona_id_owned = persona_id.to_string();
        let msg = message.to_string();
        tokio::spawn(async move {
            let spawn_config = match crate::local_services::spawn_config() {
                Ok(c) => c,
                Err(_) => return,
            };
            if let Some(handle) = spawn_config
                .session_connections
                .get(&chat_session_id)
                .await
                .map(|conn| conn.message_handle())
            {
                if let Err(e) = handle.send_initial_user_message(&msg).await {
                    log::error!(
                        "[TaskGraph] Failed to send group summary to persona {}: {}",
                        persona_id_owned,
                        e
                    );
                }
            }
        });
    }
}

// ============================================================================
// PersonaGraphDelegate — bridges TaskGraphEngine to Persona system
// ============================================================================

struct PersonaGraphDelegate {
    db_path: PathBuf,
    store: PersonaStore,
}

impl TaskGraphDelegate for PersonaGraphDelegate {
    fn spawn_worker(
        &self,
        owner_id: &str,
        task_message: &str,
        workdir: Option<&str>,
        profile_id: Option<&str>,
        skill_ids: Option<&[String]>,
        agent_type: Option<&str>,
    ) -> Result<String, String> {
        // Delegate to PersonaManager.dispatch_task (via local_services)
        let persona_manager = crate::local_services::persona_manager()?;
        persona_manager.dispatch_task(
            owner_id,
            task_message,
            workdir,
            profile_id,
            skill_ids,
            agent_type,
            None,
            None, // no orchestration label for graph tasks
            None,
        )
    }

    fn extract_final_output(&self, session_id: &str) -> String {
        let session_mgr = crate::agent_session::AgentSessionManager::new(self.db_path.clone());
        session_mgr.extract_final_output(session_id)
    }

    fn on_graph_complete(&self, _group_id: &str, owner_id: &str, nodes: &[TaskNodeState]) {
        let any_failed = nodes.iter().any(|n| n.status == TaskNodeStatus::Failed);
        let status = if any_failed { "failed" } else { "completed" };
        let summary = build_group_summary(nodes, status);

        // Send summary to persona's chat session
        let persona = match self.store.get_persona(owner_id) {
            Ok(Some(p)) => p,
            _ => return,
        };

        let chat_session_id = persona.chat_session_id.clone();
        let owner_id_owned = owner_id.to_string();
        let _summary = summary;
        tokio::spawn(async move {
            let spawn_config = match crate::local_services::spawn_config() {
                Ok(c) => c,
                Err(_) => return,
            };
            if let Some(handle) = spawn_config
                .session_connections
                .get(&chat_session_id)
                .await
                .map(|conn| conn.message_handle())
            {
                if let Err(e) = handle.send_initial_user_message(&_summary).await {
                    log::error!(
                        "[TaskGraph] Failed to send group summary to persona {}: {}",
                        owner_id_owned,
                        e
                    );
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::PersonaManager;
    use crate::agent_owner::{AgentOwnerInfo, OwnerKind};

    fn owner(agent_flavor: &str) -> AgentOwnerInfo {
        AgentOwnerInfo {
            owner_id: "persona-1".to_string(),
            owner_kind: OwnerKind::Persona,
            chat_session_id: "chat-1".to_string(),
            agent_flavor: agent_flavor.to_string(),
            workdir: "~".to_string(),
            profile_id: None,
            name: "Persona".to_string(),
        }
    }

    #[test]
    fn dispatch_uses_persona_agent_when_override_missing() {
        assert_eq!(
            PersonaManager::resolve_dispatch_agent_flavor(&owner("cteno"), None),
            "cteno"
        );
    }

    #[test]
    fn dispatch_override_takes_precedence_over_persona_agent() {
        assert_eq!(
            PersonaManager::resolve_dispatch_agent_flavor(&owner("cteno"), Some("codex")),
            "codex"
        );
    }

    #[test]
    fn blank_override_falls_back_to_persona_agent() {
        assert_eq!(
            PersonaManager::resolve_dispatch_agent_flavor(&owner("cteno"), Some("   ")),
            "cteno"
        );
    }

    #[test]
    fn claude_persona_keeps_claude_as_default_dispatch_vendor() {
        assert_eq!(
            PersonaManager::resolve_dispatch_agent_flavor(&owner("claude"), None),
            "claude"
        );
    }

    #[test]
    fn unavailable_vendor_returns_explicit_host_error() {
        assert_eq!(
            PersonaManager::validate_dispatch_vendor_availability("claude", Ok(false)),
            Err("vendor claude not available on this host".to_string())
        );
    }

    #[test]
    fn unknown_vendor_also_returns_explicit_host_error() {
        assert_eq!(
            PersonaManager::validate_dispatch_vendor_availability(
                "mystery",
                Err("unknown vendor: mystery".to_string())
            ),
            Err("vendor mystery not available on this host".to_string())
        );
    }
}
