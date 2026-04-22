//! Tauri commands bridging the Expo frontend to the daemon's `AuthStore`.
//!
//! After browser / QR / email OAuth, the frontend holds the token pair. These
//! commands are how it hands them off to the daemon so the AuthStore
//! subscribers (refresh guard / user-scoped socket boot / machine register
//! dedup) can fire.
//!
//! Without this bridge, `auth_store_boot` Wire #3 and Wire #4 would only run
//! for the headless/QR path that goes through `headless_auth::save_account_auth`.

use cteno_host_runtime::auth::AuthSnapshot;
use serde::{Deserialize, Serialize};

use crate::auth_store_boot::auth_store;

/// Frontend-supplied credentials payload. Field names match the server's
/// unified login response (camelCase) so the Expo layer can forward the
/// response object almost verbatim.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAuthCredentialsArgs {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
    /// Absolute UNIX timestamp (ms) at which the access token expires. The
    /// frontend computes this from `expiresIn` in the login response.
    pub access_expires_at_ms: u64,
    /// Absolute UNIX timestamp (ms) at which the refresh token expires.
    pub refresh_expires_at_ms: u64,
    /// Optional — some login paths (anonymous, pre-register) don't know a
    /// machine id yet. If `None`, we preserve whatever the store already has.
    #[serde(default)]
    pub machine_id: Option<String>,
}

/// Snapshot shape returned to the frontend. Access / refresh tokens are
/// **omitted** — the frontend has its own copies, and we don't want Tauri IPC
/// payloads carrying the JWT unnecessarily.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSnapshotView {
    pub is_logged_in: bool,
    pub user_id: Option<String>,
    pub machine_id: Option<String>,
    pub access_expires_at_ms: Option<u64>,
    pub refresh_expires_at_ms: Option<u64>,
}

impl From<&AuthSnapshot> for AuthSnapshotView {
    fn from(s: &AuthSnapshot) -> Self {
        Self {
            is_logged_in: s.is_logged_in(),
            user_id: s.user_id.clone(),
            machine_id: s.machine_id.clone(),
            access_expires_at_ms: s.access_expires_at_ms,
            refresh_expires_at_ms: s.refresh_expires_at_ms,
        }
    }
}

/// Persist a login outcome into the daemon's `AuthStore`. Triggers all
/// downstream subscribers: refresh guard wakes up, user-/machine-scoped
/// sockets connect, first-login machine register dedup either fires or skips.
#[tauri::command]
pub async fn cteno_auth_save_credentials(args: SaveAuthCredentialsArgs) -> Result<(), String> {
    let Some(store) = auth_store() else {
        return Err("auth store not installed (daemon boot incomplete)".to_string());
    };

    let mut next = store.snapshot();
    next.access_token = Some(args.access_token);
    next.refresh_token = Some(args.refresh_token);
    next.user_id = Some(args.user_id);
    next.access_expires_at_ms = Some(args.access_expires_at_ms);
    next.refresh_expires_at_ms = Some(args.refresh_expires_at_ms);
    // machine_id: only overwrite if frontend supplied one (daemon may have
    // pre-generated its own and it should win).
    if let Some(id) = args.machine_id {
        next.machine_id = Some(id);
    }

    store.set_tokens(next)
}

/// Clear persisted credentials. Triggers logout subscribers (sockets tear
/// down, machine register dedup resets).
#[tauri::command]
pub async fn cteno_auth_clear_credentials() -> Result<(), String> {
    log::warn!(
        "[auth-clear-trace] cteno_auth_clear_credentials invoked from JS bridge; backtrace:\n{}",
        std::backtrace::Backtrace::force_capture()
    );
    let Some(store) = auth_store() else {
        return Err("auth store not installed".to_string());
    };
    store.clear()
}

/// Read the current login state — no tokens, just identity + expiry. Useful
/// for UI that wants to decide "do I need to prompt login?" without keeping
/// the token pair in JS memory itself.
#[tauri::command]
pub async fn cteno_auth_get_snapshot() -> Result<AuthSnapshotView, String> {
    let Some(store) = auth_store() else {
        return Err("auth store not installed".to_string());
    };
    let snap = store.snapshot();
    Ok(AuthSnapshotView::from(&snap))
}

/// Snapshot shape returned to `cteno_auth_force_refresh_now`. Carries the full
/// token pair (bypassing the usual "no JWT over IPC" preference) so the JS
/// caller can update its own mirror of `auth_credentials_v2` without waiting
/// for the `auth-tokens-rotated` Tauri event to arrive. The event is still
/// emitted for any other subscribers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForceRefreshResult {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
    pub access_expires_at_ms: u64,
    pub refresh_expires_at_ms: u64,
    pub machine_id: Option<String>,
}

/// Delegate a refresh cycle to the Rust-side AuthStore. Under Plan A, JS is
/// NOT allowed to hit `/v1/auth/refresh` directly — instead it invokes this
/// command and reads the rotated token pair from the result. This keeps the
/// server-side refresh-token family in sync across both credential stores.
#[tauri::command]
pub async fn cteno_auth_force_refresh_now() -> Result<ForceRefreshResult, String> {
    // `refresh_tick_once` is idempotent: if the Rust-side guard already
    // rotated recently and needs_refresh_soon is false, it returns Ok(()) and
    // we simply read back the current snapshot. If a rotation is due, it
    // performs it and the event fires for any listeners.
    crate::auth_store_boot::refresh_tick_once().await?;

    let Some(store) = auth_store() else {
        return Err("auth store not installed".to_string());
    };
    let snap = store.snapshot();
    let (
        Some(access_token),
        Some(refresh_token),
        Some(user_id),
        Some(access_exp),
        Some(refresh_exp),
    ) = (
        snap.access_token.clone(),
        snap.refresh_token.clone(),
        snap.user_id.clone(),
        snap.access_expires_at_ms,
        snap.refresh_expires_at_ms,
    )
    else {
        return Err("auth store snapshot missing fields after refresh".to_string());
    };
    Ok(ForceRefreshResult {
        access_token,
        refresh_token,
        user_id,
        access_expires_at_ms: access_exp,
        refresh_expires_at_ms: refresh_exp,
        machine_id: snap.machine_id.clone(),
    })
}
