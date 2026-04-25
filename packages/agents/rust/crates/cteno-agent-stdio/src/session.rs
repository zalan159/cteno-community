//! Per-session state tracking for the stdio binary.
//!
//! A single `cteno-agent` process can host multiple concurrent sessions. The
//! main dispatch loop owns a `HashMap<session_id, SessionHandle>`; each
//! handle bundles the current session state (`SessionState`) with a
//! best-effort join handle for the currently-running turn and an abort flag.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use cteno_agent_runtime::agent_queue::AgentMessageQueue;
use serde_json::Value;

/// Session configuration, derived from `init` and updated by control messages.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub session_id: String,
    pub workdir: Option<String>,
    pub additional_directories: Vec<String>,
    pub agent_config: Value,
    pub system_prompt: Option<String>,
    pub db_path: PathBuf,
    pub abort_flag: Arc<AtomicBool>,
}

impl SessionState {
    pub fn new(
        session_id: String,
        workdir: Option<String>,
        additional_directories: Vec<String>,
        agent_config: Value,
        system_prompt: Option<String>,
        db_path: PathBuf,
    ) -> Self {
        Self {
            session_id,
            workdir,
            additional_directories,
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
    /// Runtime-native queue for background messages that should re-enter the
    /// parent agent session rather than only being rendered by the host.
    pub message_queue: Arc<AgentMessageQueue>,
    /// JoinHandle for the currently-running turn, if any.
    pub running_turn: Option<tokio::task::JoinHandle<()>>,
    /// JoinHandle for the SubAgent notification receiver registered with the
    /// runtime SubAgentManager.
    pub subagent_receiver: Option<tokio::task::JoinHandle<()>>,
}

impl SessionHandle {
    pub fn new(state: SessionState) -> Self {
        Self {
            state,
            message_queue: Arc::new(AgentMessageQueue::new()),
            running_turn: None,
            subagent_receiver: None,
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

    /// Cancel the current turn task, if it is still running. Returns true
    /// when a live turn was actually aborted and needs terminal protocol
    /// frames from the caller.
    pub fn abort_running_turn(&mut self) -> bool {
        self.harvest_finished();
        let Some(handle) = self.running_turn.take() else {
            return false;
        };
        if handle.is_finished() {
            return false;
        }
        handle.abort();
        true
    }

    pub fn abort_subagent_receiver(&mut self) {
        if let Some(handle) = self.subagent_receiver.take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::time::{sleep, Duration};

    fn test_state(session_id: &str) -> SessionState {
        SessionState::new(
            session_id.to_string(),
            None,
            Vec::new(),
            json!({}),
            None,
            PathBuf::from("/tmp/cteno-test.db"),
        )
    }

    #[tokio::test]
    async fn abort_running_turn_clears_live_handle() {
        let mut handle = SessionHandle::new(test_state("abort-live"));
        handle.running_turn = Some(tokio::spawn(async {
            sleep(Duration::from_secs(60)).await;
        }));

        assert!(handle.turn_in_progress());
        assert!(handle.abort_running_turn());
        assert!(!handle.turn_in_progress());
    }

    #[tokio::test]
    async fn abort_running_turn_ignores_finished_handle() {
        let mut handle = SessionHandle::new(test_state("abort-finished"));
        handle.running_turn = Some(tokio::spawn(async {}));
        sleep(Duration::from_millis(10)).await;

        assert!(!handle.abort_running_turn());
        assert!(!handle.turn_in_progress());
    }

    #[test]
    fn handle_starts_without_stdio_side_queue() {
        let handle = SessionHandle::new(test_state("no-stdio-queue"));
        assert!(!handle.turn_in_progress());
        assert!(handle.message_queue.is_empty("no-stdio-queue"));
    }
}
