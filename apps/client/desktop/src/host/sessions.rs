use crate::agent_session::{AgentSession, AgentSessionManager, SessionStatus};
use crate::persona::{PersonaSessionLink, PersonaSessionType, PersonaStore};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSessionSummary {
    pub id: String,
    pub seq: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub active: bool,
    pub active_at: i64,
    pub metadata: Value,
    pub metadata_version: u64,
    pub agent_state: Option<Value>,
    pub agent_state_version: u64,
    pub thinking: bool,
    pub thinking_at: i64,
    pub owner_session_id: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSessionMessage {
    pub id: String,
    pub local_id: Option<String>,
    pub created_at: i64,
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSessionMessagesPage {
    pub messages: Vec<HostSessionMessage>,
    pub has_more: bool,
}

pub fn list_host_sessions(
    db_path: &Path,
    machine_id: &str,
) -> Result<Vec<HostSessionSummary>, String> {
    let manager = AgentSessionManager::new(db_path.to_path_buf());
    let persona_store = PersonaStore::new(db_path.to_path_buf());
    let mut sessions = manager.list_sessions(None)?;
    settle_stale_pending_permissions(&manager, &mut sessions)?;
    sessions
        .iter()
        .map(|session| build_host_session_summary(session, machine_id, &persona_store))
        .collect()
}

pub fn get_host_session(
    db_path: &Path,
    machine_id: &str,
    session_id: &str,
) -> Result<Option<HostSessionSummary>, String> {
    let manager = AgentSessionManager::new(db_path.to_path_buf());
    let persona_store = PersonaStore::new(db_path.to_path_buf());
    let mut session = manager.get_session(session_id)?;
    if let Some(session) = session.as_mut() {
        settle_stale_pending_permissions(&manager, std::slice::from_mut(session))?;
    }
    session
        .as_ref()
        .map(|item| build_host_session_summary(item, machine_id, &persona_store))
        .transpose()
}

pub fn get_host_session_messages(
    db_path: &Path,
    session_id: &str,
) -> Result<Option<HostSessionMessagesPage>, String> {
    let manager = AgentSessionManager::new(db_path.to_path_buf());
    let session = match manager.get_session(session_id)? {
        Some(session) => session,
        None => return Ok(None),
    };

    let messages = session
        .messages
        .iter()
        .enumerate()
        .map(|(idx, message)| HostSessionMessage {
            id: format!("{}:{}", session.id, idx),
            local_id: message.local_id.clone(),
            created_at: parse_timestamp_ms(&message.timestamp),
            role: message.role.clone(),
            text: message.content.clone(),
        })
        .collect::<Vec<_>>();

    Ok(Some(HostSessionMessagesPage {
        messages,
        has_more: false,
    }))
}

fn build_host_session_summary(
    session: &AgentSession,
    machine_id: &str,
    persona_store: &PersonaStore,
) -> Result<HostSessionSummary, String> {
    let link = persona_store.get_persona_for_session(&session.id)?;
    let created_at = parse_timestamp_ms(&session.created_at);
    let updated_at = parse_timestamp_ms(&session.updated_at);
    // Subagent sessions store their messages as raw ACP envelopes
    // (`{role:assistant, content:{type:acp, data:...}}` stringified). The
    // legacy code used `message.content` verbatim as the title, which on
    // those rows surfaces the JSON envelope text (e.g.
    // `{"content":{"data":{"id":"…"}}}` was visible in BaseSessionPage's
    // header). Use the runtime's ACP-aware extractor so we pull the
    // human-readable assistant text instead.
    let summary_text = cteno_agent_runtime::agent_session::extract_last_assistant_text(
        &session.messages,
    )
    .or_else(|| link.as_ref().and_then(|item| item.task_description.clone()))
    .unwrap_or_default();

    let workdir = session
        .context_data
        .as_ref()
        .and_then(|ctx| ctx.get("workdir"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .or_else(|| {
            link.as_ref().and_then(|item| {
                persona_store
                    .get_persona(&item.persona_id)
                    .ok()
                    .flatten()
                    .map(|persona| persona.workdir)
            })
        })
        .unwrap_or_else(|| "~".to_string());

    let profile_id = session
        .context_data
        .as_ref()
        .and_then(|ctx| ctx.get("profile_id"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());

    let metadata = build_metadata(
        session,
        machine_id,
        &summary_text,
        updated_at,
        link.as_ref(),
        &workdir,
        profile_id,
    );

    Ok(HostSessionSummary {
        id: session.id.clone(),
        seq: session.messages.len() as u64,
        created_at,
        updated_at,
        active: session.status == SessionStatus::Active,
        active_at: updated_at,
        metadata,
        metadata_version: 0,
        agent_state: session.agent_state.clone(),
        agent_state_version: session.agent_state_version,
        thinking: false,
        thinking_at: 0,
        owner_session_id: session.owner_session_id.clone(),
        source: "local".to_string(),
    })
}

fn settle_stale_pending_permissions(
    manager: &AgentSessionManager,
    sessions: &mut [AgentSession],
) -> Result<(), String> {
    for session in sessions {
        if crate::happy_client::permission::has_live_pending_requests(&session.id) {
            continue;
        }
        let Some(state) = session.agent_state.as_mut() else {
            continue;
        };
        if !deny_pending_permission_requests(state, chrono::Utc::now().timestamp_millis()) {
            continue;
        }
        session.agent_state_version = session.agent_state_version.saturating_add(1);
        manager.update_agent_state(
            &session.id,
            session.agent_state.as_ref(),
            session.agent_state_version,
        )?;
        log::info!(
            "[HostSessions] settled stale pending permissions as denied for session {}",
            session.id
        );
    }
    Ok(())
}

fn deny_pending_permission_requests(state: &mut Value, completed_at: i64) -> bool {
    let Some(state_obj) = state.as_object_mut() else {
        return false;
    };

    let pending = state_obj
        .get_mut("requests")
        .and_then(Value::as_object_mut)
        .map(std::mem::take)
        .unwrap_or_default();
    if pending.is_empty() {
        return false;
    }

    let completed = state_obj
        .entry("completedRequests")
        .or_insert_with(|| json!({}));
    if !completed.is_object() {
        *completed = json!({});
    }
    let Some(completed_obj) = completed.as_object_mut() else {
        return false;
    };

    for (request_id, request) in pending {
        let mut entry = json!({
            "status": "denied",
            "decision": "denied",
            "completedAt": completed_at,
            "reason": "Permission denied because the session process restarted before a response was received.",
        });
        if let Some(tool) = request.get("tool") {
            entry["tool"] = tool.clone();
        }
        if let Some(arguments) = request.get("arguments") {
            entry["arguments"] = arguments.clone();
        }
        if let Some(created_at) = request.get("createdAt") {
            entry["createdAt"] = created_at.clone();
        }
        completed_obj.insert(request_id, entry);
    }

    true
}

fn build_metadata(
    session: &AgentSession,
    machine_id: &str,
    summary_text: &str,
    summary_updated_at: i64,
    link: Option<&PersonaSessionLink>,
    workdir: &str,
    profile_id: Option<String>,
) -> Value {
    let flavor = link
        .map(|item| match item.session_type {
            PersonaSessionType::Chat => "persona",
            PersonaSessionType::Task => "task",
            PersonaSessionType::Member => "workspace-member",
        })
        .unwrap_or("local-agent-session");

    let name = if let Some(item) = link {
        if let Some(label) = &item.label {
            format!("@{}", label)
        } else if item.session_type == PersonaSessionType::Chat {
            "Coordinator".to_string()
        } else if let Some(agent_type) = &item.agent_type {
            agent_type.clone()
        } else {
            session.agent_id.clone()
        }
    } else {
        session.agent_id.clone()
    };

    let mut metadata = json!({
        "path": workdir,
        "host": "local-shell",
        "name": name,
        "machineId": machine_id,
        "flavor": flavor,
        "vendor": session.vendor,
        "summary": {
            "text": summary_text,
            "updatedAt": summary_updated_at,
        },
    });

    if let Some(profile_id) = profile_id {
        metadata["modelId"] = Value::String(profile_id);
    }

    metadata
}

fn parse_timestamp_ms(raw: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|value| value.timestamp_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::deny_pending_permission_requests;
    use serde_json::json;

    #[test]
    fn restart_settlement_moves_pending_permissions_to_denied() {
        let mut state = json!({
            "controlledByUser": false,
            "requests": {
                "perm-1": {
                    "tool": "Bash",
                    "arguments": { "command": "git status" },
                    "createdAt": 42
                }
            },
            "completedRequests": {
                "old": { "status": "approved" }
            }
        });

        assert!(deny_pending_permission_requests(&mut state, 1000));
        assert_eq!(state["requests"], json!({}));
        assert_eq!(state["completedRequests"]["perm-1"]["status"], "denied");
        assert_eq!(state["completedRequests"]["perm-1"]["decision"], "denied");
        assert_eq!(state["completedRequests"]["perm-1"]["tool"], "Bash");
        assert_eq!(
            state["completedRequests"]["perm-1"]["arguments"],
            json!({ "command": "git status" })
        );
        assert_eq!(state["completedRequests"]["perm-1"]["createdAt"], 42);
        assert_eq!(state["completedRequests"]["old"]["status"], "approved");
    }

    #[test]
    fn restart_settlement_ignores_states_without_pending_permissions() {
        let mut state = json!({
            "controlledByUser": false,
            "requests": {},
            "completedRequests": {}
        });

        assert!(!deny_pending_permission_requests(&mut state, 1000));
        assert_eq!(state["requests"], json!({}));
        assert_eq!(state["completedRequests"], json!({}));
    }
}
