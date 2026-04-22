//! Per-session state tracking for the stdio binary.
//!
//! A single `cteno-agent` process can host multiple concurrent sessions. The
//! main dispatch loop owns a `HashMap<session_id, SessionHandle>`; each
//! handle bundles the current session state (`SessionState`) with a
//! best-effort join handle for the currently-running turn and an abort flag.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_json::Value;

/// Session configuration, derived from `init` and updated by control messages.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub session_id: String,
    pub workdir: Option<String>,
    pub agent_config: Value,
    pub system_prompt: Option<String>,
    pub db_path: PathBuf,
    pub abort_flag: Arc<AtomicBool>,
}

impl SessionState {
    pub fn new(
        session_id: String,
        workdir: Option<String>,
        agent_config: Value,
        system_prompt: Option<String>,
        db_path: PathBuf,
    ) -> Self {
        Self {
            session_id,
            workdir,
            agent_config,
            system_prompt,
            db_path,
            abort_flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Runtime handle for a session. Owned exclusively by the main dispatch loop.
pub struct SessionHandle {
    pub state: SessionState,
    /// JoinHandle for the currently-running turn, if any.
    pub running_turn: Option<tokio::task::JoinHandle<()>>,
}

impl SessionHandle {
    pub fn new(state: SessionState) -> Self {
        Self {
            state,
            running_turn: None,
        }
    }

    /// True if there is a turn currently executing for this session.
    pub fn turn_in_progress(&self) -> bool {
        self.running_turn
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false)
    }

    /// Harvest a finished JoinHandle so subsequent `turn_in_progress` calls
    /// return false.
    pub fn harvest_finished(&mut self) {
        if let Some(handle) = &self.running_turn {
            if handle.is_finished() {
                self.running_turn = None;
            }
        }
    }
}
