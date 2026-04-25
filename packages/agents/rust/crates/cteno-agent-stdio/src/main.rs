//! `cteno-agent` — stdio-driven Cteno session runner.
//!
//! Architecture: speaks a line-delimited JSON protocol on stdin/stdout. This
//! binary links `cteno-agent-runtime` directly and routes inbound messages
//! into the runtime's autonomous agent loop, translating streamed events
//! back onto stdout. stderr is reserved for `log::*` diagnostics.
//!
//! A single agent process supports multiple concurrent sessions (the main
//! loop dispatches inbound messages to a `HashMap<session_id, SessionHandle>`).

mod auth;
mod hooks_mvp;
mod host_call_dispatcher;
mod injected_tool;
mod io;
mod pending;
mod protocol;
mod runner;
mod session;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use tokio::sync::mpsc;

use crate::auth::{apply_init_auth, apply_token_refresh, AuthSlot, StdioCredentialsProvider};
use crate::host_call_dispatcher::StdioHostCallDispatcher;
use crate::injected_tool::inject_tool;
use crate::io::{spawn_stdin_reader, OutboundWriter};
use crate::pending::{
    new_pending_host_calls, new_pending_permissions, new_pending_tool_execs, parse_decision,
    PendingPermissions,
};
use crate::protocol::{Inbound, Outbound};
use crate::session::{SessionHandle, SessionState};
use cteno_agent_runtime::agent_queue::AgentMessage;

#[derive(Debug)]
enum InternalEvent {
    TurnFinished { session_id: String },
    SubagentMessage { session_id: String, content: String },
}

fn data_dir() -> PathBuf {
    if let Ok(v) = std::env::var("CTENO_AGENT_DATA_DIR") {
        return PathBuf::from(v);
    }
    if let Ok(v) = std::env::var("CTENO_APP_DATA_DIR") {
        return PathBuf::from(v);
    }
    let base = dirs_next_home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".cteno").join("agent-stdio")
}

fn dirs_next_home_dir() -> Option<PathBuf> {
    // Reuse std env for portability; dirs crate not a dependency here.
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env<T>(agent_dir: Option<&str>, app_dir: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_agent = std::env::var_os("CTENO_AGENT_DATA_DIR");
        let old_app = std::env::var_os("CTENO_APP_DATA_DIR");

        match agent_dir {
            Some(value) => std::env::set_var("CTENO_AGENT_DATA_DIR", value),
            None => std::env::remove_var("CTENO_AGENT_DATA_DIR"),
        }
        match app_dir {
            Some(value) => std::env::set_var("CTENO_APP_DATA_DIR", value),
            None => std::env::remove_var("CTENO_APP_DATA_DIR"),
        }

        let output = f();

        match old_agent {
            Some(value) => std::env::set_var("CTENO_AGENT_DATA_DIR", value),
            None => std::env::remove_var("CTENO_AGENT_DATA_DIR"),
        }
        match old_app {
            Some(value) => std::env::set_var("CTENO_APP_DATA_DIR", value),
            None => std::env::remove_var("CTENO_APP_DATA_DIR"),
        }

        output
    }

    #[test]
    fn data_dir_prefers_explicit_agent_dir() {
        let path = with_env(
            Some("/tmp/cteno-agent-explicit"),
            Some("/tmp/cteno-app-data"),
            data_dir,
        );
        assert_eq!(path, PathBuf::from("/tmp/cteno-agent-explicit"));
    }

    #[test]
    fn data_dir_falls_back_to_host_app_data_dir() {
        let path = with_env(None, Some("/tmp/cteno-app-data"), data_dir);
        assert_eq!(path, PathBuf::from("/tmp/cteno-app-data"));
    }

}

/// Rehydrate `profile_id` / `model` / `effort` into `agent_config` from the
/// agent_sessions row when an Init arrives with `resume_session_id` but no
/// profile selection. This is the resume-after-respawn fix: the Cteno vendor
/// adapter strips profile info from the resume Init frame, and without this
/// restore the first turn would re-resolve to whatever local "default"
/// profile happens to be on disk (typically a non-proxy, empty-api-key
/// placeholder that errors with "请先登录").
///
/// Cteno-only fix: lives in cteno-agent-stdio per the CLAUDE.md rule that
/// vendor-specific bugs are fixed at the vendor layer, not in the shared
/// scheduler.
fn restore_resume_profile_from_db(
    db_path: &std::path::Path,
    session_id: &str,
    agent_config: &mut serde_json::Value,
) {
    let Some(obj) = agent_config.as_object() else {
        return;
    };
    // Only rehydrate on a resume init that's missing profile selection.
    let is_resume = obj.contains_key("resume_session_id");
    let has_profile = obj.get("profile_id").and_then(|v| v.as_str()).is_some();
    if !is_resume || has_profile {
        return;
    }

    let mgr = cteno_agent_runtime::agent_session::AgentSessionManager::new(db_path.to_path_buf());
    let Ok(Some(row)) = mgr.get_session(session_id) else {
        return;
    };
    let Some(context) = row.context_data.as_ref() else {
        return;
    };
    let Some(stored) = context.get("cteno_profile") else {
        return;
    };
    let Some(stored_obj) = stored.as_object() else {
        return;
    };

    let cfg_obj = agent_config
        .as_object_mut()
        .expect("checked is_object above");
    if let Some(pid) = stored_obj.get("profile_id").and_then(|v| v.as_str()) {
        cfg_obj.insert(
            "profile_id".to_string(),
            serde_json::Value::String(pid.to_string()),
        );
    }
    if let Some(model) = stored_obj.get("model") {
        cfg_obj.insert("model".to_string(), model.clone());
    }
    if let Some(effort) = stored_obj.get("effort").and_then(|v| v.as_str()) {
        cfg_obj.insert(
            "effort".to_string(),
            serde_json::Value::String(effort.to_string()),
        );
    }
    log::info!("rehydrated profile selection for resumed session {session_id} from sessions.db");
}

/// Persist the current profile selection back into the agent_sessions row so
/// a future resume can rehydrate it. No-op if the session row does not yet
/// exist (run_turn's `create_session_with_id` handles that path separately).
fn persist_profile_to_db(
    db_path: &std::path::Path,
    session_id: &str,
    agent_config: &serde_json::Value,
) {
    let Some(obj) = agent_config.as_object() else {
        return;
    };

    let mut stored = serde_json::Map::new();
    if let Some(pid) = obj.get("profile_id").and_then(|v| v.as_str()) {
        stored.insert(
            "profile_id".to_string(),
            serde_json::Value::String(pid.to_string()),
        );
    }
    if let Some(model) = obj.get("model") {
        stored.insert("model".to_string(), model.clone());
    }
    if let Some(effort) = obj.get("effort").and_then(|v| v.as_str()) {
        stored.insert(
            "effort".to_string(),
            serde_json::Value::String(effort.to_string()),
        );
    }
    if stored.is_empty() {
        return;
    }

    let mgr = cteno_agent_runtime::agent_session::AgentSessionManager::new(db_path.to_path_buf());
    if let Err(err) = mgr.update_context_field(
        session_id,
        "cteno_profile",
        serde_json::Value::Object(stored),
    ) {
        log::warn!("failed to persist cteno_profile for session {session_id}: {err}");
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // env_logger writes to stderr, which is exactly what we want.
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .try_init();

    let data_dir = data_dir();
    std::fs::create_dir_all(&data_dir).ok();
    log::info!("cteno-agent data dir: {:?}", data_dir);

    let writer = OutboundWriter::new();
    let mut rx = spawn_stdin_reader(writer.clone());

    // Shared pending-request maps. Keyed by request_id (globally unique), not
    // by session_id: each request_id has a dedicated oneshot sender.
    let pending_permissions = new_pending_permissions();
    let pending_tool_execs = new_pending_tool_execs();
    let pending_host_calls = new_pending_host_calls();

    // Register default tool registry + URL provider. We keep a direct handle
    // on the registry so we can dynamically add tools via `tool_inject`.
    let installed = hooks_mvp::install_default_registry(
        data_dir.clone(),
        writer.clone(),
        pending_permissions.clone(),
    );
    log::info!(
        "cteno-agent stdio bootstrap complete: {} builtin tools registered",
        installed.builtin_count
    );
    let registry = installed.handle;
    let mcp_registries = installed.mcp_registries;
    let subagent_bootstrap = installed.subagent_bootstrap;

    let db_path = data_dir.join("sessions.db");

    // Install the generic HostCallDispatcher into the runtime. In-runtime hook
    // impls (added by later waves) can look up this dispatcher and proxy their
    // calls across the stdio boundary via `host_call_request`.
    cteno_agent_runtime::hooks::install_host_call(Arc::new(StdioHostCallDispatcher::new(
        writer.clone(),
        pending_host_calls.clone(),
    )));

    // Process-wide auth slot: single accessToken shared by every session this
    // agent process hosts. Seeded by the first Init.auth_token, rotated by
    // Inbound::TokenRefreshed. The CredentialsProvider hook reads through to
    // this slot.
    let auth_slot: Arc<RwLock<AuthSlot>> = Arc::new(RwLock::new(AuthSlot::default()));
    cteno_agent_runtime::hooks::install_credentials(Arc::new(StdioCredentialsProvider::new(
        auth_slot.clone(),
    )));

    let (internal_tx, mut internal_rx) = mpsc::unbounded_channel::<InternalEvent>();
    let mut sessions: HashMap<String, SessionHandle> = HashMap::new();

    loop {
        let msg = tokio::select! {
            msg = rx.recv() => {
                let Some(msg) = msg else {
                    break;
                };
                Some(msg)
            }
            event = internal_rx.recv() => {
                let Some(event) = event else {
                    continue;
                };
                match event {
                    InternalEvent::TurnFinished { session_id } => {
                        if let Some(handle) = sessions.get_mut(&session_id) {
                            handle.harvest_finished();
                            // A turn just ended — if SubagentMessage(s) had
                            // arrived during it and were pushed onto the
                            // queue (we deferred wake while the turn was
                            // active), drain them now and start an
                            // autonomous turn so persona reacts to the
                            // queued handoffs. Without this, queue content
                            // accumulated mid-turn would sit forever until
                            // the next user message.
                            try_drain_and_wake(
                                handle,
                                &session_id,
                                "subagent_handoff_after_turn",
                                &writer,
                                &pending_permissions,
                                &internal_tx,
                            )
                            .await;
                        }
                    }
                    InternalEvent::SubagentMessage { session_id, content } => {
                        let Some(handle) = sessions.get_mut(&session_id) else {
                            log::warn!(
                                "SubAgent notification for unknown session {session_id} (dropping)"
                            );
                            continue;
                        };

                        let push_result = handle
                            .message_queue
                            .push(AgentMessage::subagent(session_id.clone(), content));
                        if let Err(err) = push_result {
                            log::warn!(
                                "failed to enqueue SubAgent notification for {session_id}: {err}"
                            );
                            continue;
                        }

                        handle.harvest_finished();
                        if handle.turn_in_progress() {
                            // Turn-active path: leave the message in the
                            // queue. The TurnFinished handler will drain and
                            // wake when the active turn ends. (The active
                            // turn's ReAct loop's `pop_all` only fires at
                            // the END of an iteration that ran tools — if
                            // the LLM ends the turn with a final assistant
                            // text and no tool call, pop_all is skipped.
                            // Hence the TurnFinished safety net.)
                            log::info!(
                                "session {session_id} has an in-progress turn; queued for post-turn wake"
                            );
                            continue;
                        }

                        try_drain_and_wake(
                            handle,
                            &session_id,
                            "subagent_handoff",
                            &writer,
                            &pending_permissions,
                            &internal_tx,
                        )
                        .await;
                    }
                }
                None
            }
        };

        let Some(msg) = msg else {
            continue;
        };

        match msg {
            Inbound::Init {
                session_id,
                workdir,
                additional_directories,
                mut agent_config,
                system_prompt,
                auth_token,
                user_id,
                machine_id,
            } => {
                // Fold any non-None auth fields into the shared slot before
                // the session spawns so the first hook call sees the right
                // credentials. Empty fields preserve prior values.
                apply_init_auth(&auth_slot, auth_token, user_id, machine_id);

                // Resume restoration: the Cteno vendor adapter's
                // `resume_session` rebuilds a minimal `{"resume_session_id": ...}`
                // agent_config that drops profile_id / model / effort. Without
                // this, a subprocess respawned from resume falls back to the
                // default (empty-key) local profile and the first turn bails
                // with "请先登录". So: when resume_session_id is set AND
                // profile_id is missing, rehydrate profile_id (+ model, +
                // effort) from the agent_sessions row persisted by the last
                // SetModel or by the last run_turn's context-data writes.
                restore_resume_profile_from_db(&db_path, &session_id, &mut agent_config);

                match hooks_mvp::install_session_mcp_tools(
                    &registry,
                    &mcp_registries,
                    &session_id,
                    &data_dir,
                    workdir.as_deref(),
                )
                .await
                {
                    Ok(count) => {
                        if count > 0 {
                            log::info!("registered {count} MCP tools for session {session_id}");
                        }
                    }
                    Err(err) => {
                        log::warn!("failed to load MCP tools for session {session_id}: {err}");
                    }
                }

                // Replace-on-reinit: abort the prior stdio-side workers for
                // this id before installing the fresh session state.
                if let Some(mut old) = sessions.remove(&session_id) {
                    old.state.abort_flag.store(true, Ordering::SeqCst);
                    old.abort_running_turn();
                    old.abort_subagent_receiver();
                    cteno_agent_runtime::subagent::manager::global()
                        .unregister_session(&session_id)
                        .await;
                }

                let new_state = SessionState::new(
                    session_id.clone(),
                    workdir,
                    additional_directories,
                    agent_config,
                    system_prompt,
                    db_path.clone(),
                );
                subagent_bootstrap
                    .register_session(
                        session_id.clone(),
                        new_state.workdir.clone(),
                        new_state.additional_directories.clone(),
                        new_state.agent_config.clone(),
                    )
                    .await;

                let mut subagent_rx = cteno_agent_runtime::subagent::manager::global()
                    .register_session(session_id.clone())
                    .await;
                let subagent_session_id = session_id.clone();
                let subagent_internal_tx = internal_tx.clone();
                let subagent_receiver = tokio::spawn(async move {
                    log::info!(
                        "stdio SubAgent notification receiver started for session {subagent_session_id}"
                    );
                    while let Some(content) = subagent_rx.recv().await {
                        if subagent_internal_tx
                            .send(InternalEvent::SubagentMessage {
                                session_id: subagent_session_id.clone(),
                                content,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    log::info!(
                        "stdio SubAgent notification receiver stopped for session {subagent_session_id}"
                    );
                });

                let mut handle = SessionHandle::new(new_state);
                handle.subagent_receiver = Some(subagent_receiver);
                sessions.insert(session_id.clone(), handle);
                writer.send(Outbound::Ready { session_id }).await;
            }

            Inbound::UserMessage {
                session_id,
                content,
                task_id,
                attachments,
            } => {
                let handle = match sessions.get_mut(&session_id) {
                    Some(h) => h,
                    None => {
                        writer
                            .send(Outbound::Error {
                                session_id,
                                message: "unknown session_id; init must be sent first".to_string(),
                            })
                            .await;
                        continue;
                    }
                };

                handle.harvest_finished();
                if handle.turn_in_progress() {
                    log::warn!(
                        "user_message received while session {session_id} is busy; rejecting duplicate in-flight turn"
                    );
                    writer
                        .send(Outbound::Error {
                            session_id,
                            message: "session is busy; send the next user message after the current turn completes".to_string(),
                        })
                        .await;
                    continue;
                }

                start_turn(
                    handle,
                    content,
                    task_id,
                    attachments,
                    writer.clone(),
                    pending_permissions.clone(),
                    internal_tx.clone(),
                );
            }

            Inbound::Abort { session_id, reason } => {
                if let Some(handle) = sessions.get_mut(&session_id) {
                    handle.state.abort_flag.store(true, Ordering::SeqCst);
                    let aborted = handle.abort_running_turn();
                    log::info!("abort requested for session {session_id} (aborted={aborted})");
                    if aborted {
                        let message = reason.unwrap_or_else(|| {
                            "Turn aborted. You can retry when ready.".to_string()
                        });
                        writer
                            .send(Outbound::Error {
                                session_id: session_id.clone(),
                                message,
                            })
                            .await;
                        writer
                            .send(Outbound::TurnComplete {
                                session_id,
                                final_text: String::new(),
                                iteration_count: 0,
                                usage: Default::default(),
                                context_usage: None,
                            })
                            .await;
                    }
                } else {
                    log::warn!("abort for unknown session {session_id} (dropping)");
                }
            }

            Inbound::CloseSession { session_id } => {
                let removed = sessions.remove(&session_id);
                let existed = removed.is_some();
                if let Some(mut handle) = removed {
                    handle.state.abort_flag.store(true, Ordering::SeqCst);
                    handle.abort_running_turn();
                    handle.abort_subagent_receiver();
                }
                let cleaned =
                    hooks_mvp::cleanup_session_mcp_tools(&registry, &mcp_registries, &session_id)
                        .await;
                subagent_bootstrap.unregister_session(&session_id).await;
                cteno_agent_runtime::subagent::manager::global()
                    .unregister_session(&session_id)
                    .await;
                log::info!(
                    "closed session {session_id} in stdio runner (existed={} mcp_tools_cleaned={cleaned})",
                    existed
                );
            }

            Inbound::SetModel {
                session_id,
                model,
                effort,
            } => {
                if let Some(handle) = sessions.get_mut(&session_id) {
                    let app_data_dir = handle
                        .state
                        .db_path
                        .parent()
                        .map(std::path::Path::to_path_buf)
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    runner::apply_model_control(
                        &mut handle.state.agent_config,
                        model,
                        effort,
                        &app_data_dir,
                    );
                    subagent_bootstrap
                        .register_session(
                            session_id.clone(),
                            handle.state.workdir.clone(),
                            handle.state.additional_directories.clone(),
                            handle.state.agent_config.clone(),
                        )
                        .await;
                    // Persist the new profile_id / model so a later resume
                    // (subprocess respawn, daemon restart) can rehydrate the
                    // selection instead of falling back to the default local
                    // profile. See `restore_resume_profile_from_db` below.
                    persist_profile_to_db(&db_path, &session_id, &handle.state.agent_config);
                    log::info!("updated session model config for {session_id}");
                } else {
                    writer
                        .send(Outbound::Error {
                            session_id,
                            message: "set_model: unknown session_id; init must be sent first"
                                .to_string(),
                        })
                        .await;
                }
            }

            Inbound::SetPermissionMode { session_id, mode } => {
                if let Some(handle) = sessions.get_mut(&session_id) {
                    runner::apply_permission_mode_control(&mut handle.state.agent_config, mode);
                    subagent_bootstrap
                        .register_session(
                            session_id.clone(),
                            handle.state.workdir.clone(),
                            handle.state.additional_directories.clone(),
                            handle.state.agent_config.clone(),
                        )
                        .await;
                    log::info!("updated session permission mode for {session_id}");
                } else {
                    writer
                        .send(Outbound::Error {
                            session_id,
                            message:
                                "set_permission_mode: unknown session_id; init must be sent first"
                                    .to_string(),
                        })
                        .await;
                }
            }

            Inbound::PermissionResponse {
                session_id,
                request_id,
                decision,
                reason: _,
            } => {
                log::info!(
                    "[stdio PermissionResponse RECV] session={session_id} req={request_id} decision={decision}"
                );
                let parsed_decision = parse_decision(&decision);
                let taken = {
                    let mut guard = pending_permissions.lock().await;
                    guard.remove(&request_id)
                };
                match taken {
                    Some(tx) => {
                        log::info!(
                            "[stdio PermissionResponse DELIVER] session={session_id} req={request_id} (pending sender found)"
                        );
                        if tx.send(parsed_decision).is_err() {
                            log::warn!(
                                "permission_response: receiver for request_id={request_id} gone (session={session_id})"
                            );
                        }
                    }
                    None => {
                        log::warn!(
                            "[stdio PermissionResponse NO-PENDING] session={session_id} req={request_id} (pending_permissions map empty for this id)"
                        );
                        writer
                            .send(Outbound::Error {
                                session_id,
                                message: format!(
                                    "permission_response: no pending request for request_id={request_id}"
                                ),
                            })
                            .await;
                    }
                }
            }

            Inbound::ToolInject { session_id, tool } => {
                // Sessions share one injected tool surface: the host is
                // expected to register the same orchestration tool set once
                // per session, and replays are idempotent. We still validate
                // the session exists so the host cannot inject tools into a
                // never-initialised namespace.
                if !sessions.contains_key(&session_id) {
                    writer
                        .send(Outbound::Error {
                            session_id,
                            message: "tool_inject: unknown session_id; init must be sent first"
                                .to_string(),
                        })
                        .await;
                    continue;
                }
                inject_tool(&registry, tool, writer.clone(), pending_tool_execs.clone()).await;
            }

            Inbound::ToolExecutionResponse {
                session_id,
                request_id,
                ok,
                output,
                error,
            } => {
                let taken = {
                    let mut guard = pending_tool_execs.lock().await;
                    guard.remove(&request_id)
                };
                match taken {
                    Some(tx) => {
                        let result = if ok {
                            Ok(output.unwrap_or_default())
                        } else {
                            Err(error.unwrap_or_else(|| {
                                "host tool execution failed (no error message)".to_string()
                            }))
                        };
                        if tx.send(result).is_err() {
                            log::warn!(
                                "tool_execution_response: receiver for request_id={request_id} gone (session={session_id})"
                            );
                        }
                    }
                    None => {
                        writer
                            .send(Outbound::Error {
                                session_id,
                                message: format!(
                                    "tool_execution_response: no pending request for request_id={request_id}"
                                ),
                            })
                            .await;
                    }
                }
            }

            Inbound::HostCallResponse {
                session_id,
                request_id,
                ok,
                output,
                error,
            } => {
                let taken = {
                    let mut guard = pending_host_calls.lock().await;
                    guard.remove(&request_id)
                };
                match taken {
                    Some(tx) => {
                        let result = if ok {
                            Ok(output.unwrap_or(serde_json::Value::Null))
                        } else {
                            Err(error.unwrap_or_else(|| {
                                "host call failed (no error message)".to_string()
                            }))
                        };
                        if tx.send(result).is_err() {
                            log::warn!(
                                "host_call_response: receiver for request_id={request_id} gone (session={session_id})"
                            );
                        }
                    }
                    None => {
                        writer
                            .send(Outbound::Error {
                                session_id,
                                message: format!(
                                    "host_call_response: no pending request for request_id={request_id}"
                                ),
                            })
                            .await;
                    }
                }
            }

            Inbound::TokenRefreshed { access_token } => {
                apply_token_refresh(&auth_slot, access_token);
                log::info!("access token rotated");
            }

            Inbound::Unknown => {
                log::warn!(
                    "unknown inbound message type received (forward-compat drop); \
                     ignoring so newer protocol fields do not hard-fail this agent"
                );
            }
        }

        // Harvest finished turn handles across all sessions so
        // `turn_in_progress()` stays accurate. Normal progression is driven
        // by `InternalEvent::TurnFinished`; this is just best-effort cleanup
        // after inbound control frames.
        for h in sessions.values_mut() {
            h.harvest_finished();
        }
    }

    // Graceful shutdown: wait for any in-flight turn to finish.
    let mut handles = Vec::new();
    for (session_id, mut h) in sessions.drain() {
        hooks_mvp::cleanup_session_mcp_tools(&registry, &mcp_registries, &session_id).await;
        subagent_bootstrap.unregister_session(&session_id).await;
        cteno_agent_runtime::subagent::manager::global()
            .unregister_session(&session_id)
            .await;
        h.abort_subagent_receiver();
        if let Some(handle) = h.running_turn.take() {
            handles.push(handle);
        }
    }
    for handle in handles {
        let _ = handle.await;
    }

    Ok(())
}

// Convenience re-exports, though nothing external uses them right now.
#[allow(dead_code)]
fn _keep_channel_imports(_rx: mpsc::Receiver<Inbound>) {}

/// If the session is idle and has queued SubAgent handoffs, drain the queue,
/// emit the explicit `Outbound::AutonomousTurnStart` boundary frame (carrying
/// the synthetic user-message text the host renders as a user-bubble), and
/// kick a fresh turn via `start_turn`. No-op if the queue is empty or the
/// session is busy.
///
/// Called from both:
/// - `InternalEvent::SubagentMessage` when the message arrives at an idle
///   session (immediate wake)
/// - `InternalEvent::TurnFinished` when a turn just ended and queue may have
///   accumulated content during the active turn (post-turn wake — required
///   because `autonomous_agent`'s in-loop `pop_all` only fires after a tool
///   iteration; a turn that ends with a final assistant text never reaches
///   it, leaving queued messages stranded without this safety net).
async fn try_drain_and_wake(
    handle: &mut SessionHandle,
    session_id: &str,
    reason: &str,
    writer: &OutboundWriter,
    pending_permissions: &PendingPermissions,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
) {
    if handle.turn_in_progress() {
        return;
    }
    let queue_msgs = handle.message_queue.pop_all(session_id);
    if queue_msgs.is_empty() {
        return;
    }
    let combined = queue_msgs
        .into_iter()
        .map(|m| m.content)
        .collect::<Vec<_>>()
        .join("\n\n");
    log::info!(
        "auto-wake persona session {session_id} ({reason}): {} chars",
        combined.len()
    );
    writer
        .send(Outbound::AutonomousTurnStart {
            session_id: session_id.to_string(),
            reason: Some(reason.to_string()),
            synthetic_user_message: Some(combined.clone()),
        })
        .await;
    start_turn(
        handle,
        combined,
        None,
        Vec::new(),
        writer.clone(),
        pending_permissions.clone(),
        internal_tx.clone(),
    );
}

fn start_turn(
    handle: &mut SessionHandle,
    content: String,
    task_id: Option<String>,
    attachments: Vec<crate::protocol::Attachment>,
    writer: OutboundWriter,
    pending_permissions: PendingPermissions,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
) {
    handle.state.abort_flag.store(false, Ordering::SeqCst);
    let state = handle.state.clone();
    let session_id_for_event = state.session_id.clone();
    let writer_for_turn = writer.clone();
    let pending_for_turn = pending_permissions.clone();
    let message_queue = handle.message_queue.clone();
    handle.running_turn = Some(tokio::spawn(async move {
        runner::run_turn(
            &state,
            content,
            task_id,
            attachments,
            writer_for_turn,
            pending_for_turn,
            Some(message_queue),
        )
        .await;
        let _ = internal_tx.send(InternalEvent::TurnFinished {
            session_id: session_id_for_event,
        });
    }));
}
