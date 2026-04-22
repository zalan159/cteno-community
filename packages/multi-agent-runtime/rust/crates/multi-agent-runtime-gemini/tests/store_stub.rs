//! Shared in-memory `SessionStoreProvider` for connection-level tests.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Utc;
use multi_agent_runtime_core::{
    NativeMessage, NativeSessionId, Pagination, SessionFilter, SessionInfo, SessionMeta,
    SessionRecord, SessionStoreProvider,
};

#[derive(Default)]
pub struct RecordingStore {
    pub records: Mutex<Vec<(String, SessionRecord)>>,
    pub infos: Mutex<HashMap<(String, String), SessionInfo>>,
    #[allow(dead_code)]
    pub messages: Mutex<HashMap<(String, String), Vec<NativeMessage>>>,
}

#[async_trait]
impl SessionStoreProvider for RecordingStore {
    async fn record_session(&self, vendor: &str, session: SessionRecord) -> Result<(), String> {
        self.records
            .lock()
            .unwrap()
            .push((vendor.to_string(), session.clone()));
        self.infos.lock().unwrap().insert(
            (vendor.to_string(), session.session_id.as_str().to_string()),
            SessionInfo {
                meta: SessionMeta {
                    id: session.session_id.clone(),
                    workdir: session.workdir.clone(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    title: None,
                },
                permission_mode: None,
                model: None,
                usage: Default::default(),
                extras: session.context,
            },
        );
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
        vendor: &str,
        session_id: &NativeSessionId,
    ) -> Result<SessionInfo, String> {
        self.infos
            .lock()
            .unwrap()
            .get(&(vendor.to_string(), session_id.as_str().to_string()))
            .cloned()
            .ok_or_else(|| format!("missing info for {}", session_id.as_str()))
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
