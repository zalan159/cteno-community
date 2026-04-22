//! Headless account auth glue (Cteno 2.0 refactor).
//!
//! Previously this module owned an end-to-end QR-login flow built on a
//! pre-2.0 server that returned `{token, response: b64(box(secret))}`. In 2.0
//! the QR path is gone (see commit faf39a6 — browser OAuth + email login on
//! mobile). Account auth persistence now lives in `cteno-host-runtime`'s
//! `AuthStore` (`auth.json`).
//!
//! What remains here is a set of thin wrappers the rest of the app crate
//! still expects:
//!
//! - Path helpers so `local_rpc_server` auth status RPCs keep returning the
//!   right paths.
//! - `load_account_auth` / `save_account_auth` / `clear_account_auth` that
//!   now go through `AuthStore`.
//! - `HeadlessAccountAuth` kept as a *view* onto the store's `access_token`
//!   field for legacy callers. Note the struct shape is unified with the
//!   AuthStore snapshot; there is no separate "account" token any more.
//!
//! Gone:
//!
//! - `PendingAccountAuth` / `start_account_auth` / `wait_for_account_auth` /
//!   `login_with_account_qr` (QR flow — see faf39a6)
//! - `ParsedQrKind` / `parse_qr_uri` / `build_account_auth_uri_from_public_key`
//! - `decrypt_box_from_bundle` / crypto_box key handling
//! - `secret_b64url` / `saved_at` persistent fields — they encoded escrow
//!   secrets that no longer exist in the unified protocol.

use cteno_host_runtime::auth::{AuthSnapshot, AuthStore};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// View onto `auth.json` for legacy code paths that only care whether the
/// user is "logged in". Matches the subset of fields those call sites read.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HeadlessAccountAuth {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub user_id: Option<String>,
    pub machine_id: Option<String>,
    pub access_expires_at_ms: Option<u64>,
    pub refresh_expires_at_ms: Option<u64>,
}

impl From<AuthSnapshot> for HeadlessAccountAuth {
    fn from(s: AuthSnapshot) -> Self {
        Self {
            access_token: s.access_token,
            refresh_token: s.refresh_token,
            user_id: s.user_id,
            machine_id: s.machine_id,
            access_expires_at_ms: s.access_expires_at_ms,
            refresh_expires_at_ms: s.refresh_expires_at_ms,
        }
    }
}

impl From<HeadlessAccountAuth> for AuthSnapshot {
    fn from(h: HeadlessAccountAuth) -> Self {
        Self {
            access_token: h.access_token,
            refresh_token: h.refresh_token,
            user_id: h.user_id,
            machine_id: h.machine_id,
            access_expires_at_ms: h.access_expires_at_ms,
            refresh_expires_at_ms: h.refresh_expires_at_ms,
        }
    }
}

pub fn resolve_app_data_dir() -> PathBuf {
    crate::host::core::default_headless_app_data_dir()
}

pub fn ensure_app_data_dir() -> Result<PathBuf, String> {
    let dir = resolve_app_data_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create app data dir {}: {}", dir.display(), e))?;
    Ok(dir)
}

/// Path of the unified auth file (was `headless_account_auth.json`; now
/// `auth.json`). Kept public so auth-status RPCs can surface the actual path.
pub fn account_auth_store_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(cteno_host_runtime::auth::AUTH_STORE_FILE)
}

/// Load the account auth view from `auth.json`. `Ok(None)` when not logged
/// in; `Err` only for filesystem errors (corrupt JSON is treated as "not
/// logged in" and logged, per `AuthStore::load`).
pub fn load_account_auth(app_data_dir: &Path) -> Result<Option<HeadlessAccountAuth>, String> {
    let store = AuthStore::load(app_data_dir)?;
    let snap = store.snapshot();
    if snap.is_logged_in() {
        Ok(Some(snap.into()))
    } else {
        Ok(None)
    }
}

/// Persist an account auth payload.
///
/// Routing rules:
/// - If the process-wide `AuthStore` is installed AND its on-disk path lives
///   in `app_data_dir`, we go through the global store so subscribers
///   (refresh daemon, socket guard, machine-register guard, agent
///   subprocesses) actually fire.
/// - Otherwise we fall back to a fresh `AuthStore::load(app_data_dir)` — same
///   behaviour as pre-2.0 and required by tests (which each use a private
///   tempdir distinct from the globally installed store's tempdir).
pub fn save_account_auth(app_data_dir: &Path, auth: &HeadlessAccountAuth) -> Result<(), String> {
    if let Some(store) = crate::auth_store_boot::auth_store() {
        if store.path().parent() == Some(app_data_dir) {
            return store.set_tokens(auth.clone().into());
        }
    }
    let store = AuthStore::load(app_data_dir)?;
    store.set_tokens(auth.clone().into())
}

pub fn clear_account_auth(app_data_dir: &Path) -> Result<(), String> {
    if let Some(store) = crate::auth_store_boot::auth_store() {
        if store.path().parent() == Some(app_data_dir) {
            return store.clear();
        }
    }
    let store = AuthStore::load(app_data_dir)?;
    store.clear()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_missing_returns_none() {
        let dir = tempdir().unwrap();
        let loaded = load_account_auth(dir.path()).unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempdir().unwrap();
        let auth = HeadlessAccountAuth {
            access_token: Some("acc".into()),
            refresh_token: Some("ref".into()),
            user_id: Some("u1".into()),
            ..Default::default()
        };
        save_account_auth(dir.path(), &auth).unwrap();
        let loaded = load_account_auth(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.access_token.as_deref(), Some("acc"));
        assert_eq!(loaded.user_id.as_deref(), Some("u1"));
    }

    #[test]
    fn clear_removes_file() {
        let dir = tempdir().unwrap();
        let auth = HeadlessAccountAuth {
            access_token: Some("acc".into()),
            refresh_token: Some("ref".into()),
            ..Default::default()
        };
        save_account_auth(dir.path(), &auth).unwrap();
        assert!(load_account_auth(dir.path()).unwrap().is_some());
        clear_account_auth(dir.path()).unwrap();
        assert!(load_account_auth(dir.path()).unwrap().is_none());
    }
}
