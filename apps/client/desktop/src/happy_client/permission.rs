//! Permission System for Autonomous Agent
//!
//! Provides permission checking for tool calls. Mutating tools (shell, edit, file write, etc.)
//! require user approval via Happy Server protocol:
//! 1. Update AgentState.requests via `update-state` Socket.IO event
//! 2. Send ACP `permission-request` message
//! 3. Send push notification (fire-and-forget)
//! 4. Await user response via `{sessionId}:permission` RPC
//! 5. Move to AgentState.completedRequests

use super::socket::HappySocket;
use crate::happy_client::session::encode_session_payload;
use crate::session_message_codec::SessionMessageCodec;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use tokio::sync::oneshot;

/// Process-global registry of per-session `PermissionHandler` instances.
///
/// Previously every `establish_local_connection` / `establish_remote_connection`
/// call minted a fresh handler and bound `{sessionId}:permission` to it. On a
/// disconnect+reconnect the RPC re-binding landed on a NEW handler, while any
/// turn/normalizer spawned before the reconnect still held the OLD handler —
/// so `register_pending_request` and `handle_rpc_response` operated on
/// different maps and user approvals were silently dropped.
///
/// The registry makes handlers session-scoped and survives connection churn:
/// `get_or_create_handler(session_id, …)` hands back the same `Arc` for the
/// lifetime of the process unless `remove_handler` is called on true session
/// closure.
static HANDLER_REGISTRY: OnceLock<RwLock<HashMap<String, Arc<PermissionHandler>>>> =
    OnceLock::new();

fn handler_registry() -> &'static RwLock<HashMap<String, Arc<PermissionHandler>>> {
    HANDLER_REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Get the process-global `PermissionHandler` for this session, inserting a
/// fresh one if none exists. Connection establish paths MUST use this instead
/// of `PermissionHandler::new` so that pending requests survive reconnect.
pub fn get_or_create_handler(
    session_id: &str,
    initial_agent_state_version: u32,
) -> Arc<PermissionHandler> {
    {
        let guard = handler_registry().read().unwrap();
        if let Some(existing) = guard.get(session_id) {
            return existing.clone();
        }
    }
    let mut guard = handler_registry().write().unwrap();
    if let Some(existing) = guard.get(session_id) {
        return existing.clone();
    }
    let handler = Arc::new(PermissionHandler::new(
        session_id.to_string(),
        initial_agent_state_version,
    ));
    guard.insert(session_id.to_string(), handler.clone());
    handler
}

/// Remove the handler from the registry. Call on true session termination
/// (not disconnect) to release pending oneshots and free the map entry.
pub fn remove_handler(session_id: &str) -> Option<Arc<PermissionHandler>> {
    let mut guard = handler_registry().write().unwrap();
    guard.remove(session_id)
}

/// Whether this process still has a live permission waiter for the session.
/// Host session listing uses this to distinguish a normal frontend refresh
/// from a daemon restart: persisted `agentState.requests` are only stale when
/// no in-process handler can receive the user's reply anymore.
pub fn has_live_pending_requests(session_id: &str) -> bool {
    let guard = handler_registry().read().unwrap();
    guard
        .get(session_id)
        .map(|handler| handler.has_pending_requests())
        .unwrap_or(false)
}

// Permission data types now live in `cteno_agent_runtime::permission`.  The
// `PermissionHandler` below (real impl that drives Happy Server RPC) stays here
// because it needs `HappySocket` / machine key context that only the host has.
pub use cteno_agent_runtime::permission::{
    PermissionCheckResult, PermissionDecision, PermissionMode, PermissionRpcResponse,
};

pub fn parse_runtime_permission_mode(
    mode_str: &str,
) -> Option<(
    multi_agent_runtime_core::PermissionMode,
    Option<PermissionMode>,
)> {
    use multi_agent_runtime_core::PermissionMode as ExecMode;

    match mode_str {
        "default" => Some((ExecMode::Default, Some(PermissionMode::Default))),
        "auto" => Some((ExecMode::Auto, None)),
        "acceptEdits" => Some((ExecMode::AcceptEdits, Some(PermissionMode::AcceptEdits))),
        "bypassPermissions" => Some((
            ExecMode::BypassPermissions,
            Some(PermissionMode::BypassPermissions),
        )),
        "dontAsk" => Some((ExecMode::DontAsk, None)),
        "plan" => Some((ExecMode::Plan, Some(PermissionMode::Plan))),
        "read-only" => Some((ExecMode::ReadOnly, None)),
        "safe-yolo" => Some((ExecMode::WorkspaceWrite, None)),
        "yolo" => Some((ExecMode::DangerFullAccess, None)),
        _ => None,
    }
}

/// A pending permission request awaiting user response
struct PendingRequest {
    tool_name: String,
    sender: Option<oneshot::Sender<PermissionRpcResponse>>,
}

/// Session-scoped tool allowlist, shaped like Happy Coder's model:
///   * `tools` — bare tool names like `"Read"`, `"Bash"` — every invocation
///     passes if the name matches.
///   * `bash_literals` — exact commands like `"ls /tmp"`, passes only when
///     the Bash command string matches verbatim.
///   * `bash_prefixes` — prefix patterns from `Bash(<prefix>:*)`, passes
///     when the Bash command starts with `<prefix>`.
///
/// Populated from the RPC `allowTools` field when the user picks "是,不再询问"
/// / "approved for session". Frontend sends `Bash(<command>)` for specific
/// commands and bare tool names for everything else.
#[derive(Default)]
struct SessionAllowedTools {
    tools: HashSet<String>,
    bash_literals: HashSet<String>,
    bash_prefixes: HashSet<String>,
}

impl SessionAllowedTools {
    fn insert(&mut self, entry: &str) {
        if let Some(inner) = entry
            .strip_prefix("Bash(")
            .and_then(|s| s.strip_suffix(')'))
        {
            if let Some(prefix) = inner.strip_suffix(":*") {
                self.bash_prefixes.insert(prefix.to_string());
            } else {
                self.bash_literals.insert(inner.to_string());
            }
        } else {
            self.tools.insert(entry.to_string());
        }
    }

    fn matches(&self, tool_name: &str, input: &Value) -> bool {
        if self.tools.contains(tool_name) {
            return true;
        }
        if tool_name == "Bash" {
            if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
                if self.bash_literals.contains(cmd) {
                    return true;
                }
                for prefix in &self.bash_prefixes {
                    if cmd.starts_with(prefix) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Tool-name-only membership check. Legacy call sites (and existing
    /// tests) relied on `HashSet::contains` semantics without considering
    /// tool input; keep that behavior for them by looking only at the bare
    /// `tools` set.
    #[cfg(test)]
    fn contains(&self, tool_name: &str) -> bool {
        self.tools.contains(tool_name)
    }
}

/// Permission handler for a single session
pub struct PermissionHandler {
    session_id: String,
    mode: std::sync::Mutex<PermissionMode>,
    pending_requests: std::sync::Mutex<HashMap<String, PendingRequest>>,
    session_allowed_tools: std::sync::Mutex<SessionAllowedTools>,
    agent_state: std::sync::Mutex<Value>,
    agent_state_version: AtomicU32,
}

impl PermissionHandler {
    /// Create a new PermissionHandler for a session
    ///
    /// `initial_agent_state_version` should match the server's current `agentStateVersion`
    /// for this session (from session creation response or GET /v2/sessions/active).
    pub fn new(session_id: String, initial_agent_state_version: u32) -> Self {
        log::info!(
            "[Permission] Created handler for session {} with agentStateVersion={}",
            session_id,
            initial_agent_state_version
        );
        Self {
            session_id,
            mode: std::sync::Mutex::new(PermissionMode::Default),
            pending_requests: std::sync::Mutex::new(HashMap::new()),
            session_allowed_tools: std::sync::Mutex::new(SessionAllowedTools::default()),
            agent_state: std::sync::Mutex::new(json!({
                "controlledByUser": false,
            })),
            agent_state_version: AtomicU32::new(initial_agent_state_version),
        }
    }

    /// Check permission for a tool call
    ///
    /// Returns Allowed if the tool can proceed, Denied if rejected, Aborted to stop the agent.
    #[allow(clippy::too_many_arguments)]
    pub async fn check_permission(
        &self,
        tool_name: &str,
        input: &Value,
        call_id: &str,
        socket: &Arc<HappySocket>,
        message_codec: &SessionMessageCodec,
        server_url: &str,
        auth_token: &str,
    ) -> PermissionCheckResult {
        // 1. Read-only tools always pass
        if Self::is_read_only(tool_name, input) {
            log::debug!("[Permission] Auto-approve read-only tool: {}", tool_name);
            return PermissionCheckResult::Allowed;
        }

        // 2. Check current mode
        let mode = *self.mode.lock().unwrap();
        match mode {
            PermissionMode::BypassPermissions => {
                log::debug!(
                    "[Permission] BypassPermissions mode, auto-approve: {}",
                    tool_name
                );
                return PermissionCheckResult::Allowed;
            }
            PermissionMode::Plan => {
                log::info!("[Permission] Plan mode, denying mutation: {}", tool_name);
                return PermissionCheckResult::Denied(
                    "Plan mode: mutations not allowed".to_string(),
                );
            }
            PermissionMode::AcceptEdits => {
                if Self::is_edit_tool(tool_name, input) {
                    log::debug!(
                        "[Permission] AcceptEdits mode, auto-approve edit tool: {}",
                        tool_name
                    );
                    return PermissionCheckResult::Allowed;
                }
            }
            PermissionMode::Default => {}
        }

        // 3. Check session-allowed tools
        {
            let allowed = self.session_allowed_tools.lock().unwrap();
            if allowed.matches(tool_name, input) {
                log::info!(
                    "[Permission] Tool already approved for session: {}",
                    tool_name
                );
                return PermissionCheckResult::Allowed;
            }
        }

        // 4. Need to request permission from user
        log::info!(
            "[Permission] Requesting permission for tool: {} (call_id: {})",
            tool_name,
            call_id
        );
        self.request_permission(
            tool_name,
            input,
            call_id,
            socket,
            message_codec,
            server_url,
            auth_token,
        )
        .await
    }

    /// Check if a tool call is read-only (or safe enough to auto-approve)
    pub fn is_read_only(tool_name: &str, input: &Value) -> bool {
        match tool_name {
            // Pure read-only tools
            "read" | "websearch" | "query_subagent" | "list_subagents" | "tool_search"
            | "update_plan" => true,

            // Memory tool: all operations (recall/save/read/list) are local workspace only
            "memory" => true,

            // Persona read-only tools
            "list_task_sessions" | "update_personality" => true,

            _ => false,
        }
    }

    /// Check if a tool is an edit/file-write tool (for AcceptEdits mode)
    fn is_edit_tool(tool_name: &str, input: &Value) -> bool {
        matches!(tool_name, "edit")
    }

    /// Request permission from user and wait for response
    #[allow(clippy::too_many_arguments)]
    async fn request_permission(
        &self,
        tool_name: &str,
        input: &Value,
        call_id: &str,
        socket: &Arc<HappySocket>,
        message_codec: &SessionMessageCodec,
        server_url: &str,
        auth_token: &str,
    ) -> PermissionCheckResult {
        // Create oneshot channel
        let (tx, rx) = oneshot::channel::<PermissionRpcResponse>();

        // Store pending request
        {
            let mut pending = self.pending_requests.lock().unwrap();
            pending.insert(
                call_id.to_string(),
                PendingRequest {
                    tool_name: tool_name.to_string(),
                    sender: Some(tx),
                },
            );
        }

        // Update AgentState: add to requests
        self.update_agent_state_add_request(socket, message_codec, call_id, tool_name, input)
            .await;

        // Send ACP permission-request message
        self.send_permission_request_acp(socket, message_codec, call_id, tool_name, input, None)
            .await;

        // Send push notification (fire-and-forget)
        let push_server_url = server_url.to_string();
        let push_auth_token = auth_token.to_string();
        let push_session_id = self.session_id.clone();
        let push_call_id = call_id.to_string();
        let push_tool_name = tool_name.to_string();
        tokio::spawn(async move {
            Self::send_push_notification(
                &push_server_url,
                &push_auth_token,
                &push_session_id,
                &push_call_id,
                &push_tool_name,
            )
            .await;
        });

        // Wait for a user decision. Permissions are an input gate, so they do
        // not auto-deny or auto-abort just because the user is away.
        let result = match rx.await {
            Ok(response) => {
                log::info!("[Permission] Received response for {}: approved={}, decision={:?}, mode={:?}, allow_tools={:?}",
                    call_id, response.approved, response.decision, response.mode, response.allow_tools);
                self.process_response(response, call_id, tool_name)
            }
            Err(_) => {
                log::warn!("[Permission] Channel closed for {}", call_id);
                PermissionCheckResult::Denied("Permission channel closed".to_string())
            }
        };

        // Clean up pending request
        {
            let mut pending = self.pending_requests.lock().unwrap();
            pending.remove(call_id);
        }

        // Update AgentState: move to completedRequests
        // Note: frontend only recognizes "approved", "denied", "canceled" statuses
        let status = match &result {
            PermissionCheckResult::Allowed => "approved",
            PermissionCheckResult::Denied(_) => "denied",
            PermissionCheckResult::Aborted => "canceled",
        };
        self.update_agent_state_complete_request(
            socket,
            message_codec,
            call_id,
            status,
            None,
            None,
            None,
            None,
        )
        .await;

        result
    }

    /// Process a permission RPC response and apply side effects
    fn process_response(
        &self,
        response: PermissionRpcResponse,
        call_id: &str,
        tool_name: &str,
    ) -> PermissionCheckResult {
        if !response.approved {
            // Check for abort
            if response.decision.as_deref() == Some("abort") {
                log::info!("[Permission] User aborted agent for call_id: {}", call_id);
                return PermissionCheckResult::Aborted;
            }
            return PermissionCheckResult::Denied("User denied permission".to_string());
        }

        // Handle decision side effects
        if let Some(ref decision) = response.decision {
            if decision.as_str() == "approved_for_session" {
                let mut allowed = self.session_allowed_tools.lock().unwrap();
                allowed.insert(tool_name);
                log::info!(
                    "[Permission] Tool '{}' approved for entire session",
                    tool_name
                );
            }
        }

        // Handle mode change
        if let Some(ref mode_str) = response.mode {
            let new_mode = match mode_str.as_str() {
                "acceptEdits" => Some(PermissionMode::AcceptEdits),
                "bypassPermissions" => Some(PermissionMode::BypassPermissions),
                "plan" => Some(PermissionMode::Plan),
                "default" => Some(PermissionMode::Default),
                _ => None,
            };
            if let Some(mode) = new_mode {
                *self.mode.lock().unwrap() = mode;
                log::info!("[Permission] Mode changed to {:?}", mode);
            }
        }

        // Handle allowTools — frontend sends entries like "Bash(rm ...)" for
        // specific commands and bare tool names for everything-of-this-type.
        // SessionAllowedTools dispatches on the `Bash(…)` wrapper.
        if let Some(ref tools) = response.allow_tools {
            let mut allowed = self.session_allowed_tools.lock().unwrap();
            for t in tools {
                allowed.insert(t);
            }
            log::info!(
                "[Permission] Added {} tools to session allowed list",
                tools.len()
            );
        }

        PermissionCheckResult::Allowed
    }

    /// Handle an incoming RPC response for a permission request (called from sync RPC callback)
    ///
    /// This is called from the session socket's RPC handler, which is synchronous.
    /// Uses std::sync::Mutex (not tokio) so it can be called from sync context.
    pub fn handle_rpc_response(&self, response: PermissionRpcResponse) {
        log::info!(
            "[Permission] handle_rpc_response: id={}, approved={}",
            response.id,
            response.approved
        );

        let mut pending = self.pending_requests.lock().unwrap();
        if let Some(req) = pending.get_mut(&response.id) {
            if let Some(sender) = req.sender.take() {
                if sender.send(response).is_err() {
                    log::warn!("[Permission] Failed to send response (receiver dropped)");
                }
            } else {
                log::warn!(
                    "[Permission] Sender already consumed for request: {}",
                    &response.id
                );
            }
        } else {
            log::warn!(
                "[Permission] No pending request found for id: {}",
                response.id
            );
        }
    }

    /// Register a pending permission request and return the receiver half of
    /// the oneshot channel that [`handle_rpc_response`] will resolve.
    ///
    /// Unlike [`check_permission`], this does **not** await the user reply —
    /// callers are free to publish the ACP/agent-state updates, spawn a
    /// wait-for-reply task, and keep draining the executor stream in
    /// parallel. Used by the async permission flow in [`ExecutorNormalizer`].
    ///
    /// If a pending request with the same `call_id` already exists (double
    /// registration — e.g. re-entry during a retry), the old sender is
    /// dropped. The next `handle_rpc_response` will resolve the new receiver.
    pub fn register_pending_request(
        &self,
        call_id: &str,
        tool_name: &str,
    ) -> oneshot::Receiver<PermissionRpcResponse> {
        let (tx, rx) = oneshot::channel::<PermissionRpcResponse>();
        let mut pending = self.pending_requests.lock().unwrap();
        pending.insert(
            call_id.to_string(),
            PendingRequest {
                tool_name: tool_name.to_string(),
                sender: Some(tx),
            },
        );
        rx
    }

    /// Remove a previously-registered pending request (e.g. on channel close
    /// or abort). Idempotent.
    pub fn clear_pending_request(&self, call_id: &str) {
        let mut pending = self.pending_requests.lock().unwrap();
        pending.remove(call_id);
    }

    fn has_pending_requests(&self) -> bool {
        !self.pending_requests.lock().unwrap().is_empty()
    }

    /// Evaluate pre-approval shortcuts for a tool call without opening a
    /// user-facing permission prompt.
    ///
    /// Returns `Some(result)` when the tool can be decided without user
    /// input (read-only, session-allowed, bypass mode, plan mode). Returns
    /// `None` when a user decision is required — the caller must then
    /// publish the permission request and await the user's reply.
    pub fn evaluate_pre_approval(
        &self,
        tool_name: &str,
        input: &Value,
    ) -> Option<PermissionCheckResult> {
        if Self::is_read_only(tool_name, input) {
            log::debug!("[Permission] Auto-approve read-only tool: {}", tool_name);
            return Some(PermissionCheckResult::Allowed);
        }

        let mode = *self.mode.lock().unwrap();
        match mode {
            PermissionMode::BypassPermissions => {
                log::debug!(
                    "[Permission] BypassPermissions mode, auto-approve: {}",
                    tool_name
                );
                return Some(PermissionCheckResult::Allowed);
            }
            PermissionMode::Plan => {
                log::info!("[Permission] Plan mode, denying mutation: {}", tool_name);
                return Some(PermissionCheckResult::Denied(
                    "Plan mode: mutations not allowed".to_string(),
                ));
            }
            PermissionMode::AcceptEdits => {
                if Self::is_edit_tool(tool_name, input) {
                    log::debug!(
                        "[Permission] AcceptEdits mode, auto-approve edit tool: {}",
                        tool_name
                    );
                    return Some(PermissionCheckResult::Allowed);
                }
            }
            PermissionMode::Default => {}
        }

        let allowed = self.session_allowed_tools.lock().unwrap();
        if allowed.matches(tool_name, input) {
            log::info!(
                "[Permission] Tool already approved for session: {}",
                tool_name
            );
            return Some(PermissionCheckResult::Allowed);
        }

        None
    }

    /// Public wrapper around `process_response` — used by the async permission
    /// flow after the caller has received the RPC reply via the receiver
    /// returned by [`register_pending_request`].
    pub fn apply_response(
        &self,
        response: PermissionRpcResponse,
        call_id: &str,
        tool_name: &str,
    ) -> PermissionCheckResult {
        self.process_response(response, call_id, tool_name)
    }

    /// Publish a permission request to agent-state + ACP without waiting for a user reply.
    pub async fn publish_permission_request(
        &self,
        socket: &Arc<HappySocket>,
        message_codec: &SessionMessageCodec,
        call_id: &str,
        tool_name: &str,
        input: &Value,
        description: Option<&str>,
    ) {
        self.update_agent_state_add_request(socket, message_codec, call_id, tool_name, input)
            .await;

        self.send_permission_request_acp(
            socket,
            message_codec,
            call_id,
            tool_name,
            input,
            description,
        )
        .await;
    }

    /// Mark a previously-published permission request as completed.
    pub async fn complete_permission_request(
        &self,
        socket: &Arc<HappySocket>,
        message_codec: &SessionMessageCodec,
        call_id: &str,
        status: &str,
        decision: Option<&str>,
        mode: Option<&str>,
        allow_tools: Option<&[String]>,
        reason: Option<&str>,
    ) {
        self.update_agent_state_complete_request(
            socket,
            message_codec,
            call_id,
            status,
            decision,
            mode,
            allow_tools,
            reason,
        )
        .await;
    }

    /// Update AgentState: add request to requests array
    async fn update_agent_state_add_request(
        &self,
        socket: &Arc<HappySocket>,
        message_codec: &SessionMessageCodec,
        call_id: &str,
        tool_name: &str,
        input: &Value,
    ) {
        // Build request entry (frontend expects: tool, arguments, createdAt)
        // Truncate large inputs to prevent Socket.IO payload size issues
        let truncated_input = Self::truncate_input_for_acp(input);
        let request_entry = json!({
            "tool": tool_name,
            "arguments": truncated_input,
            "createdAt": chrono::Utc::now().timestamp_millis(),
        });

        // Update local agent state (requests is an object keyed by call_id, not an array)
        {
            let mut state = self.agent_state.lock().unwrap();
            if state.get("requests").is_none() || !state["requests"].is_object() {
                state["requests"] = json!({});
            }
            state["requests"][call_id] = request_entry;
        }

        self.emit_agent_state(socket, message_codec).await;
    }

    /// Update AgentState: move request to completedRequests
    async fn update_agent_state_complete_request(
        &self,
        socket: &Arc<HappySocket>,
        message_codec: &SessionMessageCodec,
        call_id: &str,
        status: &str,
        decision: Option<&str>,
        mode: Option<&str>,
        allow_tools: Option<&[String]>,
        reason: Option<&str>,
    ) {
        {
            let mut state = self.agent_state.lock().unwrap();

            // Remove from requests and preserve original data (object keyed by call_id)
            let original_request =
                if let Some(requests) = state.get_mut("requests").and_then(|r| r.as_object_mut()) {
                    requests.remove(call_id)
                } else {
                    None
                };

            // Add to completedRequests (object keyed by call_id)
            // Preserve tool/arguments from original request for frontend schema compatibility
            if state.get("completedRequests").is_none() || !state["completedRequests"].is_object() {
                state["completedRequests"] = json!({});
            }
            let mut completed_entry = json!({
                "status": status,
                "completedAt": chrono::Utc::now().timestamp_millis(),
            });
            if let Some(ref orig) = original_request {
                if let Some(tool) = orig.get("tool") {
                    completed_entry["tool"] = tool.clone();
                }
                if let Some(args) = orig.get("arguments") {
                    completed_entry["arguments"] = args.clone();
                }
                if let Some(created) = orig.get("createdAt") {
                    completed_entry["createdAt"] = created.clone();
                }
            }
            // Preserve the RPC-side fields the frontend reducer needs to
            // paint "是，不再询问" / Plan / acceptEdits visual states. Without
            // these the Modal only ever shows the generic "approved" badge.
            if let Some(d) = decision {
                completed_entry["decision"] = json!(d);
            }
            if let Some(m) = mode {
                completed_entry["mode"] = json!(m);
            }
            if let Some(tools) = allow_tools {
                if !tools.is_empty() {
                    completed_entry["allowTools"] = json!(tools);
                    // Reducer also looks at `allowedTools` (naming mismatch
                    // between frontend reducer and ACP). Write both keys to
                    // avoid another round of "where's the field?" debugging.
                    completed_entry["allowedTools"] = json!(tools);
                }
            }
            if let Some(r) = reason {
                completed_entry["reason"] = json!(r);
            }
            state["completedRequests"][call_id] = completed_entry;
        }

        self.emit_agent_state(socket, message_codec).await;
    }

    /// Encrypt and emit the current agent state to the server
    async fn emit_agent_state(
        &self,
        socket: &Arc<HappySocket>,
        message_codec: &SessionMessageCodec,
    ) {
        let state_json = {
            let state = self.agent_state.lock().unwrap();
            let json = serde_json::to_string(&*state).unwrap_or_default();
            let preview_end = json.floor_char_boundary(json.len().min(500));
            log::info!(
                "[Permission] Agent state before encrypt: {}",
                &json[..preview_end]
            );
            json
        };

        let outbound_state = match encode_session_payload(state_json.as_bytes(), message_codec) {
            Ok(data) => data,
            Err(e) => {
                log::error!("[Permission] Failed to encode agent state: {}", e);
                return;
            }
        };
        let version = self.agent_state_version.fetch_add(1, Ordering::SeqCst);

        match socket
            .update_session_state(&self.session_id, Some(&outbound_state), version)
            .await
        {
            Ok(_) => {
                log::info!(
                    "[Permission] Agent state emitted (version: {}, session: {})",
                    version,
                    self.session_id
                );
            }
            Err(e) => {
                log::error!("[Permission] Failed to emit agent state: {}", e);
            }
        }
    }

    /// Send ACP permission-request message
    async fn send_permission_request_acp(
        &self,
        socket: &Arc<HappySocket>,
        message_codec: &SessionMessageCodec,
        call_id: &str,
        tool_name: &str,
        input: &Value,
        description: Option<&str>,
    ) {
        // Truncate large tool inputs to prevent Socket.IO serialization errors.
        // The full input is already available in agentState.requests via update-state.
        let truncated_input = Self::truncate_input_for_acp(input);

        let mut acp_data = json!({
            "type": "permission-request",
            "permissionId": call_id,
            "toolName": tool_name,
            "options": truncated_input,
        });
        if let Some(description) = description.filter(|value| !value.is_empty()) {
            acp_data["description"] = Value::String(description.to_string());
        }

        let message = json!({
            "role": "agent",
            "content": {
                "type": "acp",
                "provider": "cteno",
                "data": acp_data
            },
            "meta": {
                "sentFrom": "cli"
            }
        });

        let message_json = match serde_json::to_string(&message) {
            Ok(j) => j,
            Err(e) => {
                log::error!("[Permission] Failed to serialize ACP message: {}", e);
                return;
            }
        };

        let outbound_message = match encode_session_payload(message_json.as_bytes(), message_codec)
        {
            Ok(data) => data,
            Err(e) => {
                log::error!("[Permission] Failed to encode ACP message: {}", e);
                return;
            }
        };

        if let Err(e) = socket
            .send_message(&self.session_id, &outbound_message, None)
            .await
        {
            log::error!("[Permission] Failed to send permission-request ACP: {}", e);
        } else {
            log::info!(
                "[Permission] ACP permission-request sent for call_id: {}",
                call_id
            );
        }
    }

    /// Get the current permission mode
    pub fn get_mode(&self) -> PermissionMode {
        *self.mode.lock().unwrap()
    }

    /// Set the permission mode
    pub fn set_mode(&self, mode: PermissionMode) {
        log::info!(
            "[Permission] Mode set to {:?} for session {}",
            mode,
            self.session_id
        );
        *self.mode.lock().unwrap() = mode;
    }

    /// Convert a PermissionMode to its string representation (for KV persistence)
    pub fn mode_to_string(mode: PermissionMode) -> &'static str {
        match mode {
            PermissionMode::Default => "default",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::BypassPermissions => "bypassPermissions",
            PermissionMode::Plan => "plan",
        }
    }

    /// Get the session_id of this handler
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Parse a mode string into PermissionMode
    pub fn parse_mode(mode_str: &str) -> Option<PermissionMode> {
        match mode_str {
            "default" => Some(PermissionMode::Default),
            "acceptEdits" => Some(PermissionMode::AcceptEdits),
            "bypassPermissions" => Some(PermissionMode::BypassPermissions),
            "plan" => Some(PermissionMode::Plan),
            _ => None,
        }
    }

    /// Add tool to session allowed list (for testing)
    #[cfg(test)]
    pub fn add_session_allowed_tool(&self, tool_name: &str) {
        self.session_allowed_tools.lock().unwrap().insert(tool_name);
    }

    /// Truncate tool input for ACP messages to prevent Socket.IO serialization errors.
    /// Large inputs (e.g., file contents in edit tools) can exceed ~8KB which crashes rust_socketio.
    /// The full input is already sent via agentState update-state event.
    fn truncate_input_for_acp(input: &Value) -> Value {
        const MAX_STRING_LEN: usize = 500;

        match input {
            Value::String(s) => {
                if s.len() > MAX_STRING_LEN {
                    let end = s.floor_char_boundary(MAX_STRING_LEN);
                    Value::String(format!(
                        "{}... [truncated, {} chars total]",
                        &s[..end],
                        s.len()
                    ))
                } else {
                    input.clone()
                }
            }
            Value::Object(map) => {
                let mut truncated = serde_json::Map::new();
                for (key, value) in map {
                    truncated.insert(key.clone(), Self::truncate_input_for_acp(value));
                }
                Value::Object(truncated)
            }
            Value::Array(arr) => {
                Value::Array(arr.iter().map(Self::truncate_input_for_acp).collect())
            }
            _ => input.clone(),
        }
    }

    /// Public fire-and-forget wrapper around [`send_push_notification`] used
    /// by the async permission flow in the normalizer.
    pub async fn send_push_notification_public(
        server_url: &str,
        auth_token: &str,
        session_id: &str,
        call_id: &str,
        tool_name: &str,
    ) {
        Self::send_push_notification(server_url, auth_token, session_id, call_id, tool_name).await;
    }

    /// Send push notification to user's mobile devices (fire-and-forget)
    async fn send_push_notification(
        server_url: &str,
        auth_token: &str,
        session_id: &str,
        call_id: &str,
        tool_name: &str,
    ) {
        let client = reqwest::Client::new();

        // Step 1: Get push tokens
        let tokens_url = format!("{}/v1/push-tokens", server_url);
        let tokens_response = match client
            .get(&tokens_url)
            .header("Authorization", format!("Bearer {}", auth_token))
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                log::warn!("[Permission] Failed to fetch push tokens: {}", e);
                return;
            }
        };

        let tokens_json: Value = match tokens_response.json().await {
            Ok(j) => j,
            Err(e) => {
                log::warn!("[Permission] Failed to parse push tokens response: {}", e);
                return;
            }
        };

        let tokens = match tokens_json.get("tokens").and_then(|t| t.as_array()) {
            Some(t) => t.clone(),
            None => {
                log::debug!("[Permission] No push tokens found");
                return;
            }
        };

        if tokens.is_empty() {
            log::debug!("[Permission] No push tokens to notify");
            return;
        }

        // Step 2: Send push notifications via Expo Push API
        let expo_url = "https://exp.host/--/api/v2/push/send";
        let title = "Cteno Permission Request";
        let body = format!("Tool '{}' needs your approval", tool_name);

        let messages: Vec<Value> = tokens
            .iter()
            .filter_map(|t| t.get("token").and_then(|v| v.as_str()))
            .map(|token| {
                json!({
                    "to": token,
                    "title": title,
                    "body": body,
                    "data": {
                        "type": "permission-request",
                        "sessionId": session_id,
                        "callId": call_id,
                        "toolName": tool_name,
                    },
                    "sound": "default",
                    "priority": "high",
                })
            })
            .collect();

        if messages.is_empty() {
            return;
        }

        match client.post(expo_url).json(&messages).send().await {
            Ok(resp) => {
                log::info!(
                    "[Permission] Push notification sent ({} devices), status: {}",
                    messages.len(),
                    resp.status()
                );
            }
            Err(e) => {
                log::warn!("[Permission] Failed to send push notification: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ======================== is_read_only tests ========================

    #[test]
    fn test_read_only_tools() {
        // Pure read-only tools
        assert!(PermissionHandler::is_read_only("read", &json!({})));
        assert!(PermissionHandler::is_read_only(
            "websearch",
            &json!({"query": "test"})
        ));
        assert!(PermissionHandler::is_read_only(
            "query_subagent",
            &json!({})
        ));
        assert!(PermissionHandler::is_read_only(
            "list_subagents",
            &json!({})
        ));
    }

    #[test]
    fn test_mutating_tools() {
        assert!(!PermissionHandler::is_read_only(
            "shell",
            &json!({"command": "ls"})
        ));
        assert!(!PermissionHandler::is_read_only("edit", &json!({})));
        assert!(!PermissionHandler::is_read_only(
            "start_subagent",
            &json!({})
        ));
        assert!(!PermissionHandler::is_read_only(
            "stop_subagent",
            &json!({})
        ));
        assert!(!PermissionHandler::is_read_only(
            "unknown_mcp_tool",
            &json!({})
        ));
    }

    // ======================== handle_rpc_response tests ========================

    #[test]
    fn test_handle_rpc_response_approve() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        let (tx, mut rx) = oneshot::channel();

        // Insert a pending request
        {
            let mut pending = handler.pending_requests.lock().unwrap();
            pending.insert(
                "call-1".to_string(),
                PendingRequest {
                    tool_name: "shell".to_string(),
                    sender: Some(tx),
                },
            );
        }

        // Simulate RPC response
        handler.handle_rpc_response(PermissionRpcResponse {
            id: "call-1".to_string(),
            approved: true,
            decision: None,
            mode: None,
            allow_tools: None,
            vendor_option: None,
        });

        // Verify response was received
        let result = rx.try_recv().unwrap();
        assert!(result.approved);
    }

    #[test]
    fn test_handle_rpc_response_deny() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        let (tx, mut rx) = oneshot::channel();

        {
            let mut pending = handler.pending_requests.lock().unwrap();
            pending.insert(
                "call-2".to_string(),
                PendingRequest {
                    tool_name: "shell".to_string(),
                    sender: Some(tx),
                },
            );
        }

        handler.handle_rpc_response(PermissionRpcResponse {
            id: "call-2".to_string(),
            approved: false,
            decision: None,
            mode: None,
            allow_tools: None,
            vendor_option: None,
        });

        let result = rx.try_recv().unwrap();
        assert!(!result.approved);
    }

    #[test]
    fn test_handle_rpc_response_unknown_id() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);

        // Should not panic when no pending request exists
        handler.handle_rpc_response(PermissionRpcResponse {
            id: "nonexistent".to_string(),
            approved: true,
            decision: None,
            mode: None,
            allow_tools: None,
            vendor_option: None,
        });
    }

    // ======================== process_response tests ========================

    #[test]
    fn test_process_response_approved() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        let result = handler.process_response(
            PermissionRpcResponse {
                id: "c1".to_string(),
                approved: true,
                decision: None,
                mode: None,
                allow_tools: None,
                vendor_option: None,
            },
            "c1",
            "shell",
        );
        assert!(matches!(result, PermissionCheckResult::Allowed));
    }

    #[test]
    fn test_process_response_denied() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        let result = handler.process_response(
            PermissionRpcResponse {
                id: "c1".to_string(),
                approved: false,
                decision: None,
                mode: None,
                allow_tools: None,
                vendor_option: None,
            },
            "c1",
            "shell",
        );
        assert!(matches!(result, PermissionCheckResult::Denied(_)));
    }

    #[test]
    fn test_process_response_abort() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        let result = handler.process_response(
            PermissionRpcResponse {
                id: "c1".to_string(),
                approved: false,
                decision: Some("abort".to_string()),
                mode: None,
                allow_tools: None,
                vendor_option: None,
            },
            "c1",
            "shell",
        );
        assert!(matches!(result, PermissionCheckResult::Aborted));
    }

    #[test]
    fn test_process_response_approved_for_session() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        let result = handler.process_response(
            PermissionRpcResponse {
                id: "c1".to_string(),
                approved: true,
                decision: Some("approved_for_session".to_string()),
                mode: None,
                allow_tools: None,
                vendor_option: None,
            },
            "c1",
            "shell",
        );
        assert!(matches!(result, PermissionCheckResult::Allowed));
        // Verify tool added to session allowed list
        let allowed = handler.session_allowed_tools.lock().unwrap();
        assert!(allowed.contains("shell"));
    }

    #[test]
    fn test_process_response_mode_change() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        assert_eq!(*handler.mode.lock().unwrap(), PermissionMode::Default);

        handler.process_response(
            PermissionRpcResponse {
                id: "c1".to_string(),
                approved: true,
                decision: None,
                mode: Some("bypassPermissions".to_string()),
                allow_tools: None,
                vendor_option: None,
            },
            "c1",
            "shell",
        );
        assert_eq!(
            *handler.mode.lock().unwrap(),
            PermissionMode::BypassPermissions
        );

        handler.process_response(
            PermissionRpcResponse {
                id: "c2".to_string(),
                approved: true,
                decision: None,
                mode: Some("acceptEdits".to_string()),
                allow_tools: None,
                vendor_option: None,
            },
            "c2",
            "edit",
        );
        assert_eq!(*handler.mode.lock().unwrap(), PermissionMode::AcceptEdits);

        handler.process_response(
            PermissionRpcResponse {
                id: "c3".to_string(),
                approved: true,
                decision: None,
                mode: Some("plan".to_string()),
                allow_tools: None,
                vendor_option: None,
            },
            "c3",
            "shell",
        );
        assert_eq!(*handler.mode.lock().unwrap(), PermissionMode::Plan);
    }

    #[test]
    fn test_process_response_allow_tools() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        handler.process_response(
            PermissionRpcResponse {
                id: "c1".to_string(),
                approved: true,
                decision: None,
                mode: None,
                allow_tools: Some(vec!["shell".to_string(), "edit".to_string()]),
                vendor_option: None,
            },
            "c1",
            "shell",
        );

        let allowed = handler.session_allowed_tools.lock().unwrap();
        assert!(allowed.contains("shell"));
        assert!(allowed.contains("edit"));
    }

    // ======================== Mode behavior tests ========================

    #[test]
    fn test_bypass_mode_skips_all() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        handler.set_mode(PermissionMode::BypassPermissions);

        // Even mutating tools should not be read-only, but mode bypasses them
        assert!(!PermissionHandler::is_read_only(
            "shell",
            &json!({"command": "rm -rf /"})
        ));
        // The actual mode check happens in check_permission which needs a socket,
        // so we verify the mode is set correctly
        assert_eq!(
            *handler.mode.lock().unwrap(),
            PermissionMode::BypassPermissions
        );
    }

    #[test]
    fn test_plan_mode_denies_all() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        handler.set_mode(PermissionMode::Plan);
        assert_eq!(*handler.mode.lock().unwrap(), PermissionMode::Plan);
    }

    #[test]
    fn test_session_allowed_tools() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        handler.add_session_allowed_tool("shell");

        let allowed = handler.session_allowed_tools.lock().unwrap();
        assert!(allowed.contains("shell"));
        assert!(!allowed.contains("edit"));
    }

    // ======================== PermissionRpcResponse deserialization ========================

    #[test]
    fn test_deserialize_rpc_response() {
        let json_str = r#"{"id":"call-1","approved":true,"decision":"approved_for_session","mode":"acceptEdits","allowTools":["shell","edit"]}"#;
        let resp: PermissionRpcResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(resp.id, "call-1");
        assert!(resp.approved);
        assert_eq!(resp.decision, Some("approved_for_session".to_string()));
        assert_eq!(resp.mode, Some("acceptEdits".to_string()));
        assert_eq!(
            resp.allow_tools,
            Some(vec!["shell".to_string(), "edit".to_string()])
        );
    }

    // ======================== register_pending_request tests ========================

    #[tokio::test]
    async fn register_pending_request_resolves_via_handle_rpc_response() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        let rx = handler.register_pending_request("call-async-1", "shell");

        // Simulate RPC callback happening after request is registered.
        handler.handle_rpc_response(PermissionRpcResponse {
            id: "call-async-1".to_string(),
            approved: true,
            decision: None,
            mode: None,
            allow_tools: None,
            vendor_option: None,
        });

        let response = rx.await.expect("oneshot should resolve");
        assert_eq!(response.id, "call-async-1");
        assert!(response.approved);
    }

    #[tokio::test]
    async fn register_pending_request_double_registration_keeps_latest_sender() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        let _rx_old = handler.register_pending_request("call-dup", "shell");
        let rx_new = handler.register_pending_request("call-dup", "shell");

        handler.handle_rpc_response(PermissionRpcResponse {
            id: "call-dup".to_string(),
            approved: false,
            decision: Some("abort".to_string()),
            mode: None,
            allow_tools: None,
            vendor_option: None,
        });

        let response = rx_new.await.expect("latest receiver should resolve");
        assert_eq!(response.decision.as_deref(), Some("abort"));
    }

    #[test]
    fn clear_pending_request_is_idempotent_and_drops_sender() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        let _rx = handler.register_pending_request("call-clear", "shell");
        handler.clear_pending_request("call-clear");
        // second clear on unknown id must not panic
        handler.clear_pending_request("call-clear");
        handler.clear_pending_request("never-registered");
    }

    #[tokio::test]
    async fn get_or_create_handler_returns_same_instance_across_calls() {
        let session_id = format!("registry-reuse-{}", uuid::Uuid::new_v4());
        let first = super::get_or_create_handler(&session_id, 0);
        let second = super::get_or_create_handler(&session_id, 7);

        // Same Arc means pending registrations survive a "reconnect" (second
        // caller obtains the same handler instead of allocating a new one).
        assert!(Arc::ptr_eq(&first, &second));

        let rx = first.register_pending_request("cross-conn", "shell");
        second.handle_rpc_response(PermissionRpcResponse {
            id: "cross-conn".to_string(),
            approved: true,
            decision: None,
            mode: None,
            allow_tools: None,
            vendor_option: None,
        });
        let response = rx.await.expect("oneshot should resolve via shared handler");
        assert!(response.approved);

        super::remove_handler(&session_id);
    }

    #[test]
    fn remove_handler_frees_registry_slot() {
        let session_id = format!("registry-remove-{}", uuid::Uuid::new_v4());
        let first = super::get_or_create_handler(&session_id, 0);
        super::remove_handler(&session_id);
        let fresh = super::get_or_create_handler(&session_id, 0);
        assert!(!Arc::ptr_eq(&first, &fresh));
        super::remove_handler(&session_id);
    }

    // ======================== evaluate_pre_approval tests ========================

    #[test]
    fn pre_approval_auto_approves_read_only() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        let result = handler.evaluate_pre_approval("read", &json!({"path": "/tmp"}));
        assert!(matches!(result, Some(PermissionCheckResult::Allowed)));
    }

    #[test]
    fn pre_approval_returns_none_for_mutating_tool_in_default_mode() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        let result = handler.evaluate_pre_approval("shell", &json!({"command": "rm -rf /"}));
        assert!(result.is_none(), "default mode must require user decision");
    }

    #[test]
    fn pre_approval_bypass_mode_auto_approves_dangerous_tool() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        handler.set_mode(PermissionMode::BypassPermissions);
        let result = handler.evaluate_pre_approval("shell", &json!({"command": "rm -rf /"}));
        assert!(matches!(result, Some(PermissionCheckResult::Allowed)));
    }

    #[test]
    fn pre_approval_plan_mode_denies_all_mutations() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        handler.set_mode(PermissionMode::Plan);
        let result = handler.evaluate_pre_approval("shell", &json!({"command": "ls"}));
        match result {
            Some(PermissionCheckResult::Denied(reason)) => {
                assert!(reason.contains("Plan mode"));
            }
            other => panic!("expected Denied, got {:?}", other),
        }
    }

    #[test]
    fn pre_approval_accept_edits_only_edges_edit_tool() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        handler.set_mode(PermissionMode::AcceptEdits);
        // edit auto-approved
        assert!(matches!(
            handler.evaluate_pre_approval("edit", &json!({})),
            Some(PermissionCheckResult::Allowed)
        ));
        // shell still requires user decision
        assert!(handler
            .evaluate_pre_approval("shell", &json!({"command": "ls"}))
            .is_none());
    }

    #[test]
    fn pre_approval_respects_session_allowed_tools() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        handler.add_session_allowed_tool("shell");
        assert!(matches!(
            handler.evaluate_pre_approval("shell", &json!({"command": "ls"})),
            Some(PermissionCheckResult::Allowed)
        ));
    }

    #[test]
    fn apply_response_delegates_to_process_response() {
        let handler = PermissionHandler::new("test-session".to_string(), 0);
        let result = handler.apply_response(
            PermissionRpcResponse {
                id: "c1".to_string(),
                approved: true,
                decision: Some("approved_for_session".to_string()),
                mode: None,
                allow_tools: None,
                vendor_option: None,
            },
            "c1",
            "shell",
        );
        assert!(matches!(result, PermissionCheckResult::Allowed));
        // And shell is remembered for the rest of the session.
        assert!(matches!(
            handler.evaluate_pre_approval("shell", &json!({"command": "ls"})),
            Some(PermissionCheckResult::Allowed)
        ));
    }

    #[test]
    fn test_deserialize_minimal_rpc_response() {
        let json_str = r#"{"id":"call-1","approved":false}"#;
        let resp: PermissionRpcResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(resp.id, "call-1");
        assert!(!resp.approved);
        assert!(resp.decision.is_none());
        assert!(resp.mode.is_none());
        assert!(resp.allow_tools.is_none());
    }
}
