//! Integration tests for the Phase 1 connection-reuse seam.
//!
//! These tests spawn a **real** `cteno-agent` subprocess and drive it via the
//! `CtenoConnection` / trait API. The binary is expected to exist at
//! `CTENO_AGENT_PATH` or, failing that, at
//! `../../../agents/rust/crates/cteno-agent-stdio/target/debug/cteno-agent`
//! relative to this crate. If neither is found the tests are skipped.
//!
//! The tests focus on cheap, deterministic protocol behaviour — no LLM is
//! invoked, so turn-level flows (`send_message` / `respond_to_permission`)
//! are exercised only through the handshake path.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use multi_agent_runtime_core::executor::{ConnectionHealth, ConnectionSpec};
use multi_agent_runtime_core::{
    AgentExecutor, NativeMessage, NativeSessionId, Pagination, PermissionMode, SessionFilter,
    SessionInfo, SessionMeta, SessionRecord, SessionStoreProvider, SpawnSessionSpec,
};
use multi_agent_runtime_cteno::CtenoAgentExecutor;

#[derive(Default)]
struct InMemoryStore;

#[async_trait]
impl SessionStoreProvider for InMemoryStore {
    async fn record_session(&self, _vendor: &str, _record: SessionRecord) -> Result<(), String> {
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
        _id: &NativeSessionId,
    ) -> Result<SessionInfo, String> {
        Err("not implemented".to_string())
    }
    async fn get_session_messages(
        &self,
        _vendor: &str,
        _id: &NativeSessionId,
        _pagination: Pagination,
    ) -> Result<Vec<NativeMessage>, String> {
        Ok(Vec::new())
    }
}

fn locate_cteno_agent() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CTENO_AGENT_PATH") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }
    // Walk up from the crate manifest looking for the repo root (the one
    // that contains both `packages/` and `apps/`). Works regardless of how
    // many workspace layers there are between the test crate and the repo.
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for _ in 0..10 {
        let candidate =
            dir.join("packages/agents/rust/crates/cteno-agent-stdio/target/debug/cteno-agent");
        if candidate.exists() {
            return Some(candidate);
        }
        let candidate_release =
            dir.join("packages/agents/rust/crates/cteno-agent-stdio/target/release/cteno-agent");
        if candidate_release.exists() {
            return Some(candidate_release);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn make_executor() -> Option<CtenoAgentExecutor> {
    let binary = locate_cteno_agent()?;
    eprintln!(
        "integration_connection_reuse: using cteno-agent at {}",
        binary.display()
    );
    let store: Arc<dyn SessionStoreProvider> = Arc::new(InMemoryStore);
    Some(CtenoAgentExecutor::new(binary, store).with_spawn_ready_timeout(Duration::from_secs(5)))
}

fn make_spawn_spec(workdir: PathBuf) -> SpawnSessionSpec {
    SpawnSessionSpec {
        workdir,
        system_prompt: None,
        model: None,
        permission_mode: PermissionMode::Default,
        allowed_tools: None,
        additional_directories: Vec::new(),
        env: Default::default(),
        agent_config: serde_json::json!({}),
        resume_hint: None,
    }
}

#[tokio::test]
async fn open_connection_spawns_subprocess_without_sending_init() {
    let Some(exec) = make_executor() else {
        eprintln!("SKIP: cteno-agent binary not found");
        return;
    };
    let handle = exec
        .open_connection(ConnectionSpec::default())
        .await
        .expect("open_connection");
    // Immediately probe health — subprocess must be alive and responsive.
    let health = exec.check_connection(&handle).await.expect("check");
    assert_eq!(health, ConnectionHealth::Healthy);
    exec.close_connection(handle).await.expect("close");
}

#[tokio::test]
async fn start_session_on_registers_and_returns_ready() {
    let Some(exec) = make_executor() else {
        eprintln!("SKIP: cteno-agent binary not found");
        return;
    };
    let handle = exec
        .open_connection(ConnectionSpec::default())
        .await
        .expect("open_connection");
    let tmp = std::env::temp_dir();
    let session = exec
        .start_session_on(&handle, make_spawn_spec(tmp))
        .await
        .expect("start_session_on");
    assert_eq!(session.vendor, "cteno");
    // Close session (keeps subprocess alive).
    exec.close_session(&session).await.expect("close_session");
    // Connection should still be healthy after detaching the session.
    let health = exec.check_connection(&handle).await.expect("check");
    assert_eq!(health, ConnectionHealth::Healthy);
    exec.close_connection(handle).await.expect("close");
}

#[tokio::test]
async fn close_connection_kills_child_and_drains_demux() {
    let Some(exec) = make_executor() else {
        eprintln!("SKIP: cteno-agent binary not found");
        return;
    };
    let handle = exec
        .open_connection(ConnectionSpec::default())
        .await
        .expect("open_connection");
    exec.close_connection(handle.clone())
        .await
        .expect("close_connection");
    // A subsequent check on the same handle should now report Dead.
    // (The handle has been removed from the registry but `inner` still
    // points at the CtenoConnection.)
    let health = exec.check_connection(&handle).await.expect("check");
    match health {
        ConnectionHealth::Healthy => panic!("expected Dead after close"),
        ConnectionHealth::Dead { .. } => {}
    }
}

#[tokio::test]
async fn check_connection_reports_dead_after_kill() {
    // We can't SIGKILL the child from outside the crate easily, so exercise
    // the same code path by closing and then checking.
    let Some(exec) = make_executor() else {
        eprintln!("SKIP: cteno-agent binary not found");
        return;
    };
    let handle = exec
        .open_connection(ConnectionSpec::default())
        .await
        .expect("open_connection");
    exec.close_connection(handle.clone()).await.expect("close");
    // Give the watcher a brief moment to update liveness, then check.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let health = exec.check_connection(&handle).await.expect("check");
    match health {
        ConnectionHealth::Healthy => panic!("expected Dead"),
        ConnectionHealth::Dead { .. } => {}
    }
}

#[tokio::test]
async fn two_sessions_on_one_connection_have_independent_ready_frames() {
    let Some(exec) = make_executor() else {
        eprintln!("SKIP: cteno-agent binary not found");
        return;
    };
    let handle = exec
        .open_connection(ConnectionSpec::default())
        .await
        .expect("open_connection");
    let tmp = std::env::temp_dir();
    let sess_a = exec
        .start_session_on(&handle, make_spawn_spec(tmp.clone()))
        .await
        .expect("start_session_on A");
    let sess_b = exec
        .start_session_on(&handle, make_spawn_spec(tmp))
        .await
        .expect("start_session_on B");
    assert_ne!(sess_a.id.as_str(), sess_b.id.as_str());
    // Both should share the same process_handle … wait, they actually get
    // unique `ProcessHandleToken`s because the token is fresh per session.
    // What we really care about is that both sessions exist on the same
    // connection and both see Ready independently.
    assert_eq!(
        exec.check_connection(&handle).await.expect("check"),
        ConnectionHealth::Healthy
    );
    exec.close_session(&sess_a).await.expect("close A");
    exec.close_session(&sess_b).await.expect("close B");
    exec.close_connection(handle).await.expect("close conn");
}

#[tokio::test]
async fn abort_on_one_session_does_not_cancel_other() {
    // We can't trigger an actual in-flight turn without the LLM, so we just
    // verify that sending `interrupt` to one session neither affects the
    // other session's Ready state nor kills the subprocess.
    let Some(exec) = make_executor() else {
        eprintln!("SKIP: cteno-agent binary not found");
        return;
    };
    let handle = exec
        .open_connection(ConnectionSpec::default())
        .await
        .expect("open");
    let tmp = std::env::temp_dir();
    let a = exec
        .start_session_on(&handle, make_spawn_spec(tmp.clone()))
        .await
        .expect("start A");
    let b = exec
        .start_session_on(&handle, make_spawn_spec(tmp))
        .await
        .expect("start B");
    // Interrupt A (no-op since no turn running); B must still be healthy.
    exec.interrupt(&a).await.expect("interrupt A");
    // Wait a beat to let any error frame surface on the demuxer.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        exec.check_connection(&handle).await.expect("check"),
        ConnectionHealth::Healthy
    );
    exec.close_session(&a).await.ok();
    exec.close_session(&b).await.ok();
    exec.close_connection(handle).await.ok();
}
