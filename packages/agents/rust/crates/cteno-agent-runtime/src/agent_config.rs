//! Agent / Skill configuration types.
//!
//! Pure data types shared across the runtime (ReAct loop, sub-agent executor,
//! autonomous agent) and host (service_init loaders, RPC handlers).  They live
//! in the runtime crate because autonomous_agent and agent::executor depend on
//! them, but the loader functions (`load_all_agents`, `load_all_skills`) still
//! live in the host because they reach for FS layout decided by the app.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Skill support types
// ---------------------------------------------------------------------------

/// Compatible with Claude Code's `string | string[]` fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrVec {
    Single(String),
    Multiple(Vec<String>),
}

impl StringOrVec {
    pub fn as_vec(&self) -> Vec<String> {
        match self {
            StringOrVec::Single(s) => vec![s.clone()],
            StringOrVec::Multiple(v) => v.clone(),
        }
    }
}

/// Skill execution context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SkillContext {
    Inline,
    Fork,
}

/// Skill parameter definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillParam {
    #[serde(rename = "type")]
    pub param_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub examples: Option<serde_json::Value>,
    #[serde(default)]
    pub options: Option<Vec<String>>,
    #[serde(default)]
    pub validation: Option<serde_json::Value>,
}

/// Skill configuration (from SKILL.md or config.json).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(rename = "match", default)]
    pub url_match: Option<String>,
    #[serde(default)]
    pub entry_url: Option<String>,
    #[serde(default)]
    pub params: std::collections::HashMap<String, SkillParam>,
    #[serde(default)]
    pub responses: Option<serde_json::Value>,
    #[serde(default)]
    pub errors: Option<serde_json::Value>,
    #[serde(default)]
    pub safety: Option<serde_json::Value>,
    #[serde(default, alias = "when-to-use")]
    pub when_to_use: Option<String>,
    #[serde(default, alias = "argument-hint")]
    pub argument_hint: Option<String>,
    #[serde(default, alias = "allowed-tools")]
    pub allowed_tools: Option<StringOrVec>,
    #[serde(default = "default_true", alias = "user-invocable")]
    pub user_invocable: bool,
    #[serde(default, alias = "disable-model-invocation")]
    pub disable_model_invocation: bool,
    #[serde(default)]
    pub context: Option<SkillContext>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(skip)]
    pub is_bundled: bool,
    #[serde(skip)]
    pub path: Option<PathBuf>,
    #[serde(skip)]
    pub source: Option<String>,
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Agent support types
// ---------------------------------------------------------------------------

/// Agent type — determines how the agent processes requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentType {
    /// Direct passthrough — user requests go directly to the agent.
    #[default]
    Passthrough,
    /// Autonomous agent with its own ReAct loop.
    Autonomous,
    /// Can act as both passthrough and be called by other agents.
    Hybrid,
}

/// Session configuration for an agent.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentSessionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_session_timeout")]
    pub timeout_minutes: u32,
    #[serde(default)]
    pub key_by: Option<String>,
}

fn default_session_timeout() -> u32 {
    30
}

/// Routing configuration for an agent.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentRoutingConfig {
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub patterns: Vec<String>,
}

/// Agent configuration (from AGENT.md or config.json).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentConfig {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    #[serde(rename = "type", default)]
    pub agent_type: AgentType,
    #[serde(default)]
    pub session: AgentSessionConfig,
    #[serde(default)]
    pub routing: AgentRoutingConfig,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub params: std::collections::HashMap<String, SkillParam>,
    #[serde(default)]
    pub expose_as_tool: Option<bool>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub skills: Option<Vec<String>>,
    #[serde(default)]
    pub source: Option<String>,
    /// Allowed tool IDs (whitelist). If set, only these tools are available.
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Excluded tool IDs (blacklist). Applied on top of the base kind's filter.
    #[serde(default)]
    pub excluded_tools: Option<Vec<String>>,
}

impl AgentConfig {
    /// Convert this agent to an LLM Tool definition (for a parent agent to
    /// call as a sub-agent).  Called by
    /// `autonomous_agent::build_agent_tools`.
    pub fn to_tool(&self) -> crate::llm::Tool {
        crate::llm::Tool {
            name: format!("agent_{}", self.id),
            description: format!(
                "Call the '{}' agent to handle a specialized task.\n\n{}\n\n\
                This agent has its own reasoning loop and tools. \
                Send it a clear task description and it will work autonomously to produce a result. \
                The call is synchronous — you will receive the agent's final response as the tool result.",
                self.name, self.description
            ),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Clear task description for the agent"
                    },
                    "context": {
                        "type": "object",
                        "description": "Optional context (e.g., workdir, file paths, parameters)"
                    }
                },
                "required": ["prompt"]
            }),
        }
    }
}
