//! Cross-vendor session store contract.
//!
//! `list_sessions` / `get_session_info` / `get_session_messages` on the
//! [`AgentExecutor`](super::AgentExecutor) trait are **host-side queries** —
//! they describe metadata that lives in the caller's durable store, not data
//! that the vendor subprocess can be asked for directly.
//!
//! To keep Codex / Claude / Cteno adapters aligned on a single ground truth
//! (the Cteno local SQLite session store), each adapter accepts an
//! `Arc<dyn SessionStoreProvider>` and delegates these three methods to it.
//! The adapter is responsible for tagging queries with its own vendor name so
//! the store can scope by provider.
//!
//! The trait intentionally uses plain `Result<_, String>` so implementations
//! can live in crates that don't want to depend on `multi-agent-runtime-core`
//! for error types. Adapters wrap the `String` into
//! [`AgentExecutorError::Vendor`](super::AgentExecutorError::Vendor) before
//! returning to the session layer.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{
    NativeMessage, NativeSessionId, Pagination, SessionFilter, SessionInfo, SessionMeta,
};

/// Write payload used by executor adapters to mirror a spawned session into
/// the caller's durable store.
///
/// `session_id` is the host-visible session handle returned by
/// `spawn_session`. For vendors like Codex this may initially be a synthetic
/// placeholder; implementations should treat repeated writes for the same id
/// as an idempotent upsert and merge `context` into any existing row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Host-visible session id to persist into the local metadata store.
    pub session_id: NativeSessionId,
    /// Absolute workspace path associated with the session.
    pub workdir: PathBuf,
    /// Vendor-specific metadata to merge into the durable row.
    #[serde(default)]
    pub context: Value,
}

/// Abstract session metadata store shared by the three vendor adapters.
///
/// Implementations should be cheap to `Arc::clone` and must be `Send + Sync`.
#[async_trait]
pub trait SessionStoreProvider: Send + Sync + 'static {
    /// Record or update a vendor-scoped session row in the durable store.
    async fn record_session(&self, vendor: &str, session: SessionRecord) -> Result<(), String>;

    /// List sessions tagged with the given vendor matching the filter.
    async fn list_sessions(
        &self,
        vendor: &str,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, String>;

    /// Fetch full detail for a single session (vendor-scoped).
    async fn get_session_info(
        &self,
        vendor: &str,
        session_id: &NativeSessionId,
    ) -> Result<SessionInfo, String>;

    /// Fetch a paginated slab of messages for a session (vendor-scoped).
    async fn get_session_messages(
        &self,
        vendor: &str,
        session_id: &NativeSessionId,
        pagination: Pagination,
    ) -> Result<Vec<NativeMessage>, String>;
}
