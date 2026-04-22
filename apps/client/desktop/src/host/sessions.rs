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
    let sessions = manager.list_sessions(None)?;
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
    let session = manager.get_session(session_id)?;
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
    let summary_text = session
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant" && !message.content.trim().is_empty())
        .map(|message| message.content.clone())
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
        agent_state: None,
        agent_state_version: 0,
        thinking: false,
        thinking_at: 0,
        owner_session_id: session.owner_session_id.clone(),
        source: "local".to_string(),
    })
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
