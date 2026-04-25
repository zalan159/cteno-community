//! `AgentExecutor` implementation backed by `gemini --acp` speaking the real
//! Agent Client Protocol (JSON-RPC 2.0 over ndJSON).
//!
//! Each `GeminiAgentExecutor` owns at most one [`GeminiAcpConnection`], which
//! hosts one subprocess handling many sessions multiplexed by their
//! `sessionId`. The legacy "one subprocess per session + cold restart on
//! model / mode change" shape is gone — see `docs/gemini-p1-protocol-findings.md`
//! and `docs/gemini-p1-live-captures.md` for why.
//!
//! The adapter keeps the same public constructor as before
//! (`GeminiAgentExecutor::new(path, session_store)`) so existing call sites
//! (`apps/client/desktop/src/executor_registry.rs`) compile unchanged.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use multi_agent_runtime_core::executor::{
    ConnectionHandle, ConnectionHandleId, ConnectionHealth, ConnectionSpec,
};
use multi_agent_runtime_core::{
    AgentCapabilities, AgentExecutor, AgentExecutorError, DeltaKind, EventStream, ExecutorEvent,
    ModelChangeOutcome, ModelSpec, NativeMessage, NativeSessionId, Pagination, PermissionDecision,
    PermissionMode, PermissionModeKind, ProcessHandleToken, ResumeHints, SessionFilter,
    SessionInfo, SessionMeta, SessionRecord, SessionRef, SessionStoreProvider, SpawnSessionSpec,
    UserMessage,
};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::time::{Instant, timeout};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::connection::{
    DEFAULT_INITIALIZE_TIMEOUT, DEFAULT_TURN_TIMEOUT, GeminiAcpConnection, SessionState,
};

// Silence unused: SessionState is held inside GeminiAcpConnection's maps.
#[allow(dead_code)]
type _UnusedSessionState = SessionState;

const VENDOR_NAME: &str = "gemini";
const PROTOCOL_VERSION: &str = "1";

/// Adapter holding at most one shared connection and the session store.
pub struct GeminiAgentExecutor {
    gemini_path: PathBuf,
    session_store: Arc<dyn SessionStoreProvider>,
    /// Shared connection — lazily opened on first `spawn_session` or explicit
    /// `open_connection`. Reset to `None` when closed.
    connection: Mutex<Option<Arc<GeminiAcpConnection>>>,
    /// Maps our ProcessHandleToken to the native session id so
    /// SessionRef.process_handle → connection-session-state lookups keep
    /// O(1). The connection itself already keys by sessionId; we just need a
    /// token→id mapping.
    process_handles: Mutex<std::collections::HashMap<ProcessHandleToken, String>>,
    spawn_ready_timeout: Duration,
    turn_timeout: Duration,
}

impl GeminiAgentExecutor {
    pub fn new(gemini_path: PathBuf, session_store: Arc<dyn SessionStoreProvider>) -> Self {
        Self {
            gemini_path,
            session_store,
            connection: Mutex::new(None),
            process_handles: Mutex::new(std::collections::HashMap::new()),
            spawn_ready_timeout: DEFAULT_INITIALIZE_TIMEOUT,
            turn_timeout: DEFAULT_TURN_TIMEOUT,
        }
    }

    pub fn with_spawn_ready_timeout(mut self, timeout: Duration) -> Self {
        self.spawn_ready_timeout = timeout;
        self
    }

    pub fn with_turn_timeout(mut self, timeout: Duration) -> Self {
        self.turn_timeout = timeout;
        self
    }

    /// Snapshot of the union of `models.availableModels` seen on the shared
    /// connection so far. Empty when no session has been spawned yet.
    /// Exposed to host callers that want to surface gemini's real model list
    /// (e.g. `collect_vendor_models("gemini")`).
    #[allow(dead_code)]
    pub async fn available_models(&self) -> Vec<String> {
        let guard = self.connection.lock().await;
        if let Some(conn) = guard.as_ref() {
            conn.known_models_snapshot().await
        } else {
            Vec::new()
        }
    }

    /// Lazily open or return the cached connection.
    async fn shared_connection(&self) -> Result<Arc<GeminiAcpConnection>, AgentExecutorError> {
        let mut guard = self.connection.lock().await;
        if let Some(conn) = guard.as_ref() {
            if !conn.is_closed() {
                return Ok(Arc::clone(conn));
            }
        }
        let conn = self.open_and_initialize(&ConnectionSpec::default()).await?;
        *guard = Some(Arc::clone(&conn));
        Ok(conn)
    }

    /// Spawn gemini, run `initialize`, and optionally `authenticate`.
    async fn open_and_initialize(
        &self,
        spec: &ConnectionSpec,
    ) -> Result<Arc<GeminiAcpConnection>, AgentExecutorError> {
        let conn = GeminiAcpConnection::open(self.gemini_path.clone()).await?;

        // Send initialize.
        let init_params = json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": { "readTextFile": false, "writeTextFile": false },
                "terminal": false,
            }
        });

        let response = match timeout(
            self.spawn_ready_timeout,
            conn.call("initialize", init_params),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(err)) => {
                Arc::clone(&conn).shutdown().await;
                return Err(err);
            }
            Err(_) => {
                Arc::clone(&conn).shutdown().await;
                return Err(AgentExecutorError::Timeout {
                    operation: "initialize".to_string(),
                    seconds: self.spawn_ready_timeout.as_secs(),
                });
            }
        };

        // Cache auth methods + capabilities.
        if let Some(methods) = response.get("authMethods").and_then(Value::as_array) {
            let parsed: Vec<crate::connection::AuthMethod> = methods
                .iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(Value::as_str)?.to_string();
                    let name = m
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| id.clone());
                    Some(crate::connection::AuthMethod { id, name })
                })
                .collect();
            conn.set_auth_methods(parsed).await;
        }
        if let Some(caps) = response.get("agentCapabilities") {
            conn.set_agent_capabilities(caps.clone()).await;
        }

        // Optional authenticate: if spec.env carries GEMINI_API_KEY, try
        // gemini-api-key. probe=true skips auth entirely.
        if !spec.probe {
            if let Some(api_key) = spec.env.get("GEMINI_API_KEY") {
                let auth_params = json!({
                    "methodId": "gemini-api-key",
                    "_meta": { "api-key": api_key },
                });
                match conn.call("authenticate", auth_params).await {
                    Ok(_) => conn.mark_authenticated(),
                    Err(err) => {
                        log::warn!(
                            "gemini authenticate failed: {err} (continuing — cached credentials may suffice)"
                        );
                    }
                }
            }
        }

        Ok(conn)
    }

    async fn start_session_internal(
        &self,
        conn: &Arc<GeminiAcpConnection>,
        spec: &SpawnSessionSpec,
        resume_id: Option<NativeSessionId>,
    ) -> Result<SessionRef, AgentExecutorError> {
        let method = if resume_id.is_some() {
            "session/load"
        } else {
            "session/new"
        };
        let mut params = json!({
            "cwd": spec.workdir.to_string_lossy(),
            "mcpServers": [],
        });
        if let Some(resume_id) = resume_id.as_ref() {
            params["sessionId"] = json!(resume_id.as_str());
        }

        let response = conn.call(method, params).await?;
        let session_id = if let Some(id) = resume_id {
            id
        } else {
            let sid = response
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AgentExecutorError::Protocol("session/new missing sessionId".to_string())
                })?
                .to_string();
            NativeSessionId::new(sid)
        };

        // Ingest any `models.availableModels` reported by the server so a
        // later `apply_model` can validate the requested id instead of
        // silently forwarding bogus profile_ids from other vendors — which
        // would poison this session with a non-existent model resource and
        // make `session/prompt` fail with `[500] Requested entity was not
        // found.`. See `tests/eval/gemini-model-gate.md`.
        if let Some(available) = response.pointer("/models/availableModels") {
            conn.ingest_available_models(available).await;
        }

        let initial_model = response
            .pointer("/models/currentModelId")
            .and_then(Value::as_str)
            .or_else(|| spec.model.as_ref().map(|model| model.model_id.as_str()))
            .map(ToOwned::to_owned);

        // Register session state on the connection.
        let state = conn
            .register_session(session_id.as_str().to_string(), initial_model)
            .await;

        // Apply requested permission mode / model if necessary.
        if !matches!(spec.permission_mode, PermissionMode::Default) {
            if let Err(err) = self
                .apply_permission_mode(conn, session_id.as_str(), spec.permission_mode)
                .await
            {
                log::warn!("gemini set_mode on spawn failed: {err}");
            }
        }
        if let Some(model) = spec.model.as_ref() {
            if let Err(err) = self.apply_model(conn, session_id.as_str(), model).await {
                log::warn!("gemini set_model on spawn failed: {err}");
            }
        }

        // Persist for later resume / list.
        self.session_store
            .record_session(
                VENDOR_NAME,
                SessionRecord {
                    session_id: session_id.clone(),
                    workdir: spec.workdir.clone(),
                    context: json!({
                        "native_session_id": session_id.as_str(),
                        "permission_mode": spec.permission_mode,
                        "model": spec.model.clone(),
                    }),
                },
            )
            .await
            .map_err(|message| AgentExecutorError::Vendor {
                vendor: VENDOR_NAME,
                message,
            })?;

        // Map ProcessHandleToken → session id for O(1) lookup in send_message.
        let process_handle = ProcessHandleToken::new();
        self.process_handles
            .lock()
            .await
            .insert(process_handle.clone(), session_id.as_str().to_string());

        drop(state); // connection owns the Arc

        Ok(SessionRef {
            id: session_id,
            vendor: VENDOR_NAME,
            process_handle,
            spawned_at: Utc::now(),
            workdir: spec.workdir.clone(),
        })
    }

    async fn apply_permission_mode(
        &self,
        conn: &Arc<GeminiAcpConnection>,
        session_id: &str,
        mode: PermissionMode,
    ) -> Result<(), AgentExecutorError> {
        let mode_id = permission_mode_id(mode);
        let params = json!({
            "sessionId": session_id,
            "modeId": mode_id,
        });
        conn.call("session/set_mode", params).await?;
        Ok(())
    }

    async fn apply_model(
        &self,
        conn: &Arc<GeminiAcpConnection>,
        session_id: &str,
        model: &ModelSpec,
    ) -> Result<(), AgentExecutorError> {
        // Guard 1 — provider must target gemini. Shared spawn paths in the
        // host can route a profile that's actually meant for an OpenAI /
        // Anthropic backend (profile.api_format != Gemini → provider =
        // "openai"|"anthropic"). Forwarding such an id into `session/set_model`
        // would be accepted silently by the CLI but then fail the next
        // `session/prompt` with `[500] Requested entity was not found.`.
        // Skipping leaves the server-chosen default (e.g. auto-gemini-3) in
        // place, which is what the user wants on an unprepared gemini session.
        if !model.provider.is_empty() && !model.provider.eq_ignore_ascii_case("gemini") {
            log::info!(
                "gemini apply_model: skipping set_model for non-gemini provider '{}' (model_id='{}')",
                model.provider,
                model.model_id
            );
            return Ok(());
        }

        // Guard 2 — validate against the `models.availableModels` snapshot
        // we captured from `session/new` / `session/load`. If the cache is
        // non-empty and the requested id isn't in it, skip with a warning.
        // If the cache is empty (e.g. mock server didn't emit any) fall
        // through to preserve the historical "best-effort set_model" behaviour.
        let known_snapshot = conn.known_models_snapshot().await;
        if !known_snapshot.is_empty() && !conn.is_known_model(&model.model_id).await {
            log::warn!(
                "gemini apply_model: model_id='{}' not in server-advertised list (known={:?}); skipping set_model to avoid a [500] backend error on next session/prompt",
                model.model_id,
                known_snapshot
            );
            return Ok(());
        }

        let params = json!({
            "sessionId": session_id,
            "modelId": model.model_id,
        });
        conn.call("session/set_model", params).await?;
        if let Some(state) = conn.get_session(session_id).await {
            *state.current_model.lock().await = Some(model.model_id.clone());
        }
        Ok(())
    }

    async fn session_state_for(
        &self,
        session: &SessionRef,
    ) -> Result<(Arc<GeminiAcpConnection>, Arc<SessionState>), AgentExecutorError> {
        let conn = self
            .connection
            .lock()
            .await
            .clone()
            .ok_or_else(|| AgentExecutorError::SessionNotFound(session.id.to_string()))?;
        let state = conn
            .get_session(session.id.as_str())
            .await
            .ok_or_else(|| AgentExecutorError::SessionNotFound(session.id.to_string()))?;
        Ok((conn, state))
    }
}

fn permission_mode_id(mode: PermissionMode) -> &'static str {
    // Live capture (docs/gemini-p1-live-captures.md) showed camelCase:
    // default / autoEdit / yolo / plan.
    match mode {
        PermissionMode::Default | PermissionMode::Auto => "default",
        PermissionMode::AcceptEdits | PermissionMode::WorkspaceWrite => "autoEdit",
        PermissionMode::Plan | PermissionMode::ReadOnly => "plan",
        PermissionMode::BypassPermissions
        | PermissionMode::DontAsk
        | PermissionMode::DangerFullAccess => "yolo",
    }
}

fn permission_decision_outcome(decision: PermissionDecision) -> Value {
    // Live probe against gemini --experimental-acp 0.38.2 returned these
    // option IDs in session/request_permission.options[]:
    //   { optionId: "proceed_always", kind: "allow_always" }
    //   { optionId: "proceed_once",   kind: "allow_once"  }
    //   { optionId: "cancel",         kind: "reject_once" }
    // Note "reject_once" is the *kind*, not the optionId; the reject
    // optionId is "cancel". An earlier version of this adapter sent
    // optionId="reject_once" on Deny, which gemini did not recognize.
    match decision {
        PermissionDecision::Allow => {
            json!({ "outcome": { "outcome": "selected", "optionId": "proceed_once" } })
        }
        PermissionDecision::Deny => {
            json!({ "outcome": { "outcome": "selected", "optionId": "cancel" } })
        }
        PermissionDecision::Abort => {
            json!({ "outcome": { "outcome": "cancelled" } })
        }
        // The frontend surfaced the vendor option list (see
        // route_incoming_request) and the user clicked one of them. The
        // id is passed through verbatim — we don't try to second-guess
        // what kind it is, gemini owns the semantics.
        PermissionDecision::SelectedOption { option_id } => {
            json!({ "outcome": { "outcome": "selected", "optionId": option_id } })
        }
    }
}

#[async_trait]
impl AgentExecutor for GeminiAgentExecutor {
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            name: Cow::Borrowed(VENDOR_NAME),
            protocol_version: Cow::Borrowed(PROTOCOL_VERSION),
            supports_list_sessions: true,
            supports_get_messages: true,
            supports_runtime_set_model: true,
            permission_mode_kind: PermissionModeKind::Dynamic,
            supports_resume: true,
            // Phase B flips this to true — one gemini --acp subprocess hosts
            // N sessions multiplexed by sessionId over a shared JSON-RPC
            // transport.
            supports_multi_session_per_process: true,
            supports_injected_tools: false,
            supports_permission_closure: true,
            supports_interrupt: true,
            autonomous_turn: false,
        }
    }

    async fn spawn_session(
        &self,
        spec: SpawnSessionSpec,
    ) -> Result<SessionRef, AgentExecutorError> {
        let conn = self.shared_connection().await?;
        self.start_session_internal(&conn, &spec, None).await
    }

    async fn resume_session(
        &self,
        session_id: NativeSessionId,
        hints: ResumeHints,
    ) -> Result<SessionRef, AgentExecutorError> {
        let conn = self.shared_connection().await?;
        let caps = conn.agent_capabilities().await;
        let supports_load = caps
            .get("loadSession")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !supports_load {
            return Err(AgentExecutorError::Unsupported {
                capability: "resume_session".to_string(),
            });
        }

        // Resolve workdir, preferring caller-supplied hints, then the
        // SessionStore row we wrote at spawn time. Falling through to the
        // process CWD (`.`) means gemini's new session would run in whatever
        // directory the cteno daemon was started in — almost always wrong,
        // so only hit that branch if the store lookup itself errors.
        let workdir = if let Some(w) = hints.workdir.clone() {
            w
        } else {
            match self
                .session_store
                .get_session_info(VENDOR_NAME, &session_id)
                .await
            {
                Ok(info) => info.meta.workdir,
                Err(err) => {
                    log::warn!(
                        "gemini resume_session({session_id}): workdir lookup failed: {err}; falling back to CWD"
                    );
                    PathBuf::from(".")
                }
            }
        };

        let spec = SpawnSessionSpec {
            workdir,
            system_prompt: None,
            model: None,
            permission_mode: PermissionMode::Default,
            allowed_tools: None,
            additional_directories: Vec::new(),
            env: BTreeMap::new(),
            agent_config: Value::Null,
            resume_hint: Some(hints),
        };

        self.start_session_internal(&conn, &spec, Some(session_id))
            .await
    }

    async fn send_message(
        &self,
        session: &SessionRef,
        message: UserMessage,
    ) -> Result<EventStream, AgentExecutorError> {
        let (conn, state) = self.session_state_for(session).await?;

        // Subscribe to the session's broadcast *before* sending the prompt so
        // we don't miss the first chunks.
        let rx = state.events_tx.subscribe();

        let session_id = session.id.as_str().to_string();
        let params = json!({
            "sessionId": session_id,
            "prompt": [ { "type": "text", "text": message.content } ],
        });

        // Dispatch session/prompt and await the PromptResponse separately in
        // a spawned task — so session/update notifications can stream to the
        // caller in the meantime.
        let prompt_conn = Arc::clone(&conn);
        let turn_timeout = self.turn_timeout;
        let events_tx = state.events_tx.clone();
        let state_for_timeout = Arc::clone(&state);

        tokio::spawn(async move {
            let prompt_call = prompt_conn.call("session/prompt", params);
            tokio::pin!(prompt_call);
            let deadline = tokio::time::sleep(turn_timeout);
            tokio::pin!(deadline);
            let mut permission_tick = tokio::time::interval(Duration::from_secs(1));

            let response = loop {
                tokio::select! {
                    response = &mut prompt_call => break response,
                    _ = &mut deadline, if state_for_timeout.pending_permission_count.load(std::sync::atomic::Ordering::SeqCst) == 0 => {
                        let _ = events_tx.send(ExecutorEvent::Error {
                            message: format!(
                                "gemini session/prompt timed out after {}s",
                                turn_timeout.as_secs()
                            ),
                            recoverable: true,
                        });
                        return;
                    }
                    _ = permission_tick.tick(), if state_for_timeout.pending_permission_count.load(std::sync::atomic::Ordering::SeqCst) > 0 => {
                        deadline
                            .as_mut()
                            .reset(Instant::now() + turn_timeout);
                    }
                }
            };

            match response {
                Ok(response) => {
                    let stop_reason = response
                        .get("stopReason")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let usage = extract_usage(&response);
                    // Quota / tier information now comes from the
                    // machine-level `cteno-host-usage-monitor`, which calls
                    // Google's Code-Assist `retrieveUserQuota` endpoint
                    // directly. The session/prompt response's `_meta.quota`
                    // payload is token-accounting only (no percentage), so
                    // we no longer emit it as a usage event.
                    // Emit UsageUpdate *before* TurnComplete so the host
                    // normalizer persists a `token_count` ACP side-effect —
                    // that's what the frontend's `session.contextTokens`
                    // reads (see docs/context-indicator-integration.md).
                    // Gemini's session/prompt response is the only place
                    // token counts arrive, so without this the context
                    // indicator X value never updates for gemini sessions.
                    if usage.input_tokens > 0 || usage.output_tokens > 0 {
                        let _ = events_tx.send(ExecutorEvent::UsageUpdate(usage.clone()));
                        let model_hint = state_for_timeout.current_model.lock().await.clone();
                        if let Some(event) = gemini_context_usage_native_event(
                            &response,
                            &usage,
                            model_hint.as_deref(),
                        ) {
                            let _ = events_tx.send(event);
                        }
                    }
                    let _ = events_tx.send(ExecutorEvent::TurnComplete {
                        final_text: None,
                        iteration_count: 1,
                        usage,
                    });
                    if let Some(reason) = stop_reason {
                        log::debug!("gemini turn end stopReason={reason}");
                    }
                }
                Err(err) => {
                    let _ = events_tx.send(ExecutorEvent::Error {
                        message: format!("gemini session/prompt failed: {err}"),
                        recoverable: true,
                    });
                }
            }
        });

        // Adapt the broadcast receiver into an EventStream. Stop after the
        // first `TurnComplete` or `Error { recoverable: false }` event is
        // emitted — use an async_stream to make the "inclusive take until
        // terminal event" semantics explicit (take_while would stall because
        // the broadcast channel stays open).
        let stream = async_stream::stream! {
            use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
            let mut s = BroadcastStream::new(rx);
            // Gemini's session/prompt response only carries `stopReason`; the
            // assistant text arrives exclusively as `session/update` deltas.
            // Accumulate Text deltas here so we can fill `TurnComplete.final_text`,
            // which is what the host normalizer persists as the final message.
            let mut accumulated_text = String::new();
            while let Some(item) = s.next().await {
                match item {
                    Ok(event) => {
                        if let ExecutorEvent::StreamDelta { kind: DeltaKind::Text, content } = &event {
                            accumulated_text.push_str(content);
                        }
                        let is_terminal = matches!(
                            &event,
                            ExecutorEvent::TurnComplete { .. }
                                | ExecutorEvent::Error { recoverable: false, .. }
                        );
                        let out = match event {
                            ExecutorEvent::TurnComplete {
                                final_text,
                                iteration_count,
                                usage,
                            } => ExecutorEvent::TurnComplete {
                                final_text: final_text.or_else(|| {
                                    if accumulated_text.is_empty() {
                                        None
                                    } else {
                                        Some(std::mem::take(&mut accumulated_text))
                                    }
                                }),
                                iteration_count,
                                usage,
                            },
                            other => other,
                        };
                        yield Ok::<_, AgentExecutorError>(out);
                        if is_terminal {
                            break;
                        }
                    }
                    Err(BroadcastStreamRecvError::Lagged(n)) => {
                        log::warn!("gemini session event stream lagged by {n} frames");
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn respond_to_permission(
        &self,
        session: &SessionRef,
        request_id: String,
        decision: PermissionDecision,
    ) -> Result<(), AgentExecutorError> {
        let (_, state) = self.session_state_for(session).await?;
        let reply_tx = state
            .pending_inbound
            .lock()
            .await
            .remove(&request_id)
            .ok_or_else(|| {
                AgentExecutorError::SessionNotFound(format!(
                    "no pending permission for request_id={request_id}"
                ))
            })?;
        let outcome = permission_decision_outcome(decision);
        let _ = state.pending_permission_count.fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |count| Some(count.saturating_sub(1)),
        );
        reply_tx.send(outcome).map_err(|_| {
            AgentExecutorError::Protocol(
                "gemini permission reply channel dropped before forwarding".to_string(),
            )
        })
    }

    async fn interrupt(&self, session: &SessionRef) -> Result<(), AgentExecutorError> {
        let (conn, _) = self.session_state_for(session).await?;
        let params = json!({ "sessionId": session.id.as_str() });
        conn.notify("session/cancel", params).await
    }

    async fn close_session(&self, session: &SessionRef) -> Result<(), AgentExecutorError> {
        // Gemini doesn't expose session delete — drop local state only.
        let Some(conn) = self.connection.lock().await.clone() else {
            return Ok(());
        };
        let _ = conn.remove_session(session.id.as_str()).await;
        self.process_handles
            .lock()
            .await
            .remove(&session.process_handle);
        Ok(())
    }

    async fn set_permission_mode(
        &self,
        session: &SessionRef,
        mode: PermissionMode,
    ) -> Result<(), AgentExecutorError> {
        let (conn, _) = self.session_state_for(session).await?;
        self.apply_permission_mode(&conn, session.id.as_str(), mode)
            .await
    }

    async fn set_model(
        &self,
        session: &SessionRef,
        model: ModelSpec,
    ) -> Result<ModelChangeOutcome, AgentExecutorError> {
        let (conn, _) = self.session_state_for(session).await?;
        match self.apply_model(&conn, session.id.as_str(), &model).await {
            Ok(()) => Ok(ModelChangeOutcome::Applied),
            Err(AgentExecutorError::Vendor { message, .. })
                if message.contains("-32601") || message.contains("method not found") =>
            {
                // Gemini dropped session/set_model in this version.
                Ok(ModelChangeOutcome::Unsupported)
            }
            Err(other) => Err(other),
        }
    }

    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, AgentExecutorError> {
        self.session_store
            .list_sessions(VENDOR_NAME, filter)
            .await
            .map_err(|message| AgentExecutorError::Vendor {
                vendor: VENDOR_NAME,
                message,
            })
    }

    async fn get_session_info(
        &self,
        session_id: &NativeSessionId,
    ) -> Result<SessionInfo, AgentExecutorError> {
        self.session_store
            .get_session_info(VENDOR_NAME, session_id)
            .await
            .map_err(|message| AgentExecutorError::Vendor {
                vendor: VENDOR_NAME,
                message,
            })
    }

    async fn get_session_messages(
        &self,
        session_id: &NativeSessionId,
        pagination: Pagination,
    ) -> Result<Vec<NativeMessage>, AgentExecutorError> {
        self.session_store
            .get_session_messages(VENDOR_NAME, session_id, pagination)
            .await
            .map_err(|message| AgentExecutorError::Vendor {
                vendor: VENDOR_NAME,
                message,
            })
    }

    async fn open_connection(
        &self,
        spec: ConnectionSpec,
    ) -> Result<ConnectionHandle, AgentExecutorError> {
        let conn = self.open_and_initialize(&spec).await?;
        let arc_any: Arc<dyn std::any::Any + Send + Sync> = Arc::clone(&conn) as _;
        // Stash as the shared connection so subsequent spawn_session sees it.
        {
            let mut guard = self.connection.lock().await;
            if guard.as_ref().map(|c| c.is_closed()).unwrap_or(true) {
                *guard = Some(Arc::clone(&conn));
            }
        }
        Ok(ConnectionHandle {
            id: ConnectionHandleId::new(),
            vendor: VENDOR_NAME,
            inner: arc_any,
        })
    }

    async fn close_connection(&self, handle: ConnectionHandle) -> Result<(), AgentExecutorError> {
        let conn = handle
            .inner
            .downcast::<GeminiAcpConnection>()
            .map_err(|_| {
                AgentExecutorError::Protocol(
                    "ConnectionHandle.inner is not a GeminiAcpConnection".to_string(),
                )
            })?;
        // Clear the shared-connection slot first (via ptr equality) so we
        // don't hand out a reference to a shutting-down connection.
        {
            let mut guard = self.connection.lock().await;
            if guard
                .as_ref()
                .map(|c| Arc::ptr_eq(c, &conn))
                .unwrap_or(false)
            {
                *guard = None;
            }
        }
        conn.shutdown().await;
        Ok(())
    }

    async fn check_connection(
        &self,
        handle: &ConnectionHandle,
    ) -> Result<ConnectionHealth, AgentExecutorError> {
        let conn = handle
            .inner
            .clone()
            .downcast::<GeminiAcpConnection>()
            .map_err(|_| {
                AgentExecutorError::Protocol(
                    "ConnectionHandle.inner is not a GeminiAcpConnection".to_string(),
                )
            })?;
        if conn.is_closed() {
            return Ok(ConnectionHealth::Dead {
                reason: "connection marked closed".to_string(),
            });
        }
        // Probe child state.
        let mut child_guard = conn.child.lock().await;
        if let Some(child) = child_guard.as_mut() {
            let child: &mut tokio::process::Child = child;
            if let Ok(Some(status)) = child.try_wait() {
                return Ok(ConnectionHealth::Dead {
                    reason: format!("gemini --acp exited (code={:?})", status.code()),
                });
            }
        }
        Ok(ConnectionHealth::Healthy)
    }

    async fn start_session_on(
        &self,
        handle: &ConnectionHandle,
        spec: SpawnSessionSpec,
    ) -> Result<SessionRef, AgentExecutorError> {
        let conn: Arc<GeminiAcpConnection> = handle
            .inner
            .clone()
            .downcast::<GeminiAcpConnection>()
            .map_err(|_| {
                AgentExecutorError::Protocol(
                    "ConnectionHandle.inner is not a GeminiAcpConnection".to_string(),
                )
            })?;
        self.start_session_internal(&conn, &spec, None).await
    }
}

/// Pull `TokenUsage` out of `PromptResponse._meta.quota.token_count`. Returns
/// zero usage if the fields are absent, matching the previous behaviour.
fn extract_usage(response: &Value) -> multi_agent_runtime_core::TokenUsage {
    use multi_agent_runtime_core::TokenUsage;
    let tok = response
        .pointer("/_meta/quota/token_count")
        .cloned()
        .unwrap_or(Value::Null);
    TokenUsage {
        input_tokens: tok.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
        output_tokens: tok
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        reasoning_tokens: tok
            .get("reasoning_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

fn gemini_context_usage_native_event(
    response: &Value,
    usage: &multi_agent_runtime_core::TokenUsage,
    model_hint: Option<&str>,
) -> Option<ExecutorEvent> {
    let total_tokens = usage
        .input_tokens
        .saturating_add(usage.output_tokens)
        .saturating_add(usage.cache_creation_tokens)
        .saturating_add(usage.cache_read_tokens)
        .saturating_add(usage.reasoning_tokens);
    if total_tokens == 0 {
        return None;
    }

    let model = response_model(response).or_else(|| {
        model_hint
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    });
    let context_window = explicit_context_window(response)
        .or_else(|| model.as_deref().and_then(gemini_model_context_window));

    let mut payload = serde_json::Map::new();
    payload.insert(
        "kind".to_string(),
        Value::String("context_usage".to_string()),
    );
    payload.insert("total_tokens".to_string(), Value::from(total_tokens));
    if let Some(context_window) = context_window {
        payload.insert("max_tokens".to_string(), Value::from(context_window));
        payload.insert("raw_max_tokens".to_string(), Value::from(context_window));
    }
    if let Some(model) = model {
        payload.insert("model".to_string(), Value::String(model));
    }

    Some(ExecutorEvent::NativeEvent {
        provider: Cow::Borrowed(VENDOR_NAME),
        payload: Value::Object(payload),
    })
}

fn response_model(response: &Value) -> Option<String> {
    response
        .pointer("/_meta/quota/model_usage/0/model")
        .and_then(Value::as_str)
        .or_else(|| response.get("model").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn explicit_context_window(response: &Value) -> Option<u64> {
    find_u64_by_keys(
        response,
        &[
            "context_window",
            "contextWindow",
            "model_context_window",
            "modelContextWindow",
            "max_tokens",
            "maxTokens",
            "raw_max_tokens",
            "rawMaxTokens",
        ],
    )
}

fn gemini_model_context_window(model: &str) -> Option<u64> {
    let model = model.trim();
    if model.is_empty() {
        return None;
    }

    // Mirrors Gemini CLI's `tokenLimit()` as of the local reference:
    // all supported Gemini 2.5 / 3.x CLI models use a 1,048,576-token window,
    // and the CLI defaults unknown Gemini model ids to the same value.
    if model.starts_with("gemini-") || model.starts_with("auto-gemini-") {
        Some(1_048_576)
    } else {
        None
    }
}

fn find_u64_by_keys(value: &Value, keys: &[&str]) -> Option<u64> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(found) = map.get(*key).and_then(Value::as_u64) {
                    return Some(found);
                }
            }
            map.values()
                .find_map(|nested| find_u64_by_keys(nested, keys))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|nested| find_u64_by_keys(nested, keys)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_mode_mapping_matches_gemini_acp_mode_ids() {
        assert_eq!(permission_mode_id(PermissionMode::Default), "default");
        assert_eq!(permission_mode_id(PermissionMode::AcceptEdits), "autoEdit");
        assert_eq!(permission_mode_id(PermissionMode::Plan), "plan");
        assert_eq!(
            permission_mode_id(PermissionMode::BypassPermissions),
            "yolo"
        );
        assert_eq!(
            permission_mode_id(PermissionMode::WorkspaceWrite),
            "autoEdit"
        );
        assert_eq!(permission_mode_id(PermissionMode::DangerFullAccess), "yolo");
    }

    #[test]
    fn prompt_response_maps_model_usage_to_context_usage_native_event() {
        let usage = extract_usage(&json!({
            "stopReason": "end_turn",
            "_meta": {
                "quota": {
                    "token_count": {
                        "input_tokens": 11_700,
                        "output_tokens": 2,
                        "reasoning_tokens": 3
                    },
                    "model_usage": [{
                        "model": "gemini-3-flash-preview",
                        "token_count": {
                            "input_tokens": 11_700,
                            "output_tokens": 2
                        }
                    }]
                }
            }
        }));
        let event = gemini_context_usage_native_event(
            &json!({
                "_meta": {
                    "quota": {
                        "model_usage": [{
                            "model": "gemini-3-flash-preview"
                        }]
                    }
                }
            }),
            &usage,
            Some("auto-gemini-3"),
        )
        .expect("context usage event");

        match event {
            ExecutorEvent::NativeEvent { provider, payload } => {
                assert_eq!(provider.as_ref(), VENDOR_NAME);
                assert_eq!(
                    payload,
                    json!({
                        "kind": "context_usage",
                        "total_tokens": 11_705_u64,
                        "max_tokens": 1_048_576_u64,
                        "raw_max_tokens": 1_048_576_u64,
                        "model": "gemini-3-flash-preview"
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn prompt_response_explicit_context_window_wins_over_model_limit() {
        let usage = extract_usage(&json!({
            "_meta": {
                "quota": {
                    "token_count": {
                        "input_tokens": 4,
                        "output_tokens": 1
                    }
                },
                "context": {
                    "modelContextWindow": 123_456
                }
            }
        }));
        let event = gemini_context_usage_native_event(
            &json!({
                "_meta": {
                    "context": {
                        "modelContextWindow": 123_456
                    }
                }
            }),
            &usage,
            Some("gemini-2.5-pro"),
        )
        .expect("context usage event");

        match event {
            ExecutorEvent::NativeEvent { payload, .. } => {
                assert_eq!(
                    payload.get("max_tokens").and_then(Value::as_u64),
                    Some(123_456)
                );
                assert_eq!(
                    payload.get("raw_max_tokens").and_then(Value::as_u64),
                    Some(123_456)
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
