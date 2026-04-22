use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::session_relay::{
    load_session_messages_from_db, resolve_db_path, LocalSessionMessagesPage,
    SessionMessagesLoadError,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[tauri::command]
pub async fn get_sessions() -> Result<Vec<Session>, String> {
    Ok(vec![])
}

#[tauri::command]
pub async fn create_session(user_id: String) -> Result<Session, String> {
    let session = Session {
        id: uuid::Uuid::new_v4().to_string(),
        user_id,
        status: "active".to_string(),
        created_at: chrono::Utc::now().timestamp(),
        updated_at: chrono::Utc::now().timestamp(),
    };

    Ok(session)
}

#[tauri::command]
pub async fn get_session_messages(
    app: AppHandle,
    session_id: String,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<LocalSessionMessagesPage, String> {
    let db_path = resolve_db_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let conn = rusqlite::Connection::open(db_path).map_err(|e| format!("open db: {e}"))?;
        load_session_messages_from_db(&conn, &session_id, limit, offset).map_err(
            |error| match error {
                SessionMessagesLoadError::SessionNotFound => "session not found".to_string(),
                SessionMessagesLoadError::DecodeFailed => {
                    "parse session messages json failed".to_string()
                }
                SessionMessagesLoadError::QueryFailed(message) => message,
            },
        )
    })
    .await
    .map_err(|e| format!("get_session_messages join: {e}"))?
}
