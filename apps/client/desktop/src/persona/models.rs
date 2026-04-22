//! Persona data models.

use serde::{Deserialize, Serialize};

/// A persona is a persistent AI agent with identity, personality, and task dispatch capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Persona {
    pub id: String,
    pub name: String,
    pub avatar_id: String,
    pub description: String,
    /// Personality traits (updated by the persona itself via `update_personality` tool).
    pub personality_notes: String,
    /// Default LLM model for this persona (e.g. "deepseek-chat").
    pub model: String,
    /// Optional LLM profile override.
    pub profile_id: Option<String>,
    #[serde(default = "default_persona_agent")]
    pub agent: Option<String>,
    /// Home directory for this persona (tasks inherit this as default workdir).
    pub workdir: String,
    /// The persistent chat session owned by this persona.
    pub chat_session_id: String,
    pub is_default: bool,
    /// When true, the persona's chat session automatically receives a vendor-aware
    /// continue message (e.g. cteno: "继续", others: "continue") after each turn,
    /// enabling continuous autonomous browsing.
    pub continuous_browsing: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// The type of relationship between a persona and a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonaSessionType {
    /// The persona's own persistent chat session.
    Chat,
    /// A task session dispatched by the persona.
    Task,
    /// A persistent role/member session attached to a group workspace.
    Member,
}

impl PersonaSessionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Task => "task",
            Self::Member => "member",
        }
    }

    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "chat" => Ok(Self::Chat),
            "task" => Ok(Self::Task),
            "member" => Ok(Self::Member),
            _ => Err(format!("Invalid session type: {}", s)),
        }
    }
}

/// A link between a persona and a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaSessionLink {
    pub persona_id: String,
    pub session_id: String,
    pub session_type: PersonaSessionType,
    /// Task description (only for Task sessions).
    pub task_description: Option<String>,
    /// Agent type for task sessions: None = general worker, Some("browser") = browser agent.
    pub agent_type: Option<String>,
    /// Owner kind: "persona" or "hypothesis". Defaults to "persona".
    #[serde(default = "default_owner_kind")]
    pub owner_kind: String,
    /// Orchestration label: identifies which flow node this session corresponds to.
    /// Set by `ctenoctl dispatch --label <label>` for orchestration visualization.
    pub label: Option<String>,
    pub created_at: String,
}

/// Persistent metadata for a multi-agent workspace rooted at a persona.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceBinding {
    pub persona_id: String,
    pub workspace_id: String,
    pub template_id: String,
    pub provider: String,
    pub default_role_id: Option<String>,
    pub model: String,
    pub workdir: String,
    pub created_at: String,
    pub updated_at: String,
}

fn default_owner_kind() -> String {
    "persona".to_string()
}

fn default_persona_agent() -> Option<String> {
    Some("cteno".to_string())
}

// Re-export Task Graph models from the shared engine module.
pub use crate::task_graph::{TaskGraphState, TaskNodeInput, TaskNodeState, TaskNodeStatus};
