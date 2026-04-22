use super::local_rpc::unregister_session_local_rpcs;
use super::*;
use crate::llm_profile::ApiFormat;
use crate::session_message_codec::SessionMessageCodec;
use multi_agent_runtime_core::{AgentExecutor, ModelChangeOutcome, ModelSpec, SessionRef};

/// Configuration for agent execution within a session.
#[derive(Clone)]
pub struct SessionAgentConfig {
    pub db_path: PathBuf,
    pub profile_id: Arc<RwLock<String>>,
    pub profile_store: Arc<RwLock<ProfileStore>>,
    pub global_api_key: String,
    pub system_prompt: String,
    pub skills: Vec<SkillConfig>,
    pub server_url: String,
    pub auth_token: String,
    pub builtin_skills_dir: PathBuf,
    pub user_skills_dir: PathBuf,
    /// Active MCP server IDs for this session (empty = all MCP tools available)
    pub session_mcp_server_ids: Arc<RwLock<Vec<String>>>,
    /// Builtin agents directory (src-tauri/agents/)
    pub builtin_agents_dir: PathBuf,
    /// User agents directory (~/.cteno/agents/)
    pub user_agents_dir: PathBuf,
    /// Dynamic proxy profiles fetched from server
    pub proxy_profiles: Arc<RwLock<Vec<LlmProfile>>>,
    /// If set, only these tool IDs are available to the agent.
    /// None = all tools available (default for regular sessions).
    pub allowed_tool_ids: Option<Vec<String>>,
    /// Skill IDs to pre-activate when the session starts.
    /// These are injected as <activated_skill> blocks in runtime context.
    pub pre_activated_skill_ids: Option<Vec<String>>,
}

/// Manages a single session-scoped Socket.IO connection.
#[derive(Clone)]
pub struct SessionConnection {
    pub(super) session_id: String,
    pub(super) socket: Arc<HappySocket>,
    /// Session transport codec. This may be plaintext when no session data key
    /// was negotiated (represented upstream as an all-zero encryption key).
    pub(super) message_codec: SessionMessageCodec,
    pub(super) heartbeat_running: Arc<AtomicBool>,
    pub(super) execution_state: ExecutionState,
    pub(super) consecutive_failures: Arc<AtomicU32>,
    pub(super) permission_handler: Arc<PermissionHandler>,
    pub(super) profile_id: Arc<RwLock<String>>,
    pub(super) context_tokens: Arc<AtomicU32>,
    pub(super) compression_threshold: Arc<AtomicU32>,
    pub(super) agent_config: SessionAgentConfig,
    /// When true, the current message originated from Tauri IPC (desktop local).
    /// Streaming deltas are sent via Tauri events instead of Socket.IO transient messages.
    pub(super) local_origin: Arc<AtomicBool>,
    /// Session connections map, needed for worker loop to pass to handle_user_message.
    pub(super) session_connections: SessionRegistry,
    /// Optional vendor executor handle. Session turns require this together
    /// with `session_ref`; missing executor state now fails the turn instead
    /// of falling back to in-process autonomous-agent execution.
    ///
    /// The `ExecutorNormalizer` itself is built fresh per turn inside
    /// `handle_user_message` (its `task_id` has turn-scoped lifetime), so
    /// it does not live on the connection.
    pub(super) executor: Option<Arc<dyn AgentExecutor>>,
    /// Vendor-native session handle returned by `executor.spawn_session` /
    /// `resume_session`. Present iff `executor` is set.
    pub(super) session_ref: Option<SessionRef>,
}

/// Cloneable handle for delivering messages to a session without holding
/// the global `session_connections` map lock across async awaits.
#[derive(Clone)]
pub struct SessionConnectionHandle {
    pub(super) session_id: String,
    pub(super) socket: Arc<HappySocket>,
    /// Mirror of `SessionConnection::message_codec`; may be plaintext for
    /// zero-key sessions.
    pub(super) message_codec: SessionMessageCodec,
    pub(super) execution_state: ExecutionState,
    pub(super) permission_handler: Arc<PermissionHandler>,
    pub(super) context_tokens: Arc<AtomicU32>,
    pub(super) compression_threshold: Arc<AtomicU32>,
    pub(super) agent_config: SessionAgentConfig,
    /// Mirror of `SessionConnection::executor` so the handle can route
    /// turns through [`AgentExecutor`] without re-locking the registry.
    pub(super) executor: Option<Arc<dyn AgentExecutor>>,
    /// Mirror of `SessionConnection::session_ref`.
    pub(super) session_ref: Option<SessionRef>,
}

impl SessionConnection {
    fn provider_for_api_format(api_format: &ApiFormat) -> &'static str {
        match api_format {
            ApiFormat::Anthropic => "anthropic",
            ApiFormat::OpenAI => "openai",
            ApiFormat::Gemini => "gemini",
        }
    }

    fn provider_for_vendor(vendor: &str) -> Option<&'static str> {
        match vendor {
            "claude" => Some("anthropic"),
            "codex" => Some("openai"),
            "gemini" => Some("gemini"),
            _ => None,
        }
    }

    fn normalize_reasoning_effort(reasoning_effort: Option<String>) -> Option<String> {
        reasoning_effort.and_then(|value| match value.trim() {
            "low" | "medium" | "high" | "xhigh" | "max" => Some(value.trim().to_string()),
            _ => None,
        })
    }

    fn default_context_window_for_vendor(vendor: &str) -> Option<u32> {
        match vendor {
            "claude" => Some(200_000),
            "codex" => Some(128_000),
            "gemini" => Some(1_000_000),
            _ => None,
        }
    }

    async fn is_vendor_native_model_id(vendor: &str, model_id: &str) -> bool {
        crate::commands::collect_vendor_models(vendor)
            .await
            .map(|models| models.into_iter().any(|model| model.id == model_id))
            .unwrap_or(false)
    }

    async fn build_model_spec(
        &self,
        profile_id: &str,
        reasoning_effort: Option<String>,
    ) -> Result<ModelSpec, String> {
        let normalized_reasoning_effort = Self::normalize_reasoning_effort(reasoning_effort);
        let vendor = self
            .session_ref
            .as_ref()
            .map(|session_ref| session_ref.vendor.as_ref())
            .unwrap_or_default();

        if let Some(provider) = Self::provider_for_vendor(vendor) {
            if Self::is_vendor_native_model_id(vendor, profile_id).await {
                log::info!(
                    "Session {} preserving vendor-native model '{}' for vendor={} during live model switch",
                    self.session_id,
                    profile_id,
                    vendor
                );
                return Ok(ModelSpec {
                    provider: provider.to_string(),
                    model_id: profile_id.to_string(),
                    reasoning_effort: normalized_reasoning_effort,
                    temperature: None,
                });
            }
        }

        let proxy_profiles = self.agent_config.proxy_profiles.read().await;
        let store = self.agent_config.profile_store.read().await;

        if let Some(profile) = store.get_profile_or_proxy(profile_id, &proxy_profiles) {
            return Ok(ModelSpec {
                provider: Self::provider_for_api_format(&profile.api_format).to_string(),
                model_id: profile.chat.model.clone(),
                reasoning_effort: normalized_reasoning_effort,
                temperature: Some(profile.chat.temperature),
            });
        }

        let provider = Self::provider_for_vendor(vendor)
            .ok_or_else(|| format!("Unknown profile/model selection: {}", profile_id))?;

        Ok(ModelSpec {
            provider: provider.to_string(),
            model_id: profile_id.to_string(),
            reasoning_effort: normalized_reasoning_effort,
            temperature: None,
        })
    }

    async fn persist_switched_profile(&self, new_profile_id: &str) {
        let mut pid = self.profile_id.write().await;
        log::info!(
            "Session {} switching profile: {} -> {}",
            self.session_id,
            *pid,
            new_profile_id
        );
        *pid = new_profile_id.to_string();
        drop(pid);

        if let Err(e) = upsert_agent_session_profile_id(
            &self.agent_config.db_path,
            &self.session_id,
            new_profile_id,
        ) {
            log::warn!(
                "Session {} failed to persist switched profile '{}': {}",
                self.session_id,
                new_profile_id,
                e
            );
        }

        let proxy_profiles = self.agent_config.proxy_profiles.read().await;
        let (model, context_window_tokens) = {
            let vendor = self
                .session_ref
                .as_ref()
                .map(|session_ref| session_ref.vendor.as_ref())
                .unwrap_or_default();
            if Self::provider_for_vendor(vendor).is_some() {
                if Self::is_vendor_native_model_id(vendor, new_profile_id).await {
                    (
                        new_profile_id.to_string(),
                        Self::default_context_window_for_vendor(vendor),
                    )
                } else {
                    let store = self.agent_config.profile_store.read().await;
                    if let Some(profile) =
                        store.get_profile_or_proxy(new_profile_id, &proxy_profiles)
                    {
                        (
                            profile.chat.model.clone(),
                            profile.chat.context_window_tokens,
                        )
                    } else {
                        (new_profile_id.to_string(), None)
                    }
                }
            } else {
                let store = self.agent_config.profile_store.read().await;
                if let Some(profile) = store.get_profile_or_proxy(new_profile_id, &proxy_profiles) {
                    (
                        profile.chat.model.clone(),
                        profile.chat.context_window_tokens,
                    )
                } else {
                    (new_profile_id.to_string(), None)
                }
            }
        };
        drop(proxy_profiles);

        let threshold = crate::chat_compression::CompressionService::for_model_with_context_window(
            &model,
            context_window_tokens,
        )
        .token_threshold();
        self.compression_threshold
            .store(threshold, Ordering::SeqCst);
        log::info!(
            "Session {} updated compression_threshold={} for model={}",
            self.session_id,
            threshold,
            model
        );
    }

    /// Build a detached message handle that can be used outside the
    /// `session_connections` map lock scope.
    pub fn message_handle(&self) -> SessionConnectionHandle {
        SessionConnectionHandle {
            session_id: self.session_id.clone(),
            socket: self.socket.clone(),
            message_codec: self.message_codec,
            execution_state: self.execution_state.clone(),
            permission_handler: self.permission_handler.clone(),
            context_tokens: self.context_tokens.clone(),
            compression_threshold: self.compression_threshold.clone(),
            agent_config: self.agent_config.clone(),
            executor: self.executor.clone(),
            session_ref: self.session_ref.clone(),
        }
    }

    /// Check if this connection is dead (heartbeat stopped due to consecutive failures).
    pub fn is_dead(&self) -> bool {
        !self.heartbeat_running.load(Ordering::SeqCst)
            || self.consecutive_failures.load(Ordering::SeqCst) >= MAX_HEARTBEAT_FAILURES
    }

    /// Get the session ID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Get the message queue for this session.
    pub fn queue(&self) -> &Arc<AgentMessageQueue> {
        &self.execution_state.queue
    }

    /// Get the current profile ID for this session.
    pub async fn get_profile_id(&self) -> String {
        self.profile_id.read().await.clone()
    }

    /// Switch the selected model for this session and apply it to the live
    /// executor when supported.
    pub async fn switch_profile(
        &self,
        new_profile_id: String,
        reasoning_effort: Option<String>,
    ) -> Result<ModelChangeOutcome, String> {
        let model = self
            .build_model_spec(&new_profile_id, reasoning_effort)
            .await?;
        let outcome = if let (Some(executor), Some(session_ref)) =
            (self.executor.as_ref(), self.session_ref.as_ref())
        {
            executor.set_model(session_ref, model).await.map_err(|e| {
                format!(
                    "Failed to apply model selection for session {}: {}",
                    self.session_id, e
                )
            })?
        } else {
            ModelChangeOutcome::Applied
        };

        self.persist_switched_profile(&new_profile_id).await;
        Ok(outcome)
    }

    /// Get the current permission mode of this session's permission handler.
    pub fn get_permission_mode(&self) -> crate::happy_client::permission::PermissionMode {
        self.permission_handler.get_mode()
    }

    /// Set the permission mode on this session's permission handler.
    pub fn set_permission_mode(&self, mode: crate::happy_client::permission::PermissionMode) {
        self.permission_handler.set_mode(mode);
    }

    /// Kill/archive this session: stop heartbeat, kill background runs, emit session-end, disconnect.
    pub async fn kill(&self) {
        self.heartbeat_running.store(false, Ordering::SeqCst);

        if let Ok(run_manager) = crate::local_services::run_manager() {
            let _ = run_manager.kill_by_session(&self.session_id).await;
        }

        match crate::local_services::rpc_registry() {
            Ok(registry) => unregister_session_local_rpcs(&registry, &self.session_id).await,
            Err(e) => log::warn!(
                "[Session] kill: failed to load RPC registry for {} cleanup: {}",
                self.session_id,
                e
            ),
        }

        // Release the process-global PermissionHandler registry entry now that
        // the session is truly closed. Plain disconnect() must NOT do this —
        // the handler has to survive a reconnect so pending permission
        // requests keep resolving against the same map.
        let _ = crate::happy_client::permission::remove_handler(&self.session_id);

        if let Err(e) = self.socket.emit_session_end(&self.session_id).await {
            log::warn!(
                "[Session] kill: failed to emit session-end for {}: {}",
                self.session_id,
                e
            );
        }

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        if let Err(e) = self.socket.disconnect().await {
            log::warn!(
                "[Session] kill: failed to disconnect {}: {}",
                self.session_id,
                e
            );
        }

        log::info!(
            "[Session] Session {} killed and cleaned up",
            self.session_id
        );
    }

    /// Disconnect and cleanup.
    pub async fn disconnect(&self) {
        self.heartbeat_running.store(false, Ordering::SeqCst);
        if let Err(e) = self.socket.disconnect().await {
            log::warn!(
                "Failed to disconnect session socket {}: {}",
                self.session_id,
                e
            );
        }
        log::info!("Session connection disconnected: {}", self.session_id);
    }

    /// Send a user-role message into this session, triggering agent processing.
    ///
    /// Used by `dispatch_task` to inject the task prompt as if the user sent it.
    pub async fn send_initial_user_message(&self, content: &str) -> Result<(), String> {
        self.message_handle()
            .send_initial_user_message(content)
            .await
    }

    /// Inject a user message directly from Tauri IPC (desktop local path).
    /// Bypasses Socket.IO and Happy Server entirely for real-time processing.
    /// The message is asynchronously synced to the server for mobile access/history.
    pub async fn inject_local_message(
        &self,
        text: String,
        images: Vec<serde_json::Value>,
        permission_mode: Option<String>,
        local_id: Option<String>,
        channel: tauri::ipc::Channel<crate::AgentStreamEvent>,
    ) -> Result<(), String> {
        self.message_handle()
            .inject_local_message(text, images, permission_mode, local_id, channel)
            .await
    }
}

impl Drop for SessionConnection {
    fn drop(&mut self) {
        self.heartbeat_running.store(false, Ordering::SeqCst);
    }
}
