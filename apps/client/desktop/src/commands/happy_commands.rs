//! Happy Client Commands
//!
//! Tauri commands for Happy Server integration

use std::sync::OnceLock;
use tokio::sync::Notify;

static GLOBAL_MACHINE_AUTH_STATE: OnceLock<MachineAuthState> = OnceLock::new();

/// Shared state for Machine auth request
#[derive(Clone)]
pub struct MachineAuthState {
    /// Signal to trigger daemon reauth (sent by frontend on logout)
    pub reauth_signal: std::sync::Arc<Notify>,
}

impl MachineAuthState {
    pub fn new() -> Self {
        let state = Self {
            reauth_signal: std::sync::Arc::new(Notify::new()),
        };
        let _ = GLOBAL_MACHINE_AUTH_STATE.set(state.clone());
        state
    }
}

pub fn global_machine_auth_state() -> Option<MachineAuthState> {
    GLOBAL_MACHINE_AUTH_STATE.get().cloned()
}

pub fn signal_machine_reauth() -> bool {
    if let Some(state) = global_machine_auth_state() {
        state.reauth_signal.notify_one();
        true
    } else {
        false
    }
}
