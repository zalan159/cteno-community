//! Host-side auth store — single source of truth for the unified access /
//! refresh token pair.
//!
//! Cteno 2.0 collapses the pre-2.0 multi-token soup (user JWT + 5min bootstrap
//! + 12h machine + escrow) into a single `accessToken` (30-minute TTL, JWT) +
//! `refreshToken` (60-day TTL, rotation). This module owns the persistent
//! on-disk slot (`auth.json`), exposes an in-memory `AuthSnapshot`, and
//! notifies subscribers on every mutation so the refresh daemon, running agent
//! subprocesses, and Socket.IO handshake code all observe the same value.
//!
//! Important invariants:
//!
//! - The store is **never** loaded lazily. Boot seeds it from disk (or an
//!   empty snapshot if `auth.json` is missing / malformed) before any code
//!   reads `access_token()`.
//! - Writes are atomic: we write to a sibling tmp file and rename over the
//!   target. Concurrent readers see either old or new payload, never torn
//!   JSON.
//! - Subscribers are called synchronously under the subscribe-list lock but
//!   *after* the snapshot lock is released, so a callback that wants to
//!   re-read the snapshot does not deadlock.
//! - `clear()` removes the on-disk file and resets in-memory state to
//!   `Default` — callers must then assume the user is logged out.
//!
//! This module intentionally does **not** know about the HTTP refresh
//! endpoint. The refresh client (`refresh_tokens`) is a separate plain
//! function; the guard task (lives in the app crate) calls it and pushes
//! results back via `set_tokens`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Name of the auth store file inside the app data directory. Hard-reset on
/// 2.0 — any pre-2.0 store (`headless_account_auth.json`) is ignored, which
/// surfaces as "not logged in" on first boot.
pub const AUTH_STORE_FILE: &str = "auth.json";

/// In-memory snapshot of the persisted auth state. Also used as the on-disk
/// JSON shape (field order is stable, `Default` is "empty / not logged in").
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSnapshot {
    /// Short-lived (30min) ephemeral JWT for all Happy Server calls.
    #[serde(default)]
    pub access_token: Option<String>,
    /// Long-lived (60d) refresh token. Rotation: server returns a new refresh
    /// on every `/v1/auth/refresh` call.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Server-assigned opaque user id.
    #[serde(default)]
    pub user_id: Option<String>,
    /// Stable host-generated machine id. May be `Some` even when not logged
    /// in (we pre-generate the id at machine-level bootstrap).
    #[serde(default)]
    pub machine_id: Option<String>,
    /// Absolute UNIX timestamp (ms) at which `access_token` expires.
    #[serde(default)]
    pub access_expires_at_ms: Option<u64>,
    /// Absolute UNIX timestamp (ms) at which `refresh_token` expires.
    #[serde(default)]
    pub refresh_expires_at_ms: Option<u64>,
}

impl AuthSnapshot {
    pub fn is_logged_in(&self) -> bool {
        self.access_token.is_some() && self.refresh_token.is_some()
    }
}

/// Persistent + observable auth state. `Arc<RwLock<_>>` for snapshot; separate
/// `Mutex<Vec<_>>` for subscribers so reads don't block each other and
/// subscriber add/notify don't hold the snapshot lock.
pub struct AuthStore {
    inner: Arc<RwLock<AuthSnapshot>>,
    path: PathBuf,
    on_change: Arc<Mutex<Vec<Box<dyn Fn(&AuthSnapshot) + Send + Sync>>>>,
}

impl AuthStore {
    /// Load from `{app_data_dir}/auth.json`. Missing / corrupted files yield
    /// an empty snapshot (we never error out on disk issues — worst case
    /// user has to re-login).
    pub fn load(app_data_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(app_data_dir)
            .map_err(|e| format!("Failed to create app data dir {:?}: {}", app_data_dir, e))?;
        let path = app_data_dir.join(AUTH_STORE_FILE);

        let snapshot = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(raw) => match serde_json::from_str::<AuthSnapshot>(&raw) {
                    Ok(snap) => snap,
                    Err(err) => {
                        log::warn!(
                            "auth.json at {} is corrupt or pre-2.0; treating as logged out (err: {err})",
                            path.display()
                        );
                        AuthSnapshot::default()
                    }
                },
                Err(err) => {
                    log::warn!(
                        "Failed to read auth.json at {}: {err}; treating as logged out",
                        path.display()
                    );
                    AuthSnapshot::default()
                }
            }
        } else {
            AuthSnapshot::default()
        };

        Ok(Self {
            inner: Arc::new(RwLock::new(snapshot)),
            path,
            on_change: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Path of the on-disk `auth.json` file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return a clone of the current snapshot. Cheap (tiny fields).
    pub fn snapshot(&self) -> AuthSnapshot {
        self.inner.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// Overwrite the snapshot, persist to disk, and notify subscribers.
    ///
    /// The caller is expected to pass a *complete* snapshot (same shape as
    /// `load()` observed). Partial updates are not supported — use `snapshot()`
    /// first, mutate fields on the clone, then call `set_tokens`.
    pub fn set_tokens(&self, new: AuthSnapshot) -> Result<(), String> {
        {
            let mut guard = self
                .inner
                .write()
                .map_err(|e| format!("AuthStore write lock poisoned: {e}"))?;
            *guard = new.clone();
        }
        self.persist(&new)?;
        self.notify(&new);
        Ok(())
    }

    /// Delete the on-disk file and reset in-memory state to `Default`. Fires
    /// subscribers with the empty snapshot.
    pub fn clear(&self) -> Result<(), String> {
        {
            let mut guard = self
                .inner
                .write()
                .map_err(|e| format!("AuthStore write lock poisoned: {e}"))?;
            *guard = AuthSnapshot::default();
        }
        if self.path.exists() {
            std::fs::remove_file(&self.path)
                .map_err(|e| format!("Failed to remove auth.json: {e}"))?;
        }
        self.notify(&AuthSnapshot::default());
        Ok(())
    }

    /// Remaining lifetime of the current access token in milliseconds. `None`
    /// if no expiry is tracked; negative when already expired.
    pub fn access_remaining_ms(&self) -> Option<i64> {
        let snap = self.snapshot();
        let expires = snap.access_expires_at_ms?;
        let now = now_ms();
        Some((expires as i64).saturating_sub(now as i64))
    }

    /// True when the current access token will expire within `threshold_ms`
    /// *or* has already expired. Returns `false` when no expiry is tracked —
    /// we conservatively avoid spurious refreshes on a freshly-loaded store
    /// that hasn't yet received a token pair.
    pub fn needs_refresh_soon(&self, threshold_ms: u64) -> bool {
        match self.access_remaining_ms() {
            Some(rem) => rem <= threshold_ms as i64,
            None => false,
        }
    }

    /// Register a callback fired *after* every successful `set_tokens` or
    /// `clear`. Callbacks are invoked synchronously on the writer's thread —
    /// keep them fast (push on a channel, spawn a task, etc.).
    pub fn subscribe(&self, cb: Box<dyn Fn(&AuthSnapshot) + Send + Sync>) {
        if let Ok(mut guard) = self.on_change.lock() {
            guard.push(cb);
        }
    }

    fn persist(&self, snap: &AuthSnapshot) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent dir for auth.json: {e}"))?;
        }
        let serialized = serde_json::to_string_pretty(snap)
            .map_err(|e| format!("Failed to serialize auth snapshot: {e}"))?;

        let tmp_path = self.path.with_extension("json.tmp");
        std::fs::write(&tmp_path, serialized.as_bytes())
            .map_err(|e| format!("Failed to write tmp auth.json: {e}"))?;
        std::fs::rename(&tmp_path, &self.path)
            .map_err(|e| format!("Failed to rename tmp auth.json: {e}"))?;
        Ok(())
    }

    fn notify(&self, snap: &AuthSnapshot) {
        let subs = match self.on_change.lock() {
            Ok(g) => g.iter().map(|b| b.as_ref() as *const _).collect::<Vec<_>>(),
            Err(_) => return,
        };
        // Can't hand out raw pointers to closures easily across threads; instead
        // lock once, collect (no Clone on Box<dyn Fn>), and invoke while holding
        // the lock. Callbacks are expected to be trivial.
        let _ = subs;
        if let Ok(guard) = self.on_change.lock() {
            for cb in guard.iter() {
                cb(snap);
            }
        }
    }
}

/// Current UNIX epoch in milliseconds. Returns 0 on clock-skew / pre-epoch
/// times (should never happen in practice; keeps the API infallible).
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Convert server-returned `expiresIn` (seconds) to an absolute ms timestamp.
pub fn expires_at_ms_from_seconds(expires_in_seconds: i64) -> u64 {
    if expires_in_seconds <= 0 {
        return now_ms();
    }
    now_ms().saturating_add((expires_in_seconds as u64).saturating_mul(1000))
}

// ---------------------------------------------------------------------------
// Refresh client
// ---------------------------------------------------------------------------
//
// Decoupled from AuthStore so the guard task (in app crate) can run it on a
// plain Tokio timer without holding the store. Returns a typed error enum so
// the caller can distinguish "network blip, retry later" from "refresh token
// is dead, wipe the store and ask the user to log in again".

/// Response body from `POST /v1/auth/refresh`. Mirrors the server's shape in
/// `apps/happy-server/src/app/api/routes/authRoutes.ts`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthTokensResponse {
    pub access_token: String,
    pub refresh_token: String,
    /// Remaining seconds on the new access token.
    pub expires_in: i64,
    /// Remaining seconds on the new refresh token.
    pub refresh_expires_in: i64,
    pub user_id: String,
}

/// Error categories for `refresh_tokens`. `Network` callers should retry with
/// backoff; the remaining terminal categories (`Invalid` / `NotFound` /
/// `Mismatch` / `Revoked`) mean "user must log in again — clear the store".
#[derive(Debug, Clone)]
pub enum RefreshError {
    Network(String),
    /// Server returned `refresh_token_invalid`.
    Invalid,
    /// Server returned `refresh_token_not_found`.
    NotFound,
    /// Server returned `refresh_token_mismatch`.
    Mismatch,
    /// Server returned `refresh_token_revoked`.
    Revoked,
    /// Any other server error — treat as terminal by default. Callers may
    /// retry depending on context.
    Other(String),
}

impl RefreshError {
    /// Terminal errors (non-retriable) require re-login and AuthStore clear.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Invalid | Self::NotFound | Self::Mismatch | Self::Revoked
        )
    }
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(msg) => write!(f, "network error: {msg}"),
            Self::Invalid => write!(f, "refresh_token_invalid"),
            Self::NotFound => write!(f, "refresh_token_not_found"),
            Self::Mismatch => write!(f, "refresh_token_mismatch"),
            Self::Revoked => write!(f, "refresh_token_revoked"),
            Self::Other(msg) => write!(f, "other: {msg}"),
        }
    }
}

/// Call `POST {server}/v1/auth/refresh` with `{refreshToken}` and parse the
/// response. Returns `AuthTokensResponse` on success (200), structured errors
/// otherwise. HTTP timeouts / connection errors map to `Network`.
pub async fn refresh_tokens(
    happy_server_url: &str,
    refresh_token: &str,
) -> Result<AuthTokensResponse, RefreshError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| RefreshError::Network(format!("build http client: {e}")))?;
    let url = format!("{}/v1/auth/refresh", happy_server_url.trim_end_matches('/'));
    let body = serde_json::json!({ "refreshToken": refresh_token });

    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| RefreshError::Network(format!("refresh send: {e}")))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .unwrap_or_else(|e| format!("<no body: {e}>"));

    if status.is_success() {
        serde_json::from_str::<AuthTokensResponse>(&text)
            .map_err(|e| RefreshError::Other(format!("parse success body: {e}: {text}")))
    } else {
        // Server error code contract: `{ "error": "refresh_token_invalid" | ... }`
        let err_code = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .and_then(|e| e.as_str())
                    .map(|s| s.to_string())
            });
        match err_code.as_deref() {
            Some("refresh_token_invalid") => Err(RefreshError::Invalid),
            Some("refresh_token_not_found") => Err(RefreshError::NotFound),
            Some("refresh_token_mismatch") => Err(RefreshError::Mismatch),
            Some("refresh_token_revoked") => Err(RefreshError::Revoked),
            _ => Err(RefreshError::Other(format!("{}: {}", status, text))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_missing_returns_empty_snapshot() {
        let dir = tempdir().unwrap();
        let store = AuthStore::load(dir.path()).unwrap();
        let snap = store.snapshot();
        assert!(snap.access_token.is_none());
        assert!(!snap.is_logged_in());
    }

    #[test]
    fn set_and_reload_round_trip() {
        let dir = tempdir().unwrap();
        let store = AuthStore::load(dir.path()).unwrap();
        let snap = AuthSnapshot {
            access_token: Some("acc".into()),
            refresh_token: Some("ref".into()),
            user_id: Some("u1".into()),
            machine_id: Some("m1".into()),
            access_expires_at_ms: Some(now_ms() + 30_000),
            refresh_expires_at_ms: Some(now_ms() + 5_000_000),
        };
        store.set_tokens(snap.clone()).unwrap();

        let reloaded = AuthStore::load(dir.path()).unwrap();
        let r = reloaded.snapshot();
        assert_eq!(r.access_token.as_deref(), Some("acc"));
        assert_eq!(r.refresh_token.as_deref(), Some("ref"));
        assert_eq!(r.user_id.as_deref(), Some("u1"));
        assert_eq!(r.machine_id.as_deref(), Some("m1"));
    }

    #[test]
    fn clear_removes_file() {
        let dir = tempdir().unwrap();
        let store = AuthStore::load(dir.path()).unwrap();
        let snap = AuthSnapshot {
            access_token: Some("acc".into()),
            refresh_token: Some("ref".into()),
            user_id: Some("u1".into()),
            ..Default::default()
        };
        store.set_tokens(snap).unwrap();
        assert!(store.path().exists());
        store.clear().unwrap();
        assert!(!store.path().exists());
        assert!(!store.snapshot().is_logged_in());
    }

    #[test]
    fn needs_refresh_soon_returns_false_without_expiry() {
        let dir = tempdir().unwrap();
        let store = AuthStore::load(dir.path()).unwrap();
        assert!(!store.needs_refresh_soon(60_000));
    }

    #[test]
    fn needs_refresh_soon_fires_when_near_expiry() {
        let dir = tempdir().unwrap();
        let store = AuthStore::load(dir.path()).unwrap();
        let snap = AuthSnapshot {
            access_token: Some("acc".into()),
            refresh_token: Some("ref".into()),
            access_expires_at_ms: Some(now_ms() + 1_000),
            ..Default::default()
        };
        store.set_tokens(snap).unwrap();
        assert!(store.needs_refresh_soon(5_000));
        assert!(!store.needs_refresh_soon(500));
    }

    #[test]
    fn subscribe_fires_on_set() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = tempdir().unwrap();
        let store = AuthStore::load(dir.path()).unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let c2 = counter.clone();
        store.subscribe(Box::new(move |_snap| {
            c2.fetch_add(1, Ordering::Relaxed);
        }));

        let snap = AuthSnapshot {
            access_token: Some("acc".into()),
            refresh_token: Some("r".into()),
            ..Default::default()
        };
        store.set_tokens(snap).unwrap();
        store.clear().unwrap();
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn refresh_error_terminal_detection() {
        assert!(RefreshError::Invalid.is_terminal());
        assert!(RefreshError::NotFound.is_terminal());
        assert!(RefreshError::Mismatch.is_terminal());
        assert!(RefreshError::Revoked.is_terminal());
        assert!(!RefreshError::Network("x".into()).is_terminal());
        assert!(!RefreshError::Other("x".into()).is_terminal());
    }
}
