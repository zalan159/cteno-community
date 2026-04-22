//! Community [`SessionStoreProvider`] implementation backed by the local
//! `cteno_agent_runtime::agent_session::AgentSessionManager` SQLite store.
//!
//! Scoping by vendor goes through the `agent_sessions.vendor` column, added
//! by the T2 migration (see `db.rs` and
//! `AgentSessionManager::ensure_vendor_column`). Legacy rows pre-migration
//! default to `"cteno"`.
//!
//! Writers that mirror Claude / Codex subprocess sessions into the local
//! store should call [`SessionStoreProvider::record_session`] so the row
//! carries the correct vendor tag and workdir metadata.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::DateTime;
use multi_agent_runtime_core::{
    NativeMessage, NativeSessionId, Pagination, SessionFilter, SessionInfo, SessionMeta,
    SessionRecord, SessionStoreProvider,
};
use serde_json::{json, Value};

use cteno_agent_runtime::agent_session::{AgentSession, AgentSessionManager, SessionStatus};

/// Local-SQLite-backed [`SessionStoreProvider`] used by every vendor adapter.
///
/// Internally wraps [`AgentSessionManager`] — cheap to clone (holds only a
/// path buffer).
#[derive(Clone)]
pub struct CommunitySessionStore {
    db_path: PathBuf,
}

impl CommunitySessionStore {
    /// Build a new store pointing at the given SQLite file. Caller is
    /// responsible for ensuring the schema is present (the session layer
    /// already bootstraps it).
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    fn manager(&self) -> AgentSessionManager {
        AgentSessionManager::new(self.db_path.clone())
    }

    /// Translate an `AgentSession` into the cross-vendor `SessionMeta` shape.
    fn session_to_meta(session: &AgentSession) -> SessionMeta {
        let created_at = parse_timestamp(&session.created_at);
        let updated_at = parse_timestamp(&session.updated_at);
        // No workdir column in agent_sessions yet — reuse context_data.workdir
        // if present, otherwise default to the current CWD (best-effort).
        let workdir = session
            .context_data
            .as_ref()
            .and_then(|v| v.get("workdir"))
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        SessionMeta {
            id: NativeSessionId::new(session.id.clone()),
            workdir,
            created_at,
            updated_at,
            title: session
                .messages
                .iter()
                .find(|m| m.role == "user")
                .map(|m| m.content.chars().take(80).collect::<String>()),
        }
    }

    fn record_session_sync(&self, vendor: &str, session: SessionRecord) -> Result<(), String> {
        let manager = self.manager();
        let session_id = session.session_id.as_str().to_string();
        let workdir = session.workdir.to_string_lossy().to_string();
        let mut existing = manager.get_session(&session_id)?;

        if existing.is_none() {
            if let Err(err) =
                manager.create_session_with_id_and_vendor(&session_id, "worker", None, None, vendor)
            {
                if !err.contains("UNIQUE constraint failed") {
                    return Err(err);
                }
            }
            existing = manager.get_session(&session_id)?;
        }

        if existing.as_ref().map(|row| row.vendor.as_str()) != Some(vendor) {
            manager.set_vendor(&session_id, vendor)?;
        }

        let mut context = existing
            .and_then(|row| row.context_data)
            .unwrap_or_else(|| json!({}));
        let context_obj = context
            .as_object_mut()
            .ok_or_else(|| "Session context_data is not an object".to_string())?;
        context_obj.insert("workdir".to_string(), Value::String(workdir));

        match session.context {
            Value::Null => {}
            Value::Object(extra) => {
                for (key, value) in extra {
                    context_obj.insert(key, value);
                }
            }
            _ => {
                return Err("SessionRecord.context must be a JSON object or null".to_string());
            }
        }

        manager.update_context_data(&session_id, &context)
    }
}

#[async_trait]
impl SessionStoreProvider for CommunitySessionStore {
    async fn record_session(&self, vendor: &str, session: SessionRecord) -> Result<(), String> {
        let store = self.clone();
        let vendor = vendor.to_string();
        tokio::task::spawn_blocking(move || store.record_session_sync(&vendor, session))
            .await
            .map_err(|e| format!("record_session join: {e}"))?
    }

    async fn list_sessions(
        &self,
        vendor: &str,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, String> {
        let manager = self.manager();
        let status = filter.status.and_then(map_status_filter);

        // `AgentSessionManager::list_sessions_by_vendor` runs synchronous
        // rusqlite — we push it to the blocking pool to avoid stalling the
        // async runtime. Vendor filtering happens in SQL (`WHERE vendor = ?`)
        // so the result set is already scoped to the requested bucket.
        let vendor_owned = vendor.to_string();
        let sessions = tokio::task::spawn_blocking(move || {
            manager.list_sessions_by_vendor(&vendor_owned, status)
        })
        .await
        .map_err(|e| format!("list_sessions join: {e}"))??;

        let mut out: Vec<SessionMeta> = sessions
            .iter()
            .filter(|s| match filter.workdir.as_ref() {
                None => true,
                Some(wanted) => s
                    .context_data
                    .as_ref()
                    .and_then(|v| v.get("workdir"))
                    .and_then(|v| v.as_str())
                    .map(|w| PathBuf::from(w) == *wanted)
                    .unwrap_or(false),
            })
            .map(Self::session_to_meta)
            .collect();

        if let Some(limit) = filter.limit {
            out.truncate(limit as usize);
        }

        Ok(out)
    }

    async fn get_session_info(
        &self,
        vendor: &str,
        session_id: &NativeSessionId,
    ) -> Result<SessionInfo, String> {
        let manager = self.manager();
        let id = session_id.as_str().to_string();
        let session = tokio::task::spawn_blocking(move || manager.get_session(&id))
            .await
            .map_err(|e| format!("get_session_info join: {e}"))??
            .ok_or_else(|| format!("session {} not found", session_id.as_str()))?;

        if session.vendor != vendor {
            return Err(format!(
                "session {} does not belong to vendor {vendor}",
                session_id.as_str()
            ));
        }

        let meta = Self::session_to_meta(&session);
        Ok(SessionInfo {
            meta,
            permission_mode: None,
            model: None,
            usage: Default::default(),
            extras: session.context_data.unwrap_or_else(|| json!({})),
        })
    }

    async fn get_session_messages(
        &self,
        vendor: &str,
        session_id: &NativeSessionId,
        pagination: Pagination,
    ) -> Result<Vec<NativeMessage>, String> {
        let manager = self.manager();
        let id = session_id.as_str().to_string();
        let session = tokio::task::spawn_blocking(move || manager.get_session(&id))
            .await
            .map_err(|e| format!("get_session_messages join: {e}"))??
            .ok_or_else(|| format!("session {} not found", session_id.as_str()))?;

        if session.vendor != vendor {
            return Err(format!(
                "session {} does not belong to vendor {vendor}",
                session_id.as_str()
            ));
        }

        // Native schema stores `SessionMessage { role, content, timestamp }`.
        // Translate straight into `NativeMessage` with the vendor payload
        // being a JSON object wrapping the original content string.
        let mut out: Vec<NativeMessage> = session
            .messages
            .iter()
            .enumerate()
            .map(|(idx, m)| NativeMessage {
                id: m
                    .local_id
                    .clone()
                    .unwrap_or_else(|| format!("{}-{idx}", session.id)),
                role: m.role.clone(),
                payload: json!({ "content": m.content }),
                created_at: chrono::DateTime::parse_from_rfc3339(&m.timestamp)
                    .ok()
                    .map(|d| d.with_timezone(&chrono::Utc)),
            })
            .collect();

        if !pagination.ascending {
            out.reverse();
        }
        if let Some(limit) = pagination.limit {
            out.truncate(limit as usize);
        }

        Ok(out)
    }
}

/// Translate a `SessionStatusFilter` into the internal `SessionStatus` enum,
/// returning `None` for the `Any` bucket (which means "no filter").
fn map_status_filter(f: multi_agent_runtime_core::SessionStatusFilter) -> Option<SessionStatus> {
    use multi_agent_runtime_core::SessionStatusFilter as F;
    match f {
        F::Active => Some(SessionStatus::Active),
        F::Completed => Some(SessionStatus::Closed),
        F::Errored => Some(SessionStatus::Expired),
        F::Any => None,
    }
}

/// Best-effort RFC3339 timestamp parser — falls back to epoch on failure
/// (avoids panics in metadata queries when a row has a malformed timestamp).
fn parse_timestamp(raw: &str) -> chrono::DateTime<chrono::Utc> {
    DateTime::parse_from_rfc3339(raw)
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap())
}

/// Convenience constructor used by the service init path.
pub fn build_session_store(db_path: PathBuf) -> Arc<dyn SessionStoreProvider> {
    Arc::new(CommunitySessionStore::new(db_path))
}

#[cfg(test)]
mod tests {
    use super::CommunitySessionStore;
    use multi_agent_runtime_core::{
        AgentExecutor, NativeSessionId, PermissionMode, SessionFilter, SessionRecord,
        SessionStoreProvider, SpawnSessionSpec,
    };
    use rusqlite::Connection;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn init_agent_sessions_table(db_path: &std::path::Path) {
        let conn = Connection::open(db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE agent_sessions (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                user_id TEXT,
                messages TEXT NOT NULL DEFAULT '[]',
                context_data TEXT,
                status TEXT DEFAULT 'active',
                created_at TEXT,
                updated_at TEXT,
                expires_at TEXT,
                owner_session_id TEXT
            );
            "#,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn record_session_sets_vendor_and_merges_context_idempotently() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        init_agent_sessions_table(&db_path);
        let store = CommunitySessionStore::new(db_path);

        store
            .record_session(
                "claude",
                SessionRecord {
                    session_id: NativeSessionId::new("claude-local-1"),
                    workdir: PathBuf::from("/tmp/workspace-a"),
                    context: json!({
                        "profile_id": "profile-a",
                    }),
                },
            )
            .await
            .unwrap();
        store
            .record_session(
                "claude",
                SessionRecord {
                    session_id: NativeSessionId::new("claude-local-1"),
                    workdir: PathBuf::from("/tmp/workspace-b"),
                    context: json!({
                        "profile_id": "profile-b",
                        "native_session_id": "claude-native-1",
                    }),
                },
            )
            .await
            .unwrap();

        let sessions = store
            .list_sessions("claude", SessionFilter::default())
            .await
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id.as_str(), "claude-local-1");
        assert_eq!(sessions[0].workdir, PathBuf::from("/tmp/workspace-b"));

        let info = store
            .get_session_info("claude", &NativeSessionId::new("claude-local-1"))
            .await
            .unwrap();
        assert_eq!(
            info.extras.get("profile_id").and_then(|v| v.as_str()),
            Some("profile-b")
        );
        assert_eq!(
            info.extras
                .get("native_session_id")
                .and_then(|v| v.as_str()),
            Some("claude-native-1")
        );
    }

    #[tokio::test]
    async fn record_session_makes_codex_rows_listable() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        init_agent_sessions_table(&db_path);
        let store = CommunitySessionStore::new(db_path);

        store
            .record_session(
                "codex",
                SessionRecord {
                    session_id: NativeSessionId::new("codex-local-1"),
                    workdir: PathBuf::from("/tmp/codex-workdir"),
                    context: json!({
                        "native_session_id": "codex-thread-1",
                    }),
                },
            )
            .await
            .unwrap();

        let sessions = store
            .list_sessions("codex", SessionFilter::default())
            .await
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id.as_str(), "codex-local-1");
        assert_eq!(sessions[0].workdir, PathBuf::from("/tmp/codex-workdir"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn claude_spawned_session_is_visible_via_list_sessions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        init_agent_sessions_table(&db_path);
        let store = CommunitySessionStore::new(db_path);
        let workdir = temp.path().join("claude-workdir");
        std::fs::create_dir_all(&workdir).unwrap();

        let cli_path = temp.path().join("fake-claude.sh");
        std::fs::write(
            &cli_path,
            "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"claude-native-xyz\"}'\nwhile IFS= read -r _line; do :; done\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&cli_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&cli_path, perms).unwrap();

        let executor =
            multi_agent_runtime_claude::ClaudeAgentExecutor::new(cli_path, Arc::new(store.clone()));

        let session = executor
            .spawn_session(SpawnSessionSpec {
                workdir: workdir.clone(),
                system_prompt: None,
                model: None,
                permission_mode: PermissionMode::Default,
                allowed_tools: None,
                additional_directories: Vec::new(),
                env: Default::default(),
                agent_config: serde_json::Value::Null,
                resume_hint: None,
            })
            .await
            .unwrap();

        let sessions = store
            .list_sessions("claude", SessionFilter::default())
            .await
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id.as_str(), session.id.as_str());
        assert_eq!(sessions[0].workdir, workdir);

        executor.close_session(&session).await.unwrap();
    }

    #[tokio::test]
    async fn codex_spawned_session_is_visible_via_list_sessions() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        init_agent_sessions_table(&db_path);
        let store = CommunitySessionStore::new(db_path);
        let workdir = temp.path().join("codex-workdir");
        std::fs::create_dir_all(&workdir).unwrap();

        let executor = multi_agent_runtime_codex::CodexAgentExecutor::new(
            PathBuf::from("codex"),
            Arc::new(store.clone()),
        );

        let session = executor
            .spawn_session(SpawnSessionSpec {
                workdir: workdir.clone(),
                system_prompt: None,
                model: None,
                permission_mode: PermissionMode::Default,
                allowed_tools: None,
                additional_directories: Vec::new(),
                env: Default::default(),
                agent_config: serde_json::Value::Null,
                resume_hint: None,
            })
            .await
            .unwrap();

        let sessions = store
            .list_sessions("codex", SessionFilter::default())
            .await
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id.as_str(), session.id.as_str());
        assert_eq!(sessions[0].workdir, workdir);

        executor.close_session(&session).await.unwrap();
    }
}
