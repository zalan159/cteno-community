//! Shared session registry abstraction.
//!
//! This is a thin wrapper around the existing
//! `Arc<tokio::sync::Mutex<HashMap<String, SessionConnection>>>` shape.
//! It keeps the current storage model intact while giving the rest of the
//! desktop app a stable boundary to depend on.

use super::SessionConnection;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, MutexGuard};

/// Legacy storage shape kept under a named abstraction.
pub type SessionConnectionsMap = Arc<Mutex<HashMap<String, SessionConnection>>>;

/// Shared registry for active session connections.
#[derive(Clone, Default)]
pub struct SessionRegistry(SessionConnectionsMap);

impl SessionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    // Compatibility shims kept while legacy callers are migrated off the raw
    // `Arc<Mutex<HashMap<...>>>` shape. Future removal candidates once the
    // remaining `session/spawn.rs` call sites are migrated are:
    // `from_map`, `into_inner`, `handle`, and `lock`.

    /// Wrap an existing map in the registry abstraction.
    pub fn from_map(session_connections: SessionConnectionsMap) -> Self {
        Self(session_connections)
    }

    /// Expose the underlying map for compatibility with legacy call sites.
    pub fn into_inner(self) -> SessionConnectionsMap {
        self.0
    }

    /// Clone the underlying map handle.
    pub fn handle(&self) -> SessionConnectionsMap {
        self.0.clone()
    }

    /// Mirror the `Mutex::lock` API so existing call sites can keep using
    /// `registry.lock().await` with minimal churn.
    pub async fn lock(&self) -> MutexGuard<'_, HashMap<String, SessionConnection>> {
        self.0.lock().await
    }

    /// Insert or replace a session connection.
    pub async fn insert(
        &self,
        session_id: impl Into<String>,
        connection: SessionConnection,
    ) -> Option<SessionConnection> {
        self.0.lock().await.insert(session_id.into(), connection)
    }

    /// Remove and return all registered session connections.
    pub async fn drain(&self) -> Vec<(String, SessionConnection)> {
        self.0.lock().await.drain().collect()
    }

    /// Remove a session connection by ID.
    pub async fn remove(&self, session_id: &str) -> Option<SessionConnection> {
        self.0.lock().await.remove(session_id)
    }

    /// Retrieve a cloned session connection.
    pub async fn get(&self, session_id: &str) -> Option<SessionConnection> {
        self.0.lock().await.get(session_id).cloned()
    }

    /// Snapshot the current registry entries as cloned pairs.
    pub async fn snapshot(&self) -> Vec<(String, SessionConnection)> {
        self.0
            .lock()
            .await
            .iter()
            .map(|(session_id, connection)| (session_id.clone(), connection.clone()))
            .collect()
    }

    /// Check whether a session is registered.
    pub async fn contains_key(&self, session_id: &str) -> bool {
        self.0.lock().await.contains_key(session_id)
    }

    /// Return the number of registered sessions.
    pub async fn len(&self) -> usize {
        self.0.lock().await.len()
    }

    /// Check whether the registry is empty.
    pub async fn is_empty(&self) -> bool {
        self.0.lock().await.is_empty()
    }

    /// Clear all registered sessions.
    pub async fn clear(&self) {
        self.0.lock().await.clear();
    }
}

impl From<SessionConnectionsMap> for SessionRegistry {
    fn from(session_connections: SessionConnectionsMap) -> Self {
        Self::from_map(session_connections)
    }
}
