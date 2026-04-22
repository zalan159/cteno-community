//! Machine-level usage/rate-limit monitor for external agent vendors.
//!
//! Owns one background poller per vendor (Claude / Codex / Gemini) that
//! probes each provider's own quota API on a timer and caches the result
//! in a shared map. The daemon exposes the cache to the frontend via an
//! RPC method; no session-scoped plumbing is involved — the data is
//! global to the machine.

pub mod probe;
pub mod probes;
pub mod store;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, RwLock};

pub use probe::VendorUsageProbe;
pub use store::{
    UsageBucket, UsageCredits, UsageWindow, VendorId, VendorUsage, VendorUsageMap, VendorUsageShape,
};

/// Retry after an error is reported slower than the normal poll so we don't
/// burn cycles + logs on a persistently broken probe (e.g. unsupported auth
/// type). Successful probes switch back to the regular interval.
pub const ERROR_BACKOFF: Duration = Duration::from_secs(30 * 60);

pub struct UsageMonitor {
    store: Arc<RwLock<VendorUsageMap>>,
    tx: broadcast::Sender<VendorUsage>,
}

impl UsageMonitor {
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(32);
        Arc::new(Self {
            store: Arc::new(RwLock::new(VendorUsageMap::default())),
            tx,
        })
    }

    /// Read a cloned snapshot of the current cache.
    pub async fn snapshot(&self) -> VendorUsageMap {
        self.store.read().await.clone()
    }

    /// Subscribe to live updates. Each successful or errored probe tick
    /// emits one `VendorUsage` value on this channel.
    pub fn subscribe(&self) -> broadcast::Receiver<VendorUsage> {
        self.tx.subscribe()
    }

    /// Kick off a background task that runs `probe` on `interval`, writes
    /// each result to the cache, and broadcasts it on the update channel.
    /// Cold start: first poll fires immediately, then loops on `interval`.
    pub fn spawn_probe<P>(self: &Arc<Self>, probe: P, interval: Duration)
    where
        P: VendorUsageProbe + Send + Sync + 'static,
    {
        let store = self.store.clone();
        let tx = self.tx.clone();
        let vendor = probe.vendor();
        tokio::spawn(async move {
            let probe = probe;
            let mut consecutive_errors: u32 = 0;
            loop {
                let snapshot = match probe.poll().await {
                    Ok(s) => {
                        consecutive_errors = 0;
                        if let Some(err) = &s.error {
                            log::info!(
                                "[usage-monitor] {} probe reported user-visible error: {}",
                                vendor.as_str(),
                                err
                            );
                        } else {
                            log::info!(
                                "[usage-monitor] {} probe ok: windows={} buckets={} plan={:?}",
                                vendor.as_str(),
                                s.windows.len(),
                                s.buckets.len(),
                                s.plan_type,
                            );
                        }
                        s
                    }
                    Err(e) => {
                        consecutive_errors = consecutive_errors.saturating_add(1);
                        log::warn!(
                            "[usage-monitor] {} probe error ({} in a row): {}",
                            vendor.as_str(),
                            consecutive_errors,
                            e
                        );
                        VendorUsage::error(vendor, e.to_string())
                    }
                };

                {
                    let mut guard = store.write().await;
                    guard.insert(vendor, snapshot.clone());
                }
                let _ = tx.send(snapshot);

                // Back off after 3 consecutive errors to avoid log spam.
                let delay = if consecutive_errors >= 3 {
                    ERROR_BACKOFF
                } else {
                    interval
                };
                tokio::time::sleep(delay).await;
            }
        });
    }

    /// Force the store to return a fresh snapshot for a vendor. Callers that
    /// pre-warm the cache (e.g. RPC "refresh-now") can write directly.
    pub async fn set(&self, usage: VendorUsage) {
        let vendor = usage.provider;
        let mut guard = self.store.write().await;
        guard.insert(vendor, usage.clone());
        drop(guard);
        let _ = self.tx.send(usage);
    }
}
