//! Executor-driven helpers shared by desktop session entrypoints.
//!
//! Desktop session execution is executor-only now. Local happy sessions,
//! restored sessions, workspace role turns, and `agent.execute` all resolve an
//! [`AgentExecutor`] from [`ExecutorRegistry`] and stream turns through
//! [`ExecutorNormalizer`].
//!
//! This module keeps the low-level one-turn driving pattern explicit:
//!
//! ```text
//!   registry.resolve(vendor)?          // Arc<dyn AgentExecutor>
//!     .spawn_session(spec)?            // SessionRef
//!     .send_message(msg)?              // EventStream
//!   while let Some(event) = stream.next().await {
//!     normalizer.process_event(event?).await?;
//!   }
//! ```

use std::sync::Arc;

use futures_util::StreamExt;
use multi_agent_runtime_core::{
    AgentExecutor, PermissionMode, SessionRef, SpawnSessionSpec, UserMessage,
};

use crate::executor_normalizer::{
    surface_executor_failure, user_visible_executor_error, ExecutorNormalizer,
};
use crate::executor_registry::ExecutorRegistry;
use crate::happy_client::permission::PermissionHandler;
use crate::happy_client::socket::HappySocket;
use crate::session_message_codec::SessionMessageCodec;

fn ensure_local_session_row(
    db_path: &std::path::Path,
    session_id: &str,
    vendor: &str,
) -> Result<(), String> {
    let manager = crate::agent_session::AgentSessionManager::new(db_path.to_path_buf());
    match manager.get_session(session_id)? {
        Some(session) if session.vendor != vendor => manager.set_vendor(session_id, vendor),
        Some(_) => Ok(()),
        None => manager
            .create_session_with_id_and_vendor(session_id, "worker", None, None, vendor)
            .map(|_| ()),
    }
}

async fn send_persisted_acp(
    socket: &HappySocket,
    session_id: &str,
    message_codec: &SessionMessageCodec,
    data: serde_json::Value,
) -> Result<(), String> {
    let message = serde_json::json!({
        "role": "agent",
        "content": {
            "type": "acp",
            "provider": "cteno",
            "data": data,
        },
        "meta": {
            "sentFrom": "cli",
        },
    });
    let message_json = serde_json::to_string(&message)
        .map_err(|e| format!("Failed to serialize ACP message: {e}"))?;
    let outbound_message = message_codec
        .encode_payload(message_json.as_bytes())
        .map_err(|e| format!("Failed to encode ACP message: {e}"))?;
    socket
        .send_message(session_id, &outbound_message, None)
        .await
}

async fn emit_spawn_failure(
    session_id: &str,
    vendor: &str,
    socket: &HappySocket,
    message_codec: &SessionMessageCodec,
    db_path: &std::path::Path,
    task_id: &str,
    reason: &str,
) -> Result<(), String> {
    if socket.is_local() {
        ensure_local_session_row(db_path, session_id, vendor)?;
    }

    send_persisted_acp(
        socket,
        session_id,
        message_codec,
        serde_json::json!({
            "type": "error",
            "message": format!("会话启动失败：{reason}"),
            "recoverable": false,
        }),
    )
    .await?;
    send_persisted_acp(
        socket,
        session_id,
        message_codec,
        serde_json::json!({
            "type": "task_complete",
            "id": task_id,
        }),
    )
    .await
}

/// Run one turn of an executor-driven session end-to-end.
///
/// Resolves the vendor adapter, spawns a session (one subprocess per call in
/// the current adapter topology), pushes the user message, and normalises
/// the resulting event stream into ACP messages on `socket`.
///
/// Returns the final assistant text when the turn completes with
/// `TurnComplete { final_text }`, `None` otherwise.
#[allow(clippy::too_many_arguments)]
pub async fn run_one_turn(
    registry: Arc<ExecutorRegistry>,
    vendor: &str,
    spec: SpawnSessionSpec,
    user_message: UserMessage,
    session_id: String,
    socket: Arc<HappySocket>,
    message_codec: SessionMessageCodec,
    permission_handler: Arc<PermissionHandler>,
    task_id: String,
    server_url: String,
    auth_token: String,
    db_path: std::path::PathBuf,
) -> Result<Option<SessionRef>, String> {
    let executor = registry.resolve(vendor)?;
    let vendor_key: Option<&'static str> = match vendor {
        "cteno" => Some("cteno"),
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "gemini" => Some("gemini"),
        _ => None,
    };
    let spawn_result = if let Some(vendor_key) = vendor_key {
        // Registry helper health-checks the cached handle and retries once
        // on a "connection is closed"-class error; keeps the legacy
        // spawn_session fallback for vendors/paths the registry cannot
        // service.
        match registry
            .start_session_with_autoreopen(vendor_key, spec.clone())
            .await
        {
            Ok(session) => Ok(session),
            Err(err) => {
                log::warn!(
                    "run_one_turn: start_session_with_autoreopen({vendor}) failed: {err} — falling back to spawn_session"
                );
                executor.spawn_session(spec).await
            }
        }
    } else {
        executor.spawn_session(spec).await
    };
    let session_ref = match spawn_result {
        Ok(session_ref) => session_ref,
        Err(error) => {
            let reason = user_visible_executor_error(&error);
            emit_spawn_failure(
                &session_id,
                vendor,
                &socket,
                &message_codec,
                &db_path,
                &task_id,
                &reason,
            )
            .await?;
            return Err(format!("spawn_session({vendor}) failed: {reason}"));
        }
    };

    let normalizer = ExecutorNormalizer::new(
        session_id,
        socket,
        message_codec,
        None,
        permission_handler,
        task_id,
        executor.clone(),
        session_ref.clone(),
        server_url,
        auth_token,
        db_path,
        None,
        None,
    );

    // Persist the user turn locally before the vendor swallows it; the
    // event stream below only carries assistant/tool output.
    normalizer
        .persist_user_message(&user_message.content, None)
        .map_err(|e| format!("persist user message failed: {e}"))?;

    let mut stream = match executor.send_message(&session_ref, user_message).await {
        Ok(stream) => stream,
        Err(error) => {
            surface_executor_failure(&normalizer, &error).await?;
            return Ok(Some(session_ref));
        }
    };

    while let Some(event) = stream.next().await {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                surface_executor_failure(&normalizer, &error).await?;
                return Ok(Some(session_ref));
            }
        };
        let done = normalizer.process_event(event).await?;
        if done {
            break;
        }
    }

    Ok(Some(session_ref))
}

/// Convenience helper — build a minimally-configured [`SpawnSessionSpec`]
/// from scalar inputs. Callers that need fine-grained control (injected
/// tools, custom env, resume hints) should construct the spec directly.
///
/// Pulls the current access token from [`crate::auth_store_boot`] and packs
/// it into `agent_config["auth"]`; the Cteno adapter lifts those fields onto
/// the stdio `Init` frame during spawn. Non-logged-in sessions simply emit
/// no `auth` block.
pub fn default_spawn_spec(
    workdir: std::path::PathBuf,
    system_prompt: Option<String>,
    permission_mode: PermissionMode,
) -> SpawnSessionSpec {
    SpawnSessionSpec {
        workdir,
        system_prompt,
        model: None,
        permission_mode,
        allowed_tools: None,
        additional_directories: Vec::new(),
        env: std::collections::BTreeMap::new(),
        agent_config: default_agent_config_with_auth(),
        resume_hint: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::happy_client::socket::LocalEventSink;
    use cteno_host_session_wire::ConnectionType;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingLocalSink {
        persisted: Mutex<Vec<String>>,
    }

    impl RecordingLocalSink {
        fn persisted_messages(&self) -> Vec<Value> {
            self.persisted
                .lock()
                .unwrap()
                .iter()
                .map(|raw| serde_json::from_str(raw).unwrap())
                .collect()
        }
    }

    impl LocalEventSink for RecordingLocalSink {
        fn on_message(&self, _session_id: &str, encrypted_message: &str, _local_id: Option<&str>) {
            self.persisted
                .lock()
                .unwrap()
                .push(encrypted_message.to_string());
        }

        fn on_transient_message(&self, _session_id: &str, _encrypted_message: &str) {}

        fn on_state_update(
            &self,
            _session_id: &str,
            _encrypted_state: Option<&str>,
            _version: u32,
        ) {
        }

        fn on_metadata_update(&self, _session_id: &str, _encrypted_metadata: &str, _version: u32) {}
    }

    #[tokio::test]
    async fn emit_spawn_failure_persists_readable_error_and_task_complete() {
        let temp = tempfile::tempdir().unwrap();
        crate::db::init_at_data_dir(temp.path()).unwrap();
        let db_path = temp.path().join("db").join("cteno.db");

        let session_id = "spawn-failure-session";
        let task_id = "task-spawn-failure";
        let sink = Arc::new(RecordingLocalSink::default());
        let socket = Arc::new(HappySocket::local(ConnectionType::SessionScoped {
            session_id: session_id.to_string(),
        }));
        socket.install_local_sink(sink.clone());

        emit_spawn_failure(
            session_id,
            "cteno",
            socket.as_ref(),
            &SessionMessageCodec::plaintext(),
            &db_path,
            task_id,
            "timeout after 30s: spawn_session",
        )
        .await
        .unwrap();

        let persisted = sink.persisted_messages();
        assert_eq!(persisted.len(), 2);
        assert_eq!(persisted[0]["content"]["data"]["type"], "error");
        assert_eq!(
            persisted[0]["content"]["data"]["message"],
            "会话启动失败：timeout after 30s: spawn_session"
        );
        assert_eq!(persisted[0]["content"]["data"]["recoverable"], false);
        assert_eq!(persisted[1]["content"]["data"]["type"], "task_complete");
        assert_eq!(persisted[1]["content"]["data"]["id"], task_id);
    }
}

/// Build the baseline `agent_config` JSON object, pre-populated with the
/// current auth snapshot (when logged in). Downstream callers that want to
/// add their own keys should pass the result through `merge_auth_into`.
pub fn default_agent_config_with_auth() -> serde_json::Value {
    let mut cfg = serde_json::json!({});
    merge_auth_into(&mut cfg);
    cfg
}

/// Fold the current access token / user / machine id into an `agent_config`
/// JSON object under the `auth` key. No-op when not logged in. The Cteno
/// adapter knows to strip this key before forwarding the rest to
/// `cteno-agent`.
pub fn merge_auth_into(agent_config: &mut serde_json::Value) {
    use serde_json::Value;
    let Some(store) = crate::auth_store_boot::auth_store() else {
        return;
    };
    let snap = store.snapshot();
    if snap.access_token.is_none() {
        return;
    }
    let auth_obj = serde_json::json!({
        "accessToken": snap.access_token,
        "userId": snap.user_id,
        "machineId": snap.machine_id,
    });
    match agent_config {
        Value::Object(map) => {
            map.insert("auth".to_string(), auth_obj);
        }
        _ => {
            // Callers that pass `Null` / a non-object get a fresh object with
            // just the auth block. This keeps the invariant that `agent_config`
            // reaching the adapter is always an object when auth is present.
            *agent_config = serde_json::json!({ "auth": auth_obj });
        }
    }
}

/// Best-effort session teardown. Logs but does not propagate failures.
pub async fn close_session_best_effort(executor: Arc<dyn AgentExecutor>, session: &SessionRef) {
    if let Err(e) = executor.close_session(session).await {
        log::warn!("executor.close_session({}) failed: {e}", session.vendor);
    }
}
