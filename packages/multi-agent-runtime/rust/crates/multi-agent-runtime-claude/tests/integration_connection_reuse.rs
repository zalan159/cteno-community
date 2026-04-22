//! End-to-end integration test for the `AgentExecutor` connection-reuse seam
//! on the Claude vendor adapter.
//!
//! **Empirical invariant** confirmed in Phase A (see
//! `docs/claude-p1-protocol-findings.md`): the `claude` CLI ignores inbound
//! `session_id` on user frames and enforces a single session per subprocess.
//! Therefore `start_session_on` must spawn a fresh subprocess for every
//! session — this test locks in that behavior.
//!
//! Env-gated (`CLAUDE_PATH`) so CI can skip when the binary is absent. Also
//! requires a reachable claude account; unset `CLAUDE_PATH` to skip silently.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use multi_agent_runtime_claude::ClaudeAgentExecutor;
use multi_agent_runtime_core::{
    AgentExecutor, ConnectionHealth, ConnectionSpec, NativeMessage, NativeSessionId, Pagination,
    PermissionMode, SessionFilter, SessionInfo, SessionMeta, SessionRecord, SessionStoreProvider,
    SpawnSessionSpec,
};
use serde_json::Value;

struct NoopStore;

#[async_trait]
impl SessionStoreProvider for NoopStore {
    async fn record_session(&self, _vendor: &str, _session: SessionRecord) -> Result<(), String> {
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
        Err("not implemented".to_string())
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

fn claude_path_from_env() -> Option<PathBuf> {
    std::env::var("CLAUDE_PATH").ok().map(PathBuf::from)
}

fn make_spec(workdir: PathBuf) -> SpawnSessionSpec {
    SpawnSessionSpec {
        workdir,
        system_prompt: None,
        model: None,
        permission_mode: PermissionMode::Default,
        allowed_tools: None,
        additional_directories: Vec::new(),
        env: BTreeMap::new(),
        agent_config: Value::Null,
        resume_hint: None,
    }
}

#[tokio::test]
#[ignore = "requires CLAUDE_PATH env + working claude CLI"]
async fn connection_reuse_end_to_end() {
    let Some(claude_path) = claude_path_from_env() else {
        eprintln!("[skip] CLAUDE_PATH not set");
        return;
    };
    let executor = Arc::new(
        ClaudeAgentExecutor::new(claude_path, Arc::new(NoopStore))
            .with_spawn_ready_timeout(Duration::from_secs(30))
            .with_turn_timeout(Duration::from_secs(300)),
    );

    // Open a connection (version probe + cached inner state).
    let handle = executor
        .open_connection(ConnectionSpec::default())
        .await
        .expect("open_connection should succeed on healthy CLI");

    // Health probe should return Healthy.
    let health = executor.check_connection(&handle).await.unwrap();
    assert_eq!(health, ConnectionHealth::Healthy);

    // Start two sessions — same workdir, same defaults. Per the Phase A
    // empirical invariant, these MUST end up as distinct subprocesses.
    let temp_a = std::env::temp_dir().join(format!("claude-int-a-{}", uuid::Uuid::new_v4()));
    let temp_b = std::env::temp_dir().join(format!("claude-int-b-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_a).unwrap();
    std::fs::create_dir_all(&temp_b).unwrap();

    let s_a = executor
        .start_session_on(&handle, make_spec(temp_a.clone()))
        .await
        .expect("start_session_on(sessA) should succeed");
    let s_b = executor
        .start_session_on(&handle, make_spec(temp_b.clone()))
        .await
        .expect("start_session_on(sessB) should succeed");

    assert_ne!(
        s_a.process_handle, s_b.process_handle,
        "two sessions must have two distinct subprocess tokens"
    );
    assert_ne!(
        s_a.id, s_b.id,
        "the CLI should mint distinct native session ids for two subprocesses"
    );

    // Third session with different workdir — still must spawn fresh.
    let temp_c = std::env::temp_dir().join(format!("claude-int-c-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_c).unwrap();
    let s_c = executor
        .start_session_on(&handle, make_spec(temp_c.clone()))
        .await
        .expect("start_session_on(sessC) should succeed");
    assert_ne!(s_c.process_handle, s_a.process_handle);
    assert_ne!(s_c.process_handle, s_b.process_handle);

    // Clean up.
    executor.close_session(&s_a).await.ok();
    executor.close_session(&s_b).await.ok();
    executor.close_session(&s_c).await.ok();
    executor.close_connection(handle).await.ok();

    for d in [&temp_a, &temp_b, &temp_c] {
        let _ = std::fs::remove_dir_all(d);
    }
}
