//! Agent Session Management
//!
//! Manages sessions for autonomous agents to enable multi-turn conversations
//! with context retention.

use chrono::{Duration, Utc};
use rusqlite::{params, Connection, Result as SqliteResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn non_empty_trimmed(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn extract_text_from_acp_data(data: &serde_json::Value) -> Option<String> {
    let normalized = match data {
        serde_json::Value::String(raw) => serde_json::from_str::<serde_json::Value>(raw).ok()?,
        value => value.clone(),
    };

    match normalized.get("type").and_then(|v| v.as_str()) {
        Some("assistant_message") => normalized
            .get("text")
            .and_then(|v| v.as_str())
            .and_then(non_empty_trimmed),
        Some("error") => normalized
            .get("message")
            .and_then(|v| v.as_str())
            .and_then(non_empty_trimmed),
        _ => None,
    }
}

enum AcpEnvelopeText {
    NotAcp,
    NoText,
    Text(String),
}

fn extract_text_from_acp_envelope(value: &serde_json::Value) -> AcpEnvelopeText {
    if let Some(raw) = value.as_str() {
        let Some(parsed) = serde_json::from_str::<serde_json::Value>(raw).ok() else {
            return AcpEnvelopeText::NotAcp;
        };
        return extract_text_from_acp_envelope(&parsed);
    }

    if value.get("type").and_then(|v| v.as_str()) != Some("acp") {
        return AcpEnvelopeText::NotAcp;
    }

    match value.get("data").and_then(extract_text_from_acp_data) {
        Some(text) => AcpEnvelopeText::Text(text),
        None => AcpEnvelopeText::NoText,
    }
}

fn extract_text_from_persisted_acp_record(content: &str) -> Option<Option<String>> {
    let parsed = serde_json::from_str::<serde_json::Value>(content).ok()?;

    if let Some(content_value) = parsed.get("content") {
        match extract_text_from_acp_envelope(content_value) {
            AcpEnvelopeText::Text(text) => return Some(Some(text)),
            AcpEnvelopeText::NoText => return Some(None),
            AcpEnvelopeText::NotAcp => {}
        }
    }

    match extract_text_from_acp_envelope(&parsed) {
        AcpEnvelopeText::Text(text) => Some(Some(text)),
        AcpEnvelopeText::NoText => Some(None),
        AcpEnvelopeText::NotAcp => None,
    }
}

pub fn extract_last_assistant_text(messages: &[SessionMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if message.role != "assistant" {
            return None;
        }

        if let Some(acp_text) = extract_text_from_persisted_acp_record(&message.content) {
            return acp_text;
        }

        match crate::llm::parse_session_content(&message.content) {
            crate::llm::MessageContent::Text(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty()
                    || trimmed.starts_with("[Tool:")
                    || trimmed.starts_with("[Tool Result:")
                {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            crate::llm::MessageContent::Blocks(blocks) => blocks.iter().rev().find_map(|block| {
                if let crate::llm::ContentBlock::Text { text } = block {
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                } else {
                    None
                }
            }),
        }
    })
}

/// Agent session with conversation history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: String,
    pub agent_id: String,
    pub user_id: Option<String>,
    pub messages: Vec<SessionMessage>,
    pub context_data: Option<serde_json::Value>,
    #[serde(default)]
    pub agent_state: Option<serde_json::Value>,
    #[serde(default)]
    pub agent_state_version: u64,
    pub status: SessionStatus,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: Option<String>,
    /// For task sessions dispatched by a persona, points to the persona's chat session
    pub owner_session_id: Option<String>,
    /// Vendor bucket the session belongs to (e.g. "cteno", "claude", "codex").
    /// Defaults to "cteno" for pre-migration rows.
    #[serde(default = "default_vendor")]
    pub vendor: String,
}

fn default_vendor() -> String {
    "cteno".to_string()
}

/// The default vendor tag assigned to sessions created without an explicit
/// vendor (legacy behaviour). New vendor-aware call-sites must pass
/// `"claude"` / `"codex"` etc. explicitly via `create_session_with_vendor`.
pub const DEFAULT_VENDOR: &str = "cteno";

/// Session message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: String, // "user" or "assistant"
    pub content: String,
    pub timestamp: String,
    #[serde(default)]
    pub local_id: Option<String>,
}

/// Session status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Active,
    Expired,
    Closed,
}

impl SessionStatus {
    fn as_str(&self) -> &str {
        match self {
            SessionStatus::Active => "active",
            SessionStatus::Expired => "expired",
            SessionStatus::Closed => "closed",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "expired" => SessionStatus::Expired,
            "closed" => SessionStatus::Closed,
            _ => SessionStatus::Active,
        }
    }
}

impl AgentSession {
    /// Get a specific field from the session's context_data JSON object.
    pub fn get_context_field(&self, key: &str) -> Option<&serde_json::Value> {
        self.context_data.as_ref()?.get(key)
    }
}

/// Default session timeout in minutes
const DEFAULT_TIMEOUT_MINUTES: i64 = 30;

/// Agent session manager
pub struct AgentSessionManager {
    db_path: PathBuf,
}

impl AgentSessionManager {
    /// Create a new session manager
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    /// Get the database path.
    pub fn db_path(&self) -> &PathBuf {
        &self.db_path
    }

    /// Open a connection to the database.
    ///
    /// Runs the idempotent schema bootstrap + vendor-column migration so
    /// every caller of this manager sees an `agent_sessions` table with the
    /// expected columns, regardless of which process bootstrapped the DB
    /// file. Two concrete callers rely on this:
    ///   - The desktop host's `db.rs` runs a canonical migration at daemon
    ///     start, but that only covers the desktop DB path.
    ///   - The stdio `cteno-agent` subprocess opens its own SQLite at
    ///     `$CTENO_AGENT_DATA_DIR/sessions.db`. On a fresh daemon / fresh
    ///     user install that file has no tables yet, so the very first
    ///     `get_session` would fail with `no such table: agent_sessions`
    ///     unless we create it here.
    fn connect(&self) -> SqliteResult<Connection> {
        let conn = Connection::open(&self.db_path)?;
        Self::ensure_table(&conn)?;
        Ok(conn)
    }

    /// Idempotent schema bootstrap. Creates the `agent_sessions` table if
    /// missing (matching the canonical shape in `apps/client/desktop/src/db.rs`),
    /// then applies the owner/vendor column migrations the same way the
    /// desktop bootstrap does. Safe to call on every `connect()`.
    fn ensure_table(conn: &Connection) -> SqliteResult<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS agent_sessions (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                user_id TEXT,
                messages TEXT NOT NULL DEFAULT '[]',
                context_data TEXT,
                agent_state TEXT,
                agent_state_version INTEGER NOT NULL DEFAULT 0,
                status TEXT DEFAULT 'active' CHECK(status IN ('active', 'expired', 'closed')),
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now')),
                expires_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_agent_sessions_agent ON agent_sessions(agent_id);
            CREATE INDEX IF NOT EXISTS idx_agent_sessions_user ON agent_sessions(user_id);
            CREATE INDEX IF NOT EXISTS idx_agent_sessions_status ON agent_sessions(status);
            CREATE INDEX IF NOT EXISTS idx_agent_sessions_expires ON agent_sessions(expires_at);
            "#,
        )?;

        // owner_session_id — added by the desktop-side db.rs migration;
        // mirror it here so a stdio-first DB also carries the column.
        let has_owner_col: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('agent_sessions') WHERE name='owner_session_id'",
            [],
            |row| row.get(0),
        )?;
        if has_owner_col == 0 {
            conn.execute(
                "ALTER TABLE agent_sessions ADD COLUMN owner_session_id TEXT",
                [],
            )?;
        }

        Self::ensure_vendor_column(conn)?;
        Self::ensure_agent_state_columns(conn)?;
        Ok(())
    }

    /// Idempotently add `vendor TEXT NOT NULL DEFAULT 'cteno'` to
    /// `agent_sessions` if the table exists and the column is missing.
    ///
    /// Safe to call on every `connect()`: cost is a single
    /// `pragma_table_info` lookup. If the table itself does not exist yet
    /// (fresh DB without the desktop-side `CREATE TABLE` bootstrap), this
    /// is a no-op — the migration applies lazily once the table appears.
    fn ensure_vendor_column(conn: &Connection) -> SqliteResult<()> {
        // Check whether the table exists at all — guards against opening a
        // brand-new file before `CREATE TABLE` has been run elsewhere.
        let table_exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_sessions'",
            [],
            |row| row.get(0),
        )?;
        if table_exists == 0 {
            return Ok(());
        }

        let has_vendor: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('agent_sessions') WHERE name='vendor'",
            [],
            |row| row.get(0),
        )?;
        if has_vendor == 0 {
            conn.execute(
                "ALTER TABLE agent_sessions ADD COLUMN vendor TEXT NOT NULL DEFAULT 'cteno'",
                [],
            )?;
            log::info!("[agent_session] migrated agent_sessions: added vendor column");
        }
        Ok(())
    }

    fn ensure_agent_state_columns(conn: &Connection) -> SqliteResult<()> {
        let table_exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_sessions'",
            [],
            |row| row.get(0),
        )?;
        if table_exists == 0 {
            return Ok(());
        }

        let has_agent_state: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('agent_sessions') WHERE name='agent_state'",
            [],
            |row| row.get(0),
        )?;
        if has_agent_state == 0 {
            conn.execute("ALTER TABLE agent_sessions ADD COLUMN agent_state TEXT", [])?;
            conn.execute(
                "ALTER TABLE agent_sessions ADD COLUMN agent_state_version INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            log::info!("[agent_session] migrated agent_sessions: added agent_state columns");
        }
        Ok(())
    }

    /// Create a new session (backward-compatible: tags the row with the
    /// default `"cteno"` vendor). Vendor-aware callers should use
    /// [`Self::create_session_with_vendor`] instead.
    pub fn create_session(
        &self,
        agent_id: &str,
        user_id: Option<&str>,
        timeout_minutes: Option<i64>,
    ) -> Result<AgentSession, String> {
        self.create_session_with_vendor(agent_id, user_id, timeout_minutes, DEFAULT_VENDOR)
    }

    /// Create a new session explicitly tagged with a vendor bucket
    /// (e.g. `"cteno"`, `"claude"`, `"codex"`).
    pub fn create_session_with_vendor(
        &self,
        agent_id: &str,
        user_id: Option<&str>,
        timeout_minutes: Option<i64>,
        vendor: &str,
    ) -> Result<AgentSession, String> {
        let conn = self.connect().map_err(|e| e.to_string())?;

        let session_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let timeout = timeout_minutes.unwrap_or(DEFAULT_TIMEOUT_MINUTES);
        let expires_at = now + Duration::minutes(timeout);

        let created_at = now.to_rfc3339();
        let expires_at_str = expires_at.to_rfc3339();

        conn.execute(
            "INSERT INTO agent_sessions (id, agent_id, user_id, messages, status, created_at, updated_at, expires_at, vendor)
             VALUES (?1, ?2, ?3, '[]', 'active', ?4, ?4, ?5, ?6)",
            params![session_id, agent_id, user_id, created_at, expires_at_str, vendor],
        )
        .map_err(|e| e.to_string())?;

        Ok(AgentSession {
            id: session_id,
            agent_id: agent_id.to_string(),
            user_id: user_id.map(|s| s.to_string()),
            messages: vec![],
            context_data: None,
            agent_state: None,
            agent_state_version: 0,
            status: SessionStatus::Active,
            created_at,
            updated_at: now.to_rfc3339(),
            expires_at: Some(expires_at_str),
            owner_session_id: None,
            vendor: vendor.to_string(),
        })
    }

    /// Create a new session with a custom ID (backward-compatible: tags
    /// the row with the default `"cteno"` vendor).
    pub fn create_session_with_id(
        &self,
        session_id: &str,
        agent_id: &str,
        user_id: Option<&str>,
        timeout_minutes: Option<i64>,
    ) -> Result<AgentSession, String> {
        self.create_session_with_id_and_vendor(
            session_id,
            agent_id,
            user_id,
            timeout_minutes,
            DEFAULT_VENDOR,
        )
    }

    /// Create a new session with a custom ID and explicit vendor tag.
    pub fn create_session_with_id_and_vendor(
        &self,
        session_id: &str,
        agent_id: &str,
        user_id: Option<&str>,
        timeout_minutes: Option<i64>,
        vendor: &str,
    ) -> Result<AgentSession, String> {
        let conn = self.connect().map_err(|e| e.to_string())?;

        let now = Utc::now();
        let timeout = timeout_minutes.unwrap_or(DEFAULT_TIMEOUT_MINUTES);
        let expires_at = now + Duration::minutes(timeout);

        let created_at = now.to_rfc3339();
        let expires_at_str = expires_at.to_rfc3339();

        conn.execute(
            "INSERT INTO agent_sessions (id, agent_id, user_id, messages, status, created_at, updated_at, expires_at, vendor)
             VALUES (?1, ?2, ?3, '[]', 'active', ?4, ?4, ?5, ?6)",
            params![session_id, agent_id, user_id, created_at, expires_at_str, vendor],
        )
        .map_err(|e| e.to_string())?;

        Ok(AgentSession {
            id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            user_id: user_id.map(|s| s.to_string()),
            messages: vec![],
            context_data: None,
            agent_state: None,
            agent_state_version: 0,
            status: SessionStatus::Active,
            created_at,
            updated_at: now.to_rfc3339(),
            expires_at: Some(expires_at_str),
            owner_session_id: None,
            vendor: vendor.to_string(),
        })
    }

    /// Update the vendor tag on an existing session. Intended for vendor
    /// adapters that lazily mirror subprocess-backed sessions into the
    /// local store after they already exist with the default `"cteno"` tag.
    pub fn set_vendor(&self, session_id: &str, vendor: &str) -> Result<(), String> {
        let conn = self.connect().map_err(|e| e.to_string())?;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE agent_sessions SET vendor = ?1, updated_at = ?2 WHERE id = ?3",
            params![vendor, now, session_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// List sessions belonging to a specific vendor bucket. `status_filter`
    /// narrows by lifecycle state; pass `None` for all statuses.
    pub fn list_sessions_by_vendor(
        &self,
        vendor: &str,
        status_filter: Option<SessionStatus>,
    ) -> Result<Vec<AgentSession>, String> {
        let conn = self.connect().map_err(|e| e.to_string())?;

        let sessions: Vec<AgentSession> = if let Some(status) = status_filter {
            let status_str = status.as_str();
            let mut stmt = conn
                .prepare(
                    "SELECT id, agent_id, user_id, messages, context_data, status, created_at, updated_at, expires_at, owner_session_id, vendor, agent_state, agent_state_version
                     FROM agent_sessions WHERE vendor = ?1 AND status = ?2 ORDER BY updated_at DESC",
                )
                .map_err(|e| e.to_string())?;

            let rows = stmt
                .query_map(rusqlite::params![vendor, status_str], Self::row_to_session)
                .map_err(|e| e.to_string())?;

            rows.collect::<Result<Vec<_>, rusqlite::Error>>()
                .map_err(|e| e.to_string())?
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT id, agent_id, user_id, messages, context_data, status, created_at, updated_at, expires_at, owner_session_id, vendor, agent_state, agent_state_version
                     FROM agent_sessions WHERE vendor = ?1 ORDER BY updated_at DESC",
                )
                .map_err(|e| e.to_string())?;

            let rows = stmt
                .query_map(rusqlite::params![vendor], Self::row_to_session)
                .map_err(|e| e.to_string())?;

            rows.collect::<Result<Vec<_>, rusqlite::Error>>()
                .map_err(|e| e.to_string())?
        };

        Ok(sessions)
    }

    /// Get a session by ID
    pub fn get_session(&self, session_id: &str) -> Result<Option<AgentSession>, String> {
        let conn = self.connect().map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, user_id, messages, context_data, status, created_at, updated_at, expires_at, owner_session_id, vendor, agent_state, agent_state_version
                 FROM agent_sessions WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;

        let mut rows = stmt.query(params![session_id]).map_err(|e| e.to_string())?;

        if let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let messages_json: String = row.get(3).map_err(|e| e.to_string())?;
            let messages: Vec<SessionMessage> =
                serde_json::from_str(&messages_json).unwrap_or_default();

            let context_data_json: Option<String> = row.get(4).map_err(|e| e.to_string())?;
            let context_data = context_data_json.and_then(|s| serde_json::from_str(&s).ok());

            let status_str: String = row.get(5).map_err(|e| e.to_string())?;

            Ok(Some(AgentSession {
                id: row.get(0).map_err(|e| e.to_string())?,
                agent_id: row.get(1).map_err(|e| e.to_string())?,
                user_id: row.get(2).map_err(|e| e.to_string())?,
                messages,
                context_data,
                agent_state: row
                    .get::<_, Option<String>>(11)
                    .map_err(|e| e.to_string())?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                agent_state_version: row
                    .get::<_, Option<i64>>(12)
                    .map_err(|e| e.to_string())?
                    .unwrap_or_default()
                    .max(0) as u64,
                status: SessionStatus::from_str(&status_str),
                created_at: row.get(6).map_err(|e| e.to_string())?,
                updated_at: row.get(7).map_err(|e| e.to_string())?,
                expires_at: row.get(8).map_err(|e| e.to_string())?,
                owner_session_id: row.get(9).map_err(|e| e.to_string())?,
                vendor: row
                    .get::<_, Option<String>>(10)
                    .map_err(|e| e.to_string())?
                    .unwrap_or_else(|| DEFAULT_VENDOR.to_string()),
            }))
        } else {
            Ok(None)
        }
    }

    /// Update session messages
    pub fn update_messages(
        &self,
        session_id: &str,
        messages: &[SessionMessage],
    ) -> Result<(), String> {
        let conn = self.connect().map_err(|e| e.to_string())?;

        let messages_json = serde_json::to_string(messages).map_err(|e| e.to_string())?;
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE agent_sessions SET messages = ?1, updated_at = ?2 WHERE id = ?3",
            params![messages_json, now, session_id],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Update the owner_session_id field
    pub fn update_owner_session_id(
        &self,
        session_id: &str,
        owner_session_id: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.connect().map_err(|e| e.to_string())?;
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE agent_sessions SET owner_session_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![owner_session_id, now, session_id],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Update session context_data JSON blob
    pub fn update_context_data(
        &self,
        session_id: &str,
        context_data: &serde_json::Value,
    ) -> Result<(), String> {
        let conn = self.connect().map_err(|e| e.to_string())?;

        let context_json = serde_json::to_string(context_data)
            .map_err(|e| format!("Invalid context_data: {}", e))?;
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE agent_sessions SET context_data = ?1, updated_at = ?2 WHERE id = ?3",
            params![context_json, now, session_id],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Persist the latest host-facing agent state snapshot.
    pub fn update_agent_state(
        &self,
        session_id: &str,
        agent_state: Option<&serde_json::Value>,
        version: u64,
    ) -> Result<(), String> {
        let conn = self.connect().map_err(|e| e.to_string())?;

        let state_json = agent_state
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| format!("Invalid agent_state: {}", e))?;
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE agent_sessions SET agent_state = ?1, agent_state_version = ?2, updated_at = ?3 WHERE id = ?4",
            params![state_json, version as i64, now, session_id],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Permanently delete a session row from `agent_sessions`. Returns
    /// `Ok(true)` when a row was removed, `Ok(false)` when no row matched.
    ///
    /// Called by the machine-scoped `delete-session` RPC now that
    /// happy-server is relay-only and the legacy `DELETE /v1/sessions/:id`
    /// HTTP endpoint no longer exists.
    pub fn delete_session(&self, session_id: &str) -> Result<bool, String> {
        let conn = self.connect().map_err(|e| e.to_string())?;
        let affected = conn
            .execute(
                "DELETE FROM agent_sessions WHERE id = ?1",
                params![session_id],
            )
            .map_err(|e| e.to_string())?;
        Ok(affected > 0)
    }

    /// Update a single field within the session's context_data JSON blob.
    /// Reads the existing context_data, merges the key, and writes back.
    pub fn update_context_field(
        &self,
        session_id: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), String> {
        let conn = self.connect().map_err(|e| e.to_string())?;

        // Read existing context_data
        let existing_json: Option<String> = conn
            .query_row(
                "SELECT context_data FROM agent_sessions WHERE id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;

        let mut context: serde_json::Value = existing_json
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({}));

        // Merge key
        if let Some(obj) = context.as_object_mut() {
            obj.insert(key.to_string(), value);
        }

        // Write back via existing method
        self.update_context_data(session_id, &context)
    }

    /// Extend session expiration time
    pub fn extend_session(
        &self,
        session_id: &str,
        timeout_minutes: Option<i64>,
    ) -> Result<(), String> {
        let conn = self.connect().map_err(|e| e.to_string())?;

        let timeout = timeout_minutes.unwrap_or(DEFAULT_TIMEOUT_MINUTES);
        let new_expires_at = (Utc::now() + Duration::minutes(timeout)).to_rfc3339();
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE agent_sessions SET expires_at = ?1, updated_at = ?2 WHERE id = ?3",
            params![new_expires_at, now, session_id],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Close a session
    pub fn close_session(&self, session_id: &str) -> Result<(), String> {
        let conn = self.connect().map_err(|e| e.to_string())?;
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE agent_sessions SET status = 'closed', updated_at = ?1 WHERE id = ?2",
            params![now, session_id],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Clear session messages (reset conversation history)
    pub fn clear_messages(&self, session_id: &str) -> Result<(), String> {
        let conn = self.connect().map_err(|e| e.to_string())?;
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE agent_sessions SET messages = '[]', updated_at = ?1 WHERE id = ?2",
            params![now, session_id],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Cleanup expired sessions
    pub fn cleanup_expired(&self) -> Result<usize, String> {
        let conn = self.connect().map_err(|e| e.to_string())?;
        let now = Utc::now().to_rfc3339();

        let count = conn
            .execute(
                "UPDATE agent_sessions SET status = 'expired'
                 WHERE status = 'active' AND expires_at < ?1",
                params![now],
            )
            .map_err(|e| e.to_string())?;

        log::info!("Cleaned up {} expired agent sessions", count);
        Ok(count)
    }

    /// List active sessions for a specific agent
    pub fn list_by_agent(
        &self,
        agent_id: &str,
        status_filter: Option<SessionStatus>,
    ) -> Result<Vec<AgentSession>, String> {
        let conn = self.connect().map_err(|e| e.to_string())?;

        let sessions: Vec<AgentSession> = if let Some(status) = status_filter {
            let status_str = status.as_str();
            let mut stmt = conn.prepare(
                "SELECT id, agent_id, user_id, messages, context_data, status, created_at, updated_at, expires_at, owner_session_id, vendor, agent_state, agent_state_version
                 FROM agent_sessions WHERE agent_id = ?1 AND status = ?2 ORDER BY updated_at DESC"
            ).map_err(|e| e.to_string())?;

            let rows = stmt
                .query_map(
                    rusqlite::params![agent_id, status_str],
                    Self::row_to_session,
                )
                .map_err(|e| e.to_string())?;

            rows.collect::<Result<Vec<_>, rusqlite::Error>>()
                .map_err(|e| e.to_string())?
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, agent_id, user_id, messages, context_data, status, created_at, updated_at, expires_at, owner_session_id, vendor, agent_state, agent_state_version
                 FROM agent_sessions WHERE agent_id = ?1 ORDER BY updated_at DESC"
            ).map_err(|e| e.to_string())?;

            let rows = stmt
                .query_map(rusqlite::params![agent_id], Self::row_to_session)
                .map_err(|e| e.to_string())?;

            rows.collect::<Result<Vec<_>, rusqlite::Error>>()
                .map_err(|e| e.to_string())?
        };

        Ok(sessions)
    }

    /// List all local agent sessions.
    pub fn list_sessions(
        &self,
        status_filter: Option<SessionStatus>,
    ) -> Result<Vec<AgentSession>, String> {
        let conn = self.connect().map_err(|e| e.to_string())?;

        let sessions: Vec<AgentSession> = if let Some(status) = status_filter {
            let status_str = status.as_str();
            let mut stmt = conn
                .prepare(
                    "SELECT id, agent_id, user_id, messages, context_data, status, created_at, updated_at, expires_at, owner_session_id, vendor, agent_state, agent_state_version
                     FROM agent_sessions WHERE status = ?1 ORDER BY updated_at DESC",
                )
                .map_err(|e| e.to_string())?;

            let rows = stmt
                .query_map(rusqlite::params![status_str], Self::row_to_session)
                .map_err(|e| e.to_string())?;

            rows.collect::<Result<Vec<_>, rusqlite::Error>>()
                .map_err(|e| e.to_string())?
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT id, agent_id, user_id, messages, context_data, status, created_at, updated_at, expires_at, owner_session_id, vendor, agent_state, agent_state_version
                     FROM agent_sessions ORDER BY updated_at DESC",
                )
                .map_err(|e| e.to_string())?;

            let rows = stmt
                .query_map([], Self::row_to_session)
                .map_err(|e| e.to_string())?;

            rows.collect::<Result<Vec<_>, rusqlite::Error>>()
                .map_err(|e| e.to_string())?
        };

        Ok(sessions)
    }

    /// Extract the final text output from a session's conversation history.
    ///
    /// Walks backward through messages to find the final assistant-visible text.
    /// Handles both plain-text persistence and structured BLOCKS: content.
    pub fn extract_final_output(&self, session_id: &str) -> String {
        match self.get_session(session_id) {
            Ok(Some(session)) => extract_last_assistant_text(&session.messages).unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// Helper to convert a row to AgentSession
    fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<AgentSession> {
        let messages_json: String = row.get(3)?;
        let messages: Vec<SessionMessage> =
            serde_json::from_str(&messages_json).unwrap_or_default();

        let context_data_json: Option<String> = row.get(4)?;
        let context_data = context_data_json.and_then(|s| serde_json::from_str(&s).ok());

        let status_str: String = row.get(5)?;

        Ok(AgentSession {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            user_id: row.get(2)?,
            messages,
            context_data,
            agent_state: row
                .get::<_, Option<String>>(11)
                .ok()
                .flatten()
                .and_then(|s| serde_json::from_str(&s).ok()),
            agent_state_version: row
                .get::<_, Option<i64>>(12)
                .ok()
                .flatten()
                .unwrap_or_default()
                .max(0) as u64,
            status: SessionStatus::from_str(&status_str),
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
            expires_at: row.get(8)?,
            owner_session_id: row.get(9).ok().flatten(),
            vendor: row
                .get::<_, Option<String>>(10)
                .ok()
                .flatten()
                .unwrap_or_else(|| DEFAULT_VENDOR.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_last_assistant_text, SessionMessage};

    fn assistant(content: String) -> SessionMessage {
        SessionMessage {
            role: "assistant".to_string(),
            content,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            local_id: None,
        }
    }

    fn user(content: &str) -> SessionMessage {
        SessionMessage {
            role: "user".to_string(),
            content: content.to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            local_id: None,
        }
    }

    #[test]
    fn extracts_plain_text_assistant_reply() {
        let messages = vec![user("hi"), assistant("done".to_string())];

        assert_eq!(
            extract_last_assistant_text(&messages).as_deref(),
            Some("done")
        );
    }

    #[test]
    fn extracts_last_text_block_from_structured_content() {
        let content =
            crate::llm::serialize_content_for_session(&crate::llm::MessageContent::Blocks(vec![
                crate::llm::ContentBlock::Thinking {
                    thinking: "working".to_string(),
                    signature: "sig".to_string(),
                },
                crate::llm::ContentBlock::Text {
                    text: "final answer".to_string(),
                },
            ]));
        let messages = vec![user("hi"), assistant(content)];

        assert_eq!(
            extract_last_assistant_text(&messages).as_deref(),
            Some("final answer")
        );
    }

    #[test]
    fn skips_tool_only_blocks_and_finds_latest_text_reply() {
        let tool_only =
            crate::llm::serialize_content_for_session(&crate::llm::MessageContent::Blocks(vec![
                crate::llm::ContentBlock::ToolUse {
                    id: "tool_1".to_string(),
                    name: "search".to_string(),
                    input: serde_json::json!({}),
                    gemini_thought_signature: None,
                },
            ]));
        let final_text =
            crate::llm::serialize_content_for_session(&crate::llm::MessageContent::Blocks(vec![
                crate::llm::ContentBlock::Text {
                    text: "wrapped up".to_string(),
                },
            ]));
        let messages = vec![user("hi"), assistant(tool_only), assistant(final_text)];

        assert_eq!(
            extract_last_assistant_text(&messages).as_deref(),
            Some("wrapped up")
        );
    }

    fn acp_message(data: serde_json::Value) -> String {
        serde_json::json!({
            "role": "agent",
            "content": {
                "type": "acp",
                "provider": "cteno",
                "data": data,
            },
            "meta": {
                "sentFrom": "cli",
            },
        })
        .to_string()
    }

    #[test]
    fn skips_task_complete_acp_and_extracts_assistant_message_text() {
        let messages = vec![
            assistant(acp_message(
                serde_json::json!({ "type": "assistant_message", "text": "done" }),
            )),
            assistant(acp_message(serde_json::json!({
                "type": "task_complete",
                "id": "task-1"
            }))),
        ];

        assert_eq!(
            extract_last_assistant_text(&messages).as_deref(),
            Some("done")
        );
    }

    #[test]
    fn extracts_assistant_message_from_stringified_acp_content() {
        let nested_content = serde_json::json!({
            "type": "acp",
            "provider": "cteno",
            "data": {
                "type": "assistant_message",
                "text": "nested reply",
            },
        })
        .to_string();
        let message = serde_json::json!({
            "role": "agent",
            "content": nested_content,
        })
        .to_string();
        let messages = vec![assistant(message)];

        assert_eq!(
            extract_last_assistant_text(&messages).as_deref(),
            Some("nested reply")
        );
    }

    /// End-to-end test for the T2 vendor migration + vendor-aware CRUD.
    ///
    /// 1. Bootstrap a legacy schema (no vendor column) with two rows.
    /// 2. Open `AgentSessionManager` — migration should add the column.
    /// 3. Create a new row tagged `"claude"` via the vendor-aware API.
    /// 4. `list_sessions_by_vendor("cteno")` returns the two legacy rows.
    /// 5. `list_sessions_by_vendor("claude")` returns only the new row.
    /// 6. `set_vendor` flips one legacy row to `"codex"` and filters update.
    #[test]
    fn vendor_migration_and_filtering() {
        use super::{AgentSessionManager, SessionStatus};
        use rusqlite::Connection;

        let db_path =
            std::env::temp_dir().join(format!("cteno-vendor-test-{}.db", uuid::Uuid::new_v4()));
        let _ = std::fs::remove_file(&db_path);

        // Step 1: bootstrap legacy schema (intentionally no vendor column).
        {
            let conn = Connection::open(&db_path).unwrap();
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
                INSERT INTO agent_sessions (id, agent_id, status, created_at, updated_at)
                VALUES ('legacy-1', 'worker', 'active', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
                INSERT INTO agent_sessions (id, agent_id, status, created_at, updated_at)
                VALUES ('legacy-2', 'persona', 'active', '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z');
                "#,
            ).unwrap();
        }

        // Step 2: opening the manager triggers the idempotent migration.
        let mgr = AgentSessionManager::new(db_path.clone());

        // Step 3: create a vendor-tagged row.
        let claude_session = mgr
            .create_session_with_id_and_vendor(
                "claude-1",
                "claude:subprocess",
                None,
                Some(60),
                "claude",
            )
            .unwrap();
        assert_eq!(claude_session.vendor, "claude");

        // Step 4: cteno bucket has the two legacy rows (defaulted to 'cteno').
        let cteno_rows = mgr
            .list_sessions_by_vendor("cteno", Some(SessionStatus::Active))
            .unwrap();
        assert_eq!(cteno_rows.len(), 2);
        for row in &cteno_rows {
            assert_eq!(row.vendor, "cteno");
        }

        // Step 5: claude bucket has only the newly tagged row.
        let claude_rows = mgr.list_sessions_by_vendor("claude", None).unwrap();
        assert_eq!(claude_rows.len(), 1);
        assert_eq!(claude_rows[0].id, "claude-1");

        // get_session round-trips vendor correctly.
        let fetched = mgr.get_session("legacy-1").unwrap().unwrap();
        assert_eq!(fetched.vendor, "cteno");

        // Step 6: retag via set_vendor and verify filtering follows.
        mgr.set_vendor("legacy-1", "codex").unwrap();
        let codex_rows = mgr.list_sessions_by_vendor("codex", None).unwrap();
        assert_eq!(codex_rows.len(), 1);
        assert_eq!(codex_rows[0].id, "legacy-1");

        let cteno_after = mgr.list_sessions_by_vendor("cteno", None).unwrap();
        assert_eq!(cteno_after.len(), 1);
        assert_eq!(cteno_after[0].id, "legacy-2");

        let _ = std::fs::remove_file(&db_path);
    }
}
