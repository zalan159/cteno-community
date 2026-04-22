use std::path::Path;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Host-owned message envelope for optional cloud session sync.
///
/// The payload stays as JSON so the host runtime does not depend on any
/// vendor-specific or commercial message crate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSyncMessage {
    pub payload: serde_json::Value,
}

#[async_trait]
pub trait SessionSyncService: Send + Sync {
    async fn on_session_created(&self, session_id: &str, workdir: &Path, vendor: &str);

    async fn on_message_sent(&self, session_id: &str, message: &SessionSyncMessage);

    async fn on_message_received(&self, session_id: &str, message: &SessionSyncMessage);

    async fn on_session_closed(&self, session_id: &str);
}

struct NoopSessionSyncService;

#[async_trait]
impl SessionSyncService for NoopSessionSyncService {
    async fn on_session_created(&self, _session_id: &str, _workdir: &Path, _vendor: &str) {}

    async fn on_message_sent(&self, _session_id: &str, _message: &SessionSyncMessage) {}

    async fn on_message_received(&self, _session_id: &str, _message: &SessionSyncMessage) {}

    async fn on_session_closed(&self, _session_id: &str) {}
}

static SESSION_SYNC_SERVICE: OnceLock<Arc<dyn SessionSyncService>> = OnceLock::new();

pub fn install_session_sync_service(
    service: Arc<dyn SessionSyncService>,
) -> Result<(), Arc<dyn SessionSyncService>> {
    SESSION_SYNC_SERVICE.set(service)
}

pub fn session_sync_service() -> Arc<dyn SessionSyncService> {
    SESSION_SYNC_SERVICE
        .get_or_init(|| Arc::new(NoopSessionSyncService))
        .clone()
}
