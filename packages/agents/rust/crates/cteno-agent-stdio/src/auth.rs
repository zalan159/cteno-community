//! Process-wide auth slot for the stdio agent.
//!
//! A single `cteno-agent` process can host multiple concurrent sessions, but
//! all sessions belong to the same logged-in user and share a single
//! access token. This module owns that shared state:
//!
//! - `AuthSlot` — interior mutable triple (access_token / user_id / machine_id).
//! - `StdioCredentialsProvider` — `cteno_agent_runtime::hooks::CredentialsProvider`
//!   impl that reads from an `AuthSlot`. Returned values are clones, so the
//!   slot can be rotated under the provider's feet without blocking call sites.
//!
//! The main loop seeds the slot from the first `Init.auth_token` and rotates
//! it in response to `Inbound::TokenRefreshed`.

use std::sync::{Arc, RwLock};

use cteno_agent_runtime::hooks::CredentialsProvider;

/// Shared auth state. Mutable from the main loop; read from
/// `StdioCredentialsProvider` on every hook call.
#[derive(Default)]
pub struct AuthSlot {
    pub access_token: Option<String>,
    pub user_id: Option<String>,
    pub machine_id: Option<String>,
}

/// Credentials provider that reads from a shared `AuthSlot`. Created once at
/// boot and installed via `hooks::install_credentials`.
pub struct StdioCredentialsProvider {
    slot: Arc<RwLock<AuthSlot>>,
}

impl StdioCredentialsProvider {
    pub fn new(slot: Arc<RwLock<AuthSlot>>) -> Self {
        Self { slot }
    }
}

impl CredentialsProvider for StdioCredentialsProvider {
    fn access_token(&self) -> Option<String> {
        self.slot.read().ok()?.access_token.clone()
    }

    fn user_id(&self) -> Option<String> {
        self.slot.read().ok()?.user_id.clone()
    }

    fn machine_id(&self) -> Option<String> {
        self.slot.read().ok()?.machine_id.clone()
    }
}

/// Fold an `Init` message's auth fields into the slot. Non-`None` fields
/// overwrite; `None` fields preserve existing values so a second `Init` with
/// an empty `auth_token` does not clobber a token set by the first `Init`.
pub fn apply_init_auth(
    slot: &Arc<RwLock<AuthSlot>>,
    auth_token: Option<String>,
    user_id: Option<String>,
    machine_id: Option<String>,
) {
    if auth_token.is_none() && user_id.is_none() && machine_id.is_none() {
        return;
    }
    if let Ok(mut guard) = slot.write() {
        if auth_token.is_some() {
            guard.access_token = auth_token;
        }
        if user_id.is_some() {
            guard.user_id = user_id;
        }
        if machine_id.is_some() {
            guard.machine_id = machine_id;
        }
    }
}

/// Rotate the access token. Never clears user_id / machine_id.
pub fn apply_token_refresh(slot: &Arc<RwLock<AuthSlot>>, access_token: String) {
    if let Ok(mut guard) = slot.write() {
        guard.access_token = Some(access_token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_fields_populate_empty_slot() {
        let slot = Arc::new(RwLock::new(AuthSlot::default()));
        apply_init_auth(
            &slot,
            Some("tok-1".to_string()),
            Some("user-1".to_string()),
            Some("machine-1".to_string()),
        );
        let guard = slot.read().unwrap();
        assert_eq!(guard.access_token.as_deref(), Some("tok-1"));
        assert_eq!(guard.user_id.as_deref(), Some("user-1"));
        assert_eq!(guard.machine_id.as_deref(), Some("machine-1"));
    }

    #[test]
    fn init_none_fields_preserve_existing_values() {
        let slot = Arc::new(RwLock::new(AuthSlot {
            access_token: Some("keep-token".to_string()),
            user_id: Some("keep-user".to_string()),
            machine_id: Some("keep-machine".to_string()),
        }));
        // Second init with all None fields — must not clobber.
        apply_init_auth(&slot, None, None, None);
        let guard = slot.read().unwrap();
        assert_eq!(guard.access_token.as_deref(), Some("keep-token"));
        assert_eq!(guard.user_id.as_deref(), Some("keep-user"));
        assert_eq!(guard.machine_id.as_deref(), Some("keep-machine"));
    }

    #[test]
    fn init_partial_fields_overwrite_only_the_non_none_ones() {
        let slot = Arc::new(RwLock::new(AuthSlot {
            access_token: Some("old".to_string()),
            user_id: Some("keep-user".to_string()),
            machine_id: None,
        }));
        apply_init_auth(
            &slot,
            Some("new".to_string()),
            None,
            Some("new-machine".to_string()),
        );
        let guard = slot.read().unwrap();
        assert_eq!(guard.access_token.as_deref(), Some("new"));
        assert_eq!(guard.user_id.as_deref(), Some("keep-user"));
        assert_eq!(guard.machine_id.as_deref(), Some("new-machine"));
    }

    #[test]
    fn token_refresh_only_touches_access_token() {
        let slot = Arc::new(RwLock::new(AuthSlot {
            access_token: Some("old".to_string()),
            user_id: Some("user".to_string()),
            machine_id: Some("machine".to_string()),
        }));
        apply_token_refresh(&slot, "rotated".to_string());
        let guard = slot.read().unwrap();
        assert_eq!(guard.access_token.as_deref(), Some("rotated"));
        assert_eq!(guard.user_id.as_deref(), Some("user"));
        assert_eq!(guard.machine_id.as_deref(), Some("machine"));
    }

    #[test]
    fn provider_returns_none_on_empty_slot() {
        let slot = Arc::new(RwLock::new(AuthSlot::default()));
        let provider = StdioCredentialsProvider::new(slot);
        assert_eq!(provider.access_token(), None);
        assert_eq!(provider.user_id(), None);
        assert_eq!(provider.machine_id(), None);
    }

    #[test]
    fn provider_reads_live_slot_updates() {
        let slot = Arc::new(RwLock::new(AuthSlot::default()));
        let provider = StdioCredentialsProvider::new(slot.clone());
        assert_eq!(provider.access_token(), None);
        apply_token_refresh(&slot, "live".to_string());
        assert_eq!(provider.access_token().as_deref(), Some("live"));
    }
}
