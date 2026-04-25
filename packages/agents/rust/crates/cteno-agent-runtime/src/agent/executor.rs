//! Sub-Agent Executor
//!
//! Executes sub-agents synchronously: the parent agent's ReAct loop calls a sub-agent
//! as a tool, waits for it to complete, and receives the result inline.
//! Each sub-agent runs its own independent ReAct loop with its own tool set.

use crate::agent_config::AgentConfig;
use crate::autonomous_agent::{
    execute_autonomous_agent_with_session, fetch_native_tools, AcpMessageSender, PermissionChecker,
};
use crate::llm::Tool;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8};
use std::sync::Arc;

/// Maximum recursion depth for sub-agent calls.
/// Depth 0 = parent agent, depth 1 = sub-agent. Sub-agents cannot spawn further sub-agents.
const MAX_DEPTH: u32 = 1;

/// Builds a session-tagged `AcpMessageSender` for a subagent. The runtime calls
/// this with the **subagent's own** session id (the one the SubAgentManager
/// generated and that `agent_sessions.id` will match) so the subagent's ACP
/// frames are routed under its own id — not the parent's. This is what lets
/// the desktop project subagent transcripts into a separate `agent_sessions`
/// row keyed by `subagent.id` (so `BaseSessionPage` via
/// `useSession(subagent.id)` finds them) without bleeding subagent thinking /
/// tool-call frames into the parent persona's transcript.
pub type AcpSenderFactory = Arc<dyn Fn(String) -> AcpMessageSender + Send + Sync>;

/// Context needed to execute a sub-agent, passed from the parent agent's handler.
#[derive(Clone)]
pub struct SubAgentContext {
    pub db_path: PathBuf,
    pub builtin_skills_dir: PathBuf,
    pub user_skills_dir: PathBuf,
    pub global_api_key: String,
    pub default_base_url: String,
    /// Parent session profile ID (for tool context injection and profile inheritance)
    pub profile_id: Option<String>,
    /// Whether to route LLM calls through Happy Server proxy
    pub use_proxy: bool,
    /// Parent session model from resolved profile (overrides agent default when present)
    pub profile_model: Option<String>,
    /// Factory for building an AcpMessageSender bound to the subagent's own
    /// session id. The runtime calls `(factory)(subagent_session_id)` and
    /// passes the result down to `execute_autonomous_agent_with_session`. See
    /// [`AcpSenderFactory`] for why this is a factory and not a pre-built
    /// sender (which would be tagged with the parent's id and bleed the
    /// subagent's stream into the persona transcript). `None` = silent
    /// execution (no ACP routing).
    pub acp_sender_factory: Option<AcpSenderFactory>,
    /// Permission checker from parent agent — sub-agent asks the same user for permission
    pub permission_checker: Option<PermissionChecker>,
    /// Parent's abort flag — aborting the parent also aborts sub-agents
    pub abort_flag: Option<Arc<AtomicBool>>,
    pub thinking_flag: Option<Arc<AtomicU8>>,
    /// API format inherited from parent profile
    pub api_format: crate::llm_profile::ApiFormat,
    /// Sandbox policy inherited from the parent session.
    pub sandbox_policy: Option<crate::tool_executors::SandboxPolicy>,
}

/// Execute a sub-agent synchronously.
///
/// The parent agent's ReAct loop calls this when it encounters an `agent_xxx` tool use.
/// This function starts the sub-agent's own ReAct loop, waits for completion (with timeout),
/// and returns the sub-agent's final response text.
///
/// Uses `Box::pin` internally to handle async recursion (execute_sub_agent → ReAct loop → execute_tool → execute_sub_agent).
///
/// `session_id_override`: if provided, the subagent's transcript persists
/// under this session id in `agent_sessions`. Used by `SubAgentManager` so
/// the SubAgent record id and the agent_sessions row id match — clicking a
/// SubAgent in the UI then navigates to `/session/{subagent.id}` and finds
/// its conversation. When `None` (legacy callers, ad-hoc sub-agent calls),
/// a synthetic `sub_{agent_id}_{uuid}` id is generated.
pub fn execute_sub_agent<'a>(
    agent_config: &'a AgentConfig,
    prompt: &'a str,
    _context: Option<serde_json::Value>,
    exec_ctx: &'a SubAgentContext,
    depth: u32,
    session_id_override: Option<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>> {
    Box::pin(execute_sub_agent_inner(
        agent_config,
        prompt,
        exec_ctx,
        depth,
        session_id_override,
    ))
}

async fn execute_sub_agent_inner(
    agent_config: &AgentConfig,
    prompt: &str,
    exec_ctx: &SubAgentContext,
    depth: u32,
    session_id_override: Option<String>,
) -> Result<String, String> {
    if depth >= MAX_DEPTH {
        return Err(format!(
            "Sub-agent recursion depth limit reached (max {}). Agent '{}' cannot spawn further sub-agents.",
            MAX_DEPTH, agent_config.id
        ));
    }

    log::info!(
        "[SubAgent] Executing agent '{}' (depth={}) with prompt: {}",
        agent_config.id,
        depth,
        &prompt[..prompt.floor_char_boundary(prompt.len().min(100))]
    );

    // Resolve model: parent profile model first, then agent override, then default.
    let model = exec_ctx
        .profile_model
        .as_deref()
        .or(agent_config.model.as_deref())
        .unwrap_or("deepseek-chat");
    let temperature = agent_config.temperature.unwrap_or(0.2);
    let max_tokens = agent_config.max_tokens.unwrap_or(4096);
    let timeout_secs = agent_config.timeout_seconds.unwrap_or(300);
    let base_url = &exec_ctx.default_base_url;

    // Build sub-agent's isolated tool set
    let tools = build_sub_agent_tools(agent_config, exec_ctx).await;
    log::info!(
        "[SubAgent] Agent '{}' loaded {} tools",
        agent_config.id,
        tools.len()
    );

    // Use the SubAgentManager-supplied id when provided so the
    // agent_sessions row id matches the SubAgent record id (UI navigation
    // depends on this); fall back to the legacy synthetic id for ad-hoc
    // callers that don't go through SubAgentManager.
    let sub_session_id = session_id_override
        .unwrap_or_else(|| format!("sub_{}_{}", agent_config.id, uuid::Uuid::new_v4()));

    // Build an ACP sender bound to *this* subagent's session id so its frames
    // are routed under its own id (see [`AcpSenderFactory`] doc). Cloned so we
    // can emit a final `message` + `task_complete` after the inner runtime
    // returns — the inner loop only emits intermediate frames (thinking,
    // tool-call, tool-result), it never emits the final assistant text or a
    // task-complete marker. The parent runner does that after `execute_*`
    // returns; the subagent path needs the same closing frames so its own
    // `agent_sessions` row ends with the final answer + a clean
    // `task_complete` (otherwise the BackgroundRunsModal detail page only
    // shows intermediate `thinking` blocks and the user sees no result).
    let sub_acp_sender = exec_ctx
        .acp_sender_factory
        .as_ref()
        .map(|factory| factory(sub_session_id.clone()));
    let sub_acp_sender_for_close = sub_acp_sender.clone();

    // System prompt from the AGENT.md markdown body.
    let base_system_prompt = agent_config.instructions.as_deref().unwrap_or(
        "You are a helpful AI assistant. Complete the given task and return the result.",
    );
    let runtime_context_messages = vec![crate::system_prompt::build_runtime_datetime_context(
        base_system_prompt,
    )];

    // Execute with timeout
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        execute_autonomous_agent_with_session(
            exec_ctx.db_path.clone(),
            &agent_config.id,
            &exec_ctx.global_api_key,
            base_url,
            model,
            base_system_prompt,
            prompt,
            None,
            &tools,
            temperature,
            max_tokens,
            None,
            Some(&sub_session_id),
            None, // user_id
            Some(runtime_context_messages),
            sub_acp_sender,
            None, // skill_activation_handler
            exec_ctx.permission_checker.clone(),
            None, // compress_client — sub-agents have short conversations, no compression needed
            exec_ctx.profile_id.as_deref(),
            exec_ctx.abort_flag.clone(),
            exec_ctx.thinking_flag.clone(),
            None, // context_tokens
            None, // agent_configs — sub-agents cannot spawn further sub-agents
            None, // sub_agent_ctx — no recursion
            None, // No message queue for sub-agent path
            exec_ctx.use_proxy,
            None, // No stream callback for sub-agent path
            None, // No persona_id for sub-agent path
            None, // No persona_workdir for sub-agent path
            exec_ctx.api_format.clone(),
            false, // Sub-agents don't need vision support
            false, // No thinking for sub-agent path
            None,  // No reasoning effort for sub-agent path
            true,  // Supports function calling for sub-agent path
            false, // No image output for sub-agent path
            None,  // No user images for sub-agent path
            exec_ctx.sandbox_policy.as_ref(),
        ),
    )
    .await;

    match result {
        Ok(Ok(agent_result)) => {
            log::info!(
                "[SubAgent] Agent '{}' completed in {} iterations, response length: {}",
                agent_config.id,
                agent_result.iteration_count,
                agent_result.response.len()
            );
            // Mirror the parent runner's closing frames so the subagent's
            // own session transcript ends with the final assistant text and
            // a `task_complete` marker. See `sub_acp_sender_for_close` doc.
            if let Some(sender) = sub_acp_sender_for_close.as_ref() {
                if !agent_result.response.is_empty() {
                    sender(serde_json::json!({
                        "type": "message",
                        "message": agent_result.response.clone(),
                    }))
                    .await;
                }
                sender(serde_json::json!({
                    "type": "task_complete",
                    "id": uuid::Uuid::new_v4().to_string(),
                }))
                .await;
            }
            Ok(agent_result.response)
        }
        Ok(Err(e)) => {
            log::error!(
                "[SubAgent] Agent '{}' execution failed: {}",
                agent_config.id,
                e
            );
            Err(format!("Sub-agent '{}' failed: {}", agent_config.name, e))
        }
        Err(_) => {
            log::error!(
                "[SubAgent] Agent '{}' timed out after {}s",
                agent_config.id,
                timeout_secs
            );
            Err(format!(
                "Sub-agent '{}' timed out after {} seconds",
                agent_config.name, timeout_secs
            ))
        }
    }
}

/// Build the tool set for a sub-agent based on its AGENT.md `tools` and `skills` fields.
///
/// If `tools` is specified, only those native tools are included.
/// If `skills` is specified, only those skills are included.
/// If either is None, all available tools/skills are included.
/// Sub-agents never get agent_xxx tools (no recursion).
async fn build_sub_agent_tools(
    agent_config: &AgentConfig,
    exec_ctx: &SubAgentContext,
) -> Vec<Tool> {
    let mut tools = Vec::new();

    // 1. Native tools (shell, read, edit, websearch, etc.)
    let all_native = fetch_native_tools().await;
    if let Some(ref allowed_tools) = agent_config.tools {
        // Only include tools explicitly listed in AGENT.md
        tools.extend(
            all_native
                .into_iter()
                .filter(|t| allowed_tools.contains(&t.name)),
        );
    } else {
        // No restriction — include all native tools
        tools.extend(all_native);
    }

    // 2. Skills are now injected as context, not as LLM tools
    // (skills provide instructions that guide the agent to use native tools)

    // 3. Sub-agents do NOT get agent_xxx tools — prevents recursion

    // 4. Filter out MCP tools — sub-agents should not have access to parent's MCP servers
    tools.retain(|t| !t.name.starts_with("mcp__"));

    tools
}
