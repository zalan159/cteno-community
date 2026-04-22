use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;

#[cfg(feature = "tauri-commands")]
pub mod commands;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DesktopAttentionState {
    pub initialized: bool,
    pub active_session_id: Option<String>,
    pub app_active: bool,
}

impl Default for DesktopAttentionState {
    fn default() -> Self {
        Self {
            initialized: false,
            active_session_id: None,
            app_active: true,
        }
    }
}

impl DesktopAttentionState {
    pub fn should_notify_for_session(&self, session_id: &str) -> bool {
        if !self.initialized {
            return false;
        }

        !self.app_active || self.active_session_id.as_deref() != Some(session_id)
    }
}

type SharedAttentionState = Arc<Mutex<DesktopAttentionState>>;

static ATTENTION_STATE: OnceLock<SharedAttentionState> = OnceLock::new();

fn shared_state() -> &'static SharedAttentionState {
    ATTENTION_STATE.get_or_init(|| Arc::new(Mutex::new(DesktopAttentionState::default())))
}

pub async fn set_attention_state(
    active_session_id: Option<String>,
    app_active: bool,
) -> Result<(), String> {
    let mut state = shared_state().lock().await;
    state.initialized = true;
    state.active_session_id = active_session_id.and_then(|id| {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    state.app_active = app_active;
    Ok(())
}

pub async fn should_send_completion_notification(session_id: &str) -> bool {
    shared_state()
        .lock()
        .await
        .should_notify_for_session(session_id)
}
