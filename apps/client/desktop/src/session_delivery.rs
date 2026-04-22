//! Session Delivery Helpers
//!
//! Shared logic for delivering messages to sessions with automatic
//! executor-session resume. Replaces duplicated resume+deliver patterns across
//! persona/manager.rs and hypothesis/manager.rs.

use crate::happy_client::manager::SpawnSessionConfig;

/// Ensure a session connection is active, recreating it from persisted session
/// state if needed.
///
/// Returns `true` if the session is now in active connections, `false` otherwise.
pub async fn ensure_session_connected(
    config: &SpawnSessionConfig,
    session_id: &str,
    label: &str,
) -> bool {
    // 1. Already active?
    if let Some(existing) = config.session_connections.get(session_id).await {
        if !existing.is_dead() {
            return true;
        }

        let _ = config.session_connections.remove(session_id).await;
        existing.disconnect().await;
    }

    log::info!(
        "[{}] Session {} is inactive, resuming executor-backed connection...",
        label,
        session_id
    );
    if let Err(e) =
        crate::happy_client::manager::resume_session_connection(config, session_id, None).await
    {
        log::error!("[{}] Failed to resume session {}: {}", label, session_id, e);
        return false;
    }

    config.session_connections.contains_key(session_id).await
}

/// Deliver a message to a session, automatically resuming its local connection if needed.
///
/// This is the unified replacement for the 5 duplicated reconnect+deliver patterns.
pub async fn deliver_message_to_session(
    config: &SpawnSessionConfig,
    session_id: &str,
    message: &str,
    label: &str,
) -> Result<(), String> {
    // 1. Try active connections first
    if let Some(conn) = config.session_connections.get(session_id).await {
        if !conn.is_dead() {
            return conn
                .message_handle()
                .send_initial_user_message(message)
                .await
                .map_err(|e| {
                    format!(
                        "[{}] Failed to send to session {}: {}",
                        label, session_id, e
                    )
                });
        }

        let _ = config.session_connections.remove(session_id).await;
        conn.disconnect().await;
    }

    log::info!(
        "[{}] Session {} is inactive, resuming before delivery...",
        label,
        session_id
    );
    crate::happy_client::manager::resume_session_connection(config, session_id, None)
        .await
        .map_err(|e| format!("[{}] Failed to resume session {}: {}", label, session_id, e))?;

    if let Some(handle) = config
        .session_connections
        .get(session_id)
        .await
        .map(|conn| conn.message_handle())
    {
        return handle
            .send_initial_user_message(message)
            .await
            .map_err(|e| {
                format!(
                    "[{}] Failed to send after resume to session {}: {}",
                    label, session_id, e
                )
            });
    }

    Err(format!(
        "[{}] Session {} resumed but not found in active connections",
        label, session_id
    ))
}
