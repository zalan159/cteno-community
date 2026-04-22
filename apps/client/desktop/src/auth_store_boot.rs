//! App-side glue for the unified `AuthStore`.
//!
//! `cteno-host-runtime` owns the on-disk store + refresh HTTP client; this
//! module wires the store into the agent runtime's `CredentialsProvider` hook,
//! starts the refresh guard task, and exposes helpers for the rest of the app
//! crate.
//!
//! Lifetime: called exactly once per process during boot (GUI or headless).
//! Subsequent logins / logouts go through `AuthStore::set_tokens` / `clear`
//! directly — all downstream components (transport handshake, agent
//! subprocess stdin, Tauri events) pick up updates via subscribers.
//!
//! The four wire points this module owns:
//!
//!  1. `subscribe_near_expiry_poke` — called by the machine/user socket boot
//!     path to register a `token:near-expiry` handler that pokes
//!     `refresh_tick_once`.
//!  2. `StoreAuthRefreshHook` — concrete impl of
//!     `cteno_happy_client_transport::AuthRefreshHook` so the transport layer
//!     can trigger a refresh on a 401 without reverse-depending on this crate.
//!  3. `spawn_user_and_machine_sockets_guard` — subscribes to AuthStore
//!     transitions to bring up / tear down the long-lived user-scoped and
//!     machine-scoped Socket.IO connections that daemons hold open.
//!  4. `subscribe_register_machine_once` — watches the access-token slot and
//!     issues `POST /v1/machines/register` exactly once per logged-in
//!     access-token session.

use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use cteno_agent_runtime::hooks::CredentialsProvider;
#[cfg(feature = "commercial-cloud")]
use cteno_happy_client_machine::MachineManager;
use cteno_host_runtime::auth::{refresh_tokens, AuthSnapshot, AuthStore, RefreshError};
use cteno_host_session_wire::ConnectionType;

use crate::happy_client::socket::{
    auth_hook::install_auth_refresh_hook, AuthRefreshHook, HappySocket,
};

/// Process-wide `AuthStore`. Installed by `install_auth_store` during boot;
/// any later call to `auth_store()` returns the same `Arc`.
static AUTH_STORE: OnceLock<Arc<AuthStore>> = OnceLock::new();

/// Adapter that plugs into `cteno-agent-runtime::hooks::CredentialsProvider`.
/// The adapter holds an `Arc<AuthStore>` and reads the snapshot on every call
/// so rotation is transparent (no stale clones).
pub struct StoreCredentialsProvider {
    store: Arc<AuthStore>,
}

impl StoreCredentialsProvider {
    pub fn new(store: Arc<AuthStore>) -> Self {
        Self { store }
    }
}

impl CredentialsProvider for StoreCredentialsProvider {
    fn access_token(&self) -> Option<String> {
        self.store.snapshot().access_token
    }

    fn user_id(&self) -> Option<String> {
        self.store.snapshot().user_id
    }

    fn machine_id(&self) -> Option<String> {
        self.store.snapshot().machine_id
    }
}

/// Load `auth.json` from `app_data_dir`, install it as the process-wide store,
/// and wire it into the agent runtime's credentials hook. Idempotent — a
/// second call is a no-op (first installer wins).
pub fn install_auth_store(app_data_dir: &Path) -> Result<Arc<AuthStore>, String> {
    if let Some(store) = AUTH_STORE.get() {
        return Ok(store.clone());
    }
    let store = Arc::new(AuthStore::load(app_data_dir)?);
    let _ = AUTH_STORE.set(store.clone());

    cteno_agent_runtime::hooks::install_credentials(Arc::new(StoreCredentialsProvider::new(
        store.clone(),
    )));

    // Seed machine_id into the snapshot so the CredentialsProvider can answer
    // before the user logs in. The machine_id file is resolved independently
    // of login state, so we fold it back into the store once we know it.
    if let Ok(mid) = crate::auth_anonymous::ensure_local_machine_id(app_data_dir) {
        let mut snap = store.snapshot();
        if snap.machine_id.as_deref() != Some(mid.as_str()) {
            snap.machine_id = Some(mid);
            let _ = store.set_tokens(snap);
        }
    }

    log::info!(
        "AuthStore installed at {} (logged_in={})",
        store.path().display(),
        store.snapshot().is_logged_in()
    );
    Ok(store)
}

/// Return the globally installed `AuthStore`, if any. Panics in debug builds
/// when called before `install_auth_store`; returns `None` in release to let
/// callers surface a soft error.
pub fn auth_store() -> Option<Arc<AuthStore>> {
    AUTH_STORE.get().cloned()
}

/// Convenience accessor for the current access token. `None` when not
/// logged in or before the store is installed.
pub fn current_access_token() -> Option<String> {
    auth_store().and_then(|s| s.snapshot().access_token)
}

/// Convenience accessor for the current machine id from the store.
pub fn current_machine_id() -> Option<String> {
    auth_store().and_then(|s| s.snapshot().machine_id)
}

#[allow(clippy::type_complexity)]
pub fn load_persisted_machine_auth(
    app_data_dir: &Path,
) -> Result<
    Option<(
        String,
        [u8; 32],
        cteno_host_session_codec::EncryptionVariant,
        Option<[u8; 32]>,
    )>,
    String,
> {
    let store = AuthStore::load(app_data_dir)?;
    Ok(store.snapshot().access_token.map(|token| {
        (
            token,
            [0u8; 32],
            cteno_host_session_codec::EncryptionVariant::DataKey,
            None,
        )
    }))
}

pub fn machine_auth_cache_path(app_data_dir: &Path) -> std::path::PathBuf {
    app_data_dir.join(cteno_host_runtime::auth::AUTH_STORE_FILE)
}

// ---------------------------------------------------------------------------
// Refresh guard task
// ---------------------------------------------------------------------------

const REFRESH_THRESHOLD_MS: u64 = 5 * 60 * 1000;
const REFRESH_TICK_SECS: u64 = 60;
const NETWORK_RETRY_LIMIT: u32 = 3;

/// Spawn the long-running refresh guard. Safe to call multiple times — the
/// first caller wins; later calls return immediately.
///
/// Behaviour:
///
/// - Sleeps `REFRESH_TICK_SECS` between checks.
/// - If `access_remaining_ms` < `REFRESH_THRESHOLD_MS`, calls `refresh_tokens`.
/// - Network failures: retries up to `NETWORK_RETRY_LIMIT` times within one
///   tick, then gives up and waits for the next tick.
/// - Terminal refresh errors (`invalid` / `not_found` / `mismatch` / `revoked`):
///   `store.clear()` and emit a Tauri event `"auth-require-login"`. Guard
///   keeps running; a fresh login writes tokens back and the cycle resumes.
pub fn spawn_refresh_guard() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    tokio::spawn(async move {
        log::info!(
            "auth refresh guard started (tick={}s, threshold={}ms)",
            REFRESH_TICK_SECS,
            REFRESH_THRESHOLD_MS
        );
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(REFRESH_TICK_SECS)).await;
            if let Err(e) = refresh_tick_once().await {
                log::debug!("refresh guard tick returned soft error: {e}");
            }
        }
    });
}

/// Run one refresh check. Exposed for tests and for the Socket.IO
/// `token:near-expiry` ephemeral event handler (which pokes the guard ahead
/// of its normal 60s cadence).
pub async fn refresh_tick_once() -> Result<(), String> {
    let Some(store) = auth_store() else {
        return Err("AuthStore not installed".into());
    };
    let snap = store.snapshot();
    let Some(refresh_token) = snap.refresh_token else {
        return Ok(()); // Not logged in.
    };

    if !store.needs_refresh_soon(REFRESH_THRESHOLD_MS) {
        return Ok(());
    }

    let server_url = crate::resolved_happy_server_url();
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match refresh_tokens(&server_url, &refresh_token).await {
            Ok(resp) => {
                let rotated_access_token = resp.access_token.clone();
                let new_snap = AuthSnapshot {
                    access_token: Some(resp.access_token),
                    refresh_token: Some(resp.refresh_token),
                    user_id: Some(resp.user_id),
                    machine_id: snap.machine_id.clone(),
                    access_expires_at_ms: Some(
                        cteno_host_runtime::auth::expires_at_ms_from_seconds(resp.expires_in),
                    ),
                    refresh_expires_at_ms: Some(
                        cteno_host_runtime::auth::expires_at_ms_from_seconds(
                            resp.refresh_expires_in,
                        ),
                    ),
                };
                store.set_tokens(new_snap.clone())?;
                log::info!(
                    "access token rotated (user_id={:?})",
                    store.snapshot().user_id
                );

                // Push the new token to every active Cteno sub-agent via stdin
                // so in-flight turns don't surface 401s on their next cloud
                // call. Best-effort: we log individual failures but don't
                // abort the tick.
                broadcast_token_to_agents(&rotated_access_token).await;

                // Notify the webview-side JS AuthContext so its localStorage
                // `auth_credentials_v2` tracks the server-side token family.
                // Without this, JS would eventually try to use the now-revoked
                // old refresh token and trigger a hard-expiry clear.
                emit_tokens_rotated(&new_snap);
                return Ok(());
            }
            Err(err) if err.is_terminal() => {
                log::warn!(
                    "[auth-clear-trace] refresh token dead ({err}); clearing AuthStore and requesting re-login"
                );
                let _ = store.clear();
                emit_require_login();
                return Err(format!("refresh terminal: {err}"));
            }
            Err(RefreshError::Network(msg)) => {
                if attempt >= NETWORK_RETRY_LIMIT {
                    log::warn!(
                        "refresh network failed {} attempts; will retry on next tick ({msg})",
                        NETWORK_RETRY_LIMIT
                    );
                    return Err(format!("refresh network: {msg}"));
                }
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
                continue;
            }
            Err(other) => {
                log::warn!("refresh non-terminal error: {other}; will retry on next tick");
                return Err(format!("refresh other: {other}"));
            }
        }
    }
}

/// Push the rotated access token into every live Cteno sub-agent so their
/// shared auth slot sees the new value. Claude / Codex adapters still use
/// their own CLI auth; they are unaffected.
async fn broadcast_token_to_agents(access_token: &str) {
    let Ok(registry) = crate::local_services::executor_registry() else {
        return;
    };
    let cteno = registry.cteno_concrete();
    cteno.broadcast_token_refresh(access_token).await;
}

fn emit_require_login() {
    // Tauri event emission is best-effort: if no app handle is registered
    // (headless daemon), we just log; the frontend has no way to observe
    // directly in that shell anyway.
    use tauri::Emitter;
    if let Some(handle) = crate::APP_HANDLE.get() {
        if let Err(e) = handle.emit("auth-require-login", ()) {
            log::warn!("failed to emit auth-require-login: {e}");
        }
    } else {
        log::info!("AuthStore cleared; headless shell has no frontend to notify");
    }
}

/// Payload shape handed to the JS AuthContext so it can mirror the Rust
/// `auth.json` into webview localStorage without going back out to the
/// network. Field names are camelCase to match the JS `AuthCredentials`
/// shape — Tauri's `emit` uses serde, so we match the wire contract.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AuthTokensRotatedPayload {
    access_token: String,
    refresh_token: String,
    user_id: String,
    access_expires_at_ms: u64,
    refresh_expires_at_ms: u64,
    machine_id: Option<String>,
}

fn emit_tokens_rotated(snap: &AuthSnapshot) {
    use tauri::Emitter;
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
        log::warn!("emit_tokens_rotated called with incomplete snapshot; skipping emit");
        return;
    };
    let payload = AuthTokensRotatedPayload {
        access_token,
        refresh_token,
        user_id,
        access_expires_at_ms: access_exp,
        refresh_expires_at_ms: refresh_exp,
        machine_id: snap.machine_id.clone(),
    };
    if let Some(handle) = crate::APP_HANDLE.get() {
        match handle.emit("auth-tokens-rotated", payload) {
            Ok(()) => log::info!(
                "emitted auth-tokens-rotated to webview (access_exp={}, refresh_exp={})",
                snap.access_expires_at_ms.unwrap_or(0),
                snap.refresh_expires_at_ms.unwrap_or(0),
            ),
            Err(e) => log::warn!("failed to emit auth-tokens-rotated: {e}"),
        }
    } else {
        // Headless shell — no webview to notify, skip silently.
    }
}

// ---------------------------------------------------------------------------
// Wire #2 — transport-side AuthRefreshHook
// ---------------------------------------------------------------------------

/// Adapter that satisfies `cteno_happy_client_transport::AuthRefreshHook` by
/// delegating to this module's `refresh_tick_once` / `auth_store` / Tauri
/// event helpers. Installed exactly once during boot via
/// `install_transport_auth_refresh_hook`.
struct StoreAuthRefreshHook;

impl AuthRefreshHook for StoreAuthRefreshHook {
    fn refresh_now(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async move {
            // Short-circuit when the refresh guard thinks the token is still
            // good — a stale 401 might be from a deploy / clock skew and a
            // spurious refresh is cheap but pointless here. We let the normal
            // tick cadence handle drift cases.
            refresh_tick_once().await
        })
    }

    fn notify_require_login(&self) {
        emit_require_login();
    }

    fn current_access_token(&self) -> Option<String> {
        current_access_token()
    }
}

/// Install the transport-side `AuthRefreshHook` once during boot. Safe to
/// call repeatedly — the underlying `OnceLock` is first-installer-wins.
pub fn install_transport_auth_refresh_hook() {
    install_auth_refresh_hook(Arc::new(StoreAuthRefreshHook));
    log::info!("AuthRefreshHook installed on cteno-happy-client-transport");
}

// ---------------------------------------------------------------------------
// Wire #3 — user-scoped + machine-scoped long-lived sockets
// ---------------------------------------------------------------------------
//
// Community (not logged in) mode keeps both slots empty. As soon as AuthStore
// receives its first access token (login completes), we connect two sockets:
//
//   * user-scoped   — receives user-level ephemeral events (`token:near-expiry`
//                     among them) and is the egress channel for user-level
//                     RPC calls issued from this machine.
//   * machine-scoped — receives remote-RPC invocations from other devices that
//                     picked this machine out of the user's fleet; also used
//                     by tooling like `a2ui_render` to push events.
//
// On logout (`AuthStore::clear`) we tear both down. On a pure token rotation
// (same user_id, new access token) we restart to get a connection bound to
// the fresh token.

struct LiveSockets {
    user: Option<Arc<HappySocket>>,
    machine: Option<Arc<HappySocket>>,
    /// The access token that brought these sockets up. We re-dial if this
    /// changes so the server-side handshake sees the rotated token.
    bound_token: Option<String>,
    /// Last user_id we saw — used to detect re-login-as-different-user and
    /// force a full reset of the pair.
    bound_user_id: Option<String>,
}

static LIVE_SOCKETS: OnceLock<Arc<tokio::sync::Mutex<LiveSockets>>> = OnceLock::new();

fn live_sockets() -> Arc<tokio::sync::Mutex<LiveSockets>> {
    LIVE_SOCKETS
        .get_or_init(|| {
            Arc::new(tokio::sync::Mutex::new(LiveSockets {
                user: None,
                machine: None,
                bound_token: None,
                bound_user_id: None,
            }))
        })
        .clone()
}

/// Subscribe to AuthStore changes and keep the user-scoped + machine-scoped
/// sockets in sync. Idempotent — only the first caller actually subscribes.
///
/// Behaviour:
/// * `access_token` absent → disconnect both, clear slots.
/// * `access_token` present and changed (or user_id changed) → disconnect
///   existing pair (if any), reconnect both with the new token.
/// * `access_token` unchanged → no-op.
pub fn spawn_user_and_machine_sockets_guard() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    let Some(store) = auth_store() else {
        log::warn!("socket guard: AuthStore not installed; skipping subscribe");
        return;
    };

    // Run once synchronously against the current snapshot so we don't wait
    // for the next set_tokens to bring sockets up after a cold boot with a
    // persisted access token.
    let snapshot = store.snapshot();
    reconcile_live_sockets_in_background(snapshot);

    store.subscribe(Box::new(|snap: &AuthSnapshot| {
        let snap = snap.clone();
        reconcile_live_sockets_in_background(snap);
    }));

    log::info!("user/machine socket guard subscribed to AuthStore changes");
}

/// Kick off `reconcile_live_sockets` on the tokio runtime without blocking
/// the caller (AuthStore subscribers run on the writer's thread, which may be
/// a sync tauri command handler — we never want to hold that up on a network
/// dial).
fn reconcile_live_sockets_in_background(snap: AuthSnapshot) {
    tokio::spawn(async move {
        if let Err(e) = reconcile_live_sockets(snap).await {
            log::warn!("reconcile_live_sockets failed: {e}");
        }
    });
}

async fn reconcile_live_sockets(snap: AuthSnapshot) -> Result<(), String> {
    let sockets = live_sockets();
    let mut guard = sockets.lock().await;

    let access_token = snap.access_token.clone();
    let user_id = snap.user_id.clone();
    let machine_id = snap.machine_id.clone();

    // Logout path.
    let Some(access_token) = access_token else {
        if guard.user.is_some() || guard.machine.is_some() {
            log::warn!(
                "[auth-clear-trace] AuthStore cleared; disconnecting user/machine sockets; backtrace:\n{}",
                std::backtrace::Backtrace::force_capture()
            );
        }
        if let Some(sock) = guard.user.take() {
            let _ = sock.disconnect().await;
        }
        if let Some(sock) = guard.machine.take() {
            let _ = sock.disconnect().await;
        }
        guard.bound_token = None;
        guard.bound_user_id = None;
        return Ok(());
    };

    // Token / user hasn't changed → nothing to do.
    let user_changed = guard.bound_user_id != user_id;
    let token_changed = guard.bound_token.as_deref() != Some(access_token.as_str());
    if !user_changed && !token_changed && guard.user.is_some() && guard.machine.is_some() {
        return Ok(());
    }

    // Otherwise: disconnect existing pair, reconnect with new token.
    if let Some(sock) = guard.user.take() {
        let _ = sock.disconnect().await;
    }
    if let Some(sock) = guard.machine.take() {
        let _ = sock.disconnect().await;
    }

    let server_url = crate::resolved_happy_server_url();

    // User-scoped — always present once logged in.
    match HappySocket::connect(
        &server_url,
        access_token.clone(),
        ConnectionType::UserScoped,
    )
    .await
    {
        Ok(socket) => {
            let socket = Arc::new(socket);
            subscribe_near_expiry_poke(&socket).await;
            guard.user = Some(socket);
            log::info!("User-scoped Socket.IO connected (user_id={:?})", user_id);
        }
        Err(e) => {
            log::warn!("Failed to connect user-scoped socket: {e}");
        }
    }

    // Machine-scoped — only if we have a machine id.
    if let Some(mid) = machine_id.clone() {
        match HappySocket::connect(
            &server_url,
            access_token.clone(),
            ConnectionType::MachineScoped {
                machine_id: mid.clone(),
            },
        )
        .await
        {
            Ok(socket) => {
                let socket = Arc::new(socket);
                subscribe_near_expiry_poke(&socket).await;
                if let Err(error) =
                    crate::session_relay::attach_machine_socket_listener(socket.clone()).await
                {
                    log::warn!("Failed to attach machine relay listener: {error}");
                }
                // Keep the hook that a2ui_render etc. dip into up to date.
                crate::local_services::install_machine_socket(socket.clone());
                guard.machine = Some(socket);
                log::info!("Machine-scoped Socket.IO connected (machine_id={})", mid);
            }
            Err(e) => {
                log::warn!("Failed to connect machine-scoped socket: {e}");
            }
        }
    } else {
        log::warn!("Skipping machine-scoped socket: AuthStore has no machine_id");
    }

    guard.bound_token = Some(access_token);
    guard.bound_user_id = user_id;
    Ok(())
}

/// Register the `token:near-expiry` handler on a freshly-connected socket so
/// that the server-side hint wakes `refresh_tick_once` ahead of the 60 s
/// guard cadence.
async fn subscribe_near_expiry_poke(socket: &HappySocket) {
    socket
        .on_token_near_expiry(|remaining_ms: i64| async move {
            log::info!(
                "token:near-expiry hint received (remainingMs={remaining_ms}); poking refresh_tick_once"
            );
            tokio::spawn(async move {
                if let Err(e) = refresh_tick_once().await {
                    log::debug!(
                        "refresh_tick_once after near-expiry poke returned soft error: {e}"
                    );
                }
            });
        })
        .await;
}

// ---------------------------------------------------------------------------
// Wire #4 — one-shot machine registration on first login
// ---------------------------------------------------------------------------

/// Per-user deduplication so a single login session only fires one
/// `/v1/machines/register` call, regardless of how many times `set_tokens`
/// rotates the access_token. Cleared when the user logs out.
static REGISTERED_USERS: OnceLock<StdMutex<std::collections::HashSet<String>>> = OnceLock::new();

fn registered_users() -> &'static StdMutex<std::collections::HashSet<String>> {
    REGISTERED_USERS.get_or_init(|| StdMutex::new(std::collections::HashSet::new()))
}

/// Subscribe to AuthStore and call `MachineManager::register` exactly once
/// per logged-in user. Failures are logged and swallowed so a flaky network
/// doesn't block login.
///
/// We de-duplicate on `user_id`: rotating the access token (every 30 min via
/// the refresh guard) does NOT re-register. Logging out wipes the dedup set,
/// so logging back in — even as the same user — re-registers once.
pub fn subscribe_register_machine_once() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    let Some(store) = auth_store() else {
        log::warn!("register-machine guard: AuthStore not installed; skipping subscribe");
        return;
    };

    // Run once against the current snapshot to cover the cold-boot case
    // (process restart with a persisted token).
    register_machine_in_background(store.snapshot());

    store.subscribe(Box::new(|snap: &AuthSnapshot| {
        let snap = snap.clone();
        register_machine_in_background(snap);
    }));

    log::info!("register-machine guard subscribed to AuthStore changes");
}

/// Subscribe to AuthStore and push the current access token into every live
/// Cteno sub-agent subprocess whenever `set_tokens` fires. Covers two rotation
/// paths that previously only the Rust refresh guard handled:
///
///   1. JS-side `ensureFreshAccess` rotation → `cteno_auth_save_credentials`
///      → `AuthStore::set_tokens` (no refresh-guard tick needed).
///   2. Initial login (no agent running yet — broadcast is a no-op).
///
/// Without this, a token rotated by the frontend would only reach sub-agent
/// subprocesses on the next Rust-side refresh-guard tick, so in-flight agent
/// turns could surface a stale-token 401 against the Happy proxy.
pub fn subscribe_broadcast_token_to_agents() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    let Some(store) = auth_store() else {
        log::warn!("agent-token broadcast guard: AuthStore not installed; skipping subscribe");
        return;
    };

    // Cold-boot: broadcast once against the current snapshot so any sub-agents
    // spawned before this subscriber gets wired also pick up the initial token.
    if let Some(tok) = store.snapshot().access_token.clone() {
        spawn_broadcast(tok);
    }

    store.subscribe(Box::new(|snap: &AuthSnapshot| {
        if let Some(tok) = snap.access_token.clone() {
            spawn_broadcast(tok);
        }
    }));

    log::info!("agent-token broadcast guard subscribed to AuthStore changes");
}

/// Fan the broadcast onto the tokio runtime so the AuthStore writer thread
/// (which may be a synchronous tauri command handler) is never blocked.
fn spawn_broadcast(access_token: String) {
    tokio::spawn(async move {
        broadcast_token_to_agents(&access_token).await;
    });
}

/// Outcome of the dedup pre-flight for a register-machine trigger. Exposed
/// for unit tests that want to validate the bookkeeping without running the
/// HTTP call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RegisterDedupOutcome {
    /// Skip because the access token is missing (logout / not-logged-in).
    /// Dedup set is cleared as a side effect.
    LoggedOut,
    /// Skip because this snapshot is not ready (missing machine_id or user_id).
    Incomplete,
    /// Skip because we already issued a register for this user.
    AlreadyRegistered,
    /// Proceed with the register call. Carries the values the caller needs.
    Proceed {
        token: String,
        machine_id: String,
        user_id: String,
    },
}

/// Inspect a snapshot and update the dedup set accordingly. Returns the
/// outcome so the caller decides whether to actually issue the HTTP call.
fn register_dedup(snap: &AuthSnapshot) -> RegisterDedupOutcome {
    if snap.access_token.is_none() {
        if let Ok(mut set) = registered_users().lock() {
            set.clear();
        }
        return RegisterDedupOutcome::LoggedOut;
    }
    let Some(token) = snap.access_token.clone() else {
        return RegisterDedupOutcome::Incomplete;
    };
    let Some(machine_id) = snap.machine_id.clone() else {
        return RegisterDedupOutcome::Incomplete;
    };
    let Some(user_id) = snap.user_id.clone() else {
        return RegisterDedupOutcome::Incomplete;
    };

    let mut set = match registered_users().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !set.insert(user_id.clone()) {
        return RegisterDedupOutcome::AlreadyRegistered;
    }
    RegisterDedupOutcome::Proceed {
        token,
        machine_id,
        user_id,
    }
}

fn register_machine_in_background(snap: AuthSnapshot) {
    let (token, machine_id, user_id) = match register_dedup(&snap) {
        RegisterDedupOutcome::Proceed {
            token,
            machine_id,
            user_id,
        } => (token, machine_id, user_id),
        RegisterDedupOutcome::LoggedOut => return,
        RegisterDedupOutcome::Incomplete => {
            log::debug!("register-machine: snapshot incomplete; skipping");
            return;
        }
        RegisterDedupOutcome::AlreadyRegistered => return,
    };

    #[cfg(feature = "commercial-cloud")]
    tokio::spawn(async move {
        let server_url = crate::resolved_happy_server_url();
        let manager = MachineManager::new(server_url, machine_id.clone(), token);
        match manager.register().await {
            Ok(_info) => {
                log::info!(
                    "POST /v1/machines/register ok (machine_id={machine_id}, user_id={user_id}) after login"
                );
            }
            Err(e) => {
                log::warn!("POST /v1/machines/register failed (non-fatal, user_id={user_id}): {e}");
                // Best-effort rollback: on failure, let a future set_tokens
                // retry by removing this user from the dedup set.
                if let Ok(mut set) = registered_users().lock() {
                    set.remove(&user_id);
                }
            }
        }
    });

    #[cfg(not(feature = "commercial-cloud"))]
    {
        let _ = (token, machine_id, user_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::tempdir;

    // Serialise tests that touch the process-wide `REGISTERED_USERS` slot so
    // parallel test threads don't clobber each other's dedup bookkeeping.
    static REGISTER_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn install_auth_store_is_idempotent() {
        let dir = tempdir().unwrap();
        // Force re-use of global slot across two calls.
        let s1 = install_auth_store(dir.path()).unwrap();
        let s2 = install_auth_store(dir.path()).unwrap();
        // Different dir the second time should still yield the first installed
        // store (idempotency contract). We use the same dir here to avoid
        // accidental assertion flakiness.
        assert!(Arc::ptr_eq(&s1, &s2));
    }

    fn reset_registered_users() {
        if let Ok(mut set) = registered_users().lock() {
            set.clear();
        }
    }

    #[test]
    fn register_dedup_logged_out_clears_set() {
        let _guard = REGISTER_TEST_LOCK.lock().unwrap();
        reset_registered_users();
        // Seed a user, then hand in an empty snapshot (no access_token).
        registered_users().lock().unwrap().insert("u1".into());
        let snap = AuthSnapshot::default();
        assert_eq!(register_dedup(&snap), RegisterDedupOutcome::LoggedOut);
        assert!(registered_users().lock().unwrap().is_empty());
    }

    #[test]
    fn register_dedup_incomplete_when_missing_fields() {
        let _guard = REGISTER_TEST_LOCK.lock().unwrap();
        reset_registered_users();
        let snap = AuthSnapshot {
            access_token: Some("acc".into()),
            refresh_token: Some("ref".into()),
            ..Default::default()
        };
        // Missing machine_id + user_id → Incomplete.
        assert_eq!(register_dedup(&snap), RegisterDedupOutcome::Incomplete);
        // Did not insert anything into the set.
        assert!(registered_users().lock().unwrap().is_empty());
    }

    #[test]
    fn register_dedup_first_hit_proceeds_second_hit_skipped() {
        let _guard = REGISTER_TEST_LOCK.lock().unwrap();
        reset_registered_users();
        let snap = AuthSnapshot {
            access_token: Some("acc".into()),
            refresh_token: Some("ref".into()),
            user_id: Some("user-abc".into()),
            machine_id: Some("machine-xyz".into()),
            ..Default::default()
        };
        match register_dedup(&snap) {
            RegisterDedupOutcome::Proceed {
                token,
                machine_id,
                user_id,
            } => {
                assert_eq!(token, "acc");
                assert_eq!(machine_id, "machine-xyz");
                assert_eq!(user_id, "user-abc");
            }
            other => panic!("expected Proceed, got {:?}", other),
        }
        // Second call with same user_id must dedup.
        assert_eq!(
            register_dedup(&snap),
            RegisterDedupOutcome::AlreadyRegistered
        );
        // Third call with a rotated access_token (same user_id) also dedups —
        // token rotation must NOT re-register per the Wire #4 spec.
        let rotated = AuthSnapshot {
            access_token: Some("acc-rotated".into()),
            ..snap.clone()
        };
        assert_eq!(
            register_dedup(&rotated),
            RegisterDedupOutcome::AlreadyRegistered
        );
    }

    #[test]
    fn register_dedup_logout_then_login_same_user_reregisters() {
        let _guard = REGISTER_TEST_LOCK.lock().unwrap();
        reset_registered_users();
        let snap = AuthSnapshot {
            access_token: Some("acc".into()),
            refresh_token: Some("ref".into()),
            user_id: Some("user-abc".into()),
            machine_id: Some("machine-xyz".into()),
            ..Default::default()
        };
        assert!(matches!(
            register_dedup(&snap),
            RegisterDedupOutcome::Proceed { .. }
        ));
        // Logout.
        assert_eq!(
            register_dedup(&AuthSnapshot::default()),
            RegisterDedupOutcome::LoggedOut
        );
        // Log back in as same user → should Proceed again (fresh session).
        assert!(matches!(
            register_dedup(&snap),
            RegisterDedupOutcome::Proceed { .. }
        ));
    }
}
