//! Regression test: a codex app-server child that dies between `initialize`
//! and `thread/start` must surface as `ConnectionHealth::Dead` so the
//! higher-layer ExecutorRegistry can drop the cached handle and redial.
//!
//! The failure mode this targets is exactly what the "preheat-then-idle"
//! bug in `docs/server-relay-refactor.md` and the user report described:
//!
//!   06:55  preheat dials app-server, caches handle
//!   06:55..07:53  ~57 min idle; child dies (OS pipe cleanup, auth token
//!                 rotation, OOM, etc.) — demuxer reads EOF → `closed=true`
//!   07:53  user sends a codex message → start_session_on reads `closed` →
//!          returns "codex app-server connection is closed; reopen before
//!          starting a session"
//!
//! To reproduce without waiting 57 minutes we shove a "fake codex" on the
//! path which is a python script that:
//!   * `app-server --help`  → exit 0 (lets `probe_app_server` succeed)
//!   * `app-server --listen stdio://` → perform the initialize handshake,
//!     then exit immediately. This produces the "child exited unexpectedly"
//!     state without any turn traffic, exactly like the cached-preheat case.
//!
//! The test asserts:
//!
//!   1. `open_connection` succeeds (handshake completes before the fake
//!      exits).
//!   2. After the child exits, `check_connection` returns `Dead` with a
//!      reason — this is the signal the registry's new cache-hit check
//!      relies on.
//!   3. A second `open_connection` succeeds (fresh subprocess), proving
//!      the adapter-level state isn't poisoned.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use multi_agent_runtime_codex::CodexAgentExecutor;
use multi_agent_runtime_core::executor::{
    AgentExecutor, ConnectionHealth, ConnectionSpec, NativeMessage, NativeSessionId, Pagination,
    SessionFilter, SessionInfo, SessionMeta, SessionRecord, SessionStoreProvider,
};
use tokio::sync::Mutex;

struct NoopStore {
    records: Mutex<Vec<(String, SessionRecord)>>,
}

#[async_trait]
impl SessionStoreProvider for NoopStore {
    async fn record_session(&self, vendor: &str, session: SessionRecord) -> Result<(), String> {
        self.records.lock().await.push((vendor.into(), session));
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

/// Write a fake `codex` shim to `dir/codex` that answers the handshake then
/// exits. Returns the absolute path. The shebang uses `/usr/bin/env python3`
/// so the test is portable across macOS/Linux CI runners without an extra
/// `which` dev-dependency.
fn write_fake_codex(dir: &std::path::Path) -> PathBuf {
    let script = dir.join("codex");
    let body = r#"#!/usr/bin/env python3
import json
import sys

args = sys.argv[1:]

# `codex app-server --help` — used by probe_app_server.
if args[:2] == ["app-server", "--help"]:
    sys.exit(0)

# Handshake then exit: simulates a child that died while cached.
if args[:1] == ["app-server"]:
    # Read one line (initialize).
    line = sys.stdin.readline()
    if not line:
        sys.exit(1)
    req = json.loads(line)
    resp = {
        "jsonrpc": "2.0",
        "id": req.get("id", 0),
        "result": {
            "userAgent": "fake/0.0.0",
            "codexHome": "/tmp/fake-codex-home",
            "platformFamily": "unix",
            "platformOs": "linux",
        },
    }
    sys.stdout.write(json.dumps(resp) + "\n")
    sys.stdout.flush()
    # Read the "initialized" notification and discard.
    sys.stdin.readline()
    # Exit: the parent demuxer will read EOF and set closed=true.
    sys.exit(0)

sys.exit(2)
"#;
    std::fs::write(&script, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    script
}

/// Create a unique temporary directory without pulling in `tempfile`.
fn unique_tmp_dir(prefix: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "{prefix}-{}",
        uuid::Uuid::new_v4().simple().to_string()
    ));
    std::fs::create_dir_all(&base).unwrap();
    base
}

#[tokio::test]
async fn check_connection_returns_dead_after_child_exit() {
    let tmp = unique_tmp_dir("codex-dead-handle");
    let codex_path = write_fake_codex(&tmp);
    let store = Arc::new(NoopStore {
        records: Mutex::new(Vec::new()),
    });
    let executor = CodexAgentExecutor::new(codex_path, store);

    let handle = executor
        .open_connection(ConnectionSpec::default())
        .await
        .expect("open_connection should handshake against fake codex");

    // Give the fake codex time to read the `initialized` notification and
    // exit. The demuxer task polls stdout and flips `closed` on EOF.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let health = executor
        .check_connection(&handle)
        .await
        .expect("check_connection returns Ok");
    match health {
        ConnectionHealth::Dead { reason } => {
            assert!(
                !reason.is_empty(),
                "Dead reason must be populated so the registry log is actionable"
            );
        }
        ConnectionHealth::Healthy => {
            panic!("fake codex exited immediately; check_connection should have reported Dead");
        }
    }

    // Registry would drop the slot and redial here — simulate the same.
    let _ = executor.close_connection(handle).await;

    let handle2 = executor
        .open_connection(ConnectionSpec::default())
        .await
        .expect("reopen after dead handle should succeed");
    // Sanity: the second handshake produced a handle with a fresh id.
    let _ = executor.close_connection(handle2).await;
    let _ = std::fs::remove_dir_all(&tmp);
}
