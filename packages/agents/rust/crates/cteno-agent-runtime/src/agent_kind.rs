//! Agent kind — session-scoped type discriminator and tool-filtering policy.
//!
//! Migrated from `apps/client/desktop/src/agent_kind.rs` during Wave 3.3 split.
//! This module owns:
//!   * The `AgentKind` enum (session internal taxonomy).
//!   * Static tool-filter policies for Persona / Worker / Browser.
//!   * Parsing / label helpers.
//!
//! The resolution flow (`resolve_agent_kind_from_session_id`), prompt building,
//! and dynamic custom-agent filtering stay on the app side because they require
//! persona DB access, workspace FS scans, and the `service_init::AgentConfig`
//! registry.  Runtime callers that need to resolve a session's kind should go
//! through `hooks::AgentKindResolver`.

/// The kind of agent running in a session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AgentKind {
    /// Persona's own persistent chat session (orchestrator).
    Persona,
    /// General-purpose task worker dispatched by a persona.
    Worker,
    /// Browser-specialized worker (whitelist-only tools).
    Browser,
    /// Custom agent defined via AGENT.md.
    Custom(String),
}

/// Parse a string into an AgentKind (used by ctenoctl CLI).
pub fn parse_agent_kind(s: &str) -> Result<AgentKind, String> {
    match s.to_lowercase().as_str() {
        "persona" => Ok(AgentKind::Persona),
        "worker" => Ok(AgentKind::Worker),
        "browser" => Ok(AgentKind::Browser),
        custom => Ok(AgentKind::Custom(custom.to_string())),
    }
}

pub fn agent_kind_label(kind: &AgentKind) -> String {
    match kind {
        AgentKind::Persona => "persona".to_string(),
        AgentKind::Worker => "worker".to_string(),
        AgentKind::Browser => "browser".to_string(),
        AgentKind::Custom(custom) => custom.clone(),
    }
}

/// How to filter the tool set for an agent.
pub enum ToolFilter {
    /// Keep all tools except these IDs (static, for built-in kinds).
    ExcludeIds(&'static [&'static str]),
    /// Only allow these IDs (static, for built-in kinds).
    AllowIds(&'static [&'static str]),
    /// Dynamic exclude list (for custom agents).
    DynamicExclude(Vec<String>),
    /// Dynamic allow list (for custom agents).
    DynamicAllow(Vec<String>),
}

/// Static profile for an agent kind.
pub struct AgentProfile {
    pub tool_filter: ToolFilter,
}

// Tool ID constants for clarity and deduplication.

pub const BROWSER_ONLY: &[&str] = &[
    "browser_navigate",
    "browser_action",
    "browser_manage",
    "browser_adapter",
    "browser_network",
    "browser_cdp",
];

pub const BROWSER_WHITELIST: &[&str] = &[
    "browser_navigate",
    "browser_action",
    "browser_manage",
    "browser_adapter",
    "browser_network",
    "browser_cdp",
    "websearch",
    "read",
    "write",
    "edit",
    "shell",
    "grep",
    "glob",
    "memory",
    "update_plan",
];

pub const PERSONA_ONLY: &[&str] = &[
    "dispatch_task",
    "list_task_sessions",
    "send_to_session",
    "close_task_session",
    "update_personality",
    "ask_persona",
];

/// Concatenate multiple static slices into a single static slice at compile time.
macro_rules! concated_slice {
    ($($slice:expr),+ $(,)?) => {{
        const LEN: usize = $( $slice.len() + )* 0;
        const ARR: [&str; LEN] = {
            let mut arr = [""; LEN];
            let mut i = 0;
            $(
                {
                    let mut j = 0;
                    while j < $slice.len() {
                        arr[i] = $slice[j];
                        i += 1;
                        j += 1;
                    }
                }
            )*
            arr
        };
        &ARR
    }};
}
pub(crate) use concated_slice;

impl AgentKind {
    /// Static tool-filter profile.  Returns `None` for `Custom` kinds, whose
    /// filter depends on the `AgentConfig` registry that lives in the app
    /// crate.  Callers are expected to handle `Custom` via
    /// `build_custom_profile` on the app side.
    pub fn static_profile(&self) -> Option<AgentProfile> {
        match self {
            AgentKind::Persona => Some(AgentProfile {
                // Persona chat: exclude browser-only tools
                tool_filter: ToolFilter::ExcludeIds(BROWSER_ONLY),
            }),
            AgentKind::Worker => Some(AgentProfile {
                // Worker: exclude persona-only and browser-only
                tool_filter: ToolFilter::ExcludeIds(concated_slice!(PERSONA_ONLY, BROWSER_ONLY)),
            }),
            AgentKind::Browser => Some(AgentProfile {
                tool_filter: ToolFilter::AllowIds(BROWSER_WHITELIST),
            }),
            AgentKind::Custom(_) => None,
        }
    }

    /// Worker-equivalent fallback profile used when a custom agent can't be
    /// loaded from the registry.
    pub fn worker_fallback_profile() -> AgentProfile {
        AgentProfile {
            tool_filter: ToolFilter::ExcludeIds(concated_slice!(PERSONA_ONLY, BROWSER_ONLY)),
        }
    }
}

/// Apply a `ToolFilter` to the given tool list in place.
///
/// Logs a single info line summarising before/after tool counts.  The
/// `label` is used in log output to identify the agent / call site.
pub fn apply_filter(tools: &mut Vec<crate::llm::Tool>, filter: &ToolFilter, label: &str) {
    match filter {
        ToolFilter::AllowIds(allowed) => {
            let before = tools.len();
            tools.retain(|t| allowed.contains(&t.name.as_str()));
            log::info!(
                "Agent {}: whitelist filter {} -> {} tools",
                label,
                before,
                tools.len()
            );
        }
        ToolFilter::ExcludeIds(excluded) => {
            let before = tools.len();
            tools.retain(|t| !excluded.contains(&t.name.as_str()));
            if before != tools.len() {
                log::info!(
                    "Agent {}: exclude filter {} -> {} tools",
                    label,
                    before,
                    tools.len()
                );
            }
        }
        ToolFilter::DynamicAllow(allowed) => {
            let before = tools.len();
            tools.retain(|t| allowed.iter().any(|a| a == &t.name));
            log::info!(
                "Agent {}: dynamic whitelist filter {} -> {} tools",
                label,
                before,
                tools.len()
            );
        }
        ToolFilter::DynamicExclude(excluded) => {
            let before = tools.len();
            tools.retain(|t| !excluded.iter().any(|e| e == &t.name));
            if before != tools.len() {
                log::info!(
                    "Agent {}: dynamic exclude filter {} -> {} tools",
                    label,
                    before,
                    tools.len()
                );
            }
        }
    }
}
