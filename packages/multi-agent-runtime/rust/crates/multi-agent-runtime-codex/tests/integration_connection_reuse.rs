//! End-to-end integration test for the Codex connection-reuse refactor.
//!
//! This test is `#[ignore]` by default — it exercises a live `codex
//! app-server` binary and therefore requires:
//!
//!   * A working Codex CLI 0.120.0+ (app-server subcommand) pointed at
//!     via `CODEX_PATH=/abs/path/to/codex`.
//!   * The caller's ChatGPT credentials loaded (since `thread/start`
//!     prompts for them when persistExtendedHistory is set). If you do
//!     not want a live LLM call the shell turns below still exercise
//!     the protocol round-trip because Codex accepts a `turn/start` and
//!     streams `turn/completed{status:"failed"}` when no model is
//!     reachable — both are acceptable outcomes.
//!
//! To run: `CODEX_PATH=$(which codex) cargo test -p multi-agent-runtime-codex \
//!   --test integration_connection_reuse -- --ignored --nocapture`.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use multi_agent_runtime_codex::CodexAgentExecutor;
use multi_agent_runtime_core::executor::{
    AgentExecutor, ConnectionSpec, ExecutorEvent, NativeMessage, NativeSessionId, Pagination,
    PermissionMode, SessionFilter, SessionInfo, SessionMeta, SessionRecord, SessionStoreProvider,
    SpawnSessionSpec, UserMessage,
};
use tokio::sync::Mutex;

struct InMemoryStore {
    records: Mutex<Vec<(String, SessionRecord)>>,
}

impl InMemoryStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            records: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl SessionStoreProvider for InMemoryStore {
    async fn record_session(&self, vendor: &str, session: SessionRecord) -> Result<(), String> {
        self.records
            .lock()
            .await
            .push((vendor.to_string(), session));
        Ok(())
    }
    async fn list_sessions(
        &self,
        _vendor: &str,
        _filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, String> {
        Ok(Vec::new())
    }
    async fn get_session_info(
        &self,
        _vendor: &str,
        _session_id: &NativeSessionId,
    ) -> Result<SessionInfo, String> {
        Err("not implemented".into())
    }
    async fn get_session_messages(
        &self,
        _vendor: &str,
        _session_id: &NativeSessionId,
        _pagination: Pagination,
    ) -> Result<Vec<NativeMessage>, String> {
        Ok(Vec::new())
    }
}

fn codex_path_from_env() -> Option<PathBuf> {
    std::env::var_os("CODEX_PATH").map(PathBuf::from)
}

fn spawn_spec(workdir: PathBuf) -> SpawnSessionSpec {
    SpawnSessionSpec {
        workdir,
        system_prompt: None,
        model: None,
        permission_mode: PermissionMode::WorkspaceWrite,
        allowed_tools: None,
        additional_directories: Vec::new(),
        env: BTreeMap::new(),
        agent_config: serde_json::Value::Null,
        resume_hint: None,
    }
}

#[tokio::test]
#[ignore = "live: requires CODEX_PATH env pointing at a working codex binary"]
async fn live_two_threads_share_one_app_server_then_interrupt_a_completes_b() {
    let Some(codex_path) = codex_path_from_env() else {
        eprintln!("skipping: CODEX_PATH not set");
        return;
    };

    let root = std::env::temp_dir().join(format!("codex-live-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let w1 = root.join("w1");
    let w2 = root.join("w2");
    std::fs::create_dir_all(&w1).unwrap();
    std::fs::create_dir_all(&w2).unwrap();

    let store = InMemoryStore::new();
    let executor = CodexAgentExecutor::new(codex_path, store);

    let handle = executor
        .open_connection(ConnectionSpec::default())
        .await
        .expect("open_connection should succeed against a live codex binary");

    let sess_a = executor
        .start_session_on(&handle, spawn_spec(w1))
        .await
        .expect("start_session_on A");
    let sess_b = executor
        .start_session_on(&handle, spawn_spec(w2))
        .await
        .expect("start_session_on B");

    assert_ne!(sess_a.id.as_str(), sess_b.id.as_str());

    // Session A — send a long-running counting request.
    let stream_a = executor
        .send_message(
            &sess_a,
            UserMessage {
                content:
                    "Count slowly from 1 to 50 with a short pause. Don't stop until you reach 50."
                        .to_string(),
                attachments: Vec::new(),
                parent_tool_use_id: None,
                injected_tools: Vec::new(),
            },
        )
        .await
        .expect("A send_message");

    // In parallel, session B sends a quick prompt.
    let executor_clone = Arc::new(executor);
    let sess_b_clone = sess_b.clone();
    let exec_for_b = executor_clone.clone();
    let b_task = tokio::spawn(async move {
        let stream = exec_for_b
            .send_message(
                &sess_b_clone,
                UserMessage {
                    content: "Say 'hello from B' and stop.".to_string(),
                    attachments: Vec::new(),
                    parent_tool_use_id: None,
                    injected_tools: Vec::new(),
                },
            )
            .await
            .expect("B send_message");
        let mut stream = Box::pin(stream);
        let mut saw_complete = false;
        while let Some(event) = stream.next().await {
            if let Ok(ExecutorEvent::TurnComplete { .. }) = event {
                saw_complete = true;
                break;
            }
        }
        saw_complete
    });

    // Let session A stream a few events then interrupt it.
    {
        let mut stream = Box::pin(stream_a);
        let mut event_count = 0;
        while let Some(_event) = stream.next().await {
            event_count += 1;
            if event_count >= 2 {
                break;
            }
        }
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    executor_clone
        .interrupt(&sess_a)
        .await
        .expect("interrupt A");

    // B should complete regardless of A's interrupt.
    let b_complete = tokio::time::timeout(Duration::from_secs(60), b_task)
        .await
        .expect("B did not complete within 60s")
        .expect("B join");
    assert!(b_complete, "session B did not reach TurnComplete");

    executor_clone
        .close_connection(handle)
        .await
        .expect("close_connection");
    let _ = std::fs::remove_dir_all(&root);
}
