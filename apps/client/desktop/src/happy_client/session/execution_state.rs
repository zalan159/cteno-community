use crate::agent_queue::AgentMessageQueue;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

/// Pure-local execution state shared by background and local session runtimes.
#[derive(Clone)]
pub(crate) struct ExecutionState {
    pub(crate) thinking: Arc<AtomicU8>,
    pub(crate) abort_flag: Arc<AtomicBool>,
    pub(crate) queue: Arc<AgentMessageQueue>,
}

impl ExecutionState {
    pub(crate) fn new(queue: Arc<AgentMessageQueue>) -> Self {
        Self {
            thinking: Arc::new(AtomicU8::new(0)),
            abort_flag: Arc::new(AtomicBool::new(false)),
            queue,
        }
    }

    pub(crate) fn is_idle(&self, session_id: &str) -> bool {
        !self.queue.is_processing(session_id) && self.queue.is_empty(session_id)
    }

    pub(crate) fn begin_processing(&self, session_id: &str) {
        self.queue.set_processing(session_id, true);
        self.abort_flag.store(false, Ordering::SeqCst);
        self.thinking.store(1, Ordering::SeqCst);
    }

    pub(crate) fn try_begin_processing(&self, session_id: &str) -> bool {
        if self.queue.is_processing(session_id) {
            return false;
        }

        self.begin_processing(session_id);
        true
    }

    pub(crate) fn end_processing(&self, session_id: &str) {
        self.queue.set_processing(session_id, false);
        self.thinking.store(0, Ordering::SeqCst);
    }

    pub(crate) fn request_abort(&self) {
        self.abort_flag.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_aborted(&self) -> bool {
        self.abort_flag.load(Ordering::SeqCst)
    }
}
