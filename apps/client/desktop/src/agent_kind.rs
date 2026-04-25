//! Agent Kind — app-side resolver + re-export façade for the runtime split.
//!
//! Wave 3.3a split the agent-kind module into two halves:
//!
//! * Runtime (`cteno_agent_runtime::agent_kind`) owns the pure types:
//!   `AgentKind`, tool-filter policy, static filter constants,
//!   `parse_agent_kind`, `agent_kind_label`, `apply_filter`.
//! * App side (this file) owns everything that touches persona state or the
//!   `service_init::AgentConfig` registry: `AgentKindResolution` (with
//!   concrete `Persona` / `PersonaSessionLink` fields), `resolve_agent_kind`,
//!   `build_custom_profile`, `build_agent_prompt` + variants, and the combined
//!   `apply_tool_filter` that dispatches between static and custom profiles.
//!
//! Existing callers continue to import everything through `crate::agent_kind::*`
//! — the re-export keeps the surface unchanged.

// Re-export runtime-native pieces so callers keep using `crate::agent_kind::*`.
pub use cteno_agent_runtime::agent_kind::{
    agent_kind_label, apply_filter, parse_agent_kind, AgentKind, AgentProfile, ToolFilter,
    BROWSER_ONLY, BROWSER_WHITELIST, PERSONA_ONLY,
};

use crate::persona::models::{Persona, PersonaSessionLink, PersonaSessionType};

/// Build a dynamic tool-filtering profile for a custom agent.
fn build_custom_profile(agent_id: &str, workdir: Option<&str>) -> AgentProfile {
    if let Some(config) = load_agent_config(agent_id, workdir) {
        if let Some(ref tools) = config.tools {
            return AgentProfile {
                tool_filter: ToolFilter::DynamicAllow(tools.clone()),
            };
        }
        if let Some(ref allowed) = config.allowed_tools {
            return AgentProfile {
                tool_filter: ToolFilter::DynamicAllow(allowed.clone()),
            };
        }
        if let Some(ref excluded) = config.excluded_tools {
            return AgentProfile {
                tool_filter: ToolFilter::DynamicExclude(excluded.clone()),
            };
        }
    }
    AgentKind::worker_fallback_profile()
}

/// Load an agent config by ID from builtin + global + workspace directories.
fn load_agent_config(
    agent_id: &str,
    workdir: Option<&str>,
) -> Option<crate::service_init::AgentConfig> {
    let runtime_ctx = crate::local_services::agent_runtime_context().ok()?;
    let workspace_agents_dir = workdir.map(|wd| {
        let expanded = shellexpand::tilde(wd).to_string();
        std::path::PathBuf::from(expanded)
            .join(".cteno")
            .join("agents")
    });
    let all_agents = crate::service_init::load_all_agents(
        &runtime_ctx.builtin_agents_dir,
        &runtime_ctx.user_agents_dir,
        workspace_agents_dir.as_deref(),
    );
    all_agents.into_iter().find(|a| a.id == agent_id)
}

// ---------------------------------------------------------------------------
// Resolution — detect the agent kind from session ID
// ---------------------------------------------------------------------------

/// All context gathered during agent kind resolution.
/// Cached data avoids redundant DB lookups downstream.
pub struct AgentKindResolution {
    pub kind: AgentKind,
    /// Persona link (if session is linked to a persona).
    pub persona_link: Option<PersonaSessionLink>,
    /// The persona itself (for Chat sessions, or the owner persona for Task sessions).
    pub persona: Option<Persona>,
}

impl AgentKindResolution {
    /// Get the effective owner ID (for session context injection).
    pub fn owner_id(&self) -> Option<&str> {
        if let Some(ref link) = self.persona_link {
            return Some(&link.persona_id);
        }
        None
    }

    /// Get the effective workdir.
    pub fn workdir(&self) -> Option<&str> {
        if let Some(ref p) = self.persona {
            return Some(&p.workdir);
        }
        None
    }

    /// Get the default profile ID for this agent kind.
    /// Browser agents always use BROWSER_AGENT_PROFILE.
    /// Other kinds use the owner's profile_id or fall back to the provided default.
    pub fn default_profile_id(&self, fallback: &str) -> String {
        match self.kind {
            AgentKind::Browser => crate::llm_profile::BROWSER_AGENT_PROFILE.to_string(),
            _ => self
                .persona
                .as_ref()
                .and_then(|p| p.profile_id.clone())
                .unwrap_or_else(|| fallback.to_string()),
        }
    }
}

/// Resolve the agent kind for a given session ID.
///
/// Detection order:
/// 1. CLI kind override (ctenoctl --kind)
/// 2. `persona_sessions` table → Chat/Task/Browser/Custom
/// 3. Fallback → Worker (with warning log)
pub fn resolve_agent_kind(session_id: &str) -> AgentKindResolution {
    // Step 0: Check CLI/local override (ctenoctl --kind, local workspace execution)
    // Keep looking up the owning persona/session link so custom agents still
    // inherit workdir and workspace-scoped AGENT.md discovery.
    let kind_override = cteno_host_bridge_localrpc::get_session_kind_label(session_id)
        .and_then(|label| parse_agent_kind(&label).ok());
    if let Some(kind) = &kind_override {
        log::info!(
            "resolve_agent_kind: session {} has CLI kind override -> {:?}",
            session_id,
            kind
        );
    }

    // Step 1: Check persona sessions
    if let Ok(mgr) = crate::local_services::persona_manager() {
        if let Ok(Some(link)) = mgr.store().get_persona_for_session(session_id) {
            match link.session_type {
                PersonaSessionType::Chat => {
                    // Chat session → Persona kind
                    let persona = mgr.store().get_persona(&link.persona_id).ok().flatten();
                    return AgentKindResolution {
                        kind: kind_override.unwrap_or(AgentKind::Persona),
                        persona_link: Some(link),
                        persona,
                    };
                }
                PersonaSessionType::Task | PersonaSessionType::Member => {
                    // Task/member session — check agent_type for specialization
                    let persona = mgr.store().get_persona(&link.persona_id).ok().flatten();
                    let kind = kind_override.unwrap_or_else(|| match link.agent_type.as_deref() {
                        Some("browser") => AgentKind::Browser,
                        Some("worker") | None => AgentKind::Worker,
                        Some(custom_id) => AgentKind::Custom(custom_id.to_string()),
                    });
                    return AgentKindResolution {
                        kind,
                        persona_link: Some(link),
                        persona,
                    };
                }
            }
        }
    }

    // Step 2: Fallback to Worker (no owner found)
    log::warn!(
        "resolve_agent_kind: session {} has no persona owner, falling back to Worker",
        session_id
    );
    AgentKindResolution {
        kind: kind_override.unwrap_or(AgentKind::Worker),
        persona_link: None,
        persona: None,
    }
}

// ---------------------------------------------------------------------------
// Tool filtering
// ---------------------------------------------------------------------------

/// Apply the agent kind's tool filter to the tools list.
///
/// Dispatches to the runtime's static `apply_filter` for built-in kinds, and
/// builds a dynamic profile from the workspace AGENT.md registry for
/// `AgentKind::Custom`.
pub fn apply_tool_filter(tools: &mut Vec<crate::llm::Tool>, resolution: &AgentKindResolution) {
    let workdir = resolution.persona.as_ref().map(|p| p.workdir.as_str());
    let profile = match &resolution.kind {
        AgentKind::Custom(agent_id) => build_custom_profile(agent_id, workdir),
        kind => match kind.static_profile() {
            Some(p) => p,
            None => AgentKind::worker_fallback_profile(),
        },
    };
    let label = format!("{:?}", resolution.kind);
    apply_filter(tools, &profile.tool_filter, &label);
}

// ---------------------------------------------------------------------------
// Prompt building
// ---------------------------------------------------------------------------

/// Build the effective system prompt for the resolved agent kind.
///
/// Returns `(effective_system_prompt, detected_persona_id, detected_persona_workdir)`.
pub fn build_agent_prompt(
    resolution: &AgentKindResolution,
    base_prompt: &str,
) -> (String, Option<String>, Option<String>) {
    match resolution.kind {
        AgentKind::Persona => build_persona_prompt(resolution, base_prompt),
        AgentKind::Worker | AgentKind::Browser => build_task_prompt(resolution, base_prompt),
        AgentKind::Custom(ref agent_id) => {
            build_custom_agent_prompt(resolution, base_prompt, agent_id)
        }
    }
}

fn build_persona_prompt(
    resolution: &AgentKindResolution,
    base_prompt: &str,
) -> (String, Option<String>, Option<String>) {
    let link = match &resolution.persona_link {
        Some(l) => l,
        None => return (base_prompt.to_string(), None, None),
    };
    let persona = match &resolution.persona {
        Some(p) => p,
        None => return (base_prompt.to_string(), None, None),
    };

    let persona_workdir = persona.workdir.clone();
    let mgr = match crate::local_services::persona_manager() {
        Ok(m) => m,
        Err(_) => return (base_prompt.to_string(), None, None),
    };
    let active_tasks = mgr.list_active_tasks(&link.persona_id).unwrap_or_default();

    // Read persona's private MEMORY.md
    let persona_memory = {
        let expanded = shellexpand::tilde(&persona_workdir).to_string();
        let mem_path = std::path::PathBuf::from(&expanded)
            .join(".cteno")
            .join("MEMORY.md");
        std::fs::read_to_string(&mem_path).ok()
    };

    let persona_prompt = crate::persona::prompt::build_persona_system_prompt(
        persona,
        &active_tasks,
        persona_memory.as_deref(),
    );
    log::info!(
        "Persona chat session: '{}' ({}), persona prompt {} chars",
        persona.name,
        link.persona_id,
        persona_prompt.len()
    );

    (
        format!("{}\n\n{}", base_prompt, persona_prompt),
        Some(link.persona_id.clone()),
        Some(persona_workdir),
    )
}

fn build_task_prompt(
    resolution: &AgentKindResolution,
    base_prompt: &str,
) -> (String, Option<String>, Option<String>) {
    let link = match &resolution.persona_link {
        Some(l) => l,
        None => return (base_prompt.to_string(), None, None),
    };
    let persona = match &resolution.persona {
        Some(p) => p,
        None => {
            log::info!(
                "Task session for persona {} but persona not found, using default prompt",
                link.persona_id
            );
            return (base_prompt.to_string(), Some(link.persona_id.clone()), None);
        }
    };

    let persona_workdir = persona.workdir.clone();
    let expanded = shellexpand::tilde(&persona_workdir).to_string();

    // Read MEMORY.md for task session context
    let persona_memory = {
        let project_mem_path = std::path::PathBuf::from(&expanded)
            .join(".cteno")
            .join("memory")
            .join("MEMORY.md");
        std::fs::read_to_string(&project_mem_path).ok().or_else(|| {
            let legacy_mem_path = std::path::PathBuf::from(&expanded)
                .join(".cteno")
                .join("MEMORY.md");
            std::fs::read_to_string(&legacy_mem_path).ok()
        })
    };

    // Build task session prompt with memory scoping guidance
    let mut task_prompt = base_prompt.to_string();
    task_prompt.push_str(&format!(
        "\n\n## Memory Scope\n\n\
        你在 Persona \"{}\" 的工作空间下工作。\n\
        - memory tool 的 scope=\"private\" 操作该 Persona 的项目私有记忆（{}/.cteno/memory/）\n\
        - scope=\"global\" 操作全局共享记忆\n\
        - 只保存可复用的经验和模式，不要保存一次性的任务分析或临时日志",
        persona.name, expanded
    ));
    if let Some(ref mem) = persona_memory {
        task_prompt.push_str(&format!("\n\n## Persona 记忆（只读参考）\n\n{}", mem));
    }

    // Inject browser agent specialized prompt if this is a browser agent session
    if resolution.kind == AgentKind::Browser {
        let browser_prompt = crate::persona::browser_prompt::build_browser_agent_prompt(
            link.task_description.as_deref().unwrap_or(""),
            &persona.name,
        );
        task_prompt.push_str("\n\n");
        task_prompt.push_str(&browser_prompt);
        log::info!(
            "Browser agent session for persona {} ({}), injected browser prompt ({} chars)",
            persona.name,
            link.persona_id,
            browser_prompt.len()
        );
    }

    log::info!(
        "Task session for persona {} ({}), workdir: {}",
        persona.name,
        link.persona_id,
        persona_workdir
    );

    (
        task_prompt,
        Some(link.persona_id.clone()),
        Some(persona_workdir),
    )
}

/// Build prompt for a custom agent defined via AGENT.md.
/// Starts with the Worker task prompt (memory scoping, persona context),
/// then appends the agent's custom instructions.
fn build_custom_agent_prompt(
    resolution: &AgentKindResolution,
    base_prompt: &str,
    agent_id: &str,
) -> (String, Option<String>, Option<String>) {
    // Start with the standard task prompt (memory scoping etc.)
    let (mut prompt, persona_id, persona_workdir) = build_task_prompt(resolution, base_prompt);

    // Load and append custom agent instructions
    let workdir = resolution.persona.as_ref().map(|p| p.workdir.as_str());
    if let Some(config) = load_agent_config(agent_id, workdir) {
        if let Some(ref instructions) = config.instructions {
            prompt.push_str(&format!(
                "\n\n## Agent: {} — Custom Instructions\n\n{}",
                config.name, instructions
            ));
            log::info!(
                "Custom agent '{}' ({}): injected {} chars of instructions",
                config.name,
                agent_id,
                instructions.len()
            );
        }
    } else {
        log::warn!(
            "Custom agent '{}' not found in any agent directory, using Worker defaults",
            agent_id
        );
    }

    (prompt, persona_id, persona_workdir)
}
