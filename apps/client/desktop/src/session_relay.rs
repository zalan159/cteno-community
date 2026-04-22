use std::path::PathBuf;
use std::sync::Arc;

use crate::happy_client::socket::HappySocket;
use rusqlite::{params, Connection, Error as SqlError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Manager;

const RELAY_SESSION_MESSAGES_RESPONSE_EVENT: &str = "relay:session-messages-response";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionMessage {
    pub id: String,
    pub local_id: Option<String>,
    pub created_at: i64,
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionMessagesPage {
    pub messages: Vec<LocalSessionMessage>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedSessionMessage {
    role: String,
    content: String,
    timestamp: String,
    // Runtime persistence historically writes `local_id` (snake_case) because
    // `SessionMessage` uses that field name. Keep accepting `localId` as the
    // canonical key for forward compatibility, but alias snake_case so message
    // dedup in the frontend can still reconcile optimistic user bubbles.
    #[serde(default, alias = "local_id")]
    local_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelaySessionMessagesRequest {
    session_id: String,
    request_id: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelaySessionMessagesSuccessResponse {
    request_id: String,
    session_id: String,
    messages: Vec<LocalSessionMessage>,
    has_more: bool,
    offset: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelaySessionMessagesErrorResponse<'a> {
    request_id: &'a str,
    error: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMessagesLoadError {
    SessionNotFound,
    DecodeFailed,
    QueryFailed(String),
}

impl SessionMessagesLoadError {
    fn relay_error_code(&self) -> &'static str {
        match self {
            SessionMessagesLoadError::SessionNotFound => "session_not_found",
            SessionMessagesLoadError::DecodeFailed => "decode_failed",
            SessionMessagesLoadError::QueryFailed(_) => "internal_error",
        }
    }
}

pub async fn attach_machine_socket_listener(socket: Arc<HappySocket>) -> Result<(), String> {
    let db_path = crate::local_services::agent_runtime_context()?.db_path;
    let listener_socket = socket.clone();
    socket
        .on_relay_session_messages_request(move |payload| {
            let socket = listener_socket.clone();
            let db_path = db_path.clone();
            async move {
                if let Err(error) =
                    handle_relay_session_messages_request(socket, db_path, payload).await
                {
                    log::warn!("relay:session-messages-request handler failed: {error}");
                }
            }
        })
        .await;
    Ok(())
}

pub async fn handle_relay_session_messages_request(
    socket: Arc<HappySocket>,
    db_path: PathBuf,
    payload: Value,
) -> Result<(), String> {
    let request: RelaySessionMessagesRequest =
        serde_json::from_value(payload.clone()).map_err(|error| {
            format!("relay:session-messages-request payload decode failed: {error}")
        })?;

    let request_id = request.request_id.clone();
    let session_id = request.session_id.clone();
    let query_session_id = session_id.clone();
    let offset = request.offset.unwrap_or(0);
    let limit = request.limit;

    let page = tauri::async_runtime::spawn_blocking(move || {
        let conn = Connection::open(&db_path).map_err(|error| {
            SessionMessagesLoadError::QueryFailed(format!("open db {}: {error}", db_path.display()))
        })?;
        load_session_messages_from_db(&conn, &query_session_id, limit, Some(offset))
    })
    .await
    .map_err(|error| format!("relay:session-messages-request join failed: {error}"))?;

    match page {
        Ok(page) => {
            let response = RelaySessionMessagesSuccessResponse {
                request_id,
                session_id,
                messages: page.messages,
                has_more: page.has_more,
                offset,
            };
            socket
                .emit(
                    RELAY_SESSION_MESSAGES_RESPONSE_EVENT,
                    serde_json::to_value(response)
                        .map_err(|error| format!("serialize relay response failed: {error}"))?,
                )
                .await
        }
        Err(error) => {
            log::warn!(
                "relay:session-messages-request failed for session {}: {:?}",
                session_id,
                error
            );
            emit_error_response(&socket, &request_id, error.relay_error_code()).await
        }
    }
}

async fn emit_error_response(
    socket: &HappySocket,
    request_id: &str,
    error: &str,
) -> Result<(), String> {
    socket
        .emit(
            RELAY_SESSION_MESSAGES_RESPONSE_EVENT,
            json!(RelaySessionMessagesErrorResponse { request_id, error }),
        )
        .await
}

pub fn load_session_messages_from_db(
    conn: &Connection,
    session_id: &str,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<LocalSessionMessagesPage, SessionMessagesLoadError> {
    let raw_messages = match conn.query_row(
        "SELECT messages FROM agent_sessions WHERE id = ?1",
        params![session_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(value) => value,
        Err(SqlError::QueryReturnedNoRows) => {
            return Err(SessionMessagesLoadError::SessionNotFound)
        }
        Err(error) => {
            return Err(SessionMessagesLoadError::QueryFailed(format!(
                "query session messages: {error}"
            )))
        }
    };

    let parsed: Vec<PersistedSessionMessage> =
        serde_json::from_str(&raw_messages).map_err(|_| SessionMessagesLoadError::DecodeFailed)?;

    let limit = limit.unwrap_or(100);
    let offset = offset.unwrap_or(0);
    let end = parsed.len().saturating_sub(offset);
    let start = end.saturating_sub(limit);
    let has_more = start > 0;

    let messages = parsed[start..end]
        .iter()
        .enumerate()
        .rev()
        .map(|(relative_idx, message)| {
            let absolute_idx = start + relative_idx;
            LocalSessionMessage {
                id: message
                    .local_id
                    .clone()
                    .unwrap_or_else(|| format!("{session_id}:{absolute_idx}")),
                local_id: message.local_id.clone(),
                created_at: parse_timestamp_ms(&message.timestamp),
                role: message.role.clone(),
                text: message.content.clone(),
            }
        })
        .collect();

    Ok(LocalSessionMessagesPage { messages, has_more })
}

pub fn resolve_db_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("db").join("cteno.db"))
        .map_err(|error| format!("resolve app data dir: {error}"))
}

fn parse_timestamp_ms(raw: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|value| value.timestamp_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_agent_sessions_table(conn: &Connection) {
        conn.execute_batch(
            r#"
            CREATE TABLE agent_sessions (
                id TEXT PRIMARY KEY,
                messages TEXT NOT NULL
            );
            "#,
        )
        .unwrap();
    }

    fn insert_session_messages(conn: &Connection, session_id: &str, messages: &str) {
        conn.execute(
            "INSERT INTO agent_sessions (id, messages) VALUES (?1, ?2)",
            params![session_id, messages],
        )
        .unwrap();
    }

    #[test]
    fn paginates_newest_messages_first() {
        let conn = Connection::open_in_memory().unwrap();
        init_agent_sessions_table(&conn);
        insert_session_messages(
            &conn,
            "session-1",
            &serde_json::to_string(&vec![
                PersistedSessionMessage {
                    role: "user".to_string(),
                    content: "one".to_string(),
                    timestamp: "2026-04-19T00:00:00Z".to_string(),
                    local_id: Some("local-1".to_string()),
                },
                PersistedSessionMessage {
                    role: "assistant".to_string(),
                    content: "two".to_string(),
                    timestamp: "2026-04-19T00:00:01Z".to_string(),
                    local_id: None,
                },
                PersistedSessionMessage {
                    role: "assistant".to_string(),
                    content: "three".to_string(),
                    timestamp: "2026-04-19T00:00:02Z".to_string(),
                    local_id: None,
                },
            ])
            .unwrap(),
        );

        let page = load_session_messages_from_db(&conn, "session-1", Some(2), Some(0)).unwrap();
        assert_eq!(page.messages.len(), 2);
        assert_eq!(page.messages[0].text, "three");
        assert_eq!(page.messages[1].text, "two");
        assert!(page.has_more);

        let older_page =
            load_session_messages_from_db(&conn, "session-1", Some(2), Some(2)).unwrap();
        assert_eq!(older_page.messages.len(), 1);
        assert_eq!(older_page.messages[0].id, "local-1");
        assert!(!older_page.has_more);
    }

    #[test]
    fn accepts_snake_case_local_id_from_runtime_store() {
        let conn = Connection::open_in_memory().unwrap();
        init_agent_sessions_table(&conn);
        insert_session_messages(
            &conn,
            "session-1",
            r#"[{"role":"user","content":"hello","timestamp":"2026-04-19T00:00:00Z","local_id":"local-1"}]"#,
        );

        let page = load_session_messages_from_db(&conn, "session-1", Some(10), Some(0)).unwrap();
        assert_eq!(page.messages.len(), 1);
        assert_eq!(page.messages[0].id, "local-1");
        assert_eq!(page.messages[0].local_id.as_deref(), Some("local-1"));
        assert!(!page.has_more);
    }

    #[test]
    fn returns_session_not_found_when_row_missing() {
        let conn = Connection::open_in_memory().unwrap();
        init_agent_sessions_table(&conn);

        let error = load_session_messages_from_db(&conn, "missing", Some(10), Some(0)).unwrap_err();

        assert_eq!(error, SessionMessagesLoadError::SessionNotFound);
    }

    #[test]
    fn returns_decode_failed_when_messages_json_is_invalid() {
        let conn = Connection::open_in_memory().unwrap();
        init_agent_sessions_table(&conn);
        insert_session_messages(&conn, "session-1", "{not-json");

        let error =
            load_session_messages_from_db(&conn, "session-1", Some(10), Some(0)).unwrap_err();

        assert_eq!(error, SessionMessagesLoadError::DecodeFailed);
    }
}
