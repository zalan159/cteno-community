//! Registry mapping vendor names to [`AgentExecutor`] instances.
//!
//! The registry is built once at boot by [`ExecutorRegistry::build`] and held
//! as an `Arc<ExecutorRegistry>` for the process lifetime. The vendor
//! adapters — `cteno` / `claude` / `codex` / `gemini` — share a single
//! [`SessionStoreProvider`] so their host-side metadata queries land in the
//! same local SQLite file.
//!
//! Binary discovery rules:
//!
//! - **Cteno**: `CTENO_AGENT_PATH` env var → else [`which::which`] PATH
//!   lookup → else sibling `cteno-agent` next to the current executable. The
//!   bundled sibling binary is still expected to resolve in desktop builds.
//! - **Claude / Codex / Gemini**: `CLAUDE_PATH` / `CODEX_PATH` /
//!   `GEMINI_PATH` env var → else [`which::which`] PATH lookup. When the
//!   vendor CLI is not installed the registry stores `None` for that slot and
//!   [`ExecutorRegistry::resolve`] surfaces an error; callers should treat the
//!   vendor as unavailable (UI greys out the option) rather than failing the
//!   whole boot.
//!
//! The registry itself does not consult the cached capability manifest —
//! callers wanting to know whether a feature like `supports_runtime_set_model`
//! is available must invoke [`AgentExecutor::capabilities`] after `resolve`.
//!
//! # Connection cache (Phase 2 of the pre-connection refactor)
//!
//! The registry owns at most one [`ConnectionHandle`] per vendor, opened
//! lazily by [`ExecutorRegistry::get_or_open_connection`] or eagerly at boot
//! by [`ExecutorRegistry::preheat_all`]. Each connection is wrapped in a
//! [`Mutex`] so concurrent `open_connection` callers serialize and observe the
//! first caller's result. Vendors whose capability manifest reports
//! `supports_multi_session_per_process = false` (currently Claude) still get
//! health probes but no pre-warmed handle — every session spawn opens a fresh
//! connection via the default `start_session_on` path.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use cteno_host_runtime::SubprocessSupervisor;
use multi_agent_runtime_core::{
    AgentExecutor, AgentExecutorError, ConnectionHandle, ConnectionHealth, ConnectionSpec,
    SessionRef, SessionStoreProvider, SpawnSessionSpec,
};
use tokio::sync::{Mutex, RwLock};

/// Stable vendor names used throughout the registry surface. Using `&'static
/// str` means the cache keys are cheap to clone and the UI / RPC callers can
/// match against them without allocation.
pub const VENDOR_CTENO: &str = "cteno";
pub const VENDOR_CLAUDE: &str = "claude";
pub const VENDOR_CODEX: &str = "codex";
pub const VENDOR_GEMINI: &str = "gemini";

const ALL_VENDORS: &[&str] = &[VENDOR_CTENO, VENDOR_CLAUDE, VENDOR_CODEX, VENDOR_GEMINI];

/// Lifecycle state reported for each vendor's cached connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VendorConnectionProbeState {
    /// Probe has never been attempted (or slot was cleared).
    Unknown,
    /// Probe currently in flight.
    Probing,
    /// Last probe succeeded; a handle may or may not be cached depending on
    /// whether the vendor supports multi-session reuse.
    Connected,
    /// Last probe attempt failed. `reason` captures the short error.
    Failed,
}

/// Snapshot of the most recent probe outcome for one vendor.
#[derive(Debug, Clone)]
pub struct VendorConnectionProbe {
    pub state: VendorConnectionProbeState,
    pub reason: Option<String>,
    pub checked_at_unix_ms: i64,
    pub latency_ms: Option<u64>,
}

impl VendorConnectionProbe {
    fn unknown() -> Self {
        Self {
            state: VendorConnectionProbeState::Unknown,
            reason: None,
            checked_at_unix_ms: 0,
            latency_ms: None,
        }
    }
}

struct VendorConnSlot {
    /// Currently-cached live connection. `None` when the vendor has no open
    /// connection (never opened, adapter doesn't support reuse, or the last
    /// probe detected a dead transport and cleared it).
    handle: Option<ConnectionHandle>,
    probe: VendorConnectionProbe,
}

impl VendorConnSlot {
    fn fresh() -> Self {
        Self {
            handle: None,
            probe: VendorConnectionProbe::unknown(),
        }
    }
}

/// Fully-constructed registry exposing one `Arc<dyn AgentExecutor>` per
/// available vendor.
pub struct ExecutorRegistry {
    cteno: Arc<dyn AgentExecutor>,
    /// Concrete-typed handle on the Cteno adapter kept alongside the trait
    /// object so we can reach its vendor-specific helpers (e.g.
    /// `broadcast_token_refresh` after a host-side access token rotation).
    cteno_concrete: Arc<multi_agent_runtime_cteno::CtenoAgentExecutor>,
    claude: Option<Arc<dyn AgentExecutor>>,
    codex: Option<Arc<dyn AgentExecutor>>,
    gemini: Option<Arc<dyn AgentExecutor>>,
    /// Per-vendor connection cache. Outer `RwLock` protects the map shape
    /// (added/removed keys); inner `Mutex` serializes callers racing to open
    /// the same vendor so only one subprocess is spawned.
    connections: Arc<RwLock<HashMap<&'static str, Arc<Mutex<VendorConnSlot>>>>>,
}

impl ExecutorRegistry {
    /// Build a registry sharing `session_store` across all vendor adapters.
    /// Returns `Err` only when the Cteno executor cannot be constructed
    /// (missing cteno-agent binary); missing claude/codex/gemini CLIs are
    /// tolerated by storing `None`.
    pub fn build(session_store: Arc<dyn SessionStoreProvider>) -> Result<Self, String> {
        Self::build_with_supervisor(session_store, None)
    }

    /// Variant of [`build`] that wires a `SubprocessSupervisor` into the
    /// Cteno adapter so its spawned children are tracked in the daemon's
    /// pid file. Claude / Codex / Gemini adapters do not currently take a
    /// supervisor (their subprocess spawning lives inside their own crates
    /// and will be wired in a future commit).
    pub fn build_with_supervisor(
        session_store: Arc<dyn SessionStoreProvider>,
        supervisor: Option<Arc<SubprocessSupervisor>>,
    ) -> Result<Self, String> {
        let cteno_path = resolve_cteno_agent_path()?;
        log::info!(
            "ExecutorRegistry: cteno-agent binary = {}",
            cteno_path.display()
        );
        let cteno_exec =
            multi_agent_runtime_cteno::CtenoAgentExecutor::new(cteno_path, session_store.clone());
        let cteno_exec = match supervisor.clone() {
            Some(sup) => cteno_exec.with_supervisor(sup),
            None => cteno_exec,
        };
        let cteno_concrete = Arc::new(cteno_exec);
        let cteno: Arc<dyn AgentExecutor> = cteno_concrete.clone();

        let claude = match resolve_claude_path() {
            Some(path) => {
                log::info!("ExecutorRegistry: claude CLI = {}", path.display());
                Some(
                    Arc::new(multi_agent_runtime_claude::ClaudeAgentExecutor::new(
                        path,
                        session_store.clone(),
                    )) as Arc<dyn AgentExecutor>,
                )
            }
            None => {
                log::info!(
                    "ExecutorRegistry: claude CLI not found (CLAUDE_PATH unset, not in PATH)"
                );
                None
            }
        };

        let codex = match resolve_codex_path() {
            Some(path) => {
                log::info!("ExecutorRegistry: codex CLI = {}", path.display());
                Some(Arc::new(multi_agent_runtime_codex::CodexAgentExecutor::new(
                    path,
                    session_store.clone(),
                )) as Arc<dyn AgentExecutor>)
            }
            None => {
                log::info!("ExecutorRegistry: codex CLI not found (CODEX_PATH unset, not in PATH)");
                None
            }
        };

        let gemini = match resolve_gemini_path() {
            Some(path) => {
                log::info!("ExecutorRegistry: gemini CLI = {}", path.display());
                Some(
                    Arc::new(multi_agent_runtime_gemini::GeminiAgentExecutor::new(
                        path,
                        session_store.clone(),
                    )) as Arc<dyn AgentExecutor>,
                )
            }
            None => {
                log::info!(
                    "ExecutorRegistry: gemini CLI not found (GEMINI_PATH unset, not in PATH)"
                );
                None
            }
        };

        Ok(Self {
            cteno,
            cteno_concrete,
            claude,
            codex,
            gemini,
            connections: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Concrete-typed Cteno adapter (lets callers invoke vendor-specific
    /// helpers like `broadcast_token_refresh`). The trait-object version is
    /// still the normal path — this escape hatch is only for the auth
    /// refresh guard.
    pub fn cteno_concrete(&self) -> Arc<multi_agent_runtime_cteno::CtenoAgentExecutor> {
        self.cteno_concrete.clone()
    }

    /// Resolve a vendor name to its backing executor. Fails with a
    /// `String` error when the vendor is unknown or unavailable on this host.
    pub fn resolve(&self, vendor: &str) -> Result<Arc<dyn AgentExecutor>, String> {
        match vendor {
            VENDOR_CTENO => Ok(self.cteno.clone()),
            VENDOR_CLAUDE => self
                .claude
                .clone()
                .ok_or_else(|| "claude CLI not installed on this host".to_string()),
            VENDOR_CODEX => self
                .codex
                .clone()
                .ok_or_else(|| "codex CLI not installed on this host".to_string()),
            VENDOR_GEMINI => self
                .gemini
                .clone()
                .ok_or_else(|| "gemini CLI not installed on this host".to_string()),
            other => Err(format!("unknown vendor: {other}")),
        }
    }

    /// Whether the vendor's CLI/executor is present on this host.
    pub fn is_vendor_installed(&self, vendor: &str) -> Result<bool, String> {
        match vendor {
            VENDOR_CTENO => Ok(true),
            VENDOR_CLAUDE => Ok(self.claude.is_some()),
            VENDOR_CODEX => Ok(self.codex.is_some()),
            VENDOR_GEMINI => Ok(self.gemini.is_some()),
            other => Err(format!("unknown vendor: {other}")),
        }
    }

    /// Convenience: list the vendor names that resolve successfully.
    pub fn available_vendors(&self) -> Vec<&'static str> {
        let mut v = vec![VENDOR_CTENO];
        if self.claude.is_some() {
            v.push(VENDOR_CLAUDE);
        }
        if self.codex.is_some() {
            v.push(VENDOR_CODEX);
        }
        if self.gemini.is_some() {
            v.push(VENDOR_GEMINI);
        }
        v
    }

    // ------------------------------------------------------------------
    // Connection cache
    // ------------------------------------------------------------------

    /// Canonicalize an incoming vendor string into the `&'static str` used as
    /// the cache key. Returns `None` for unknown vendors.
    fn vendor_key(vendor: &str) -> Option<&'static str> {
        match vendor {
            VENDOR_CTENO => Some(VENDOR_CTENO),
            VENDOR_CLAUDE => Some(VENDOR_CLAUDE),
            VENDOR_CODEX => Some(VENDOR_CODEX),
            VENDOR_GEMINI => Some(VENDOR_GEMINI),
            _ => None,
        }
    }

    async fn slot_for(&self, vendor: &'static str) -> Arc<Mutex<VendorConnSlot>> {
        {
            let map = self.connections.read().await;
            if let Some(slot) = map.get(vendor) {
                return slot.clone();
            }
        }
        let mut map = self.connections.write().await;
        map.entry(vendor)
            .or_insert_with(|| Arc::new(Mutex::new(VendorConnSlot::fresh())))
            .clone()
    }

    /// Open (or reuse) the cached connection for `vendor`. On race, only one
    /// subprocess is spawned and subsequent callers observe the first caller's
    /// result through the per-slot `Mutex`.
    ///
    /// Cached handles are **always** health-checked via
    /// [`AgentExecutor::check_connection`] before they are returned. If the
    /// handle is `Dead` (child exited, demuxer saw EOF, transport error) the
    /// slot is cleared and a fresh `open_connection` is attempted. Without
    /// this check a long-idle preheat handle — say one cached hours earlier —
    /// would be handed to the session-spawn caller and surface as
    /// `"codex app-server connection is closed; reopen before starting a
    /// session"` on the next `start_session_on`.
    pub async fn open_connection(
        &self,
        vendor: &'static str,
    ) -> Result<ConnectionHandle, AgentExecutorError> {
        let executor = self
            .resolve(vendor)
            .map_err(|e| AgentExecutorError::Io(e))?;
        let slot = self.slot_for(vendor).await;
        let mut guard = slot.lock().await;

        if let Some(handle) = guard.handle.clone() {
            // Validate the cached handle before handing it out. The check is
            // cheap (typically a `try_wait` on the child + an atomic load).
            match executor.check_connection(&handle).await {
                Ok(ConnectionHealth::Healthy) => return Ok(handle),
                Ok(ConnectionHealth::Dead { reason }) => {
                    log::warn!(
                        "ExecutorRegistry: cached {vendor} handle is dead ({reason}) — reopening"
                    );
                    guard.handle = None;
                    // Best-effort local cleanup on the adapter side; ignore
                    // errors because the slot invariant is what matters.
                    let _ = executor.close_connection(handle).await;
                }
                Err(err) => {
                    log::warn!(
                        "ExecutorRegistry: check_connection({vendor}) errored ({}) — reopening",
                        short_reason(&err)
                    );
                    guard.handle = None;
                }
            }
        }

        let started = Instant::now();
        match executor.open_connection(ConnectionSpec::default()).await {
            Ok(handle) => {
                guard.handle = Some(handle.clone());
                guard.probe = VendorConnectionProbe {
                    state: VendorConnectionProbeState::Connected,
                    reason: None,
                    checked_at_unix_ms: Utc::now().timestamp_millis(),
                    latency_ms: Some(started.elapsed().as_millis() as u64),
                };
                Ok(handle)
            }
            Err(err) => {
                let reason = short_reason(&err);
                log::warn!("ExecutorRegistry: open_connection({vendor}) failed: {reason}");
                guard.handle = None;
                guard.probe = VendorConnectionProbe {
                    state: VendorConnectionProbeState::Failed,
                    reason: Some(reason),
                    checked_at_unix_ms: Utc::now().timestamp_millis(),
                    latency_ms: Some(started.elapsed().as_millis() as u64),
                };
                Err(err)
            }
        }
    }

    /// Is `err` a connection-closed / broken-transport failure that a fresh
    /// `open_connection` can plausibly recover from? Intentionally broad
    /// substring matching — every vendor adapter phrases the error slightly
    /// differently and the helper is only used as a retry gate, not as part
    /// of user-visible logic.
    fn is_connection_closed_error(err: &AgentExecutorError) -> bool {
        let s = err.to_string().to_ascii_lowercase();
        s.contains("connection is closed")
            || s.contains("connection closed")
            || s.contains("connectionclosed")
            || s.contains("broken pipe")
            || s.contains("brokenpipe")
            || s.contains("stdout eof")
            || s.contains("stdin closed")
            || s.contains("app-server exited")
            || s.contains("reopen before starting")
    }

    /// Spawn a session on `vendor` via `start_session_on`, transparently
    /// reopening the cached connection once if the first attempt trips a
    /// `"connection is closed"`-class error.
    ///
    /// This is the session-spawn call-site's armor against a subtle window
    /// the health check in [`Self::open_connection`] cannot close on its own:
    /// the handle may be healthy at check time but die between the check and
    /// the `start_session_on` JSON-RPC round trip (child killed by OOM,
    /// macOS pipe harvesting after a sleep, etc.). A single in-place retry
    /// is safe because `close_connection` is idempotent and the caller has
    /// not committed any session-level state yet.
    ///
    /// Retry ceiling is **one** — a second consecutive closed-connection
    /// error is surfaced unchanged so an operator-visible Failed probe makes
    /// it into the UI instead of an infinite reopen loop.
    pub async fn start_session_with_autoreopen(
        &self,
        vendor: &'static str,
        spec: SpawnSessionSpec,
    ) -> Result<SessionRef, AgentExecutorError> {
        // Cteno stdio loads project MCP config on each Init, so make sure a
        // freshly-opened workdir has its `{project}/.cteno/*` projection before
        // the session starts. Other vendor CLIs read config at subprocess
        // startup and still rely on boot-time reconcile for cached handles.
        if vendor == VENDOR_CTENO && spec.workdir.is_absolute() && spec.workdir.exists() {
            crate::agent_sync_bridge::reconcile_project_now(&spec.workdir).await;
        }
        let executor = self.resolve(vendor).map_err(AgentExecutorError::Io)?;
        let handle = self.get_or_open_connection(vendor).await?;
        match executor.start_session_on(&handle, spec.clone()).await {
            Ok(session) => Ok(session),
            Err(err) if Self::is_connection_closed_error(&err) => {
                log::warn!(
                    "ExecutorRegistry: start_session_on({vendor}) failed on cached handle \
                     ({}) — dropping slot and retrying once",
                    short_reason(&err)
                );
                // Clear the slot so the next open_connection dials a fresh
                // subprocess. `close_connection` tolerates an already-dead
                // handle; we don't care about its return.
                let _ = self.close_connection(vendor).await;
                let fresh = self.get_or_open_connection(vendor).await?;
                executor.start_session_on(&fresh, spec).await
            }
            Err(err) => Err(err),
        }
    }

    /// Close the cached connection for `vendor` if any. Idempotent.
    pub async fn close_connection(&self, vendor: &'static str) -> Result<(), AgentExecutorError> {
        let slot = self.slot_for(vendor).await;
        let handle = {
            let mut guard = slot.lock().await;
            guard.handle.take()
        };
        if let Some(handle) = handle {
            let executor = self
                .resolve(vendor)
                .map_err(|e| AgentExecutorError::Io(e))?;
            // Best-effort close; log but don't propagate Unsupported because
            // the registry invariant ("no cached handle") already holds.
            if let Err(err) = executor.close_connection(handle).await {
                log::warn!(
                    "ExecutorRegistry: close_connection({vendor}) returned {}",
                    err
                );
            }
        }
        Ok(())
    }

    /// Session spawn call-site helper: return the cached connection, opening
    /// it on demand. Identical to [`Self::open_connection`] at present but
    /// named separately so the session-layer intent is explicit.
    pub async fn get_or_open_connection(
        &self,
        vendor: &'static str,
    ) -> Result<ConnectionHandle, AgentExecutorError> {
        self.open_connection(vendor).await
    }

    /// Probe a vendor's connection health. Semantics:
    ///
    /// - If a cached handle exists, call `check_connection` on it. On
    ///   `Healthy`, return `Connected` and keep the handle. On `Dead`, drop the
    ///   handle and attempt to re-open, returning the open result.
    /// - If no handle exists, call `open_connection` with `spec.probe = true`
    ///   so adapters may short-circuit heavyweight setup. The returned handle
    ///   is cached only when the vendor capability manifest reports
    ///   `supports_multi_session_per_process = true`; otherwise the adapter
    ///   result is treated as a one-shot handshake and closed.
    pub async fn probe(&self, vendor: &'static str) -> VendorConnectionProbe {
        let executor = match self.resolve(vendor) {
            Ok(e) => e,
            Err(err) => {
                return VendorConnectionProbe {
                    state: VendorConnectionProbeState::Failed,
                    reason: Some(err),
                    checked_at_unix_ms: Utc::now().timestamp_millis(),
                    latency_ms: None,
                };
            }
        };
        let slot = self.slot_for(vendor).await;
        let mut guard = slot.lock().await;
        guard.probe = VendorConnectionProbe {
            state: VendorConnectionProbeState::Probing,
            reason: guard.probe.reason.clone(),
            checked_at_unix_ms: guard.probe.checked_at_unix_ms,
            latency_ms: guard.probe.latency_ms,
        };

        let started = Instant::now();

        if let Some(handle) = guard.handle.clone() {
            match executor.check_connection(&handle).await {
                Ok(ConnectionHealth::Healthy) => {
                    guard.probe = VendorConnectionProbe {
                        state: VendorConnectionProbeState::Connected,
                        reason: None,
                        checked_at_unix_ms: Utc::now().timestamp_millis(),
                        latency_ms: Some(started.elapsed().as_millis() as u64),
                    };
                    return guard.probe.clone();
                }
                Ok(ConnectionHealth::Dead { reason }) => {
                    log::warn!(
                        "ExecutorRegistry: probe({vendor}) found dead handle: {reason} — retrying"
                    );
                    // Drop the dead handle; fall through to the re-open path below.
                    guard.handle = None;
                }
                Err(err) => {
                    let reason = short_reason(&err);
                    log::warn!(
                        "ExecutorRegistry: probe({vendor}) check_connection errored: {reason}"
                    );
                    guard.handle = None;
                    guard.probe = VendorConnectionProbe {
                        state: VendorConnectionProbeState::Failed,
                        reason: Some(reason),
                        checked_at_unix_ms: Utc::now().timestamp_millis(),
                        latency_ms: Some(started.elapsed().as_millis() as u64),
                    };
                    return guard.probe.clone();
                }
            }
        }

        // No (live) handle — perform a lightweight handshake probe.
        let caps = executor.capabilities();
        let multi_session = caps.supports_multi_session_per_process;

        let mut spec = ConnectionSpec::default();
        spec.probe = true;
        match executor.open_connection(spec).await {
            Ok(handle) => {
                if multi_session {
                    guard.handle = Some(handle);
                } else {
                    // Adapters without multi-session reuse can't safely hold
                    // the probe handle across session spawns. Close it now so
                    // the probe is pure.
                    if let Err(err) = executor.close_connection(handle).await {
                        log::warn!(
                            "ExecutorRegistry: probe({vendor}) close-after-probe errored: {}",
                            err
                        );
                    }
                }
                guard.probe = VendorConnectionProbe {
                    state: VendorConnectionProbeState::Connected,
                    reason: None,
                    checked_at_unix_ms: Utc::now().timestamp_millis(),
                    latency_ms: Some(started.elapsed().as_millis() as u64),
                };
            }
            Err(AgentExecutorError::Unsupported { capability }) => {
                // Adapter hasn't implemented open_connection — not a failure,
                // just an older transport. Report Connected-with-note because
                // session spawn via `spawn_session` still works.
                guard.probe = VendorConnectionProbe {
                    state: VendorConnectionProbeState::Connected,
                    reason: Some(format!(
                        "{capability} unsupported; fallback to spawn_session"
                    )),
                    checked_at_unix_ms: Utc::now().timestamp_millis(),
                    latency_ms: Some(started.elapsed().as_millis() as u64),
                };
            }
            Err(err) => {
                let reason = short_reason(&err);
                log::warn!("ExecutorRegistry: probe({vendor}) open_connection errored: {reason}");
                guard.handle = None;
                guard.probe = VendorConnectionProbe {
                    state: VendorConnectionProbeState::Failed,
                    reason: Some(reason),
                    checked_at_unix_ms: Utc::now().timestamp_millis(),
                    latency_ms: Some(started.elapsed().as_millis() as u64),
                };
            }
        }

        guard.probe.clone()
    }

    /// Run [`Self::probe`] across every installed vendor in parallel.
    pub async fn probe_all(&self) -> Vec<(&'static str, VendorConnectionProbe)> {
        let mut set = tokio::task::JoinSet::new();
        for &vendor in ALL_VENDORS {
            // Skip uninstalled vendors — they'd just error out with a
            // predictable "CLI not installed" Failed result every time, which
            // is noise for the UI.
            if !self.is_vendor_installed(vendor).unwrap_or(false) {
                continue;
            }
            // Clone the inner state we need in the spawned task.
            let connections = self.connections.clone();
            let executor = match self.resolve(vendor) {
                Ok(e) => e,
                Err(_) => continue,
            };
            set.spawn(async move {
                let probe = probe_with_resolved(vendor, executor, connections).await;
                (vendor, probe)
            });
        }

        let mut out = Vec::new();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(result) => out.push(result),
                Err(err) => log::warn!("ExecutorRegistry: probe_all join error: {err}"),
            }
        }
        out
    }

    /// Called once at boot. For each installed vendor:
    ///
    /// 1. Probe connection health.
    /// 2. If the vendor supports multi-session reuse AND the probe succeeded,
    ///    call `open_connection` to cache a live handle so the first session
    ///    spawn is instant.
    pub async fn preheat_all(&self) {
        for &vendor in ALL_VENDORS {
            if !self.is_vendor_installed(vendor).unwrap_or(false) {
                continue;
            }
            let executor = match self.resolve(vendor) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let caps = executor.capabilities();
            let multi_session = caps.supports_multi_session_per_process;

            let probe = self.probe(vendor).await;
            match probe.state {
                VendorConnectionProbeState::Connected => {
                    if multi_session {
                        // probe may already have cached the handle for
                        // multi-session vendors. Calling open_connection is
                        // still cheap (idempotent) and ensures we have a real
                        // (non-probe) handle.
                        match self.open_connection(vendor).await {
                            Ok(_) => log::info!(
                                "ExecutorRegistry: preheat({vendor}) ok; handle cached ({:?}ms)",
                                probe.latency_ms
                            ),
                            Err(err) => log::warn!(
                                "ExecutorRegistry: preheat({vendor}) open_connection failed: {}",
                                short_reason(&err)
                            ),
                        }
                    } else {
                        log::info!(
                            "ExecutorRegistry: preheat({vendor}) ok; no pre-warm (1:1 conn:session)"
                        );
                    }
                }
                VendorConnectionProbeState::Failed => {
                    log::warn!(
                        "ExecutorRegistry: preheat({vendor}) failed: {}",
                        probe.reason.as_deref().unwrap_or("<no reason>")
                    );
                }
                VendorConnectionProbeState::Unknown | VendorConnectionProbeState::Probing => {
                    // Should not happen post-probe, but tolerate gracefully.
                }
            }
        }
    }

    /// Snapshot of every vendor's last probe state. Returns an owned map
    /// keyed by `String` so callers can serialize it without holding the
    /// registry lock.
    pub async fn snapshot_probes(&self) -> HashMap<String, VendorConnectionProbe> {
        let map = self.connections.read().await;
        let mut out = HashMap::new();
        for (vendor, slot) in map.iter() {
            let guard = slot.lock().await;
            out.insert((*vendor).to_string(), guard.probe.clone());
        }
        out
    }

    /// Force a fresh probe for `vendor`, dropping any cached handle first.
    /// Used by the `probe_vendor_connection` RPC so the UI can surface the
    /// *current* vendor state regardless of the cache.
    pub async fn refresh(&self, vendor: &'static str) -> VendorConnectionProbe {
        // Close any cached handle so the subsequent probe starts from a clean
        // slate. Ignore errors — even if close fails the handle is already
        // removed from the slot.
        let _ = self.close_connection(vendor).await;
        self.probe(vendor).await
    }
}

/// Free function variant of `probe` used by `probe_all` so we can clone the
/// minimum set of state into each spawned task (the registry itself isn't
/// `Clone`, but its internal `Arc`s are).
async fn probe_with_resolved(
    vendor: &'static str,
    executor: Arc<dyn AgentExecutor>,
    connections: Arc<RwLock<HashMap<&'static str, Arc<Mutex<VendorConnSlot>>>>>,
) -> VendorConnectionProbe {
    let slot = {
        let map = connections.read().await;
        if let Some(slot) = map.get(vendor) {
            slot.clone()
        } else {
            drop(map);
            let mut write = connections.write().await;
            write
                .entry(vendor)
                .or_insert_with(|| Arc::new(Mutex::new(VendorConnSlot::fresh())))
                .clone()
        }
    };
    let mut guard = slot.lock().await;

    guard.probe = VendorConnectionProbe {
        state: VendorConnectionProbeState::Probing,
        reason: guard.probe.reason.clone(),
        checked_at_unix_ms: guard.probe.checked_at_unix_ms,
        latency_ms: guard.probe.latency_ms,
    };
    let started = Instant::now();

    if let Some(handle) = guard.handle.clone() {
        match executor.check_connection(&handle).await {
            Ok(ConnectionHealth::Healthy) => {
                guard.probe = VendorConnectionProbe {
                    state: VendorConnectionProbeState::Connected,
                    reason: None,
                    checked_at_unix_ms: Utc::now().timestamp_millis(),
                    latency_ms: Some(started.elapsed().as_millis() as u64),
                };
                return guard.probe.clone();
            }
            Ok(ConnectionHealth::Dead { reason }) => {
                log::warn!(
                    "ExecutorRegistry: probe_all({vendor}) dead handle: {reason} — retrying"
                );
                guard.handle = None;
            }
            Err(err) => {
                let reason = short_reason(&err);
                guard.handle = None;
                guard.probe = VendorConnectionProbe {
                    state: VendorConnectionProbeState::Failed,
                    reason: Some(reason),
                    checked_at_unix_ms: Utc::now().timestamp_millis(),
                    latency_ms: Some(started.elapsed().as_millis() as u64),
                };
                return guard.probe.clone();
            }
        }
    }

    let caps = executor.capabilities();
    let multi_session = caps.supports_multi_session_per_process;
    let mut spec = ConnectionSpec::default();
    spec.probe = true;
    match executor.open_connection(spec).await {
        Ok(handle) => {
            if multi_session {
                guard.handle = Some(handle);
            } else if let Err(err) = executor.close_connection(handle).await {
                log::warn!(
                    "ExecutorRegistry: probe_all({vendor}) close-after-probe errored: {err}"
                );
            }
            guard.probe = VendorConnectionProbe {
                state: VendorConnectionProbeState::Connected,
                reason: None,
                checked_at_unix_ms: Utc::now().timestamp_millis(),
                latency_ms: Some(started.elapsed().as_millis() as u64),
            };
        }
        Err(AgentExecutorError::Unsupported { capability }) => {
            guard.probe = VendorConnectionProbe {
                state: VendorConnectionProbeState::Connected,
                reason: Some(format!(
                    "{capability} unsupported; fallback to spawn_session"
                )),
                checked_at_unix_ms: Utc::now().timestamp_millis(),
                latency_ms: Some(started.elapsed().as_millis() as u64),
            };
        }
        Err(err) => {
            let reason = short_reason(&err);
            guard.probe = VendorConnectionProbe {
                state: VendorConnectionProbeState::Failed,
                reason: Some(reason),
                checked_at_unix_ms: Utc::now().timestamp_millis(),
                latency_ms: Some(started.elapsed().as_millis() as u64),
            };
        }
    }

    guard.probe.clone()
}

fn short_reason(err: &AgentExecutorError) -> String {
    let raw = err.to_string();
    if raw.len() > 240 {
        format!("{}…", &raw[..240])
    } else {
        raw
    }
}

fn resolve_vendor_cli_path(env_var: &str, binary_name: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(env_var) {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    which::which(binary_name).ok()
}

/// Locate the `cteno-agent` sidecar binary. Falls back to `CTENO_AGENT_PATH`,
/// then PATH `which`, then a sibling of the current executable.
fn resolve_cteno_agent_path() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("CTENO_AGENT_PATH") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Ok(path);
        }
        log::warn!(
            "CTENO_AGENT_PATH set but file not found: {} — falling back to PATH lookup, then sibling lookup",
            path.display()
        );
    }

    if let Some(path) = which::which("cteno-agent").ok() {
        return Ok(path);
    }

    for candidate in dev_cteno_agent_candidates() {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    let exe = std::env::current_exe().map_err(|e| format!("current_exe lookup failed: {e}"))?;
    let bin_dir = exe
        .parent()
        .ok_or_else(|| "current_exe has no parent directory".to_string())?;
    let candidate = bin_dir.join("cteno-agent");
    if candidate.is_file() {
        return Ok(candidate);
    }

    #[cfg(windows)]
    {
        let candidate_exe = bin_dir.join("cteno-agent.exe");
        if candidate_exe.is_file() {
            return Ok(candidate_exe);
        }
    }

    Err(format!(
        "cteno-agent binary not found: tried CTENO_AGENT_PATH env, PATH lookup, and {}",
        candidate.display()
    ))
}

fn dev_cteno_agent_candidates() -> Vec<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let bin = if cfg!(windows) {
        "cteno-agent.exe"
    } else {
        "cteno-agent"
    };
    vec![
        manifest_dir.join("target/debug").join(bin),
        manifest_dir.join("target/release").join(bin),
        manifest_dir
            .join("../../../packages/agents/rust/crates/cteno-agent-stdio/target/debug")
            .join(bin),
        manifest_dir
            .join("../../../packages/agents/rust/crates/cteno-agent-stdio/target/release")
            .join(bin),
    ]
}

/// Locate the `claude` CLI binary, respecting `CLAUDE_PATH`.
fn resolve_claude_path() -> Option<PathBuf> {
    resolve_vendor_cli_path("CLAUDE_PATH", "claude")
}

/// Locate the `codex` CLI binary, respecting `CODEX_PATH`.
fn resolve_codex_path() -> Option<PathBuf> {
    resolve_vendor_cli_path("CODEX_PATH", "codex")
}

/// Locate the `gemini` CLI binary, respecting `GEMINI_PATH`.
fn resolve_gemini_path() -> Option<PathBuf> {
    resolve_vendor_cli_path("GEMINI_PATH", "gemini")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use multi_agent_runtime_core::{
        AgentCapabilities, ConnectionHandleId, DeltaKind, EventStream, ExecutorEvent, ModelSpec,
        NativeMessage, NativeSessionId, Pagination, PermissionDecision, PermissionMode,
        PermissionModeKind, ResumeHints, SessionFilter, SessionInfo, SessionMeta, SessionRef,
        SpawnSessionSpec, UserMessage,
    };
    use std::borrow::Cow;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Test fake implementing `AgentExecutor` with counters + configurable
    /// failure modes covering the connection-reuse seam only.
    struct MockExecutor {
        vendor: &'static str,
        supports_multi: bool,
        open_calls: Arc<AtomicU32>,
        close_calls: Arc<AtomicU32>,
        check_calls: Arc<AtomicU32>,
        /// When true, open_connection errors out unconditionally.
        fail_open: Arc<std::sync::atomic::AtomicBool>,
        /// When true, check_connection returns `Dead`.
        force_dead: Arc<std::sync::atomic::AtomicBool>,
    }

    impl MockExecutor {
        fn new(vendor: &'static str, supports_multi: bool) -> Arc<Self> {
            Arc::new(Self {
                vendor,
                supports_multi,
                open_calls: Arc::new(AtomicU32::new(0)),
                close_calls: Arc::new(AtomicU32::new(0)),
                check_calls: Arc::new(AtomicU32::new(0)),
                fail_open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                force_dead: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            })
        }

        fn open_count(&self) -> u32 {
            self.open_calls.load(Ordering::SeqCst)
        }
        fn close_count(&self) -> u32 {
            self.close_calls.load(Ordering::SeqCst)
        }
        fn check_count(&self) -> u32 {
            self.check_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl AgentExecutor for MockExecutor {
        fn capabilities(&self) -> AgentCapabilities {
            AgentCapabilities {
                name: Cow::Borrowed(self.vendor),
                protocol_version: Cow::Borrowed("mock"),
                supports_list_sessions: false,
                supports_get_messages: false,
                supports_runtime_set_model: false,
                permission_mode_kind: PermissionModeKind::Static,
                supports_resume: false,
                supports_multi_session_per_process: self.supports_multi,
                supports_injected_tools: false,
                supports_permission_closure: false,
                supports_interrupt: false,
            }
        }

        async fn spawn_session(
            &self,
            _spec: SpawnSessionSpec,
        ) -> Result<SessionRef, AgentExecutorError> {
            Err(AgentExecutorError::Unsupported {
                capability: "spawn_session",
            })
        }
        async fn resume_session(
            &self,
            _session_id: NativeSessionId,
            _hints: ResumeHints,
        ) -> Result<SessionRef, AgentExecutorError> {
            Err(AgentExecutorError::Unsupported {
                capability: "resume_session",
            })
        }
        async fn send_message(
            &self,
            _session: &SessionRef,
            _message: UserMessage,
        ) -> Result<EventStream, AgentExecutorError> {
            Ok(Box::pin(futures_util::stream::empty::<
                Result<ExecutorEvent, AgentExecutorError>,
            >()))
        }
        async fn respond_to_permission(
            &self,
            _session: &SessionRef,
            _request_id: String,
            _decision: PermissionDecision,
        ) -> Result<(), AgentExecutorError> {
            Ok(())
        }
        async fn interrupt(&self, _session: &SessionRef) -> Result<(), AgentExecutorError> {
            Ok(())
        }
        async fn close_session(&self, _session: &SessionRef) -> Result<(), AgentExecutorError> {
            Ok(())
        }
        async fn set_permission_mode(
            &self,
            _session: &SessionRef,
            _mode: PermissionMode,
        ) -> Result<(), AgentExecutorError> {
            Ok(())
        }
        async fn set_model(
            &self,
            _session: &SessionRef,
            _model: ModelSpec,
        ) -> Result<multi_agent_runtime_core::ModelChangeOutcome, AgentExecutorError> {
            Ok(multi_agent_runtime_core::ModelChangeOutcome::Unsupported)
        }
        async fn list_sessions(
            &self,
            _filter: SessionFilter,
        ) -> Result<Vec<SessionMeta>, AgentExecutorError> {
            Ok(Vec::new())
        }
        async fn get_session_info(
            &self,
            _session_id: &NativeSessionId,
        ) -> Result<SessionInfo, AgentExecutorError> {
            Err(AgentExecutorError::Unsupported {
                capability: "get_session_info",
            })
        }
        async fn get_session_messages(
            &self,
            _session_id: &NativeSessionId,
            _pagination: Pagination,
        ) -> Result<Vec<NativeMessage>, AgentExecutorError> {
            Ok(Vec::new())
        }

        async fn open_connection(
            &self,
            _spec: ConnectionSpec,
        ) -> Result<ConnectionHandle, AgentExecutorError> {
            self.open_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_open.load(Ordering::SeqCst) {
                return Err(AgentExecutorError::Io("forced open failure".into()));
            }
            Ok(ConnectionHandle {
                id: ConnectionHandleId::new(),
                vendor: self.vendor,
                inner: Arc::new(()),
            })
        }

        async fn close_connection(
            &self,
            _handle: ConnectionHandle,
        ) -> Result<(), AgentExecutorError> {
            self.close_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn check_connection(
            &self,
            _handle: &ConnectionHandle,
        ) -> Result<ConnectionHealth, AgentExecutorError> {
            self.check_calls.fetch_add(1, Ordering::SeqCst);
            if self.force_dead.load(Ordering::SeqCst) {
                Ok(ConnectionHealth::Dead {
                    reason: "forced dead".into(),
                })
            } else {
                Ok(ConnectionHealth::Healthy)
            }
        }
    }

    /// Build a bare registry whose public vendor slots are replaced by a
    /// single `MockExecutor` for `cteno`. The other three slots stay `None`.
    fn registry_with_mock_cteno(mock: Arc<MockExecutor>) -> ExecutorRegistry {
        ExecutorRegistry {
            cteno: mock.clone() as Arc<dyn AgentExecutor>,
            // cteno_concrete is unused in these tests; we still need one so
            // the struct is valid. Allocate a bogus CtenoAgentExecutor. The
            // tests never invoke Cteno-specific helpers on it.
            cteno_concrete: Arc::new(multi_agent_runtime_cteno::CtenoAgentExecutor::new(
                PathBuf::from("/does/not/exist/cteno-agent"),
                Arc::new(NoopStore),
            )),
            claude: None,
            codex: None,
            gemini: None,
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    #[derive(Clone)]
    struct NoopStore;

    #[async_trait]
    impl SessionStoreProvider for NoopStore {
        async fn record_session(
            &self,
            _vendor: &str,
            _session: multi_agent_runtime_core::SessionRecord,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn list_sessions(
            &self,
            _vendor: &str,
            _filter: SessionFilter,
        ) -> Result<Vec<SessionMeta>, String> {
            Ok(Vec::new())
        }
        async fn get_session_info(
            &self,
            _vendor: &str,
            _session_id: &NativeSessionId,
        ) -> Result<SessionInfo, String> {
            Err("unused in tests".into())
        }
        async fn get_session_messages(
            &self,
            _vendor: &str,
            _session_id: &NativeSessionId,
            _pagination: Pagination,
        ) -> Result<Vec<NativeMessage>, String> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn get_or_open_connection_is_idempotent() {
        let mock = MockExecutor::new(VENDOR_CTENO, true);
        let reg = registry_with_mock_cteno(mock.clone());

        let h1 = reg.get_or_open_connection(VENDOR_CTENO).await.unwrap();
        let h2 = reg.get_or_open_connection(VENDOR_CTENO).await.unwrap();
        // Same cached handle id on idempotent call.
        assert_eq!(h1.id, h2.id);
        assert_eq!(mock.open_count(), 1);
    }

    #[tokio::test]
    async fn close_connection_clears_slot_and_calls_executor() {
        let mock = MockExecutor::new(VENDOR_CTENO, true);
        let reg = registry_with_mock_cteno(mock.clone());

        reg.get_or_open_connection(VENDOR_CTENO).await.unwrap();
        assert_eq!(mock.close_count(), 0);

        reg.close_connection(VENDOR_CTENO).await.unwrap();
        assert_eq!(mock.close_count(), 1);

        // Re-open should invoke open_connection again.
        reg.get_or_open_connection(VENDOR_CTENO).await.unwrap();
        assert_eq!(mock.open_count(), 2);
    }

    #[tokio::test]
    async fn probe_caches_result_until_refresh() {
        let mock = MockExecutor::new(VENDOR_CTENO, true);
        let reg = registry_with_mock_cteno(mock.clone());

        let first = reg.probe(VENDOR_CTENO).await;
        assert_eq!(first.state, VendorConnectionProbeState::Connected);
        assert_eq!(mock.open_count(), 1);

        // Subsequent probe sees cached handle → goes through check_connection,
        // not open_connection.
        let second = reg.probe(VENDOR_CTENO).await;
        assert_eq!(second.state, VendorConnectionProbeState::Connected);
        assert_eq!(mock.open_count(), 1);
        assert_eq!(mock.check_count(), 1);
    }

    #[tokio::test]
    async fn refresh_forces_new_probe() {
        let mock = MockExecutor::new(VENDOR_CTENO, true);
        let reg = registry_with_mock_cteno(mock.clone());

        let _ = reg.probe(VENDOR_CTENO).await;
        assert_eq!(mock.open_count(), 1);

        let refreshed = reg.refresh(VENDOR_CTENO).await;
        assert_eq!(refreshed.state, VendorConnectionProbeState::Connected);
        // refresh closes then re-opens → one extra close + one extra open.
        assert_eq!(mock.close_count(), 1);
        assert_eq!(mock.open_count(), 2);
    }

    #[tokio::test]
    async fn probe_all_runs_vendors_in_parallel_and_returns_installed_only() {
        let mock = MockExecutor::new(VENDOR_CTENO, true);
        let reg = registry_with_mock_cteno(mock.clone());

        let results = reg.probe_all().await;
        // Only cteno is installed in the test registry — claude / codex / gemini slots are None.
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, VENDOR_CTENO);
        assert_eq!(results[0].1.state, VendorConnectionProbeState::Connected);
    }

    #[tokio::test]
    async fn preheat_all_opens_handle_for_multi_session_vendor() {
        let mock = MockExecutor::new(VENDOR_CTENO, true);
        let reg = registry_with_mock_cteno(mock.clone());

        reg.preheat_all().await;
        // Multi-session: probe opens (1) then preheat's open_connection is
        // idempotent (no further open). Total opens = 1.
        assert!(mock.open_count() >= 1);
        // Close should not be called for multi-session vendors (handle kept live).
        assert_eq!(mock.close_count(), 0);
    }

    #[tokio::test]
    async fn preheat_all_closes_probe_handle_for_single_session_vendor() {
        let mock = MockExecutor::new(VENDOR_CTENO, false);
        let reg = registry_with_mock_cteno(mock.clone());

        reg.preheat_all().await;
        // Non-multi-session: probe opens + closes the handle. No extra
        // open_connection call from preheat_all.
        assert_eq!(mock.open_count(), 1);
        assert_eq!(mock.close_count(), 1);
    }

    #[tokio::test]
    async fn snapshot_probes_reflects_latest_state() {
        let mock = MockExecutor::new(VENDOR_CTENO, true);
        let reg = registry_with_mock_cteno(mock.clone());

        let snap_before = reg.snapshot_probes().await;
        assert!(snap_before.is_empty());

        let _ = reg.probe(VENDOR_CTENO).await;
        let snap_after = reg.snapshot_probes().await;
        let cteno_probe = snap_after.get(VENDOR_CTENO).expect("cteno probe present");
        assert_eq!(cteno_probe.state, VendorConnectionProbeState::Connected);
    }

    #[tokio::test]
    async fn open_connection_failure_marks_probe_failed_without_panic() {
        let mock = MockExecutor::new(VENDOR_CTENO, true);
        mock.fail_open.store(true, Ordering::SeqCst);
        let reg = registry_with_mock_cteno(mock.clone());

        let err = reg.get_or_open_connection(VENDOR_CTENO).await;
        assert!(err.is_err());

        let snap = reg.snapshot_probes().await;
        let probe = snap.get(VENDOR_CTENO).unwrap();
        assert_eq!(probe.state, VendorConnectionProbeState::Failed);
        assert!(probe.reason.is_some());
    }

    #[tokio::test]
    async fn open_connection_drops_dead_handle_before_returning() {
        // Arrange: open once, then flip the transport to "dead" so the
        // cached handle should be dropped and a fresh one dialed on the
        // next call.
        let mock = MockExecutor::new(VENDOR_CTENO, true);
        let reg = registry_with_mock_cteno(mock.clone());

        let h1 = reg.get_or_open_connection(VENDOR_CTENO).await.unwrap();
        assert_eq!(mock.open_count(), 1);
        mock.force_dead.store(true, Ordering::SeqCst);

        // Flip back to healthy so the *new* dial succeeds. The test is
        // really asserting that (a) check_connection was consulted, (b)
        // Dead caused a slot drop, (c) a second open_connection ran.
        let h2 = reg.get_or_open_connection(VENDOR_CTENO).await.unwrap();
        // Mock's `check_connection` honored the force_dead flag at call
        // time; slot dropped; re-open succeeded even though force_dead is
        // still set — MockExecutor's `open_connection` doesn't read it.
        assert_eq!(mock.check_count(), 1);
        assert_eq!(mock.open_count(), 2);
        assert_eq!(mock.close_count(), 1, "dead handle should be closed once");
        // Handle id changes because a new ConnectionHandleId is minted.
        assert_ne!(h1.id, h2.id);
    }

    #[tokio::test]
    async fn start_session_with_autoreopen_retries_on_connection_closed() {
        // Arrange a mock that fails start_session_on the first time with
        // a "connection is closed" error, then succeeds.
        struct FlakyExecutor {
            vendor: &'static str,
            attempts: Arc<AtomicU32>,
            opens: Arc<AtomicU32>,
            closes: Arc<AtomicU32>,
        }

        #[async_trait]
        impl AgentExecutor for FlakyExecutor {
            fn capabilities(&self) -> AgentCapabilities {
                AgentCapabilities {
                    name: Cow::Borrowed(self.vendor),
                    protocol_version: Cow::Borrowed("mock"),
                    supports_list_sessions: false,
                    supports_get_messages: false,
                    supports_runtime_set_model: false,
                    permission_mode_kind: PermissionModeKind::Static,
                    supports_resume: false,
                    supports_multi_session_per_process: true,
                    supports_injected_tools: false,
                    supports_permission_closure: false,
                    supports_interrupt: false,
                }
            }
            async fn spawn_session(
                &self,
                _spec: SpawnSessionSpec,
            ) -> Result<SessionRef, AgentExecutorError> {
                Err(AgentExecutorError::Unsupported {
                    capability: "spawn_session",
                })
            }
            async fn resume_session(
                &self,
                _id: NativeSessionId,
                _h: ResumeHints,
            ) -> Result<SessionRef, AgentExecutorError> {
                Err(AgentExecutorError::Unsupported {
                    capability: "resume_session",
                })
            }
            async fn send_message(
                &self,
                _s: &SessionRef,
                _m: UserMessage,
            ) -> Result<EventStream, AgentExecutorError> {
                Ok(Box::pin(futures_util::stream::empty::<
                    Result<ExecutorEvent, AgentExecutorError>,
                >()))
            }
            async fn respond_to_permission(
                &self,
                _s: &SessionRef,
                _id: String,
                _d: PermissionDecision,
            ) -> Result<(), AgentExecutorError> {
                Ok(())
            }
            async fn interrupt(&self, _s: &SessionRef) -> Result<(), AgentExecutorError> {
                Ok(())
            }
            async fn close_session(&self, _s: &SessionRef) -> Result<(), AgentExecutorError> {
                Ok(())
            }
            async fn set_permission_mode(
                &self,
                _s: &SessionRef,
                _m: PermissionMode,
            ) -> Result<(), AgentExecutorError> {
                Ok(())
            }
            async fn set_model(
                &self,
                _s: &SessionRef,
                _m: ModelSpec,
            ) -> Result<multi_agent_runtime_core::ModelChangeOutcome, AgentExecutorError>
            {
                Ok(multi_agent_runtime_core::ModelChangeOutcome::Unsupported)
            }
            async fn list_sessions(
                &self,
                _f: SessionFilter,
            ) -> Result<Vec<SessionMeta>, AgentExecutorError> {
                Ok(Vec::new())
            }
            async fn get_session_info(
                &self,
                _id: &NativeSessionId,
            ) -> Result<SessionInfo, AgentExecutorError> {
                Err(AgentExecutorError::Unsupported {
                    capability: "get_session_info",
                })
            }
            async fn get_session_messages(
                &self,
                _id: &NativeSessionId,
                _p: Pagination,
            ) -> Result<Vec<NativeMessage>, AgentExecutorError> {
                Ok(Vec::new())
            }
            async fn open_connection(
                &self,
                _spec: ConnectionSpec,
            ) -> Result<ConnectionHandle, AgentExecutorError> {
                self.opens.fetch_add(1, Ordering::SeqCst);
                Ok(ConnectionHandle {
                    id: ConnectionHandleId::new(),
                    vendor: self.vendor,
                    inner: Arc::new(()),
                })
            }
            async fn close_connection(
                &self,
                _h: ConnectionHandle,
            ) -> Result<(), AgentExecutorError> {
                self.closes.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            async fn check_connection(
                &self,
                _h: &ConnectionHandle,
            ) -> Result<ConnectionHealth, AgentExecutorError> {
                Ok(ConnectionHealth::Healthy)
            }
            async fn start_session_on(
                &self,
                _handle: &ConnectionHandle,
                _spec: SpawnSessionSpec,
            ) -> Result<SessionRef, AgentExecutorError> {
                let n = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if n == 1 {
                    Err(AgentExecutorError::Protocol(
                        "codex app-server connection is closed; reopen before starting a session"
                            .to_string(),
                    ))
                } else {
                    Ok(SessionRef {
                        id: NativeSessionId::new(format!("sess-{n}")),
                        vendor: self.vendor,
                        process_handle: multi_agent_runtime_core::ProcessHandleToken::new(),
                        spawned_at: Utc::now(),
                        workdir: PathBuf::from("/tmp"),
                    })
                }
            }
        }

        let flaky = Arc::new(FlakyExecutor {
            vendor: VENDOR_CTENO,
            attempts: Arc::new(AtomicU32::new(0)),
            opens: Arc::new(AtomicU32::new(0)),
            closes: Arc::new(AtomicU32::new(0)),
        });
        let reg = ExecutorRegistry {
            cteno: flaky.clone() as Arc<dyn AgentExecutor>,
            cteno_concrete: Arc::new(multi_agent_runtime_cteno::CtenoAgentExecutor::new(
                PathBuf::from("/does/not/exist/cteno-agent"),
                Arc::new(NoopStore),
            )),
            claude: None,
            codex: None,
            gemini: None,
            connections: Arc::new(RwLock::new(HashMap::new())),
        };

        let spec = SpawnSessionSpec {
            workdir: PathBuf::from("/tmp"),
            system_prompt: None,
            model: None,
            permission_mode: PermissionMode::Default,
            allowed_tools: None,
            additional_directories: Vec::new(),
            env: std::collections::BTreeMap::new(),
            agent_config: serde_json::Value::Null,
            resume_hint: None,
        };
        let session = reg
            .start_session_with_autoreopen(VENDOR_CTENO, spec)
            .await
            .expect("auto-reopen should succeed on second attempt");

        assert_eq!(flaky.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(flaky.opens.load(Ordering::SeqCst), 2);
        assert_eq!(flaky.closes.load(Ordering::SeqCst), 1);
        assert_eq!(session.id.as_str(), "sess-2");
    }

    #[tokio::test]
    async fn start_session_with_autoreopen_surfaces_second_failure() {
        // When the spawn fails a second time the helper must NOT loop —
        // it returns the error so the caller can fall back / notify the UI.
        struct AlwaysClosedExecutor {
            vendor: &'static str,
            attempts: Arc<AtomicU32>,
        }

        #[async_trait]
        impl AgentExecutor for AlwaysClosedExecutor {
            fn capabilities(&self) -> AgentCapabilities {
                AgentCapabilities {
                    name: Cow::Borrowed(self.vendor),
                    protocol_version: Cow::Borrowed("mock"),
                    supports_list_sessions: false,
                    supports_get_messages: false,
                    supports_runtime_set_model: false,
                    permission_mode_kind: PermissionModeKind::Static,
                    supports_resume: false,
                    supports_multi_session_per_process: true,
                    supports_injected_tools: false,
                    supports_permission_closure: false,
                    supports_interrupt: false,
                }
            }
            async fn spawn_session(
                &self,
                _s: SpawnSessionSpec,
            ) -> Result<SessionRef, AgentExecutorError> {
                Err(AgentExecutorError::Unsupported {
                    capability: "spawn_session",
                })
            }
            async fn resume_session(
                &self,
                _i: NativeSessionId,
                _h: ResumeHints,
            ) -> Result<SessionRef, AgentExecutorError> {
                Err(AgentExecutorError::Unsupported {
                    capability: "resume_session",
                })
            }
            async fn send_message(
                &self,
                _s: &SessionRef,
                _m: UserMessage,
            ) -> Result<EventStream, AgentExecutorError> {
                Ok(Box::pin(futures_util::stream::empty::<
                    Result<ExecutorEvent, AgentExecutorError>,
                >()))
            }
            async fn respond_to_permission(
                &self,
                _s: &SessionRef,
                _i: String,
                _d: PermissionDecision,
            ) -> Result<(), AgentExecutorError> {
                Ok(())
            }
            async fn interrupt(&self, _s: &SessionRef) -> Result<(), AgentExecutorError> {
                Ok(())
            }
            async fn close_session(&self, _s: &SessionRef) -> Result<(), AgentExecutorError> {
                Ok(())
            }
            async fn set_permission_mode(
                &self,
                _s: &SessionRef,
                _m: PermissionMode,
            ) -> Result<(), AgentExecutorError> {
                Ok(())
            }
            async fn set_model(
                &self,
                _s: &SessionRef,
                _m: ModelSpec,
            ) -> Result<multi_agent_runtime_core::ModelChangeOutcome, AgentExecutorError>
            {
                Ok(multi_agent_runtime_core::ModelChangeOutcome::Unsupported)
            }
            async fn list_sessions(
                &self,
                _f: SessionFilter,
            ) -> Result<Vec<SessionMeta>, AgentExecutorError> {
                Ok(Vec::new())
            }
            async fn get_session_info(
                &self,
                _i: &NativeSessionId,
            ) -> Result<SessionInfo, AgentExecutorError> {
                Err(AgentExecutorError::Unsupported {
                    capability: "get_session_info",
                })
            }
            async fn get_session_messages(
                &self,
                _i: &NativeSessionId,
                _p: Pagination,
            ) -> Result<Vec<NativeMessage>, AgentExecutorError> {
                Ok(Vec::new())
            }
            async fn open_connection(
                &self,
                _spec: ConnectionSpec,
            ) -> Result<ConnectionHandle, AgentExecutorError> {
                Ok(ConnectionHandle {
                    id: ConnectionHandleId::new(),
                    vendor: self.vendor,
                    inner: Arc::new(()),
                })
            }
            async fn close_connection(
                &self,
                _h: ConnectionHandle,
            ) -> Result<(), AgentExecutorError> {
                Ok(())
            }
            async fn check_connection(
                &self,
                _h: &ConnectionHandle,
            ) -> Result<ConnectionHealth, AgentExecutorError> {
                Ok(ConnectionHealth::Healthy)
            }
            async fn start_session_on(
                &self,
                _h: &ConnectionHandle,
                _spec: SpawnSessionSpec,
            ) -> Result<SessionRef, AgentExecutorError> {
                self.attempts.fetch_add(1, Ordering::SeqCst);
                Err(AgentExecutorError::Protocol(
                    "connection is closed".to_string(),
                ))
            }
        }

        let always_closed = Arc::new(AlwaysClosedExecutor {
            vendor: VENDOR_CTENO,
            attempts: Arc::new(AtomicU32::new(0)),
        });
        let reg = ExecutorRegistry {
            cteno: always_closed.clone() as Arc<dyn AgentExecutor>,
            cteno_concrete: Arc::new(multi_agent_runtime_cteno::CtenoAgentExecutor::new(
                PathBuf::from("/does/not/exist/cteno-agent"),
                Arc::new(NoopStore),
            )),
            claude: None,
            codex: None,
            gemini: None,
            connections: Arc::new(RwLock::new(HashMap::new())),
        };

        let spec = SpawnSessionSpec {
            workdir: PathBuf::from("/tmp"),
            system_prompt: None,
            model: None,
            permission_mode: PermissionMode::Default,
            allowed_tools: None,
            additional_directories: Vec::new(),
            env: std::collections::BTreeMap::new(),
            agent_config: serde_json::Value::Null,
            resume_hint: None,
        };
        let err = reg
            .start_session_with_autoreopen(VENDOR_CTENO, spec)
            .await
            .expect_err("expected propagated failure after retry");
        assert!(err.to_string().contains("connection is closed"));
        assert_eq!(
            always_closed.attempts.load(Ordering::SeqCst),
            2,
            "helper must retry exactly once and then give up"
        );
    }

    #[tokio::test]
    async fn is_connection_closed_error_matches_expected_shapes() {
        // Protect the substring-match retry gate against false negatives
        // for the error phrasings real vendor adapters actually produce.
        use AgentExecutorError::*;
        assert!(ExecutorRegistry::is_connection_closed_error(&Protocol(
            "codex app-server connection is closed; reopen before starting a session".into()
        )));
        assert!(ExecutorRegistry::is_connection_closed_error(&Protocol(
            "codex app-server stdout EOF".into()
        )));
        assert!(ExecutorRegistry::is_connection_closed_error(&Io(
            "Broken pipe (os error 32)".into()
        )));
        assert!(ExecutorRegistry::is_connection_closed_error(&Protocol(
            "app-server exited (code=Some(0))".into()
        )));
        assert!(!ExecutorRegistry::is_connection_closed_error(&Protocol(
            "some other random protocol error".into()
        )));
    }

    #[tokio::test]
    async fn probe_recovers_from_dead_handle() {
        let mock = MockExecutor::new(VENDOR_CTENO, true);
        let reg = registry_with_mock_cteno(mock.clone());

        // First probe caches a handle.
        let _ = reg.probe(VENDOR_CTENO).await;
        assert_eq!(mock.open_count(), 1);

        // Mark the transport dead and probe again — should drop and re-open.
        mock.force_dead.store(true, Ordering::SeqCst);
        let again = reg.probe(VENDOR_CTENO).await;
        // `Dead` result → handle dropped, but probe() does NOT auto-reopen
        // synchronously; the re-open path happens on the next
        // get_or_open_connection or refresh. This test only validates that
        // the dead signal clears the cache and does not leak.
        assert_eq!(again.state, VendorConnectionProbeState::Connected);
        // check_connection (dead) → slot cleared → probe falls through to a
        // fresh open_connection (with probe=true). Total opens = 2.
        assert_eq!(mock.open_count(), 2);
    }
}
