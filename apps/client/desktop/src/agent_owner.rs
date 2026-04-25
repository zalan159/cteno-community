//! Unified Agent Owner Abstraction
//!
//! Provides a single `resolve_owner()` entry point for looking up
//! the entity that owns a session / dispatches workers.

use serde_json::Value;

fn notification_title_for_session(
    db_path: &std::path::Path,
    session: &crate::agent_session::AgentSession,
) -> Result<Option<String>, String> {
    if session.owner_session_id.is_some() {
        return Ok(None);
    }

    let persona_store = crate::persona::PersonaStore::new(db_path.to_path_buf());
    if let Some(link) = persona_store.get_persona_for_session(&session.id)? {
        match link.session_type {
            crate::persona::models::PersonaSessionType::Chat => {
                let persona_name = persona_store
                    .get_persona(&link.persona_id)?
                    .map(|persona| persona.name)
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| "Agent".to_string());
                Ok(Some(format!("{} replied", persona_name)))
            }
            crate::persona::models::PersonaSessionType::Task
            | crate::persona::models::PersonaSessionType::Member => Ok(None),
        }
    } else {
        let label = session.agent_id.trim();
        if label.is_empty() {
            Ok(Some("Agent reply ready".to_string()))
        } else {
            Ok(Some(format!("{} replied", label)))
        }
    }
}

/// The kind of entity that owns a session / dispatches workers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerKind {
    Persona,
}

impl OwnerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Persona => "persona",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            _ => Self::Persona,
        }
    }
}

/// All context gathered when resolving an owner by ID.
pub struct AgentOwnerInfo {
    pub owner_id: String,
    pub owner_kind: OwnerKind,
    /// The owner's main chat/agent session.
    pub chat_session_id: String,
    pub agent_flavor: String,
    pub workdir: String,
    pub profile_id: Option<String>,
    pub name: String,
}

/// Resolve the owner of a session / task by ID.
///
/// Lookup order:
/// 1. `personas` table (→ OwnerKind::Persona)
pub fn resolve_owner(owner_id: &str) -> Result<AgentOwnerInfo, String> {
    // 1. Check personas table
    if let Ok(pm) = crate::local_services::persona_manager() {
        if let Ok(Some(persona)) = pm.store().get_persona(owner_id) {
            return Ok(AgentOwnerInfo {
                owner_id: persona.id.clone(),
                owner_kind: OwnerKind::Persona,
                chat_session_id: persona.chat_session_id.clone(),
                agent_flavor: persona
                    .agent
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("cteno")
                    .to_string(),
                workdir: persona.workdir.clone(),
                profile_id: persona.profile_id.clone(),
                name: persona.name.clone(),
            });
        }
    }

    Err(format!("Owner '{}' not found (checked personas)", owner_id))
}

/// Resolve the effective profile ID with a clear priority chain:
/// explicit parameter > owner's profile > global fallback.
pub fn resolve_profile_id(
    owner: &AgentOwnerInfo,
    explicit: Option<&str>,
    fallback: &str,
) -> String {
    explicit
        .map(|s| s.to_string())
        .or_else(|| owner.profile_id.clone())
        .unwrap_or_else(|| fallback.to_string())
}

/// Extract the owner ID from tool input, supporting both new and legacy keys.
/// Prefers `__owner_id`, falls back to `__persona_id` for backward compat.
pub fn extract_owner_id(input: &Value) -> Option<&str> {
    input
        .get("__owner_id")
        .or_else(|| input.get("__persona_id"))
        .and_then(|v| v.as_str())
}

/// Resolve an owner's name from an owner ID (for display labels).
pub fn resolve_owner_name(owner_id: &str) -> Option<String> {
    resolve_owner(owner_id).ok().map(|info| info.name)
}

/// Unified notification routing when a worker session completes.
///
/// Looks up the owner kind and routes to the appropriate manager.
pub fn notify_session_complete(session_id: &str) {
    // Route 1: Check persona_sessions table — if found, notify PersonaManager
    if let Ok(pm) = crate::local_services::persona_manager() {
        if let Ok(Some(link)) = pm.store().get_persona_for_session(session_id) {
            if link.session_type == crate::persona::models::PersonaSessionType::Task {
                pm.notify_task_result(session_id);
            }
        }
    }

    // Route 2: CLI waiter — if ctenoctl is blocking on this session, send the final text
    {
        let sid = session_id.to_string();
        tokio::spawn(async move {
            // Extract final output for CLI
            let db_path = crate::local_services::spawn_config()
                .ok()
                .map(|c| c.db_path.clone());
            let (final_text, notification_title) = if let Some(db) = db_path.as_ref() {
                let mgr = crate::agent_session::AgentSessionManager::new(db.to_path_buf());
                let final_text = mgr.extract_final_output(&sid);
                let notification_title = mgr.get_session(&sid).ok().flatten().and_then(|session| {
                    notification_title_for_session(db, &session).ok().flatten()
                });
                (final_text, notification_title)
            } else {
                (String::new(), None)
            };
            if cteno_host_bridge_localrpc::try_complete_cli_session(&sid, &final_text).await {
                log::info!("[CLI] Notified CLI waiter for session {}", sid);
            }

            if !final_text.is_empty()
                && cteno_community_core::attention_state::should_send_completion_notification(&sid)
                    .await
                && notification_title.is_some()
            {
                crate::push_notification::send_agent_reply_notification(
                    notification_title.as_deref().unwrap_or("Agent reply ready"),
                    &final_text,
                );
                log::info!(
                    "[Notification] Sent completion notification for session {}",
                    sid
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::notification_title_for_session;

    fn session(
        owner_session_id: Option<&str>,
        agent_id: &str,
    ) -> crate::agent_session::AgentSession {
        crate::agent_session::AgentSession {
            id: "session-1".to_string(),
            agent_id: agent_id.to_string(),
            user_id: None,
            messages: vec![],
            context_data: None,
            agent_state: None,
            agent_state_version: 0,
            status: crate::agent_session::SessionStatus::Active,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            expires_at: None,
            owner_session_id: owner_session_id.map(|value| value.to_string()),
            vendor: "cteno".to_string(),
        }
    }

    #[test]
    fn suppresses_notifications_for_owned_child_sessions() {
        let db_path =
            std::env::temp_dir().join(format!("cteno-notify-{}.db", uuid::Uuid::new_v4()));
        let current = session(Some("parent-1"), "worker");

        assert_eq!(
            notification_title_for_session(&db_path, &current).unwrap(),
            None
        );
    }

    #[test]
    fn falls_back_to_agent_id_for_standalone_sessions() {
        let db_path =
            std::env::temp_dir().join(format!("cteno-notify-{}.db", uuid::Uuid::new_v4()));
        let current = session(None, "local-agent");

        assert_eq!(
            notification_title_for_session(&db_path, &current)
                .unwrap()
                .as_deref(),
            Some("local-agent replied")
        );
    }
}
