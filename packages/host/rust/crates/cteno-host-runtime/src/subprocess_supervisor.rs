//! Subprocess supervisor for Tauri daemon (cteno-agentd) spawned agents.
//!
//! Responsibilities:
//! - Track PIDs of cteno-agent / claude / codex subprocesses in a pid file
//!   (`~/.cteno/cteno-agent-pids.json` by default; caller picks the path).
//! - On daemon startup, sweep orphan PIDs from a previous daemon crash and
//!   send SIGTERM to any still-alive processes.
//! - On daemon shutdown, `kill_all()` delivers SIGTERM to every tracked child
//!   to prevent orphans.
//! - Crash detection is the caller's responsibility (e.g. CtenoAgentExecutor
//!   awaits `child.wait()` and then calls `unregister()` + logs the exit code).
//!
//! # Platform support
//!
//! Unix only for now (uses `libc::kill`). Windows platform is a TODO — the
//! module is gated behind `#[cfg(unix)]` and a Windows stub returns a
//! construction error so callers can fall back to "unsupervised" mode.

#[cfg(unix)]
mod imp {
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct SupervisedProcess {
        pub pid: i32,
        /// subprocess category: "cteno-agent" / "claude" / "codex" / ...
        pub kind: String,
        pub session_id: String,
        /// unix timestamp (seconds) when the child was spawned
        pub spawned_at: i64,
    }

    pub struct SubprocessSupervisor {
        pid_file: PathBuf,
        state: Arc<Mutex<HashMap<i32, SupervisedProcess>>>,
    }

    impl SubprocessSupervisor {
        /// Build a supervisor, load any existing pid file, and sweep orphans.
        ///
        /// The pid file path is provided by the caller so tests can use tempdirs
        /// and production code can pick `~/.cteno/cteno-agent-pids.json`.
        pub fn new(pid_file: PathBuf) -> Result<Self, String> {
            let supervisor = Self {
                pid_file,
                state: Arc::new(Mutex::new(HashMap::new())),
            };
            supervisor.load_and_sweep()?;
            Ok(supervisor)
        }

        fn load_and_sweep(&self) -> Result<(), String> {
            if !self.pid_file.exists() {
                return Ok(());
            }
            let raw = std::fs::read_to_string(&self.pid_file)
                .map_err(|e| format!("read pid file {:?}: {e}", self.pid_file))?;
            let processes: Vec<SupervisedProcess> = serde_json::from_str(&raw).unwrap_or_default();
            let total = processes.len();
            let mut killed = 0usize;
            for p in &processes {
                if Self::is_process_alive(p.pid) {
                    let _ = Self::send_signal(p.pid, libc::SIGTERM);
                    killed += 1;
                    log::info!(
                        "orphan sweep: SIGTERM to pid={} kind={} session={}",
                        p.pid,
                        p.kind,
                        p.session_id
                    );
                }
            }
            if total > 0 {
                log::info!("orphan sweep complete: {killed}/{total} stale subprocess(es) signaled");
            }
            // Reset the pid file so the new daemon starts clean.
            let _ = std::fs::remove_file(&self.pid_file);
            Ok(())
        }

        /// Register a freshly spawned child. Caller should invoke this right
        /// after `Command::spawn()` succeeds.
        pub fn register(&self, proc: SupervisedProcess) -> Result<(), String> {
            let mut state = self
                .state
                .lock()
                .map_err(|e| format!("supervisor lock poisoned: {e}"))?;
            state.insert(proc.pid, proc);
            self.persist(&state)?;
            Ok(())
        }

        /// Remove a child from tracking after it exits cleanly.
        pub fn unregister(&self, pid: i32) -> Result<(), String> {
            let mut state = self
                .state
                .lock()
                .map_err(|e| format!("supervisor lock poisoned: {e}"))?;
            if let Some(p) = state.remove(&pid) {
                log::debug!(
                    "unregistered subprocess pid={pid} kind={} session={}",
                    p.kind,
                    p.session_id
                );
                self.persist(&state)?;
            }
            Ok(())
        }

        /// Best-effort SIGTERM to every tracked subprocess. Intended to be
        /// called once during daemon shutdown.
        pub fn kill_all(&self) {
            if let Ok(state) = self.state.lock() {
                for (pid, p) in state.iter() {
                    match Self::send_signal(*pid, libc::SIGTERM) {
                        Ok(_) => log::info!(
                            "daemon shutdown: SIGTERM to pid={pid} kind={} session={}",
                            p.kind,
                            p.session_id
                        ),
                        Err(e) => log::warn!("daemon shutdown: failed to signal pid={pid}: {e}"),
                    }
                }
            } else {
                log::warn!("daemon shutdown: supervisor lock poisoned; cannot kill_all");
            }
            let _ = std::fs::remove_file(&self.pid_file);
        }

        /// Snapshot of currently tracked processes (for diagnostics).
        pub fn snapshot(&self) -> Vec<SupervisedProcess> {
            self.state
                .lock()
                .map(|s| s.values().cloned().collect())
                .unwrap_or_default()
        }

        fn persist(&self, state: &HashMap<i32, SupervisedProcess>) -> Result<(), String> {
            let processes: Vec<SupervisedProcess> = state.values().cloned().collect();
            let raw = serde_json::to_string_pretty(&processes)
                .map_err(|e| format!("serialize pid file: {e}"))?;
            if let Some(parent) = self.pid_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&self.pid_file, raw)
                .map_err(|e| format!("write pid file {:?}: {e}", self.pid_file))?;
            Ok(())
        }

        fn is_process_alive(pid: i32) -> bool {
            // kill(pid, 0) returns 0 if the process exists and we have permission
            // to signal it; -1 with ESRCH means no such process.
            unsafe { libc::kill(pid, 0) == 0 }
        }

        fn send_signal(pid: i32, signal: i32) -> Result<(), String> {
            let rc = unsafe { libc::kill(pid, signal) };
            if rc != 0 {
                return Err(format!(
                    "kill({pid}, {signal}) failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tempfile::tempdir;

        fn fake_proc(pid: i32, session: &str) -> SupervisedProcess {
            SupervisedProcess {
                pid,
                kind: "cteno-agent".into(),
                session_id: session.into(),
                spawned_at: 0,
            }
        }

        #[test]
        fn register_unregister_roundtrip() {
            let dir = tempdir().unwrap();
            let pid_file = dir.path().join("pids.json");
            let s = SubprocessSupervisor::new(pid_file.clone()).unwrap();

            // Using an unlikely-to-exist pid so load_and_sweep on subsequent
            // constructions is a no-op regardless of the host.
            s.register(fake_proc(2_000_000, "s1")).unwrap();
            assert_eq!(s.snapshot().len(), 1);

            // Unregistering an unknown pid is a no-op, not a panic.
            s.unregister(11_111).unwrap();
            assert_eq!(s.snapshot().len(), 1);

            s.unregister(2_000_000).unwrap();
            assert_eq!(s.snapshot().len(), 0);
        }

        #[test]
        fn persist_survives_reload() {
            let dir = tempdir().unwrap();
            let pid_file = dir.path().join("pids.json");
            {
                let s = SubprocessSupervisor::new(pid_file.clone()).unwrap();
                s.register(SupervisedProcess {
                    pid: 2_000_001,
                    kind: "cteno-agent".into(),
                    session_id: "s1".into(),
                    spawned_at: 1000,
                })
                .unwrap();
                // File should exist after register.
                assert!(
                    pid_file.exists(),
                    "pid file should be persisted on register"
                );
            }
            // On reload, load_and_sweep reads the file, (probably) finds the pid
            // no longer alive, and resets the file. It must not panic.
            let s2 = SubprocessSupervisor::new(pid_file.clone()).unwrap();
            assert_eq!(s2.snapshot().len(), 0);
            // Sweeper should have deleted the pid file.
            assert!(
                !pid_file.exists(),
                "sweeper should reset pid file after loading"
            );
        }

        #[test]
        fn handles_missing_file() {
            let dir = tempdir().unwrap();
            let pid_file = dir.path().join("nonexistent.json");
            let s = SubprocessSupervisor::new(pid_file.clone()).unwrap();
            assert_eq!(s.snapshot().len(), 0);
            assert!(!pid_file.exists());
        }
    }
}

#[cfg(windows)]
mod imp {
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct SupervisedProcess {
        pub pid: i32,
        pub kind: String,
        pub session_id: String,
        pub spawned_at: i64,
    }

    /// Windows stub. TODO: implement using OpenProcess + TerminateProcess.
    /// For now, construction returns an error so callers can fall back to
    /// unsupervised mode on Windows.
    pub struct SubprocessSupervisor;

    impl SubprocessSupervisor {
        pub fn new(_pid_file: PathBuf) -> Result<Self, String> {
            Err("SubprocessSupervisor not implemented on Windows (TODO)".to_string())
        }

        pub fn register(&self, _proc: SupervisedProcess) -> Result<(), String> {
            Err("SubprocessSupervisor not implemented on Windows".to_string())
        }

        pub fn unregister(&self, _pid: i32) -> Result<(), String> {
            Ok(())
        }

        pub fn kill_all(&self) {}

        pub fn snapshot(&self) -> Vec<SupervisedProcess> {
            Vec::new()
        }
    }
}

pub use imp::{SubprocessSupervisor, SupervisedProcess};
