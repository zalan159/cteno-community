use super::*;
use crate::happy_client::permission::{PermissionMode, PermissionRpcResponse};
use crate::session_message_codec::SessionMessageCodec;
use cteno_host_session_codec::EncryptionVariant;
use cteno_host_session_wire::{ConnectionType, UpdateEvent, UpdatePayload};

impl SessionConnection {
    /// Establish the remote transport, encryption, and RPC wiring for a
    /// session-scoped connection without starting the runtime loops.
    pub(super) async fn establish_remote_connection(
        server_url: &str,
        auth_token: &str,
        session_id: String,
        message_codec: SessionMessageCodec,
        agent_config: SessionAgentConfig,
        session_connections: SessionRegistry,
    ) -> Result<Self, String> {
        log::info!(
            "Connecting session-scoped Socket.IO for session: {}",
            session_id
        );

        const MAX_RETRIES: u32 = 3;
        const RETRY_DELAY_MS: u64 = 2000;

        let mut last_error = String::new();
        let socket = {
            let mut attempt = 0;
            loop {
                attempt += 1;
                log::info!(
                    "[Session {}] Connection attempt {}/{}",
                    session_id,
                    attempt,
                    MAX_RETRIES
                );

                match HappySocket::connect(
                    server_url,
                    auth_token,
                    ConnectionType::SessionScoped {
                        session_id: session_id.clone(),
                    },
                )
                .await
                {
                    Ok(socket) => {
                        log::info!(
                            "[Session {}] Connection successful on attempt {}",
                            session_id,
                            attempt
                        );
                        break socket;
                    }
                    Err(e) => {
                        last_error = e.clone();
                        log::warn!(
                            "[Session {}] Connection attempt {}/{} failed: {}",
                            session_id,
                            attempt,
                            MAX_RETRIES,
                            e
                        );

                        if attempt >= MAX_RETRIES {
                            log::error!(
                                "[Session {}] All {} connection attempts failed, giving up",
                                session_id,
                                MAX_RETRIES
                            );
                            return Err(format!(
                                "Failed to connect after {} attempts: {}",
                                MAX_RETRIES, last_error
                            ));
                        }

                        log::info!(
                            "[Session {}] Waiting {}ms before retry...",
                            session_id,
                            RETRY_DELAY_MS
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    }
                }
            }
        };

        let socket = Arc::new(socket);
        let heartbeat_running = Arc::new(AtomicBool::new(false));
        let consecutive_failures = Arc::new(AtomicU32::new(0));
        let context_tokens = Arc::new(AtomicU32::new(0));
        let compression_threshold = Arc::new(AtomicU32::new(64000));
        let execution_state = ExecutionState::new(Arc::new(AgentMessageQueue::new()));

        log::info!("[Session {}] Socket.IO connection ready", session_id);

        let agent_state_version =
            Self::fetch_agent_state_version(server_url, auth_token, &session_id)
                .await
                .unwrap_or(0);

        // Reuse the process-global session handler so pending permission
        // requests survive disconnect+reconnect races. See
        // `happy_client::permission::get_or_create_handler`.
        let permission_handler = crate::happy_client::permission::get_or_create_handler(
            &session_id,
            agent_state_version,
        );

        let perm_handler_for_rpc = permission_handler.clone();
        let execution_state_for_rpc = execution_state.clone();
        let perm_session_id = session_id.clone();
        let perm_message_codec = message_codec;
        let kill_heartbeat = heartbeat_running.clone();
        let kill_socket = socket.clone();
        let session_connections_for_self = session_connections.clone();
        let session_connections_for_rpc_closure = session_connections.clone();
        let kill_session_conns = session_connections;
        let skills_builtin_dir = agent_config.builtin_skills_dir.clone();
        let skills_user_dir = agent_config.user_skills_dir.clone();
        let mcp_session_ids = agent_config.session_mcp_server_ids.clone();
        let mcp_db_path = agent_config.db_path.clone();
        let mcp_workdir = super::restored_workdir_for_session(&mcp_db_path, &session_id)
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_else(|| PathBuf::from("."))
            });
        let rpc_server_url = agent_config.server_url.clone();
        let rpc_auth_token = agent_config.auth_token.clone();
        socket
            .on_rpc_request(move |method: String, encrypted_params: String| {
                let perm_session_id = perm_session_id.clone();
                let perm_message_codec = perm_message_codec;
                let perm_handler_for_rpc = perm_handler_for_rpc.clone();
                let execution_state_for_rpc = execution_state_for_rpc.clone();
                let kill_heartbeat = kill_heartbeat.clone();
                let kill_session_conns = kill_session_conns.clone();
                let kill_socket = kill_socket.clone();
                let skills_builtin_dir = skills_builtin_dir.clone();
                let skills_user_dir = skills_user_dir.clone();
                let mcp_session_ids = mcp_session_ids.clone();
                let mcp_db_path = mcp_db_path.clone();
                let mcp_workdir = mcp_workdir.clone();
                let rpc_server_url = rpc_server_url.clone();
                let rpc_auth_token = rpc_auth_token.clone();
                let session_connections_for_self = session_connections_for_rpc_closure.clone();
                async move {
                    let permission_method = format!("{}:permission", perm_session_id);
                    let set_mode_method = format!("{}:set-permission-mode", perm_session_id);
                    let set_sandbox_method = format!("{}:set-sandbox-policy", perm_session_id);
                    let abort_method = format!("{}:abort", perm_session_id);
                    let kill_method = format!("{}:killSession", perm_session_id);
                    let send_to_bg_method = format!("{}:send-to-background", perm_session_id);
                    let get_mcp_method = format!("{}:get-session-mcp-servers", perm_session_id);
                    let set_mcp_method = format!("{}:set-session-mcp-servers", perm_session_id);

                    let decode_params = || -> Result<Value, String> {
                        // Zero-key sessions use the plaintext codec; the codec
                        // falls back to raw JSON even through this legacy path.
                        perm_message_codec.decode_payload("encrypted", &encrypted_params)
                    };

                    let encrypt_ack = |ack: Value| -> String {
                        let ack_json = serde_json::to_string(&ack).unwrap_or_default();
                        match perm_message_codec.encode_payload(ack_json.as_bytes()) {
                            Ok(encoded) => encoded,
                            Err(e) => {
                                log::error!("[Session RPC] Failed to encrypt ack: {}", e);
                                String::new()
                            }
                        }
                    };

                    if method == permission_method {
                        log::info!(
                            "[Permission] Received permission RPC for session: {}",
                            perm_session_id
                        );

                        let params = match decode_params() {
                            Ok(p) => p,
                            Err(e) => {
                                log::error!("[Permission] {}", e);
                                return String::new();
                            }
                        };

                        log::info!("[Permission] Decrypted permission RPC params: {:?}", params);

                        let response: PermissionRpcResponse = match serde_json::from_value(params) {
                            Ok(r) => r,
                            Err(e) => {
                                log::error!(
                                    "[Permission] Failed to parse PermissionRpcResponse: {}",
                                    e
                                );
                                return String::new();
                            }
                        };

                        perm_handler_for_rpc.handle_rpc_response(response);
                        encrypt_ack(serde_json::json!({"status": "ok"}))
                    } else if method == set_mode_method {
                        log::info!(
                            "[Permission] Received set-permission-mode RPC for session: {}",
                            perm_session_id
                        );

                        let params = match decode_params() {
                            Ok(p) => p,
                            Err(e) => {
                                log::error!("[Permission] set-permission-mode: {}", e);
                                return String::new();
                            }
                        };

                        log::info!("[Permission] set-permission-mode params: {:?}", params);

                        if let Some(mode_str) = params.get("mode").and_then(|v| v.as_str()) {
                            if let Some((exec_mode, host_mode)) =
                                crate::happy_client::permission::parse_runtime_permission_mode(
                                    mode_str,
                                )
                            {
                                // Forward the mode change to the vendor
                                // subprocess (Claude `/permission …`, Codex
                                // stdin frame, …) so runtime toggles take
                                // effect mid-turn. The executor/session_ref
                                // are installed onto `SessionConnection`
                                // after `establish_remote_connection`
                                // returns, so we resolve them lazily via
                                // the session registry.
                                let Some(conn) =
                                    session_connections_for_self.get(&perm_session_id).await
                                else {
                                    return encrypt_ack(serde_json::json!({
                                        "status": "error",
                                        "message": format!("Session {} has no live connection", perm_session_id),
                                    }));
                                };
                                let handle = conn.message_handle();
                                let (Some(executor), Some(session_ref)) =
                                    (handle.executor.clone(), handle.session_ref.clone())
                                else {
                                    return encrypt_ack(serde_json::json!({
                                        "status": "error",
                                        "message": format!("Session {} cannot update permission mode at runtime", perm_session_id),
                                    }));
                                };

                                if let Err(e) = executor
                                    .set_permission_mode(&session_ref, exec_mode)
                                    .await
                                {
                                    return encrypt_ack(serde_json::json!({
                                        "status": "error",
                                        "message": format!("Failed to update permission mode for {}: {}", perm_session_id, e),
                                    }));
                                }

                                if let Some(mode) = host_mode {
                                    perm_handler_for_rpc.set_mode(mode);
                                }

                                let kv_server_url = rpc_server_url.clone();
                                let kv_auth_token = rpc_auth_token.clone();
                                let kv_session_id = perm_session_id.clone();
                                if let Some(mode) = host_mode {
                                    if let Err(e) = persist_session_permission_mode_to_kv(
                                        &kv_server_url,
                                        &kv_auth_token,
                                        &kv_session_id,
                                        mode,
                                    )
                                    .await
                                    {
                                        log::warn!(
                                            "[Permission] Failed to persist mode to KV: {}",
                                            e
                                        );
                                    }
                                }

                                encrypt_ack(serde_json::json!({"status": "ok"}))
                            } else {
                                log::warn!("[Permission] Unknown mode: {}", mode_str);
                                encrypt_ack(
                                    serde_json::json!({"status": "error", "message": "unknown mode"}),
                                )
                            }
                        } else {
                            log::warn!("[Permission] Missing 'mode' field in set-permission-mode");
                            encrypt_ack(
                                serde_json::json!({"status": "error", "message": "missing mode"}),
                            )
                        }
                    } else if method == set_sandbox_method {
                        log::info!(
                            "[Sandbox] Received set-sandbox-policy RPC for session: {}",
                            perm_session_id
                        );
                        let params = match decode_params() {
                            Ok(p) => p,
                            Err(e) => {
                                log::error!("[Sandbox] set-sandbox-policy: {}", e);
                                return String::new();
                            }
                        };
                        if let Some(policy_str) = params.get("policy").and_then(|v| v.as_str()) {
                            let policy = match policy_str {
                                "workspace_write" => {
                                    Some(crate::tool_executors::SandboxPolicy::default())
                                }
                                "unrestricted" => {
                                    Some(crate::tool_executors::SandboxPolicy::Unrestricted)
                                }
                                "read_only" => {
                                    Some(crate::tool_executors::SandboxPolicy::ReadOnly)
                                }
                                _ => None,
                            };
                            if let Some(_policy) = policy {
                                log::info!("[Sandbox] Set sandbox policy to: {}", policy_str);
                                encrypt_ack(serde_json::json!({"status": "ok"}))
                            } else {
                                log::warn!("[Sandbox] Unknown policy: {}", policy_str);
                                encrypt_ack(serde_json::json!({"status": "error", "message": "unknown policy"}))
                            }
                        } else {
                            log::warn!("[Sandbox] Missing 'policy' field");
                            encrypt_ack(serde_json::json!({"status": "error", "message": "missing policy"}))
                        }
                    } else if method == kill_method {
                        log::info!(
                            "[Session] Received killSession for session: {}",
                            perm_session_id
                        );

                        kill_heartbeat.store(false, Ordering::SeqCst);

                        let cleanup_socket = kill_socket.clone();
                        let cleanup_sid = perm_session_id.clone();
                        let cleanup_conns = kill_session_conns.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                            if let Ok(run_manager) = crate::local_services::run_manager() {
                                let _ = run_manager.kill_by_session(&cleanup_sid).await;
                            }

                            if let Ok(bm) = crate::local_services::browser_manager() {
                                bm.close_session(&cleanup_sid).await;
                            }

                            match crate::local_services::scheduler() {
                                Ok(scheduler) => match scheduler.delete_tasks_by_session(&cleanup_sid)
                                {
                                    Ok(count) if count > 0 => {
                                        log::info!(
                                            "[Session] Deleted {} scheduled tasks for session {}",
                                            count,
                                            cleanup_sid
                                        );
                                    }
                                    Ok(_) => {}
                                    Err(e) => {
                                        log::warn!(
                                            "[Session] Failed to delete scheduled tasks for session {}: {}",
                                            cleanup_sid,
                                            e
                                        );
                                    }
                                },
                                Err(e) => log::warn!(
                                    "[Session] Scheduler service unavailable for {}: {}",
                                    cleanup_sid,
                                    e
                                ),
                            }

                            crate::subagent::manager::global()
                                .unregister_session(&cleanup_sid)
                                .await;

                            if let Err(e) = cleanup_socket.emit_session_end(&cleanup_sid).await {
                                log::warn!(
                                    "[Session] Failed to emit session-end for {}: {}",
                                    cleanup_sid,
                                    e
                                );
                            } else {
                                log::info!("[Session] Emitted session-end for {}", cleanup_sid);
                            }

                            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

                            if let Err(e) = cleanup_socket.disconnect().await {
                                log::warn!(
                                    "[Session] Failed to disconnect session {}: {}",
                                    cleanup_sid,
                                    e
                                );
                            }

                            if let Ok(persona_manager) = crate::local_services::persona_manager() {
                                persona_manager.on_task_complete(&cleanup_sid).await;
                            }

                            if cleanup_conns.remove(&cleanup_sid).await.is_some() {
                                log::info!(
                                    "[Session] Removed session {} from active connections",
                                    cleanup_sid
                                );
                            }

                            log::info!("[Session] Session {} archived and cleaned up", cleanup_sid);
                        });

                        encrypt_ack(json!({"success": true, "message": "Session archived"}))
                    } else if method == abort_method {
                        log::info!(
                            "[Abort] Received abort RPC for session: {}",
                            perm_session_id
                        );
                        execution_state_for_rpc.request_abort();
                        log::info!(
                            "[Abort] Abort flag set to true for session: {}",
                            perm_session_id
                        );
                        encrypt_ack(serde_json::json!({"status": "ok"}))
                    } else if method == send_to_bg_method {
                        log::info!(
                            "[SendToBackground] Received send-to-background RPC for session: {}",
                            perm_session_id
                        );

                        let params = match decode_params() {
                            Ok(p) => p,
                            Err(e) => {
                                log::error!("[SendToBackground] {}", e);
                                return encrypt_ack(
                                    serde_json::json!({"status": "error", "message": e}),
                                );
                            }
                        };

                        let call_id = params
                            .get("callId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        if call_id.is_empty() {
                            return encrypt_ack(
                                serde_json::json!({"status": "error", "message": "missing callId"}),
                            );
                        }

                        let triggered = {
                            match crate::local_services::run_manager() {
                                Ok(rm) => rm.trigger_background_signal(call_id).await,
                                Err(e) => {
                                    log::error!("[SendToBackground] RunManager unavailable: {}", e);
                                    false
                                }
                            }
                        };

                        if triggered {
                            log::info!(
                                "[SendToBackground] Triggered background for callId={} in session {}",
                                call_id,
                                perm_session_id
                            );
                            encrypt_ack(serde_json::json!({"status": "ok"}))
                        } else {
                            log::warn!(
                                "[SendToBackground] No pending sync execution for callId={} in session {}",
                                call_id,
                                perm_session_id
                            );
                            encrypt_ack(serde_json::json!({
                                "status": "error",
                                "message": "no pending sync execution for this callId"
                            }))
                        }
                    } else if method == get_mcp_method {
                        log::info!(
                            "[MCP] Received get-session-mcp-servers for session: {}",
                            perm_session_id
                        );

                        let result = {
                            let all_servers =
                                super::list_scoped_mcp_servers_for_workdir(&mcp_db_path, &mcp_workdir)
                                    .await;
                            let active_ids = mcp_session_ids.read().await.clone();

                            serde_json::json!({
                                "allServers": all_servers,
                                "activeServerIds": active_ids
                            })
                        };

                        encrypt_ack(result)
                    } else if method == set_mcp_method {
                        log::info!(
                            "[MCP] Received set-session-mcp-servers for session: {}",
                            perm_session_id
                        );

                        let params = match decode_params() {
                            Ok(p) => p,
                            Err(e) => {
                                log::error!("[MCP] set-session-mcp-servers decrypt failed: {}", e);
                                return encrypt_ack(serde_json::json!({"success": false, "error": e}));
                            }
                        };

                        let server_ids: Vec<String> = params
                            .get("serverIds")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();

                        log::info!("[MCP] Setting session MCP servers: {:?}", server_ids);

                        {
                            let mut ids = mcp_session_ids.write().await;
                            *ids = server_ids;
                        };

                        encrypt_ack(serde_json::json!({"success": true}))
                    } else {
                        log::debug!("[Session RPC] Ignoring unknown method: {}", method);
                        String::new()
                    }
                }
            })
            .await;

        log::info!(
            "[Session {}] Registering RPC methods after ready connect...",
            session_id
        );

        let perm_rpc_method = format!("{}:permission", session_id);
        let mode_rpc_method = format!("{}:set-permission-mode", session_id);
        let sandbox_rpc_method = format!("{}:set-sandbox-policy", session_id);
        let abort_rpc_method = format!("{}:abort", session_id);
        let send_to_bg_rpc_method = format!("{}:send-to-background", session_id);
        let kill_rpc_method = format!("{}:killSession", session_id);
        if let Err(e) = socket.register_rpc_method(&perm_rpc_method).await {
            log::warn!(
                "[Permission] Failed to register permission RPC method: {}",
                e
            );
        } else {
            log::info!("[Permission] Registered RPC method: {}", perm_rpc_method);
        }
        if let Err(e) = socket.register_rpc_method(&mode_rpc_method).await {
            log::warn!(
                "[Permission] Failed to register set-permission-mode RPC method: {}",
                e
            );
        } else {
            log::info!("[Permission] Registered RPC method: {}", mode_rpc_method);
        }
        if let Err(e) = socket.register_rpc_method(&sandbox_rpc_method).await {
            log::warn!(
                "[Sandbox] Failed to register set-sandbox-policy RPC method: {}",
                e
            );
        } else {
            log::info!("[Sandbox] Registered RPC method: {}", sandbox_rpc_method);
        }
        if let Err(e) = socket.register_rpc_method(&abort_rpc_method).await {
            log::warn!("[Abort] Failed to register abort RPC method: {}", e);
        } else {
            log::info!("[Abort] Registered RPC method: {}", abort_rpc_method);
        }
        if let Err(e) = socket.register_rpc_method(&send_to_bg_rpc_method).await {
            log::warn!(
                "[SendToBackground] Failed to register send-to-background RPC method: {}",
                e
            );
        } else {
            log::info!(
                "[SendToBackground] Registered RPC method: {}",
                send_to_bg_rpc_method
            );
        }
        if let Err(e) = socket.register_rpc_method(&kill_rpc_method).await {
            log::warn!("[Session] Failed to register killSession RPC method: {}", e);
        } else {
            log::info!("[Session] Registered RPC method: {}", kill_rpc_method);
        }
        let get_skills_rpc_method = format!("{}:get-session-skills", session_id);
        let set_skills_rpc_method = format!("{}:set-session-skills", session_id);
        if let Err(e) = socket.register_rpc_method(&get_skills_rpc_method).await {
            log::warn!(
                "[Skills] Failed to register get-session-skills RPC method: {}",
                e
            );
        } else {
            log::info!("[Skills] Registered RPC method: {}", get_skills_rpc_method);
        }
        if let Err(e) = socket.register_rpc_method(&set_skills_rpc_method).await {
            log::warn!(
                "[Skills] Failed to register set-session-skills RPC method: {}",
                e
            );
        } else {
            log::info!("[Skills] Registered RPC method: {}", set_skills_rpc_method);
        }
        let get_mcp_rpc_method = format!("{}:get-session-mcp-servers", session_id);
        let set_mcp_rpc_method = format!("{}:set-session-mcp-servers", session_id);
        if let Err(e) = socket.register_rpc_method(&get_mcp_rpc_method).await {
            log::warn!(
                "[MCP] Failed to register get-session-mcp-servers RPC: {}",
                e
            );
        } else {
            log::info!("[MCP] Registered RPC method: {}", get_mcp_rpc_method);
        }
        if let Err(e) = socket.register_rpc_method(&set_mcp_rpc_method).await {
            log::warn!(
                "[MCP] Failed to register set-session-mcp-servers RPC: {}",
                e
            );
        } else {
            log::info!("[MCP] Registered RPC method: {}", set_mcp_rpc_method);
        }

        let profile_id_ref = agent_config.profile_id.clone();

        let local_origin = Arc::new(AtomicBool::new(false));
        let conn = Self {
            session_id: session_id.clone(),
            socket: socket.clone(),
            message_codec,
            heartbeat_running: heartbeat_running.clone(),
            execution_state: execution_state.clone(),
            consecutive_failures: consecutive_failures.clone(),
            permission_handler,
            profile_id: profile_id_ref,
            context_tokens: context_tokens.clone(),
            compression_threshold: compression_threshold.clone(),
            agent_config: agent_config.clone(),
            local_origin: local_origin.clone(),
            session_connections: session_connections_for_self,
            // T11 scaffold: executor path is opt-in, left unwired here so the
            // legacy in-process path remains the default while downstream
            // commits add real wiring (spawn_session_internal / recovery).
            executor: None,
            session_ref: None,
        };

        Ok(conn)
    }

    /// Start the runtime tasks that drive message listening and heartbeat
    /// updates once the connection has been fully constructed.
    pub(super) async fn start_remote_runtime(&self, agent_config: SessionAgentConfig) {
        self.start_listening(agent_config).await;
        self.start_heartbeat().await;
    }

    /// Establish a session-scoped Socket.IO connection and start listening.
    pub async fn connect_and_start(
        server_url: &str,
        auth_token: &str,
        session_id: String,
        encryption_key: [u8; 32],
        encryption_variant: EncryptionVariant,
        agent_config: SessionAgentConfig,
        session_connections: SessionRegistry,
    ) -> Result<Self, String> {
        let message_codec =
            SessionMessageCodec::for_session_messages(encryption_key, encryption_variant);
        let conn = Self::establish_remote_connection(
            server_url,
            auth_token,
            session_id.clone(),
            message_codec,
            agent_config.clone(),
            session_connections,
        )
        .await?;

        conn.start_remote_runtime(agent_config).await;

        log::info!(
            "Session-scoped connection established for session: {}",
            session_id
        );

        Ok(conn)
    }

    /// Register the `on_update` callback to process incoming messages.
    async fn start_listening(&self, agent_config: SessionAgentConfig) {
        let session_id = self.session_id.clone();
        let message_codec = self.message_codec;
        let socket = self.socket.clone();
        let heartbeat_running = self.heartbeat_running.clone();
        let execution_state = self.execution_state.clone();
        let permission_handler = self.permission_handler.clone();
        let context_tokens = self.context_tokens.clone();
        let compression_threshold = self.compression_threshold.clone();
        let executor = self.executor.clone();
        let session_ref = self.session_ref.clone();

        let last_seen_msg_id: Arc<std::sync::Mutex<Option<String>>> =
            Arc::new(std::sync::Mutex::new(None));

        {
            let sid = session_id.clone();
            let poll_execution_state = execution_state.clone();
            let socket_for_response = socket.clone();
            let perm_handler = permission_handler.clone();
            let context_tokens_for_agent = context_tokens.clone();
            let compression_threshold_for_agent = compression_threshold.clone();
            let catchup_message_codec = message_codec;
            let catchup_server_url = agent_config.server_url.clone();
            let catchup_auth_token = agent_config.auth_token.clone();
            let config = agent_config.clone();
            let running = heartbeat_running.clone();
            let last_seen_for_poll = last_seen_msg_id.clone();
            let queued_executor = executor.clone();
            let queued_session_ref = session_ref.clone();

            tokio::spawn(async move {
                let mut poll_iteration: u64 = 0;
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    poll_iteration += 1;

                    if !running.load(Ordering::SeqCst) {
                        break;
                    }

                    let list = match crate::local_services::run_manager() {
                        Ok(run_manager) => run_manager.pop_notifications(&sid).await,
                        Err(e) => {
                            log::warn!(
                                "[Session {}] Failed to get run_manager for notifications: {}",
                                sid,
                                e
                            );
                            Vec::new()
                        }
                    };

                    if !list.is_empty() {
                        log::info!(
                            "[Session {}] Received {} background run notification(s)",
                            sid,
                            list.len()
                        );
                        for item in &list {
                            let msg = item.message.trim().to_string();
                            if msg.is_empty() {
                                continue;
                            }

                            if msg.starts_with("[上传完成]") {
                                let response_text = msg
                                    .lines()
                                    .filter(|l| l.starts_with('✅'))
                                    .next()
                                    .unwrap_or(&msg)
                                    .to_string();
                                log::info!(
                                    "[Session {}] Sending upload result directly: {}",
                                    sid,
                                    response_text
                                );
                                if let Err(e) = send_agent_response(
                                    &socket_for_response,
                                    &sid,
                                    &response_text,
                                    &message_codec,
                                    false,
                                )
                                .await
                                {
                                    log::error!(
                                        "[Session {}] Failed to send upload result: {}",
                                        sid,
                                        e
                                    );
                                }
                                continue;
                            }

                            log::info!(
                                "[Session {}] Queuing background notification: run_id={}, msg_len={}",
                                sid,
                                item.run_id,
                                msg.len()
                            );
                            let _ = poll_execution_state
                                .queue
                                .push(AgentMessage::system(sid.clone(), msg));
                        }
                    }

                    if poll_iteration % 15 == 0 && !poll_execution_state.queue.is_processing(&sid) {
                        let mut last_id = last_seen_for_poll.lock().unwrap().clone();
                        if let Some(msg) = check_for_missed_messages(
                            &sid,
                            &catchup_server_url,
                            &catchup_auth_token,
                            &catchup_message_codec,
                            &mut last_id,
                        )
                        .await
                        {
                            log::info!(
                                "[Session {}] Periodic catch-up found missed user message, enqueuing",
                                sid
                            );
                            let _ = poll_execution_state.queue.push(msg);
                        }
                        *last_seen_for_poll.lock().unwrap() = last_id;
                    }

                    if !poll_execution_state.queue.is_empty(&sid) {
                        let _ = worker::spawn_background_queue_worker_if_idle(
                            worker::BackgroundWorkerState {
                                session_id: sid.clone(),
                                execution_state: poll_execution_state.clone(),
                                config: config.clone(),
                                socket_for_response: socket_for_response.clone(),
                                message_codec,
                                perm_handler: perm_handler.clone(),
                                context_tokens: context_tokens_for_agent.clone(),
                                compression_threshold: compression_threshold_for_agent.clone(),
                                executor: queued_executor.clone(),
                                session_ref: queued_session_ref.clone(),
                            },
                            worker::BackgroundWorkerOptions {
                                worker_label: "queued-message",
                                auto_hibernate: true,
                                auto_rename_persona: true,
                            },
                        );
                    }
                }
            });
        }

        {
            let sid = self.session_id.clone();
            let sa_execution_state = self.execution_state.clone();
            let sa_socket = self.socket.clone();
            let sa_perm = self.permission_handler.clone();
            let sa_ctx_tokens = self.context_tokens.clone();
            let sa_comp_thresh = self.compression_threshold.clone();
            let sa_config = agent_config.clone();
            let sa_running = self.heartbeat_running.clone();
            let sa_executor = executor.clone();
            let sa_session_ref = session_ref.clone();

            let mut rx = crate::subagent::manager::global()
                .register_session(sid.clone())
                .await;

            tokio::spawn(async move {
                log::info!("[Session {}] SubAgent notification receiver started", sid);

                while let Some(message) = rx.recv().await {
                    if !sa_running.load(Ordering::SeqCst) {
                        break;
                    }

                    log::info!(
                        "[Session {}] Received SubAgent notification ({} chars)",
                        sid,
                        message.len()
                    );

                    let _ = sa_execution_state
                        .queue
                        .push(AgentMessage::user(sid.clone(), message));

                    let _ = worker::spawn_background_queue_worker_if_idle(
                        worker::BackgroundWorkerState {
                            session_id: sid.clone(),
                            execution_state: sa_execution_state.clone(),
                            config: sa_config.clone(),
                            socket_for_response: sa_socket.clone(),
                            message_codec,
                            perm_handler: sa_perm.clone(),
                            context_tokens: sa_ctx_tokens.clone(),
                            compression_threshold: sa_comp_thresh.clone(),
                            executor: sa_executor.clone(),
                            session_ref: sa_session_ref.clone(),
                        },
                        worker::BackgroundWorkerOptions {
                            worker_label: "subagent-notification",
                            auto_hibernate: true,
                            auto_rename_persona: false,
                        },
                    );
                }
                log::info!("[Session {}] SubAgent notification receiver stopped", sid);
            });
        }

        self.socket
            .on_update(move |update: UpdatePayload| {
                log::info!("Session {} received update: {:?}", session_id, update.body);

                match update.body {
                    UpdateEvent::NewMessage(ref msg) => {
                        if msg.sid != session_id {
                            log::debug!("Ignoring message for different session: {}", msg.sid);
                            return;
                        }

                        log::info!(
                            "Session {} received new-message, content_type={}",
                            session_id,
                            msg.message.content.t
                        );

                        let message_json: Value = match decode_session_payload(
                            &msg.message.content.t,
                            &msg.message.content.c,
                            &message_codec,
                        ) {
                            Ok(json) => json,
                            Err(e) => {
                                log::error!("Failed to decode message for session {}: {}", session_id, e);
                                return;
                            }
                        };

                        log::info!("Session {} decrypted message: {:?}", session_id, message_json);

                        let role =
                            message_json.get("role").and_then(|v| v.as_str()).unwrap_or("");
                        if role != "user" {
                            log::debug!("Ignoring non-user message (role={})", role);
                            return;
                        }

                        let content = message_json.get("content");
                        let mut user_text = String::new();
                        let mut user_images: Vec<serde_json::Value> = Vec::new();

                        if let Some(content_val) = content {
                            if let Some(arr) = content_val.as_array() {
                                for block in arr {
                                    match block.get("type").and_then(|t| t.as_str()) {
                                        Some("text") => {
                                            if let Some(t) =
                                                block.get("text").and_then(|t| t.as_str())
                                            {
                                                if !user_text.is_empty() {
                                                    user_text.push('\n');
                                                }
                                                user_text.push_str(t);
                                            }
                                        }
                                        Some("image") => {
                                            user_images.push(block.clone());
                                        }
                                        _ => {}
                                    }
                                }
                            } else {
                                user_text = content_val
                                    .get("text")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("")
                                    .to_string();
                            }
                        }

                        if user_text.is_empty() && user_images.is_empty() {
                            log::warn!("Empty user message, ignoring");
                            return;
                        }

                        log::info!(
                            "Session {} user message: {} (images: {})",
                            session_id,
                            user_text,
                            user_images.len()
                        );

                        if let Some(mode_str) = message_json
                            .get("meta")
                            .and_then(|m| m.get("permissionMode"))
                            .and_then(|v| v.as_str())
                        {
                            if let Some(mode) = PermissionHandler::parse_mode(mode_str) {
                                let current = permission_handler.get_mode();
                                if current == PermissionMode::BypassPermissions
                                    && mode != PermissionMode::BypassPermissions
                                {
                                    log::info!(
                                        "[Permission] Ignoring message meta downgrade from BypassPermissions to {:?} for session {}",
                                        mode,
                                        session_id
                                    );
                                } else {
                                    permission_handler.set_mode(mode);
                                    let kv_url = agent_config.server_url.clone();
                                    let kv_token = agent_config.auth_token.clone();
                                    let kv_sid = session_id.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = persist_session_permission_mode_to_kv(
                                            &kv_url, &kv_token, &kv_sid, mode,
                                        )
                                        .await
                                        {
                                            log::warn!(
                                                "[Permission] Failed to persist mode to KV: {}",
                                                e
                                            );
                                        }
                                    });
                                }
                            }
                        }

                        if let Ok(mut last_id) = last_seen_msg_id.lock() {
                            *last_id = Some(msg.message.id.clone());
                        }

                        let sid = session_id.clone();
                        let agent_msg = if user_images.is_empty() {
                            AgentMessage::user(sid.clone(), user_text.clone())
                        } else {
                            AgentMessage::user_with_images(sid.clone(), user_text.clone(), user_images)
                        };
                        if let Err(e) = execution_state.queue.push(agent_msg) {
                            log::error!(
                                "Failed to push message to queue for session {}: {}",
                                sid,
                                e
                            );
                            return;
                        }
                        log::info!(
                            "[Agent] Message queued for session {}, queue_len={}",
                            sid,
                            execution_state.queue.len(&sid)
                        );

                        let started = worker::spawn_background_queue_worker_if_idle(
                            worker::BackgroundWorkerState {
                                session_id: sid.clone(),
                                execution_state: execution_state.clone(),
                                config: agent_config.clone(),
                                socket_for_response: socket.clone(),
                                message_codec,
                                perm_handler: permission_handler.clone(),
                                context_tokens: context_tokens.clone(),
                                compression_threshold: compression_threshold.clone(),
                                executor: executor.clone(),
                                session_ref: session_ref.clone(),
                            },
                            worker::BackgroundWorkerOptions {
                                worker_label: "socket-update",
                                auto_hibernate: true,
                                auto_rename_persona: true,
                            },
                        );

                        if !started {
                            log::info!(
                                "[Agent] Session {} already processing, message queued for worker",
                                sid
                            );
                        }
                    }
                    UpdateEvent::UpdateSession(ref _update) => {
                        log::debug!("Session {} received session update", session_id);
                    }
                    _ => {
                        log::debug!("Session {} received unhandled update type", session_id);
                    }
                }
            })
            .await;
    }

    /// Start periodic session keep-alive (every 2s, matching happy-cli).
    pub(super) async fn start_heartbeat(&self) {
        let was_running = self.heartbeat_running.swap(true, Ordering::SeqCst);
        if was_running {
            log::warn!(
                "Session keep-alive already running for: {}",
                self.session_id
            );
            return;
        }

        let socket = self.socket.clone();
        let session_id = self.session_id.clone();
        let running = self.heartbeat_running.clone();
        let thinking = self.execution_state.thinking.clone();
        let failures = self.consecutive_failures.clone();
        let context_tokens = self.context_tokens.clone();
        let compression_threshold = self.compression_threshold.clone();

        tokio::spawn(async move {
            log::info!("Session keep-alive started for: {} (every 2s)", session_id);

            loop {
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                let thinking_val = thinking.load(Ordering::SeqCst);
                let is_thinking = thinking_val > 0;
                let thinking_status = match thinking_val {
                    2 => Some("compressing"),
                    _ => None,
                };
                let ctx_tokens = context_tokens.load(Ordering::SeqCst);
                let comp_threshold = compression_threshold.load(Ordering::SeqCst);
                match socket
                    .session_alive(
                        &session_id,
                        Some(is_thinking),
                        thinking_status,
                        ctx_tokens,
                        comp_threshold,
                    )
                    .await
                {
                    Ok(_) => {
                        failures.store(0, Ordering::SeqCst);
                    }
                    Err(e) => {
                        let count = failures.fetch_add(1, Ordering::SeqCst) + 1;
                        log::warn!(
                            "Session keep-alive failed for {} ({}/{}): {}",
                            session_id,
                            count,
                            MAX_HEARTBEAT_FAILURES,
                            e
                        );
                        if count >= MAX_HEARTBEAT_FAILURES {
                            log::error!(
                                "Session {} heartbeat dead after {} consecutive failures, stopping",
                                session_id,
                                count
                            );
                            running.store(false, Ordering::SeqCst);
                            break;
                        }
                    }
                }

                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }

            log::info!("Session keep-alive stopped for: {}", session_id);
        });
    }

    /// After reconnecting a session, check for messages that arrived while the
    /// session-scoped connection was down.
    pub async fn catch_up_missed_messages(&self, server_url: &str, auth_token: &str) {
        let session_id = &self.session_id;
        log::info!(
            "[Session {}] Checking for missed messages after reconnect...",
            session_id
        );

        let url = format!("{}/v1/sessions/{}/messages?limit=5", server_url, session_id);
        let client = reqwest::Client::new();
        let response = match client
            .get(&url)
            .header("Authorization", format!("Bearer {}", auth_token))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                log::warn!(
                    "[Session {}] Failed to fetch messages for catch-up: {}",
                    session_id,
                    e
                );
                return;
            }
        };

        if !response.status().is_success() {
            log::warn!(
                "[Session {}] Messages API returned {}, skipping catch-up",
                session_id,
                response.status()
            );
            return;
        }

        let body: Value = match response.json().await {
            Ok(v) => v,
            Err(e) => {
                log::warn!(
                    "[Session {}] Failed to parse messages response: {}",
                    session_id,
                    e
                );
                return;
            }
        };

        let messages = match body.get("messages").and_then(|v| v.as_array()) {
            Some(arr) => arr.clone(),
            None => {
                log::info!(
                    "[Session {}] No messages found, nothing to catch up",
                    session_id
                );
                return;
            }
        };

        if messages.is_empty() {
            log::info!("[Session {}] No messages, nothing to catch up", session_id);
            return;
        }

        let newest = &messages[0];

        let content = match newest.get("content") {
            Some(c) => c,
            None => {
                log::warn!("[Session {}] Newest message has no content", session_id);
                return;
            }
        };

        let content_type = match content.get("t").and_then(|v| v.as_str()) {
            Some(value) => value,
            None => {
                log::warn!(
                    "[Session {}] Newest message has no content type",
                    session_id
                );
                return;
            }
        };

        let payload = match content.get("c") {
            Some(value) => value,
            None => {
                log::warn!("[Session {}] Message content has no payload", session_id);
                return;
            }
        };

        let message_json: Value =
            match decode_session_payload(content_type, payload, &self.message_codec) {
                Ok(json) => json,
                Err(e) => {
                    log::warn!(
                        "[Session {}] Failed to decode {} message: {}",
                        session_id,
                        content_type,
                        e
                    );
                    return;
                }
            };

        let role = message_json
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if role != "user" {
            log::info!(
                "[Session {}] Last message is role='{}', no catch-up needed",
                session_id,
                role
            );
            return;
        }

        let content_val = message_json.get("content");
        let mut user_text = String::new();
        let mut user_images: Vec<Value> = Vec::new();

        if let Some(cv) = content_val {
            if let Some(arr) = cv.as_array() {
                for block in arr {
                    match block.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                if !user_text.is_empty() {
                                    user_text.push('\n');
                                }
                                user_text.push_str(t);
                            }
                        }
                        Some("image") => {
                            user_images.push(block.clone());
                        }
                        _ => {}
                    }
                }
            } else {
                user_text = cv
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
            }
        }

        if user_text.is_empty() && user_images.is_empty() {
            log::info!(
                "[Session {}] Last user message is empty, skipping catch-up",
                session_id
            );
            return;
        }

        log::info!(
            "[Session {}] Found missed user message: '{}' (images: {}), enqueuing for processing",
            session_id,
            if user_text.len() > 100 {
                user_text
                    .char_indices()
                    .nth(100)
                    .map_or(user_text.as_str(), |(i, _)| &user_text[..i])
            } else {
                &user_text
            },
            user_images.len()
        );

        if let Some(mode_str) = message_json
            .get("meta")
            .and_then(|m| m.get("permissionMode"))
            .and_then(|v| v.as_str())
        {
            if let Some(mode) = PermissionHandler::parse_mode(mode_str) {
                let current = self.permission_handler.get_mode();
                if current == PermissionMode::BypassPermissions
                    && mode != PermissionMode::BypassPermissions
                {
                    log::info!(
                        "[Permission] Ignoring missed-message meta downgrade from BypassPermissions to {:?} for session {}",
                        mode,
                        session_id
                    );
                } else {
                    self.permission_handler.set_mode(mode);
                }
            }
        }

        let sid = session_id.clone();
        let agent_msg = if user_images.is_empty() {
            AgentMessage::user(sid.clone(), user_text)
        } else {
            AgentMessage::user_with_images(sid.clone(), user_text, user_images)
        };

        if let Err(e) = self.execution_state.queue.push(agent_msg) {
            log::error!(
                "[Session {}] Failed to push missed message to queue: {}",
                session_id,
                e
            );
            return;
        }

        let started = worker::spawn_background_queue_worker_if_idle(
            worker::BackgroundWorkerState {
                session_id: sid.clone(),
                execution_state: self.execution_state.clone(),
                config: self.agent_config.clone(),
                socket_for_response: self.socket.clone(),
                message_codec: self.message_codec,
                perm_handler: self.permission_handler.clone(),
                context_tokens: self.context_tokens.clone(),
                compression_threshold: self.compression_threshold.clone(),
                executor: self.executor.clone(),
                session_ref: self.session_ref.clone(),
            },
            worker::BackgroundWorkerOptions {
                worker_label: "catch-up",
                auto_hibernate: false,
                auto_rename_persona: false,
            },
        );

        if !started {
            log::info!(
                "[Session {}] Missed message queued for existing worker",
                session_id
            );
        }

        log::info!("[Session {}] Missed message catch-up initiated", session_id);
    }

    /// Fetch the current agentStateVersion for a session from the server.
    async fn fetch_agent_state_version(
        server_url: &str,
        auth_token: &str,
        session_id: &str,
    ) -> Result<u32, String> {
        let client = reqwest::Client::new();
        let url = format!("{}/v2/sessions/active", server_url);

        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", auth_token))
            .send()
            .await
            .map_err(|e| format!("Failed to fetch sessions: {}", e))?;

        let body: Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse sessions: {}", e))?;

        if let Some(sessions) = body.get("sessions").and_then(|s| s.as_array()) {
            for s in sessions {
                if s.get("id").and_then(|v| v.as_str()) == Some(session_id) {
                    let version = s
                        .get("agentStateVersion")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    log::info!(
                        "[Permission] Fetched agentStateVersion={} for session {}",
                        version,
                        session_id
                    );
                    return Ok(version);
                }
            }
        }

        log::info!(
            "[Permission] Session {} not in active list, using agentStateVersion=0",
            session_id
        );
        Ok(0)
    }
}
