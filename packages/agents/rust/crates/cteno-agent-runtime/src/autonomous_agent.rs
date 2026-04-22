//! Autonomous Agent Execution with Queue Integration
//!
//! This module implements the main agent execution loop with ReAct pattern
//! and integrates the message queue system for handling user follow-up questions
//! and SubAgent completion notifications.

use crate::agent::executor::SubAgentContext;
use crate::agent_config::AgentConfig;
use crate::agent_queue::{AgentMessage, AgentMessageQueue};
use crate::agent_session::{AgentSessionManager, SessionMessage};
use crate::chat_compression::CompressionService;
use crate::llm::{
    ContentBlock, LLMClient, LLMResponseType, Message, MessageContent, MessageRole, StreamCallback,
    Tool, Usage, parse_session_content, serialize_content_for_session,
};
use crate::permission::PermissionCheckResult;
use chrono::Utc;
use futures_util::future::join_all;
use serde_json::json;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

/// Callback for sending intermediate ACP messages (tool-call, tool-result, task lifecycle).
/// Receives an ACP data payload (serde_json::Value) and sends it asynchronously.
pub type AcpMessageSender =
    Arc<dyn Fn(serde_json::Value) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Callback for post-processing successful skill activations.
/// Used for prompt-only skills (activate_skill) to update session state and notify the UI.
pub type SkillActivationHandler =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Callback for checking tool permissions before execution.
/// Receives (tool_name, call_id, input) and returns a PermissionCheckResult.
pub type PermissionChecker = Arc<
    dyn Fn(
            String,
            String,
            serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = PermissionCheckResult> + Send>>
        + Send
        + Sync,
>;

/// Maximum ReAct iterations to prevent infinite loops
const MAX_ITERATIONS: usize = 100;

/// Tool output character budget (~16K tokens at 4 chars/token).
/// Lowered from 200K to catch medium-sized outputs that previously accumulated unchecked.
const TOOL_OUTPUT_CHAR_BUDGET: usize = 64_000;
const CHARS_PER_TOKEN_ESTIMATE: usize = 4;

/// Normalize tool names/inputs for ACP UI rendering.
///
/// Internally our tool IDs are lowercase (e.g. "shell", "write", "edit") and some tools use
/// different field names (e.g. `path` vs `file_path`). The desktop UI's specialized renderers
/// are keyed on canonical tool names ("Bash", "Write", "Edit", ...), so we normalize here to keep
/// front-end display stable across providers (Codex/Claude/etc).
fn normalize_tool_call_for_ui(
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> (String, serde_json::Value) {
    let lower = tool_name.to_ascii_lowercase();
    match lower.as_str() {
        // Shell/terminal tool aliases across providers
        "shell" | "zsh" | "bash" => ("Bash".to_string(), tool_input.clone()),

        // File write: internal uses `path`, UI expects `file_path`
        "write" => {
            if let Some(obj) = tool_input.as_object() {
                let mut out = obj.clone();
                if let Some(path) = obj
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .or_else(|| obj.get("path").and_then(|v| v.as_str()))
                {
                    out.insert("file_path".to_string(), json!(path));
                }
                ("Write".to_string(), serde_json::Value::Object(out))
            } else {
                ("Write".to_string(), tool_input.clone())
            }
        }

        // File edit: internal uses `path`, UI expects `file_path`
        "edit" => {
            if let Some(obj) = tool_input.as_object() {
                let mut out = obj.clone();
                if let Some(path) = obj
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .or_else(|| obj.get("path").and_then(|v| v.as_str()))
                {
                    out.insert("file_path".to_string(), json!(path));
                }
                ("Edit".to_string(), serde_json::Value::Object(out))
            } else {
                ("Edit".to_string(), tool_input.clone())
            }
        }

        // These already match UI expectations
        "read" => ("Read".to_string(), tool_input.clone()),
        "websearch" => ("WebSearch".to_string(), tool_input.clone()),

        // Plan tool → TodoWrite for frontend TodoView rendering
        "update_plan" => ("TodoWrite".to_string(), tool_input.clone()),

        // Unknown/unmapped: keep as-is
        _ => (tool_name.to_string(), tool_input.clone()),
    }
}

/// Truncate tool output using middle truncation: keep head + tail, cut the middle.
/// This preserves both the beginning (error messages, headers, imports) and the end
/// (final output, exit codes) which are typically most informative.
/// Returns (truncated_output, was_truncated)
fn truncate_tool_output(output: &str, char_budget: usize) -> (String, bool) {
    if output.len() <= char_budget {
        return (output.to_string(), false);
    }

    let half_budget = char_budget / 2;
    let head_end = find_char_boundary_before(output, half_budget);
    let tail_start = find_char_boundary_after(output, output.len() - half_budget);

    let removed_chars = tail_start - head_end;
    let removed_tokens = removed_chars / CHARS_PER_TOKEN_ESTIMATE;

    let truncated = format!(
        "{}\n\n... [truncated: ~{} tokens removed] ...\n\n{}",
        &output[..head_end],
        removed_tokens,
        &output[tail_start..]
    );

    (truncated, true)
}

/// Find the largest byte index <= `pos` that lies on a UTF-8 char boundary.
fn find_char_boundary_before(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut i = pos;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Find the smallest byte index >= `pos` that lies on a UTF-8 char boundary.
fn find_char_boundary_after(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut i = pos;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Sanitize LLM message history to fix tool_use/tool_result pairing issues.
///
/// LLM APIs require that every `tool_result` in a user message references a `tool_use`
/// in the immediately preceding assistant message, and vice versa.
/// This can break when:
/// - Agent was interrupted mid-tool-call (abort, timeout, disconnect)
/// - Message history was truncated and cut through a tool use cycle
/// - Session was resumed with stale history
///
/// This function:
/// 1. Removes leading user messages that contain orphaned tool_results
/// 2. Removes trailing assistant messages with tool_use but no following tool_result
/// 3. Validates tool_use_id ↔ tool_use pairing between consecutive messages
fn sanitize_llm_messages(messages: &mut Vec<Message>) {
    let original_count = messages.len();
    if messages.is_empty() {
        return;
    }

    // Pass 1: Remove leading messages that start with tool_result (orphaned from truncation)
    while !messages.is_empty() {
        if let Some(first) = messages.first() {
            if has_tool_results(&first.content) && first.role == MessageRole::User {
                log::warn!(
                    "[Agent] Sanitize: removing leading user message with orphaned tool_results"
                );
                messages.remove(0);
                continue;
            }
        }
        break;
    }

    // Pass 2: Remove trailing assistant message with tool_use but no following tool_result
    while !messages.is_empty() {
        if let Some(last) = messages.last() {
            if has_tool_uses(&last.content) && last.role == MessageRole::Assistant {
                log::warn!(
                    "[Agent] Sanitize: removing trailing assistant message with unresolved tool_use"
                );
                messages.pop();
                // Also remove any preceding orphaned tool_result that might now be trailing
                continue;
            }
            if has_tool_results(&last.content) && last.role == MessageRole::User {
                log::warn!(
                    "[Agent] Sanitize: removing trailing user message with orphaned tool_results"
                );
                messages.pop();
                continue;
            }
        }
        break;
    }

    // Pass 3: Walk pairs and validate tool_use_id matching
    let mut i = 0;
    while i + 1 < messages.len() {
        let is_assistant_with_tools =
            messages[i].role == MessageRole::Assistant && has_tool_uses(&messages[i].content);
        let next_is_user_with_results = messages
            .get(i + 1)
            .map(|m| m.role == MessageRole::User && has_tool_results(&m.content))
            .unwrap_or(false);

        if is_assistant_with_tools && !next_is_user_with_results {
            // Assistant has tool_use but next message is not a tool_result user message
            log::warn!(
                "[Agent] Sanitize: assistant message at index {} has tool_use without matching tool_result, removing pair",
                i
            );
            messages.remove(i);
            continue;
        }

        if !is_assistant_with_tools && next_is_user_with_results {
            // User message has tool_result but preceding assistant doesn't have tool_use
            log::warn!(
                "[Agent] Sanitize: user message at index {} has tool_result without matching tool_use, removing",
                i + 1
            );
            messages.remove(i + 1);
            continue;
        }

        if is_assistant_with_tools && next_is_user_with_results {
            // Validate that tool_use IDs match tool_result IDs
            let use_ids = get_tool_use_ids(&messages[i].content);
            let result_ids = get_tool_result_ids(&messages[i + 1].content);

            // Check if all result IDs have a matching use ID
            let orphaned_results: Vec<_> = result_ids
                .iter()
                .filter(|id| !use_ids.contains(id))
                .collect();

            if !orphaned_results.is_empty() {
                log::warn!(
                    "[Agent] Sanitize: found {} orphaned tool_result IDs at index {}, removing both messages",
                    orphaned_results.len(),
                    i
                );
                messages.remove(i + 1);
                messages.remove(i);
                continue;
            }
        }

        i += 1;
    }

    if messages.len() != original_count {
        log::info!(
            "[Agent] Sanitize: cleaned {} → {} messages",
            original_count,
            messages.len()
        );
    }
}

/// Check if message content contains any ToolResult blocks
fn has_tool_results(content: &MessageContent) -> bool {
    match content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. })),
        _ => false,
    }
}

/// Check if message content contains any ToolUse blocks
fn has_tool_uses(content: &MessageContent) -> bool {
    match content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. })),
        _ => false,
    }
}

/// Extract tool_use IDs from message content
fn get_tool_use_ids(content: &MessageContent) -> Vec<String> {
    match content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

/// Extract tool_result tool_use_ids from message content
fn get_tool_result_ids(content: &MessageContent) -> Vec<String> {
    match content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

/// Rebuild LLM message Vec from persisted session messages.
///
/// Skips old-format tool markers and parses BLOCKS: prefixed content back into
/// structured ContentBlock arrays. Used after compression and at session start.
fn rebuild_llm_messages_from_session(messages: &[SessionMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter(|m| !m.content.starts_with("[Tool:") && !m.content.starts_with("[Tool Result:"))
        .map(|m| {
            let role = match m.role.as_str() {
                "assistant" => MessageRole::Assistant,
                "system" => MessageRole::System,
                _ => MessageRole::User,
            };
            let content = parse_session_content(&m.content);
            // Strip base64 image data from ToolResult content to avoid sending
            // huge base64 strings as text to the LLM. Images were already shown
            // to the LLM as ContentBlock::Image during the original execution.
            let content = strip_images_from_tool_results(content);
            Message { role, content }
        })
        .collect()
}

/// Strip `images` arrays from ToolResult content blocks.
/// When tool results containing screenshots are persisted, they include base64
/// image data in the JSON `content` field. On session rebuild we remove these
/// to avoid sending megabytes of base64 text to the LLM.
fn strip_images_from_tool_results(content: MessageContent) -> MessageContent {
    match content {
        MessageContent::Blocks(blocks) => {
            let cleaned: Vec<ContentBlock> = blocks
                .into_iter()
                .map(|block| match block {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let cleaned_content = if let Ok(mut json) =
                            serde_json::from_str::<serde_json::Value>(&content)
                        {
                            if json
                                .get("images")
                                .and_then(|v| v.as_array())
                                .map_or(false, |a| !a.is_empty())
                            {
                                json.as_object_mut().map(|obj| obj.remove("images"));
                                serde_json::to_string(&json).unwrap_or(content)
                            } else {
                                content
                            }
                        } else {
                            content
                        };
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content: cleaned_content,
                            is_error,
                        }
                    }
                    other => other,
                })
                .collect();
            MessageContent::Blocks(cleaned)
        }
        other => other,
    }
}

/// Estimate total tokens for a set of LLM messages (rough: chars / 4).
fn estimate_llm_messages_tokens(messages: &[Message]) -> u32 {
    let total_chars: usize = messages
        .iter()
        .map(|m| match &m.content {
            MessageContent::Text(s) => s.len(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => text.len(),
                    ContentBlock::ToolUse { input, .. } => input.to_string().len(),
                    ContentBlock::ToolResult { content, .. } => content.len(),
                    ContentBlock::Thinking { thinking, .. } => thinking.len(),
                    ContentBlock::Image { source } => source.data.len(),
                })
                .sum(),
        })
        .sum();
    (total_chars / CHARS_PER_TOKEN_ESTIMATE) as u32
}

/// Agent execution result
#[derive(Debug, Clone)]
pub struct AgentExecutionResult {
    pub response: String,
    pub session_id: String,
    pub iteration_count: usize,
    pub intermediate_messages: Vec<String>, // Tool execution progress messages
    pub total_usage: Usage,                 // Accumulated token usage across all LLM calls
}

/// Build LLM Tool definitions from agents that have expose_as_tool=true.
/// These tools allow the parent agent to call sub-agents as synchronous tool invocations.
pub fn build_agent_tools(agents: &[AgentConfig]) -> Vec<Tool> {
    let mut tools: Vec<Tool> = agents
        .iter()
        .filter(|a| a.expose_as_tool.unwrap_or(false))
        .map(|a| a.to_tool())
        .collect();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

/// Fetch native tool definitions directly from the ToolRegistry.
/// Returns ALL tools (both immediate and deferred) — use `fetch_native_tools_split`
/// when you need to separate immediate tools from deferred summaries.
pub async fn fetch_native_tools() -> Vec<Tool> {
    let registry = match crate::hooks::tool_registry_handle() {
        Some(r) => r,
        None => {
            log::warn!("[Agent] ToolRegistry not available: hook not installed");
            return vec![];
        }
    };

    let reg = registry.read().await;
    let mut tools = reg.get_tools_for_llm();

    log::info!(
        "[Agent] Fetched {} native tools from ToolRegistry",
        tools.len()
    );
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

/// Fetch native tools split into immediate tools (full schema) and deferred summaries.
///
/// - **immediate**: tools sent to the LLM with full schema on every turn
/// - **deferred_summaries**: `(tool_id, description, search_hint)` tuples listed in the
///   system prompt so the LLM knows they exist and can fetch them via `tool_search`
pub async fn fetch_native_tools_split() -> (Vec<Tool>, Vec<(String, String, Option<String>)>) {
    let registry = match crate::hooks::tool_registry_handle() {
        Some(r) => r,
        None => {
            log::warn!("[Agent] ToolRegistry not available: hook not installed");
            return (vec![], vec![]);
        }
    };

    let reg = registry.read().await;
    let mut immediate = reg.get_immediate_tools_for_llm();
    let deferred = reg.get_deferred_tool_summaries();

    log::info!(
        "[Agent] Fetched {} immediate + {} deferred native tools from ToolRegistry",
        immediate.len(),
        deferred.len()
    );
    immediate.sort_by(|a, b| a.name.cmp(&b.name));
    (immediate, deferred)
}

/// Build a `<system-reminder>` block listing deferred tools so the LLM knows they exist.
/// The LLM can then use `tool_search` to fetch full schemas on demand.
pub fn build_deferred_tools_context(
    summaries: &[(String, String, Option<String>)],
) -> Option<String> {
    if summaries.is_empty() {
        return None;
    }

    let mut lines = Vec::with_capacity(summaries.len() + 4);
    lines.push("<system-reminder>".to_string());
    lines.push("The following deferred tools are available via ToolSearch:".to_string());
    lines.push(String::new());

    for (id, desc, _hint) in summaries {
        lines.push(format!("- **{}**: {}", id, desc));
    }

    lines.push(String::new());
    lines.push(
        "Use tool_search to fetch the full schema before calling any of these tools.".to_string(),
    );
    lines.push("</system-reminder>".to_string());
    Some(lines.join("\n"))
}

/// Execute autonomous agent with session management and queue integration
#[allow(clippy::too_many_arguments)]
pub async fn execute_autonomous_agent_with_session(
    db_path: PathBuf,
    agent_id: &str,
    api_key: &str,
    base_url: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
    user_local_id: Option<&str>,
    tools: &[Tool],
    temperature: f32,
    max_tokens: u32,
    session_id: Option<&str>,
    user_id: Option<&str>,
    // Ephemeral contextual user-role messages (runtime/date/skills/etc.).
    // These are injected before the latest user prompt for this run and are
    // not persisted into session history.
    contextual_user_messages: Option<Vec<String>>,
    acp_sender: Option<AcpMessageSender>,
    skill_activation_handler: Option<SkillActivationHandler>,
    permission_checker: Option<PermissionChecker>,
    compress_client: Option<&LLMClient>,
    profile_id: Option<&str>,
    abort_flag: Option<Arc<AtomicBool>>,
    thinking_flag: Option<Arc<AtomicU8>>,
    context_tokens: Option<Arc<AtomicU32>>,
    // Sub-agent support: agent configs + execution context for dispatching agent_xxx tool calls
    agent_configs: Option<Vec<AgentConfig>>,
    sub_agent_ctx: Option<SubAgentContext>,
    // Message queue for injecting user messages during tool_use iterations
    message_queue: Option<Arc<AgentMessageQueue>>,
    // Use Happy Server LLM proxy (Bearer token auth) instead of direct API key
    use_proxy: bool,
    // Optional callback for streaming text deltas to the frontend in real-time
    stream_callback: Option<StreamCallback>,
    // Persona context: if this session belongs to a persona, inject __persona_id into tool calls
    persona_id: Option<&str>,
    // Persona workdir: used by memory tool to locate private memory at {workdir}/.cteno/memory/
    persona_workdir: Option<&str>,
    // API format: determines whether to use Anthropic or OpenAI Responses API
    api_format: crate::llm_profile::ApiFormat,
    // Whether the model supports vision (image inputs in tool results)
    supports_vision: bool,
    // Whether to enable thinking/reasoning mode (server-driven)
    enable_thinking: bool,
    // Whether the model supports function calling / tool use (default true)
    supports_function_calling: bool,
    // Whether the model supports image output (Gemini responseModalities)
    supports_image_output: bool,
    // Optional image attachments from user message (injected into latest user prompt as content blocks)
    user_images: Option<Vec<crate::llm::ImageSource>>,
    // Sandbox policy for workspace boundary enforcement (None = WorkspaceWrite default)
    sandbox_policy: Option<&crate::tool_executors::SandboxPolicy>,
) -> Result<AgentExecutionResult, String> {
    // Initialize session manager
    let session_manager = AgentSessionManager::new(db_path);

    // Get or create session
    let mut session = if let Some(sid) = session_id {
        // Try to get existing session, create if not found
        match session_manager
            .get_session(sid)
            .map_err(|e| format!("Failed to get session: {}", e))?
        {
            Some(s) => s,
            None => {
                // Session doesn't exist, create it with the provided ID
                session_manager
                    .create_session_with_id(
                        sid, agent_id, user_id, None, // Use default timeout
                    )
                    .map_err(|e| format!("Failed to create session: {}", e))?
            }
        }
    } else {
        session_manager
            .create_session(
                agent_id, user_id, None, // Use default timeout
            )
            .map_err(|e| format!("Failed to create session: {}", e))?
    };

    // NOTE: Do NOT clear old messages here. Removing prefix messages invalidates
    // the LLM's KV cache, causing expensive full recomputation on every new turn.
    // Context management is handled solely by token-based compression (CompressionService)
    // which triggers when actual API token usage exceeds the threshold.

    let session_workdir = session
        .context_data
        .as_ref()
        .and_then(extract_session_workdir_from_context);
    if let Some(ref workdir) = session_workdir {
        log::info!(
            "[Agent] Using session workdir for tool execution: {} (session={})",
            workdir,
            session.id
        );
    }

    // Load tool hooks from workspace .cteno/hooks.yaml (gracefully degrades to empty)
    let hooks_manager =
        crate::tool_hooks::HooksManager::load(session_workdir.as_deref().map(std::path::Path::new));

    // Add user message to session (persisted history only includes actual user intent).
    session.messages.push(SessionMessage {
        role: "user".to_string(),
        content: user_prompt.to_string(),
        timestamp: Utc::now().to_rfc3339(),
        local_id: user_local_id.map(|value| value.to_string()),
    });

    // --- Session Memory initialisation ---
    // Load existing SessionMemory from context_data (survives session resume).
    let session_memory: Arc<std::sync::Mutex<Option<crate::session_memory::SessionMemory>>> =
        Arc::new(std::sync::Mutex::new(
            session.get_context_field("session_memory").and_then(|v| {
                serde_json::from_value::<crate::session_memory::SessionMemory>(v.clone()).ok()
            }),
        ));
    let mut extraction_tracker = crate::session_memory::ExtractionTracker::from_meta(
        session_memory.lock().unwrap().as_ref().map(|m| &m.meta),
    );
    let extraction_handle: Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>> =
        Arc::new(std::sync::Mutex::new(None));

    // Compression is now purely token-based (triggered mid-loop by needs_compression_by_tokens).
    // Message-count trigger removed — it fired too early for tool-heavy agents (~20 msgs = ~8K tokens).
    let compression_service = CompressionService::for_model(model);

    session_manager
        .update_messages(&session.id, &session.messages)
        .map_err(|e| format!("Failed to update session: {}", e))?;

    // Store profile_id for injection into tool inputs (used by fetch tool etc.)
    let session_profile_id = profile_id.map(|s| s.to_string());

    // Sandbox policy for workspace boundary enforcement.
    // Defaults to WorkspaceWrite (writes restricted to workdir + tmp).
    // Can be overridden per-session via PermissionHandler.
    let session_sandbox_policy: Option<crate::tool_executors::SandboxPolicy> =
        sandbox_policy.cloned();

    // Initialize LLM client based on base_url / use_proxy:
    //   - openrouter.ai    → direct Bearer subkey call to openrouter.ai/messages
    //   - use_proxy=true   → Happy Server Bearer proxy (/v1/llm/chat)
    //   - otherwise        → direct x-api-key call to an Anthropic-compatible base_url
    let llm_client = if base_url.starts_with("https://openrouter.ai") {
        log::info!("[Agent] Using OpenRouter direct at {}", base_url);
        LLMClient::with_openrouter(api_key.to_string(), base_url.to_string())
    } else if use_proxy {
        log::info!("[Agent] Using Happy Server LLM proxy at {}", base_url);
        LLMClient::with_proxy_and_machine_id(api_key.to_string(), base_url.to_string())
    } else {
        LLMClient::with_base_url(api_key.to_string(), base_url.to_string())
    };

    // Keep tool ordering stable across turns to maximize prompt cache hits.
    // If model doesn't support function calling, pass empty tool list.
    let mut llm_tools = if supports_function_calling {
        tools.to_vec()
    } else {
        log::info!("[Agent] Model does not support function calling, skipping tools");
        vec![]
    };
    llm_tools.sort_by(|a, b| a.name.cmp(&b.name));

    // Build initial LLM messages from session history
    let mut llm_messages: Vec<Message> = rebuild_llm_messages_from_session(&session.messages);

    // Inject user images into the last user message (ephemeral, not persisted).
    // Save images for re-injection after compression (which rebuilds messages from session history).
    let saved_user_images: Option<Vec<crate::llm::ImageSource>> =
        if let Some(ref images) = user_images {
            if !images.is_empty() && supports_vision {
                Some(images.clone())
            } else {
                None
            }
        } else {
            None
        };
    if let Some(images) = user_images {
        if !images.is_empty() && supports_vision {
            let image_count = images.len();
            if let Some(last_user) = llm_messages
                .iter_mut()
                .rev()
                .find(|m| matches!(m.role, MessageRole::User))
            {
                let text = last_user.content.as_text();
                let mut blocks: Vec<ContentBlock> = images
                    .into_iter()
                    .map(|src| ContentBlock::Image { source: src })
                    .collect();
                blocks.push(ContentBlock::Text { text });
                last_user.content = MessageContent::Blocks(blocks);
                log::info!(
                    "[Agent] Injected {} image(s) into user message",
                    image_count
                );
            }
        }
    }

    log::info!(
        "[Agent] Built {} LLM messages from session history",
        llm_messages.len()
    );

    // Inject ephemeral runtime context near the tail of prompt input so dynamic
    // context updates do not mutate the system prompt prefix.
    // We keep a copy of these messages for reinjection after compression.
    let mut contextual_messages = contextual_user_messages.unwrap_or_default();
    if let Some(workdir) = session_workdir.as_deref() {
        contextual_messages.push(build_workdir_context_message(workdir));
    }
    // Preserve for reinjection after compression
    let saved_contextual_messages = contextual_messages.clone();
    if !contextual_messages.is_empty() {
        let context_items: Vec<Message> =
            contextual_messages.into_iter().map(Message::user).collect();
        insert_context_messages_before_latest_user(&mut llm_messages, context_items);
    }

    // Sanitize message history to fix broken tool_use/tool_result pairing
    sanitize_llm_messages(&mut llm_messages);

    // Pre-turn compression: if resuming a long session, the first LLM call may already
    // exceed the context window. Compress before entering the ReAct loop.
    {
        let estimated_tokens = estimate_llm_messages_tokens(&llm_messages)
            + (system_prompt.len() / CHARS_PER_TOKEN_ESTIMATE) as u32;
        if compression_service.needs_compression_by_tokens(estimated_tokens) {
            log::info!(
                "[Agent] Pre-turn compression triggered: ~{} estimated tokens",
                estimated_tokens
            );
            if let Some(ref flag) = thinking_flag {
                flag.store(2, Ordering::SeqCst);
            }

            // Wait for in-flight session memory extraction (up to 10s)
            {
                let handle = extraction_handle.lock().unwrap().take();
                if let Some(h) = handle {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), h).await;
                }
            }

            // Try session memory zero-cost compression first
            let memory_markdown = session_memory
                .lock()
                .unwrap()
                .as_ref()
                .map(crate::session_memory::render_as_markdown);

            if let Some(ref md) = memory_markdown {
                log::info!("[Agent] Pre-turn: using session memory for zero-cost compression");
                let compressed =
                    compression_service.compress_with_session_memory(md, &session.messages);
                session.messages = compressed;
                if let Some(ref ct) = context_tokens {
                    ct.store(0, Ordering::SeqCst);
                }

                llm_messages = rebuild_llm_messages_from_session(&session.messages);
                sanitize_llm_messages(&mut llm_messages);
                reinject_context_after_compression(
                    &mut llm_messages,
                    session_workdir.as_deref(),
                    &saved_contextual_messages,
                );

                if let Some(ref images) = saved_user_images {
                    if let Some(last_user) = llm_messages
                        .iter_mut()
                        .rev()
                        .find(|m| matches!(m.role, MessageRole::User))
                    {
                        let text = last_user.content.as_text();
                        let mut blocks: Vec<ContentBlock> = images
                            .iter()
                            .cloned()
                            .map(|src| ContentBlock::Image { source: src })
                            .collect();
                        blocks.push(ContentBlock::Text { text });
                        last_user.content = MessageContent::Blocks(blocks);
                        log::info!(
                            "[Agent] Re-injected {} image(s) after pre-turn session memory compression",
                            images.len()
                        );
                    }
                }

                session_manager
                    .update_messages(&session.id, &session.messages)
                    .map_err(|e| {
                        format!(
                            "Failed to update session after pre-turn session memory compression: {}",
                            e
                        )
                    })?;

                log::info!(
                    "[Agent] Pre-turn: rebuilt {} LLM messages after session memory compression",
                    llm_messages.len()
                );
            } else if let Some(client) = compress_client {
                // No session memory available — fall back to LLM-based compression
                match compression_service
                    .compress_history(client, &session.messages)
                    .await
                {
                    Ok(compressed) => {
                        log::info!(
                            "[Agent] Pre-turn compression: {} → {} messages",
                            session.messages.len(),
                            compressed.len()
                        );
                        session.messages = compressed;
                        if let Some(ref ct) = context_tokens {
                            ct.store(0, Ordering::SeqCst);
                        }

                        llm_messages = rebuild_llm_messages_from_session(&session.messages);
                        sanitize_llm_messages(&mut llm_messages);
                        reinject_context_after_compression(
                            &mut llm_messages,
                            session_workdir.as_deref(),
                            &saved_contextual_messages,
                        );

                        if let Some(ref images) = saved_user_images {
                            if let Some(last_user) = llm_messages
                                .iter_mut()
                                .rev()
                                .find(|m| matches!(m.role, MessageRole::User))
                            {
                                let text = last_user.content.as_text();
                                let mut blocks: Vec<ContentBlock> = images
                                    .iter()
                                    .cloned()
                                    .map(|src| ContentBlock::Image { source: src })
                                    .collect();
                                blocks.push(ContentBlock::Text { text });
                                last_user.content = MessageContent::Blocks(blocks);
                                log::info!(
                                    "[Agent] Re-injected {} image(s) after pre-turn compression",
                                    images.len()
                                );
                            }
                        }

                        session_manager
                            .update_messages(&session.id, &session.messages)
                            .map_err(|e| {
                                format!(
                                    "Failed to update session after pre-turn compression: {}",
                                    e
                                )
                            })?;

                        log::info!(
                            "[Agent] Pre-turn: rebuilt {} LLM messages after compression",
                            llm_messages.len()
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "[Agent] Pre-turn LLM compression failed: {}, falling back to hard truncation",
                            e
                        );
                        let hard_truncated = CompressionService::hard_truncate_history(
                            &session.messages,
                            compression_service.config.context_window_tokens,
                        );
                        log::warn!(
                            "[Agent] Hard truncation: {} → {} messages",
                            session.messages.len(),
                            hard_truncated.len()
                        );
                        session.messages = hard_truncated;
                        if let Some(ref ct) = context_tokens {
                            ct.store(0, Ordering::SeqCst);
                        }

                        llm_messages = rebuild_llm_messages_from_session(&session.messages);
                        sanitize_llm_messages(&mut llm_messages);
                        reinject_context_after_compression(
                            &mut llm_messages,
                            session_workdir.as_deref(),
                            &saved_contextual_messages,
                        );

                        if let Some(ref images) = saved_user_images {
                            if let Some(last_user) = llm_messages
                                .iter_mut()
                                .rev()
                                .find(|m| matches!(m.role, MessageRole::User))
                            {
                                let text = last_user.content.as_text();
                                let mut blocks: Vec<ContentBlock> = images
                                    .iter()
                                    .cloned()
                                    .map(|src| ContentBlock::Image { source: src })
                                    .collect();
                                blocks.push(ContentBlock::Text { text });
                                last_user.content = MessageContent::Blocks(blocks);
                                log::info!(
                                    "[Agent] Re-injected {} image(s) after hard truncation",
                                    images.len()
                                );
                            }
                        }

                        session_manager
                            .update_messages(&session.id, &session.messages)
                            .map_err(|e| {
                                format!("Failed to update session after hard truncation: {}", e)
                            })?;
                    }
                }
            }
            if let Some(ref flag) = thinking_flag {
                flag.store(1, Ordering::SeqCst);
            }
        }
    }

    // ReAct loop
    let mut iteration = 0;
    let mut final_response = String::new();
    let mut intermediate_messages = Vec::new();
    let mut accumulated_usage = Usage::zero();

    while iteration < MAX_ITERATIONS {
        // Check abort flag before each iteration
        if let Some(ref flag) = abort_flag {
            if flag.load(Ordering::SeqCst) {
                log::info!(
                    "[Agent] Abort flag detected at start of iteration {}",
                    iteration + 1
                );
                final_response = "Agent execution was aborted by user.".to_string();
                break;
            }
        }

        iteration += 1;
        log::info!("[Agent] Iteration {}/{}", iteration, MAX_ITERATIONS);
        log::info!("[Agent] Sending {} messages to LLM", llm_messages.len());

        // Call LLM with accumulated messages (proper content blocks).
        // On context-overflow errors, attempt emergency compression and retry once.
        let llm_call = match api_format {
            crate::llm_profile::ApiFormat::OpenAI => {
                llm_client
                    .chat_openai(
                        model,
                        system_prompt,
                        &llm_messages,
                        &llm_tools,
                        temperature,
                        max_tokens,
                        stream_callback.as_ref(),
                    )
                    .await
            }
            crate::llm_profile::ApiFormat::Gemini => {
                llm_client
                    .chat_gemini(
                        model,
                        system_prompt,
                        &llm_messages,
                        &llm_tools,
                        temperature,
                        max_tokens,
                        supports_image_output,
                        stream_callback.as_ref(),
                    )
                    .await
            }
            _ => {
                llm_client
                    .chat_anthropic(
                        model,
                        system_prompt,
                        &llm_messages,
                        &llm_tools,
                        temperature,
                        max_tokens,
                        stream_callback.as_ref(),
                        enable_thinking,
                    )
                    .await
            }
        };

        let response = match llm_call {
            Ok(resp) => resp,
            Err(e) => {
                // Determine if this error is likely caused by context overflow.
                // Two cases:
                //   1. Error message explicitly mentions token/context limits
                //   2. Generic server error (500) AND our estimated context is large (>50% of window)
                let err_lower = e.to_lowercase();
                let is_context_error = err_lower.contains("context length")
                    || err_lower.contains("too many tokens")
                    || err_lower.contains("maximum context")
                    || err_lower.contains("token limit")
                    || err_lower.contains("request too large")
                    || err_lower.contains("content_too_large")
                    || err_lower.contains("max_tokens");

                let estimated_tokens = estimate_llm_messages_tokens(&llm_messages)
                    + (system_prompt.len() / CHARS_PER_TOKEN_ESTIMATE) as u32;
                let context_is_large = estimated_tokens
                    > (compression_service.config.context_window_tokens as f64 * 0.5) as u32;

                let is_generic_server_error =
                    err_lower.contains("500") || err_lower.contains("internal server error");

                let should_compress =
                    is_context_error || (is_generic_server_error && context_is_large);

                if should_compress {
                    // Emergency compression needed — try session memory first
                    log::warn!(
                        "[Agent] LLM call failed ({}), estimated ~{} tokens (~{}% of window). Attempting emergency compression.",
                        e,
                        estimated_tokens,
                        (estimated_tokens as f64
                            / compression_service.config.context_window_tokens as f64
                            * 100.0) as u32
                    );
                    if let Some(ref flag) = thinking_flag {
                        flag.store(2, Ordering::SeqCst);
                    }

                    // Wait for in-flight extraction (up to 10s)
                    {
                        let handle = extraction_handle.lock().unwrap().take();
                        if let Some(h) = handle {
                            let _ =
                                tokio::time::timeout(std::time::Duration::from_secs(10), h).await;
                        }
                    }

                    let memory_markdown = session_memory
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map(crate::session_memory::render_as_markdown);

                    let has_compress_path = if let Some(ref md) = memory_markdown {
                        log::info!(
                            "[Agent] Emergency: using session memory for zero-cost compression"
                        );
                        let compressed =
                            compression_service.compress_with_session_memory(md, &session.messages);
                        session.messages = compressed;
                        true
                    } else if let Some(client) = compress_client {
                        // No session memory — try LLM-based compression, fall back to hard truncation
                        match compression_service
                            .compress_history(client, &session.messages)
                            .await
                        {
                            Ok(compressed) => {
                                log::info!(
                                    "[Agent] Emergency compression: {} → {} messages",
                                    session.messages.len(),
                                    compressed.len()
                                );
                                session.messages = compressed;
                            }
                            Err(ce) => {
                                log::warn!(
                                    "[Agent] Emergency LLM compression failed: {}, using hard truncation",
                                    ce
                                );
                                session.messages = CompressionService::hard_truncate_history(
                                    &session.messages,
                                    compression_service.config.context_window_tokens,
                                );
                            }
                        }
                        true
                    } else {
                        false
                    };

                    if has_compress_path {
                        // Persist compressed state so future invocations don't hit the same wall
                        if let Err(pe) =
                            session_manager.update_messages(&session.id, &session.messages)
                        {
                            log::error!("[Agent] Failed to persist emergency compression: {}", pe);
                        }

                        // Rebuild LLM messages from compressed session
                        llm_messages = rebuild_llm_messages_from_session(&session.messages);
                        sanitize_llm_messages(&mut llm_messages);
                        reinject_context_after_compression(
                            &mut llm_messages,
                            session_workdir.as_deref(),
                            &saved_contextual_messages,
                        );

                        if let Some(ref ct) = context_tokens {
                            ct.store(0, Ordering::SeqCst);
                        }
                        if let Some(ref flag) = thinking_flag {
                            flag.store(1, Ordering::SeqCst);
                        }

                        // Retry the LLM call once after compression
                        log::info!(
                            "[Agent] Retrying LLM call after emergency compression ({} messages)",
                            llm_messages.len()
                        );
                        match api_format {
                            crate::llm_profile::ApiFormat::OpenAI => {
                                llm_client
                                    .chat_openai(
                                        model,
                                        system_prompt,
                                        &llm_messages,
                                        &llm_tools,
                                        temperature,
                                        max_tokens,
                                        stream_callback.as_ref(),
                                    )
                                    .await?
                            }
                            crate::llm_profile::ApiFormat::Gemini => {
                                llm_client
                                    .chat_gemini(
                                        model,
                                        system_prompt,
                                        &llm_messages,
                                        &llm_tools,
                                        temperature,
                                        max_tokens,
                                        supports_image_output,
                                        stream_callback.as_ref(),
                                    )
                                    .await?
                            }
                            _ => {
                                llm_client
                                    .chat_anthropic(
                                        model,
                                        system_prompt,
                                        &llm_messages,
                                        &llm_tools,
                                        temperature,
                                        max_tokens,
                                        stream_callback.as_ref(),
                                        enable_thinking,
                                    )
                                    .await?
                            }
                        }
                    } else {
                        // No session memory and no compression client — propagate original error
                        return Err(e);
                    }
                } else {
                    // Not a context-overflow issue, propagate error directly
                    return Err(e);
                }
            }
        };

        log::info!(
            "[Agent] LLM response: {} content blocks, stop_reason: {}, tokens: in={} out={} cache_create={} cache_read={}",
            response.content.len(),
            response.stop_reason,
            response.usage.input_tokens,
            response.usage.output_tokens,
            response.usage.cache_creation_input_tokens,
            response.usage.cache_read_input_tokens
        );

        // Accumulate token usage for billing
        accumulated_usage += response.usage.clone();

        // Total input = input_tokens + cache_creation + cache_read (actual context window usage)
        let current_input_tokens = response.usage.total_input_tokens();

        // Update context_tokens for heartbeat reporting
        if let Some(ref ct) = context_tokens {
            ct.store(current_input_tokens, Ordering::SeqCst);
            log::info!(
                "[Agent] Updated context_tokens={} for heartbeat (in={} + cache_create={} + cache_read={})",
                current_input_tokens,
                response.usage.input_tokens,
                response.usage.cache_creation_input_tokens,
                response.usage.cache_read_input_tokens
            );
        }

        // Collect all content blocks from response
        let mut has_tool_calls = false;
        let mut response_text = String::new();
        let mut assistant_blocks: Vec<ContentBlock> = vec![];
        let mut tool_uses: Vec<crate::llm::ToolUse> = vec![];

        for content in &response.content {
            match content {
                LLMResponseType::Text { text } => {
                    log::info!("[Agent] Content block: Text (length: {})", text.len());
                    response_text.push_str(text);
                    assistant_blocks.push(ContentBlock::Text { text: text.clone() });
                }
                LLMResponseType::Thinking {
                    thinking,
                    signature,
                } => {
                    log::info!(
                        "[Agent] Content block: Thinking (length: {})",
                        thinking.len()
                    );
                    assistant_blocks.push(ContentBlock::Thinking {
                        thinking: thinking.clone(),
                        signature: signature.clone(),
                    });

                    // Send ACP thinking message so frontend can display it in history.
                    // This is always sent (even with streaming) because stream deltas are
                    // only a real-time preview — this full message is the permanent record.
                    // ACP schema: { type: "thinking", text: "..." }
                    if let Some(ref sender) = acp_sender {
                        let acp_data = json!({
                            "type": "thinking",
                            "text": thinking,
                            "id": uuid::Uuid::new_v4().to_string()
                        });
                        log::info!("[Agent] ACP thinking: length={}", thinking.len());
                        sender(acp_data).await;
                    }
                }
                LLMResponseType::ToolUse { tool_use } => {
                    has_tool_calls = true;
                    log::info!(
                        "[Agent] Content block: ToolUse - {} with input: {}",
                        tool_use.name,
                        tool_use.input
                    );
                    tool_uses.push(tool_use.clone());
                    assistant_blocks.push(ContentBlock::ToolUse {
                        id: tool_use.id.clone(),
                        name: tool_use.name.clone(),
                        input: tool_use.input.clone(),
                        gemini_thought_signature: tool_use.gemini_thought_signature.clone(),
                    });
                }
                LLMResponseType::Image { media_type, data } => {
                    log::info!(
                        "[Agent] Content block: Image ({}, {} bytes base64)",
                        media_type,
                        data.len()
                    );

                    // Save image to local file
                    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
                    let ext = match media_type.as_str() {
                        "image/jpeg" => "jpg",
                        "image/webp" => "webp",
                        "image/gif" => "gif",
                        _ => "png",
                    };
                    let filename = format!(
                        "generated_{}_{}.{}",
                        timestamp,
                        uuid::Uuid::new_v4()
                            .to_string()
                            .split('-')
                            .next()
                            .unwrap_or("0"),
                        ext
                    );
                    let save_dir = session_workdir
                        .as_ref()
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    let filepath = save_dir.join(&filename);

                    // Decode base64 and save locally
                    use base64::Engine;
                    if let Ok(image_bytes) = base64::engine::general_purpose::STANDARD.decode(&data)
                    {
                        if let Err(e) = tokio::fs::write(&filepath, &image_bytes).await {
                            log::error!("[Agent] Failed to save generated image: {}", e);
                        } else {
                            log::info!("[Agent] Saved generated image: {}", filepath.display());
                        }
                    }

                    // Send ACP image message with base64 data for frontend rendering
                    if let Some(ref sender) = acp_sender {
                        log::info!(
                            "[Agent] Sending ACP image ({} bytes base64): {}",
                            data.len(),
                            filepath.display()
                        );
                        let acp_data = json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": media_type,
                                "data": data,
                            },
                            "id": uuid::Uuid::new_v4().to_string()
                        });
                        sender(acp_data).await;
                        log::info!("[Agent] ACP image sent successfully");
                    }

                    // Add to assistant blocks for LLM conversation history
                    assistant_blocks.push(ContentBlock::Image {
                        source: crate::llm::ImageSource {
                            source_type: "base64".to_string(),
                            media_type: media_type.clone(),
                            data: data.clone(),
                        },
                    });
                }
            }
        }

        // Clear frontend streaming preview — content blocks are now finalized as ACP messages.
        // Without this, StreamingBubble and finalized thinking message coexist as duplicates.
        if let Some(ref cb) = stream_callback {
            cb(json!({ "type": "stream-end" })).await;
        }

        // If no tool calls, this is the final response.
        // Guardrail: some providers occasionally return an empty text response.
        // In that case, force one extra no-tool text-only summary call.
        if !has_tool_calls {
            if response_text.trim().is_empty() {
                log::warn!(
                    "[Agent] Empty final text response detected (no tool calls). Requesting one fallback text response."
                );

                llm_messages.push(Message::user(
                    "Your previous response contained no user-visible text. Reply now with a concise final answer in plain text only."
                ));

                let fallback_response = match api_format {
                    crate::llm_profile::ApiFormat::OpenAI => {
                        llm_client
                            .chat_openai(
                                model,
                                system_prompt,
                                &llm_messages,
                                &[],
                                temperature,
                                max_tokens,
                                stream_callback.as_ref(),
                            )
                            .await
                    }
                    crate::llm_profile::ApiFormat::Gemini => {
                        llm_client
                            .chat_gemini(
                                model,
                                system_prompt,
                                &llm_messages,
                                &[],
                                temperature,
                                max_tokens,
                                false, // fallback text response doesn't need image output
                                stream_callback.as_ref(),
                            )
                            .await
                    }
                    _ => {
                        llm_client
                            .chat_anthropic(
                                model,
                                system_prompt,
                                &llm_messages,
                                &[],
                                temperature,
                                max_tokens,
                                stream_callback.as_ref(),
                                false, // force plain text fallback without thinking
                            )
                            .await
                    }
                };

                if let Ok(resp) = fallback_response {
                    for content in &resp.content {
                        if let LLMResponseType::Text { text } = content {
                            response_text.push_str(text);
                        }
                    }
                } else if let Err(e) = fallback_response {
                    log::warn!("[Agent] Fallback text response failed: {}", e);
                }

                if response_text.trim().is_empty() {
                    response_text =
                        "No user-visible response was generated. Please retry the request."
                            .to_string();
                }
            }

            log::info!("[Agent] Final response (length: {})", response_text.len());

            // Add to LLM messages
            llm_messages.push(Message::assistant(&response_text));

            // Persist to session
            session.messages.push(SessionMessage {
                role: "assistant".to_string(),
                content: response_text.clone(),
                timestamp: Utc::now().to_rfc3339(),
                local_id: None,
            });

            final_response = response_text;
            break;
        }

        // --- Tool execution phase ---

        // Add assistant message with all content blocks to LLM messages
        llm_messages.push(Message {
            role: MessageRole::Assistant,
            content: MessageContent::blocks(assistant_blocks.clone()),
        });

        // Persist assistant message (structured format)
        session.messages.push(SessionMessage {
            role: "assistant".to_string(),
            content: serialize_content_for_session(&MessageContent::blocks(assistant_blocks)),
            timestamp: Utc::now().to_rfc3339(),
            local_id: None,
        });

        // Execute ALL tools and collect results
        // When LLM returns multiple tool_use blocks, execute them in parallel
        let mut tool_result_blocks: Vec<ContentBlock> = vec![];
        let mut persist_blocks: Vec<ContentBlock> = vec![];
        let mut aborted = false;

        // --- Phase 1: Send ACP tool-calls and check permissions (sequential) ---
        // Permission checking is interactive (user may approve/deny each tool),
        // so it stays sequential. Tool execution is parallelized in Phase 2.
        // Each entry: (tool index, approved, optional pre-filled result for denied/aborted)
        let mut tool_perm: Vec<(usize, bool, Option<ContentBlock>)> = vec![];

        // Build set of valid tool names for this session
        let valid_tool_names: std::collections::HashSet<&str> =
            tools.iter().map(|t| t.name.as_str()).collect();

        for (tool_idx, tool_use) in tool_uses.iter().enumerate() {
            // Guard: reject tool calls not in the filtered tool list.
            // LLMs may hallucinate tool calls based on system prompt text even when
            // the function definition was excluded from the tools list.
            if !valid_tool_names.contains(tool_use.name.as_str()) {
                log::warn!(
                    "[Agent] Rejected hallucinated tool call '{}' — not in session's tool list",
                    tool_use.name
                );
                tool_perm.push((
                    tool_idx,
                    false,
                    Some(ContentBlock::ToolResult {
                        tool_use_id: tool_use.id.clone(),
                        content: format!(
                            "Error: tool '{}' is not available in this session. Use only the tools provided.",
                            tool_use.name
                        ),
                        is_error: true,
                    }),
                ));
                continue;
            }

            // Check abort flag before each tool
            if let Some(ref flag) = abort_flag {
                if flag.load(Ordering::SeqCst) {
                    log::info!("[Agent] Abort flag detected before tool: {}", tool_use.name);
                    aborted = true;
                    tool_perm.push((
                        tool_idx,
                        false,
                        Some(ContentBlock::ToolResult {
                            tool_use_id: tool_use.id.clone(),
                            content: "Agent execution aborted by user".to_string(),
                            is_error: true,
                        }),
                    ));
                    break;
                }
            }

            // Send intermediate message
            let execution_msg = if tool_use.name == "zsh" {
                format!(
                    "执行命令: {}",
                    tool_use
                        .input
                        .get("command")
                        .and_then(|c| c.as_str())
                        .unwrap_or("unknown")
                )
            } else {
                format!("调用工具: {}", tool_use.name)
            };
            intermediate_messages.push(execution_msg);

            // Send ACP tool-call message
            if let Some(ref sender) = acp_sender {
                let (ui_name, ui_input) =
                    normalize_tool_call_for_ui(&tool_use.name, &tool_use.input);
                let acp_data = json!({
                    "type": "tool-call",
                    "callId": tool_use.id,
                    "name": ui_name,
                    "input": ui_input,
                    "id": uuid::Uuid::new_v4().to_string()
                });
                log::info!("[Agent] ACP tool-call: name={}", tool_use.name);
                sender(acp_data).await;
            }

            // Permission check (if permission_checker is provided)
            if let Some(ref checker) = permission_checker {
                let check_result = checker(
                    tool_use.name.clone(),
                    tool_use.id.clone(),
                    tool_use.input.clone(),
                )
                .await;

                match check_result {
                    PermissionCheckResult::Allowed => {
                        log::info!("[Agent] Permission granted for tool: {}", tool_use.name);
                        tool_perm.push((tool_idx, true, None));
                    }
                    PermissionCheckResult::Denied(reason) => {
                        log::info!(
                            "[Agent] Permission denied for tool {}: {}",
                            tool_use.name,
                            reason
                        );
                        let denied_content = format!("Permission denied: {}", reason);

                        // Send ACP tool-result with denied message
                        if let Some(ref sender) = acp_sender {
                            let acp_data = json!({
                                "type": "tool-result",
                                "callId": tool_use.id,
                                "output": denied_content,
                                "isError": true,
                                "id": uuid::Uuid::new_v4().to_string()
                            });
                            sender(acp_data).await;
                        }

                        intermediate_messages.push(denied_content.clone());
                        tool_perm.push((
                            tool_idx,
                            false,
                            Some(ContentBlock::ToolResult {
                                tool_use_id: tool_use.id.clone(),
                                content: denied_content,
                                is_error: true,
                            }),
                        ));
                    }
                    PermissionCheckResult::Aborted => {
                        log::info!("[Agent] Agent aborted by user at tool: {}", tool_use.name);
                        aborted = true;
                        tool_perm.push((
                            tool_idx,
                            false,
                            Some(ContentBlock::ToolResult {
                                tool_use_id: tool_use.id.clone(),
                                content: "Agent execution aborted by user".to_string(),
                                is_error: true,
                            }),
                        ));
                        break;
                    }
                }
            } else {
                // No permission checker, auto-approve
                tool_perm.push((tool_idx, true, None));
            }
        }

        // --- Phase 2: Execute approved tools (smart partitioned: concurrent + serial) ---
        let approved_indices: Vec<usize> = tool_perm
            .iter()
            .filter_map(|(i, approved, _)| if *approved { Some(*i) } else { None })
            .collect();

        // Partition approved tools into concurrency-safe and serial groups
        let mut concurrent_indices = Vec::new();
        let mut serial_indices = Vec::new();

        {
            let registry = crate::hooks::tool_registry_handle()
                .ok_or_else(|| "ToolRegistry not available: hook not installed".to_string())?;
            let reg = registry.read().await;
            for &idx in &approved_indices {
                let tool_name = &tool_uses[idx].name;
                let is_safe = reg
                    .get_config(tool_name)
                    .map(|c| c.is_concurrency_safe)
                    .unwrap_or(false);
                if is_safe {
                    concurrent_indices.push(idx);
                } else {
                    serial_indices.push(idx);
                }
            }
        }

        log::info!(
            "Tool execution partitioned: {} concurrent, {} serial",
            concurrent_indices.len(),
            serial_indices.len()
        );

        let mut all_results: Vec<(usize, String, bool)> = Vec::new();

        // Step A: Execute all concurrency-safe tools in parallel
        if concurrent_indices.len() > 1 {
            log::info!(
                "[Agent] Executing {} concurrency-safe tools in parallel",
                concurrent_indices.len()
            );
            let hooks_ref = &hooks_manager;
            let sandbox_policy_ref = session_sandbox_policy.as_ref();
            let futures_vec: Vec<_> = concurrent_indices
                .iter()
                .map(|&i| {
                    let tool_use = &tool_uses[i];
                    let ac = agent_configs.as_deref();
                    let sc = sub_agent_ctx.as_ref();
                    let sid = session.id.as_str();
                    let workdir = session_workdir.clone();
                    let pid = session_profile_id.clone();
                    let persona = persona_id;
                    let p_workdir = persona_workdir;
                    let cid = tool_use.id.clone();
                    async move {
                        let (content, is_error) = match execute_tool(
                            &tool_use.name,
                            &tool_use.input,
                            sid,
                            workdir.as_deref(),
                            pid.as_deref(),
                            ac,
                            sc,
                            persona,
                            p_workdir,
                            supports_vision,
                            Some(&cid),
                            Some(hooks_ref),
                            sandbox_policy_ref,
                        )
                        .await
                        {
                            Ok(output) => (output, false),
                            Err(error) => {
                                log::warn!("[Agent] Tool {} failed: {}", tool_use.name, error);
                                (format!("Error: {}\n\n[Reminder: 如果你的 system prompt 中有自进化/经验记录规则，请将此错误及解决方案保存到记忆中]", error), true)
                            }
                        };
                        (i, content, is_error)
                    }
                })
                .collect();
            all_results.extend(join_all(futures_vec).await);
        } else if concurrent_indices.len() == 1 {
            let i = concurrent_indices[0];
            let tool_use = &tool_uses[i];
            let (content, is_error) = match execute_tool(
                &tool_use.name,
                &tool_use.input,
                &session.id,
                session_workdir.as_deref(),
                session_profile_id.as_deref(),
                agent_configs.as_deref(),
                sub_agent_ctx.as_ref(),
                persona_id,
                persona_workdir,
                supports_vision,
                Some(&tool_use.id),
                Some(&hooks_manager),
                session_sandbox_policy.as_ref(),
            )
            .await
            {
                Ok(output) => (output, false),
                Err(error) => {
                    log::warn!("[Agent] Tool {} failed: {}", tool_use.name, error);
                    (
                        format!(
                            "Error: {}\n\n[Reminder: 如果你的 system prompt 中有自进化/经验记录规则，请将此错误及解决方案保存到记忆中]",
                            error
                        ),
                        true,
                    )
                }
            };
            all_results.push((i, content, is_error));
        }

        // Step B: Execute all non-concurrency-safe tools serially
        for &i in &serial_indices {
            let tool_use = &tool_uses[i];
            let (content, is_error) = match execute_tool(
                &tool_use.name,
                &tool_use.input,
                &session.id,
                session_workdir.as_deref(),
                session_profile_id.as_deref(),
                agent_configs.as_deref(),
                sub_agent_ctx.as_ref(),
                persona_id,
                persona_workdir,
                supports_vision,
                Some(&tool_use.id),
                Some(&hooks_manager),
                session_sandbox_policy.as_ref(),
            )
            .await
            {
                Ok(output) => (output, false),
                Err(error) => {
                    log::warn!("[Agent] Tool {} failed: {}", tool_use.name, error);
                    (
                        format!(
                            "Error: {}\n\n[Reminder: 如果你的 system prompt 中有自进化/经验记录规则，请将此错误及解决方案保存到记忆中]",
                            error
                        ),
                        true,
                    )
                }
            };
            all_results.push((i, content, is_error));
        }

        let execution_results = all_results;

        // Build execution result map for ordered assembly
        let mut exec_map: std::collections::HashMap<usize, (String, bool)> =
            std::collections::HashMap::new();
        for (i, content, is_error) in execution_results {
            exec_map.insert(i, (content, is_error));
        }

        // --- Phase 3: Assemble tool_result_blocks in original order + send ACP tool-results ---
        for (i, approved, pre_result) in &tool_perm {
            if *approved {
                if let Some((result_content, is_error)) = exec_map.remove(i) {
                    let tool_use = &tool_uses[*i];

                    // Send ACP tool-result message (panic-safe: always sends something)
                    if let Some(ref sender) = acp_sender {
                        let display_output =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                if result_content.len() > 2000 {
                                    let truncate_at = result_content
                                        .char_indices()
                                        .take_while(|(idx, _)| *idx <= 2000)
                                        .last()
                                        .map(|(idx, c)| idx + c.len_utf8())
                                        .unwrap_or(0);
                                    format!(
                                        "{}...\n[truncated, total {} chars]",
                                        &result_content[..truncate_at],
                                        result_content.chars().count()
                                    )
                                } else {
                                    result_content.clone()
                                }
                            }))
                            .unwrap_or_else(|_| {
                                log::error!(
                                    "[Agent] Panic during tool-result formatting for callId={}",
                                    tool_use.id
                                );
                                format!(
                                    "[Output formatting error, raw length: {} bytes]",
                                    result_content.len()
                                )
                            });
                        let acp_data = json!({
                            "type": "tool-result",
                            "callId": tool_use.id,
                            "output": display_output,
                            "isError": is_error,
                            "id": uuid::Uuid::new_v4().to_string()
                        });
                        log::info!("[Agent] ACP tool-result: callId={}", tool_use.id);
                        sender(acp_data).await;
                    }

                    // If the model activated a prompt-only skill, update session skill state + notify UI.
                    // Bug fix: previously checked "activate_skill" (non-existent tool name).
                    // Now checks "skill" (unified) + legacy "skill_context", with operation=activate.
                    if !is_error && (tool_use.name == "skill" || tool_use.name == "skill_context") {
                        let op = tool_use.input.get("operation").and_then(|v| v.as_str());
                        if op == Some("activate") {
                            if let Some(ref handler) = skill_activation_handler {
                                if let Some(skill_id) =
                                    tool_use.input.get("id").and_then(|v| v.as_str())
                                {
                                    handler(skill_id.to_string()).await;
                                }
                            }
                        }
                    }

                    // Check for embedded images in tool result JSON
                    let mut images: Vec<crate::llm::ImageSource> = vec![];
                    let mut sanitized_content = result_content.clone();
                    if supports_vision && !is_error {
                        if let Ok(mut result_json) =
                            serde_json::from_str::<serde_json::Value>(&result_content)
                        {
                            if let Some(img_array) =
                                result_json.get("images").and_then(|v| v.as_array())
                            {
                                for img in img_array {
                                    if let (Some(media_type), Some(data)) = (
                                        img.get("media_type").and_then(|v| v.as_str()),
                                        img.get("data").and_then(|v| v.as_str()),
                                    ) {
                                        let source_type = img
                                            .get("type")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("base64");
                                        images.push(crate::llm::ImageSource {
                                            source_type: source_type.to_string(),
                                            media_type: media_type.to_string(),
                                            data: data.to_string(),
                                        });
                                    }
                                }
                            }
                            // Strip images array from text content sent to LLM
                            // (images are injected as ContentBlock::Image, no need to send base64 as text)
                            if !images.is_empty() {
                                result_json.as_object_mut().map(|obj| obj.remove("images"));
                                sanitized_content = serde_json::to_string(&result_json)
                                    .unwrap_or(result_content.clone());
                            }
                        }
                    }

                    intermediate_messages.push(sanitized_content.clone());

                    // LLM gets sanitized content (no base64 text) + separate Image blocks
                    tool_result_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: tool_use.id.clone(),
                        content: sanitized_content.clone(),
                        is_error,
                    });

                    // Session persistence also uses sanitized content (no base64 images).
                    // Frontend renders via image_url (preserved in sanitized content).
                    // This prevents session history from bloating with megabytes of base64.
                    persist_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: tool_use.id.clone(),
                        content: sanitized_content.clone(),
                        is_error,
                    });

                    // Inject image blocks after the tool result for LLM only (not persisted)
                    for source in images {
                        log::info!(
                            "[Agent] Injecting image from tool '{}' result ({}, {} bytes)",
                            tool_use.name,
                            source.media_type,
                            source.data.len()
                        );
                        tool_result_blocks.push(ContentBlock::Image { source });
                    }
                }
            } else if let Some(block) = pre_result {
                tool_result_blocks.push(block.clone());
                persist_blocks.push(block.clone());
            }
        }

        // If aborted, break out of the main ReAct loop
        if aborted {
            // Add tool results collected so far
            if !tool_result_blocks.is_empty() {
                llm_messages.push(Message {
                    role: MessageRole::User,
                    content: MessageContent::blocks(tool_result_blocks),
                });
                // Persist sanitized content (no base64, image_url preserved)
                session.messages.push(SessionMessage {
                    role: "user".to_string(),
                    content: serialize_content_for_session(&MessageContent::blocks(persist_blocks)),
                    timestamp: Utc::now().to_rfc3339(),
                    local_id: None,
                });
            }
            final_response = "Agent execution was aborted by user.".to_string();
            break;
        }

        // Add user message with all tool results to LLM messages (sanitized: no base64 text, has Image blocks)
        llm_messages.push(Message {
            role: MessageRole::User,
            content: MessageContent::blocks(tool_result_blocks),
        });

        // Persist tool results with sanitized content (no base64 images, image_url preserved).
        // Frontend uses image_url for rendering. strip_images_from_tool_results() on rebuild is a safety net.
        session.messages.push(SessionMessage {
            role: "user".to_string(),
            content: serialize_content_for_session(&MessageContent::blocks(persist_blocks)),
            timestamp: Utc::now().to_rfc3339(),
            local_id: None,
        });

        // Update session messages
        session_manager
            .update_messages(&session.id, &session.messages)
            .map_err(|e| format!("Failed to update session: {}", e))?;

        // --- Session Memory extraction trigger ---
        {
            let tool_count = tool_uses.len();
            extraction_tracker.record_tool_calls(tool_count as u32);
            if extraction_tracker.should_extract(current_input_tokens) {
                let mut handle_guard = extraction_handle.lock().unwrap();
                if handle_guard.as_ref().map_or(true, |h| h.is_finished()) {
                    let sm_arc = session_memory.clone();
                    let messages_snapshot = session.messages.clone();
                    let delta_start = extraction_tracker.last_message_index();
                    let session_id_clone = session.id.clone();
                    let db_path_clone = session_manager.db_path().to_path_buf();
                    let existing = sm_arc.lock().unwrap().clone();
                    let is_proxy_clone = use_proxy;
                    let extraction_client = if base_url.starts_with("https://openrouter.ai") {
                        LLMClient::with_openrouter(api_key.to_string(), base_url.to_string())
                    } else if use_proxy {
                        LLMClient::with_proxy_and_machine_id(
                            api_key.to_string(),
                            base_url.to_string(),
                        )
                    } else {
                        LLMClient::with_base_url(api_key.to_string(), base_url.to_string())
                    };

                    *handle_guard = Some(tokio::spawn(async move {
                        match crate::session_memory::run_extraction_with_fallback(
                            &extraction_client,
                            is_proxy_clone,
                            None,
                            &messages_snapshot,
                            existing.as_ref(),
                            delta_start,
                        )
                        .await
                        {
                            Ok(new_memory) => {
                                log::info!(
                                    "[SessionMemory] Extraction #{} succeeded (title: {})",
                                    new_memory.meta.extraction_count,
                                    new_memory.title
                                );
                                *sm_arc.lock().unwrap() = Some(new_memory.clone());
                                let sm_manager = AgentSessionManager::new(db_path_clone);
                                if let Err(e) = sm_manager.update_context_field(
                                    &session_id_clone,
                                    "session_memory",
                                    serde_json::to_value(&new_memory).unwrap(),
                                ) {
                                    log::warn!("[SessionMemory] Failed to persist: {}", e);
                                }
                            }
                            Err(e) => {
                                log::warn!("[SessionMemory] Extraction failed: {}", e);
                            }
                        }
                    }));

                    extraction_tracker.mark_extracted(current_input_tokens, session.messages.len());
                }
            }
        }

        // Check message queue for new user messages that arrived during tool execution
        if let Some(ref queue) = message_queue {
            let queued = queue.pop_all(&session.id);
            if !queued.is_empty() {
                let combined = queued
                    .iter()
                    .map(|m| m.content.clone())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                log::info!(
                    "[Agent] Injecting {} queued user message(s) into ReAct loop for session {}",
                    queued.len(),
                    session.id
                );

                // Add to LLM messages
                llm_messages.push(Message {
                    role: MessageRole::User,
                    content: MessageContent::text(combined.clone()),
                });

                // Persist to session
                session.messages.push(SessionMessage {
                    role: "user".to_string(),
                    content: combined,
                    timestamp: Utc::now().to_rfc3339(),
                    local_id: None,
                });

                session_manager
                    .update_messages(&session.id, &session.messages)
                    .map_err(|e| {
                        format!("Failed to update session after queue injection: {}", e)
                    })?;
            }
        }

        // Token-based compression: if LLM input tokens exceeded threshold, compress before next iteration.
        // This follows Gemini CLI's approach: use actual API token counts, not message count heuristics.
        if compression_service.needs_compression_by_tokens(current_input_tokens) {
            if let Some(ref flag) = thinking_flag {
                flag.store(2, Ordering::SeqCst);
            }

            // Wait for in-flight session memory extraction (up to 10s)
            {
                let handle = extraction_handle.lock().unwrap().take();
                if let Some(h) = handle {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), h).await;
                }
            }

            // Try session memory zero-cost compression first
            let memory_markdown = session_memory
                .lock()
                .unwrap()
                .as_ref()
                .map(crate::session_memory::render_as_markdown);

            if let Some(ref md) = memory_markdown {
                log::info!("[Agent] Mid-loop: using session memory for zero-cost compression");
                let compressed =
                    compression_service.compress_with_session_memory(md, &session.messages);
                log::info!(
                    "[Agent] Mid-loop session memory compression: {} → {} messages (triggered at {} tokens)",
                    session.messages.len(),
                    compressed.len(),
                    current_input_tokens
                );
                session.messages = compressed;
                if let Some(ref ct) = context_tokens {
                    ct.store(0, Ordering::SeqCst);
                }

                llm_messages = rebuild_llm_messages_from_session(&session.messages);
                sanitize_llm_messages(&mut llm_messages);
                reinject_context_after_compression(
                    &mut llm_messages,
                    session_workdir.as_deref(),
                    &saved_contextual_messages,
                );

                session_manager
                    .update_messages(&session.id, &session.messages)
                    .map_err(|e| {
                        format!(
                            "Failed to update session after mid-loop session memory compression: {}",
                            e
                        )
                    })?;

                log::info!(
                    "[Agent] Rebuilt {} LLM messages after mid-loop session memory compression",
                    llm_messages.len()
                );
            } else if let Some(client) = compress_client {
                // No session memory — fall back to LLM-based compression
                match compression_service
                    .compress_history(client, &session.messages)
                    .await
                {
                    Ok(compressed) => {
                        log::info!(
                            "[Agent] Mid-loop compression: {} → {} messages (triggered at {} tokens)",
                            session.messages.len(),
                            compressed.len(),
                            current_input_tokens
                        );
                        session.messages = compressed;
                        if let Some(ref ct) = context_tokens {
                            ct.store(0, Ordering::SeqCst);
                        }

                        llm_messages = rebuild_llm_messages_from_session(&session.messages);
                        sanitize_llm_messages(&mut llm_messages);
                        reinject_context_after_compression(
                            &mut llm_messages,
                            session_workdir.as_deref(),
                            &saved_contextual_messages,
                        );

                        session_manager
                            .update_messages(&session.id, &session.messages)
                            .map_err(|e| {
                                format!("Failed to update session after compression: {}", e)
                            })?;

                        log::info!(
                            "[Agent] Rebuilt {} LLM messages after mid-loop compression",
                            llm_messages.len()
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "[Agent] Mid-loop LLM compression failed: {}, falling back to hard truncation",
                            e
                        );
                        let hard_truncated = CompressionService::hard_truncate_history(
                            &session.messages,
                            compression_service.config.context_window_tokens,
                        );
                        log::warn!(
                            "[Agent] Mid-loop hard truncation: {} → {} messages",
                            session.messages.len(),
                            hard_truncated.len()
                        );
                        session.messages = hard_truncated;
                        if let Some(ref ct) = context_tokens {
                            ct.store(0, Ordering::SeqCst);
                        }

                        llm_messages = rebuild_llm_messages_from_session(&session.messages);
                        sanitize_llm_messages(&mut llm_messages);
                        reinject_context_after_compression(
                            &mut llm_messages,
                            session_workdir.as_deref(),
                            &saved_contextual_messages,
                        );

                        session_manager
                            .update_messages(&session.id, &session.messages)
                            .map_err(|e| {
                                format!(
                                    "Failed to update session after mid-loop hard truncation: {}",
                                    e
                                )
                            })?;
                    }
                }
            }
            if let Some(ref flag) = thinking_flag {
                flag.store(1, Ordering::SeqCst);
            }
        }
    }

    // If we hit max iterations without a final response, do one last LLM call
    // without tools to force a text summary of what was accomplished
    if final_response.is_empty() && iteration >= MAX_ITERATIONS {
        log::warn!(
            "[Agent] Reached max iterations ({}), requesting final summary from LLM",
            MAX_ITERATIONS
        );

        // Add a system-like instruction as user message to ask for summary
        llm_messages.push(Message::user(
            "You have reached the maximum number of tool call iterations. Please summarize what you have accomplished so far and what remains to be done, if anything."
        ));

        // Call LLM without tools to force a text response
        let summary_response = match api_format {
            crate::llm_profile::ApiFormat::OpenAI => {
                llm_client
                    .chat_openai(
                        model,
                        system_prompt,
                        &llm_messages,
                        &[],
                        temperature,
                        max_tokens,
                        stream_callback.as_ref(),
                    )
                    .await
            }
            crate::llm_profile::ApiFormat::Gemini => {
                llm_client
                    .chat_gemini(
                        model,
                        system_prompt,
                        &llm_messages,
                        &[],
                        temperature,
                        max_tokens,
                        false, // compression never needs image output
                        stream_callback.as_ref(),
                    )
                    .await
            }
            _ => {
                llm_client
                    .chat_anthropic(
                        model,
                        system_prompt,
                        &llm_messages,
                        &[],
                        temperature,
                        max_tokens,
                        stream_callback.as_ref(),
                        false, // no thinking for summary
                    )
                    .await
            }
        };

        match summary_response {
            Ok(resp) => {
                for content in &resp.content {
                    if let LLMResponseType::Text { text } = content {
                        final_response.push_str(text);
                    }
                }
                if final_response.is_empty() {
                    final_response =
                        "Task reached maximum iterations. Please send another message to continue."
                            .to_string();
                }
                log::info!(
                    "[Agent] Summary response (length: {})",
                    final_response.len()
                );
            }
            Err(e) => {
                log::error!("[Agent] Failed to get summary: {}", e);
                final_response =
                    "Task reached maximum iterations. Please send another message to continue."
                        .to_string();
            }
        }

        // Persist the summary
        session.messages.push(SessionMessage {
            role: "assistant".to_string(),
            content: final_response.clone(),
            timestamp: Utc::now().to_rfc3339(),
            local_id: None,
        });
    }

    // Persist final state (including final_response) before closing
    session_manager
        .update_messages(&session.id, &session.messages)
        .map_err(|e| format!("Failed to persist final session state: {}", e))?;

    // Mark session as closed (completed)
    session_manager
        .close_session(&session.id)
        .map_err(|e| format!("Failed to close session: {}", e))?;

    log::info!(
        "[Agent] Final response: {}",
        final_response.chars().take(200).collect::<String>()
    );

    log::info!(
        "[Agent] Total accumulated usage: input={} output={} total={}",
        accumulated_usage.total_input_tokens(),
        accumulated_usage.output_tokens,
        accumulated_usage.total_tokens()
    );

    Ok(AgentExecutionResult {
        response: final_response,
        session_id: session.id,
        iteration_count: iteration,
        intermediate_messages,
        total_usage: accumulated_usage,
    })
}

/// Execute a tool call with output truncation
/// This wraps execute_tool_raw and applies token budget limits
async fn execute_tool(
    tool_name: &str,
    input: &serde_json::Value,
    session_id: &str,
    session_workdir: Option<&str>,
    profile_id: Option<&str>,
    agent_configs: Option<&[AgentConfig]>,
    sub_agent_ctx: Option<&SubAgentContext>,
    persona_id: Option<&str>,
    persona_workdir: Option<&str>,
    supports_vision: bool,
    call_id: Option<&str>,
    hooks_manager: Option<&crate::tool_hooks::HooksManager>,
    sandbox_policy: Option<&crate::tool_executors::SandboxPolicy>,
) -> Result<String, String> {
    // Pre-hook: check if any hook blocks this tool execution
    if let Some(hooks) = hooks_manager {
        match hooks.run_pre_hooks(tool_name, input).await {
            crate::tool_hooks::HookResult::Block(msg) => {
                return Err(msg);
            }
            crate::tool_hooks::HookResult::Continue => {}
        }
    }

    let result = execute_tool_raw(
        tool_name,
        input,
        session_id,
        session_workdir,
        profile_id,
        agent_configs,
        sub_agent_ctx,
        persona_id,
        persona_workdir,
        supports_vision,
        call_id,
        sandbox_policy,
    )
    .await?;

    // Post-hook: allow hooks to replace the output
    let result = if let Some(hooks) = hooks_manager {
        if let Some(new_output) = hooks.run_post_hooks(tool_name, input, &result).await {
            new_output
        } else {
            result
        }
    } else {
        result
    };

    // Skip truncation for results containing `images` array — these are handled
    // separately by the image extraction logic in the ReAct loop (Phase 3).
    // Truncating JSON with embedded base64 images would break the JSON structure,
    // causing image extraction to fail and the raw base64 to be sent as text to the LLM.
    let has_images = result.contains("\"images\"");
    if has_images {
        log::info!(
            "[Agent] Tool output contains images, skipping truncation ({} chars)",
            result.len()
        );
        return Ok(result);
    }

    // Apply truncation to tool output
    let (truncated_result, was_truncated) = truncate_tool_output(&result, TOOL_OUTPUT_CHAR_BUDGET);

    if was_truncated {
        log::info!(
            "[Agent] Tool output truncated: {} chars → {} chars (~{} → ~{} tokens)",
            result.len(),
            truncated_result.len(),
            result.len() / CHARS_PER_TOKEN_ESTIMATE,
            truncated_result.len() / CHARS_PER_TOKEN_ESTIMATE
        );
    }

    Ok(truncated_result)
}

/// Raw tool execution without truncation
async fn execute_tool_raw(
    tool_name: &str,
    input: &serde_json::Value,
    session_id: &str,
    session_workdir: Option<&str>,
    profile_id: Option<&str>,
    agent_configs: Option<&[AgentConfig]>,
    sub_agent_ctx: Option<&SubAgentContext>,
    persona_id: Option<&str>,
    persona_workdir: Option<&str>,
    supports_vision: bool,
    call_id: Option<&str>,
    sandbox_policy: Option<&crate::tool_executors::SandboxPolicy>,
) -> Result<String, String> {
    // Check if this is a sub-agent tool call (agent_xxx prefix)
    if let Some(agent_id) = tool_name.strip_prefix("agent_") {
        let configs = agent_configs
            .ok_or_else(|| "Sub-agent execution not available in this context".to_string())?;
        let ctx =
            sub_agent_ctx.ok_or_else(|| "Sub-agent execution context not available".to_string())?;

        let agent_config = configs
            .iter()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| format!("Agent '{}' not found", agent_id))?;

        let prompt = input
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'prompt' parameter for agent call")?;
        let context = input.get("context").cloned();

        return crate::agent::executor::execute_sub_agent(
            agent_config,
            prompt,
            context,
            ctx,
            1, // depth=1 (sub-agent level)
        )
        .await;
    }

    // Execute tool directly via ToolRegistry
    {
        let tool_input = inject_session_context(
            input,
            session_workdir,
            session_id,
            profile_id,
            persona_id,
            persona_workdir,
            supports_vision,
            call_id,
            sandbox_policy,
        );

        log::info!(
            "[Agent] Executing tool via ToolRegistry: {} with input: {}",
            tool_name,
            tool_input
        );

        let registry = crate::hooks::tool_registry_handle()
            .ok_or_else(|| "ToolRegistry not available: hook not installed".to_string())?;
        let reg = registry.read().await;
        match reg.execute(tool_name, tool_input).await {
            Ok(result) => Ok(result),
            Err(e) if e.contains("Tool not found") => {
                Err(format!("Tool '{}' not found", tool_name))
            }
            Err(e) => Err(format!("Tool execution failed: {}", e)),
        }
    }
}

fn extract_session_workdir_from_context(context: &serde_json::Value) -> Option<String> {
    context
        .get("workdir")
        .and_then(|v| v.as_str())
        .or_else(|| context.get("path").and_then(|v| v.as_str()))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn build_workdir_context_message(workdir: &str) -> String {
    format!(
        "## Working Directory\n\n\
Your current working directory is: `{}`\n\
All file operations (read, write, edit, shell commands) should target this directory.\n\
- Use relative paths (e.g., `src/main.rs`) instead of absolute paths.\n\
- Do NOT write files to the user's home directory (`~/`) unless explicitly asked.\n\
- The `workdir` parameter is automatically injected into tool calls; use relative paths.",
        workdir
    )
}

fn insert_context_messages_before_latest_user(
    llm_messages: &mut Vec<Message>,
    context_messages: Vec<Message>,
) {
    if context_messages.is_empty() {
        return;
    }

    let insert_at = match llm_messages.last() {
        Some(last) if last.role == MessageRole::User => llm_messages.len().saturating_sub(1),
        _ => llm_messages.len(),
    };

    llm_messages.splice(insert_at..insert_at, context_messages);
}

/// Re-inject runtime context (datetime, workdir, skills) into llm_messages after compression.
///
/// Compression collapses old messages into a summary, losing ephemeral context that was
/// injected at session start. This function restores that context so the agent still
/// knows the current date, working directory, and activated skills.
fn reinject_context_after_compression(
    llm_messages: &mut Vec<Message>,
    session_workdir: Option<&str>,
    contextual_user_messages: &[String],
) {
    let mut context_items: Vec<Message> = Vec::new();

    // 1. Datetime context
    let datetime_msg = format!(
        "## Current Date & Time\n\n{}",
        Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    context_items.push(Message::user(datetime_msg));

    // 2. Workdir context
    if let Some(workdir) = session_workdir {
        context_items.push(Message::user(build_workdir_context_message(workdir)));
    }

    // 3. Skill / other contextual messages (preserved from original injection)
    for msg in contextual_user_messages {
        context_items.push(Message::user(msg.clone()));
    }

    if !context_items.is_empty() {
        insert_context_messages_before_latest_user(llm_messages, context_items);
    }
}

fn inject_session_context(
    input: &serde_json::Value,
    session_workdir: Option<&str>,
    session_id: &str,
    profile_id: Option<&str>,
    persona_id: Option<&str>,
    persona_workdir: Option<&str>,
    supports_vision: bool,
    call_id: Option<&str>,
    sandbox_policy: Option<&crate::tool_executors::SandboxPolicy>,
) -> serde_json::Value {
    let Some(mut params) = input.as_object().cloned() else {
        return input.clone();
    };

    // Internal: bind tool calls to the Happy session id so background runs can be owned and cleaned
    // up on archive. Tools should ignore unknown fields.
    if !params.contains_key("__session_id") {
        params.insert(
            "__session_id".to_string(),
            serde_json::Value::String(session_id.to_string()),
        );
    }

    // Inject profile_id for tools that need LLM configuration (e.g. fetch tool compression)
    if let Some(pid) = profile_id {
        if !params.contains_key("__profile_id") {
            params.insert(
                "__profile_id".to_string(),
                serde_json::Value::String(pid.to_string()),
            );
        }
    }

    // Inject owner_id for agent-owned tools (dispatch_task, update_personality, memory, etc.)
    // Also inject __persona_id for backward compat during transition.
    if let Some(pid) = persona_id {
        if !params.contains_key("__owner_id") {
            params.insert(
                "__owner_id".to_string(),
                serde_json::Value::String(pid.to_string()),
            );
        }
        if !params.contains_key("__persona_id") {
            params.insert(
                "__persona_id".to_string(),
                serde_json::Value::String(pid.to_string()),
            );
        }
    }

    // Inject persona_workdir for memory tool (private memory lives at {workdir}/.cteno/memory/)
    if let Some(pw) = persona_workdir {
        if !params.contains_key("__persona_workdir") {
            params.insert(
                "__persona_workdir".to_string(),
                serde_json::Value::String(pw.to_string()),
            );
        }
    }

    // Inject supports_vision for read tool (enables image upload for vision-capable models)
    if supports_vision && !params.contains_key("__supports_vision") {
        params.insert(
            "__supports_vision".to_string(),
            serde_json::Value::Bool(true),
        );
    }

    // Inject call_id for tools that support send-to-background (shell executor)
    if let Some(cid) = call_id {
        if !params.contains_key("__call_id") {
            params.insert(
                "__call_id".to_string(),
                serde_json::Value::String(cid.to_string()),
            );
        }
    }

    // Inject sandbox policy for workspace boundary enforcement
    if let Some(policy) = sandbox_policy {
        if !params.contains_key("__sandbox_policy") {
            if let Ok(policy_json) = serde_json::to_value(policy) {
                params.insert("__sandbox_policy".to_string(), policy_json);
            }
        }
    }

    let Some(workdir) = session_workdir.map(str::trim).filter(|s| !s.is_empty()) else {
        return serde_json::Value::Object(params);
    };

    let has_workdir = params
        .get("workdir")
        .and_then(|v| v.as_str())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    if !has_workdir {
        params.insert(
            "workdir".to_string(),
            serde_json::Value::String(workdir.to_string()),
        );
    }

    serde_json::Value::Object(params)
}

/// Execute agent with queue integration
///
/// This function integrates the message queue system:
/// 1. Pop ALL queued messages at once
/// 2. Add them to the conversation as user messages
/// 3. Let LLM process everything in one call
#[allow(clippy::too_many_arguments)]
pub async fn execute_agent_with_queue(
    db_path: PathBuf,
    agent_id: &str,
    api_key: &str,
    base_url: &str,
    model: &str,
    system_prompt: &str,
    new_message: Option<String>,
    tools: &[Tool],
    temperature: f32,
    max_tokens: u32,
    session_id: &str,
    user_id: Option<&str>,
    contextual_user_messages: Option<Vec<String>>,
    queue: &AgentMessageQueue,
) -> Result<AgentExecutionResult, String> {
    // 1. Add new message to queue if provided
    if let Some(msg) = new_message {
        queue.push(AgentMessage::user(session_id.to_string(), msg))?;
    }

    // 2. Check if already processing
    if queue.is_processing(session_id) {
        return Err(format!(
            "消息已加入队列，当前排队 {} 条",
            queue.len(session_id)
        ));
    }

    // 3. Mark as processing
    queue.set_processing(session_id, true);

    // 4. Pop ALL messages from queue at once
    let queued_messages = queue.pop_all(session_id);

    if queued_messages.is_empty() {
        queue.set_processing(session_id, false);
        return Err("队列为空".to_string());
    }

    // 4.5. Check for Slash Commands in queued messages via the host interceptor.
    //
    // The host registers a `CommandInterceptor` hook that wraps its
    // `CommandHandler` and the app's DB.  When a slash command matches, the
    // hook returns `InterceptedOutcome { message, stop }`; `stop = true` makes
    // the runtime short-circuit with that message as the agent response.
    if let Some(interceptor) = crate::hooks::command_interceptor() {
        for msg in &queued_messages {
            if msg.role == "user" {
                if let Some(outcome) = interceptor.intercept(session_id, &msg.content).await {
                    if outcome.stop {
                        log::info!(
                            "[SlashCommand] Intercepted in queue (stop=true) for session {}",
                            session_id
                        );
                        queue.set_processing(session_id, false);
                        return Ok(AgentExecutionResult {
                            response: outcome.message,
                            session_id: session_id.to_string(),
                            iteration_count: 0,
                            intermediate_messages: Vec::new(),
                            total_usage: Usage::zero(),
                        });
                    }
                }
            }
        }
    }

    // 5. Build combined prompt from all messages
    // Simply concatenate all messages for the LLM to process naturally
    let combined_prompt = queued_messages
        .iter()
        .map(|m| match m.role.as_str() {
            "user" => m.content.clone(),
            "subagent" => format!("[后台任务通知] {}", m.content),
            "system" => format!("[系统] {}", m.content),
            _ => m.content.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    log::info!(
        "[Agent] Processing {} queued messages",
        queued_messages.len()
    );

    // 6. Execute agent with combined prompt
    let result = execute_autonomous_agent_with_session(
        db_path,
        agent_id,
        api_key,
        base_url,
        model,
        system_prompt,
        &combined_prompt,
        None,
        tools,
        temperature,
        max_tokens,
        Some(session_id),
        user_id,
        contextual_user_messages,
        None,
        None,
        None,  // No permission checker for local frontend path
        None,  // No compress client for queue path
        None,  // No profile_id for queue path
        None,  // No abort flag for local frontend path
        None,  // No thinking flag for local frontend path
        None,  // No context_tokens for local frontend path
        None,  // No agent configs for queue path
        None,  // No sub-agent context for queue path
        None,  // No message queue for local frontend path
        false, // No proxy for queue path
        None,  // No stream callback for queue path
        None,  // No persona_id for queue path
        None,  // No persona_workdir for queue path
        crate::llm_profile::ApiFormat::Anthropic, // Default for queue path
        false, // No vision for queue path
        false, // No thinking for queue path
        true,  // Supports function calling for queue path
        false, // No image output for queue path
        None,  // No images for queue path
        None,  // No sandbox policy (default WorkspaceWrite)
    )
    .await;

    // 7. Mark as not processing
    queue.set_processing(session_id, false);

    // 8. Check if new messages arrived during processing
    let remaining = queue.len(session_id);
    if remaining > 0 {
        log::info!(
            "[Agent] {} new messages arrived during processing",
            remaining
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ContentBlock, Message, MessageContent, MessageRole};
    use serde_json::json;

    fn make_user_text(text: &str) -> Message {
        Message {
            role: MessageRole::User,
            content: MessageContent::Text(text.to_string()),
        }
    }

    fn make_assistant_text(text: &str) -> Message {
        Message {
            role: MessageRole::Assistant,
            content: MessageContent::Text(text.to_string()),
        }
    }

    fn make_assistant_with_tool_use(id: &str) -> Message {
        Message {
            role: MessageRole::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: "Let me run that.".to_string(),
                },
                ContentBlock::ToolUse {
                    id: id.to_string(),
                    name: "shell".to_string(),
                    input: json!({"command": "ls"}),
                    gemini_thought_signature: None,
                },
            ]),
        }
    }

    fn make_user_with_tool_result(tool_use_id: &str) -> Message {
        Message {
            role: MessageRole::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: "file.txt".to_string(),
                is_error: false,
            }]),
        }
    }

    #[test]
    fn test_sanitize_clean_history() {
        // Normal conversation: no changes needed
        let mut msgs = vec![
            make_user_text("hello"),
            make_assistant_text("hi there"),
            make_user_text("run ls"),
            make_assistant_with_tool_use("call_1"),
            make_user_with_tool_result("call_1"),
            make_assistant_text("Here are the files."),
        ];
        sanitize_llm_messages(&mut msgs);
        assert_eq!(msgs.len(), 6);
    }

    #[test]
    fn test_sanitize_leading_orphan_tool_result() {
        // History was truncated: starts with a tool_result without preceding tool_use
        let mut msgs = vec![
            make_user_with_tool_result("call_old"),
            make_assistant_text("Done."),
            make_user_text("do something else"),
        ];
        sanitize_llm_messages(&mut msgs);
        assert_eq!(msgs.len(), 2); // orphan removed
        assert_eq!(msgs[0].content.as_text(), "Done.");
    }

    #[test]
    fn test_sanitize_trailing_tool_use_no_result() {
        // Agent was interrupted mid-tool-call
        let mut msgs = vec![
            make_user_text("run something"),
            make_assistant_with_tool_use("call_1"),
        ];
        sanitize_llm_messages(&mut msgs);
        assert_eq!(msgs.len(), 1); // trailing tool_use removed
        assert_eq!(msgs[0].content.as_text(), "run something");
    }

    #[test]
    fn test_sanitize_mismatched_ids() {
        // tool_result references a tool_use_id that doesn't exist in the preceding assistant message
        let mut msgs = vec![
            make_user_text("hello"),
            make_assistant_with_tool_use("call_1"),
            make_user_with_tool_result("call_WRONG"),
            make_user_text("try again"),
        ];
        sanitize_llm_messages(&mut msgs);
        // The mismatched pair should be removed
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content.as_text(), "hello");
        assert_eq!(msgs[1].content.as_text(), "try again");
    }

    #[test]
    fn test_sanitize_middle_orphan() {
        // An assistant has tool_use but next message is a plain user text (not tool_result)
        let mut msgs = vec![
            make_user_text("hello"),
            make_assistant_with_tool_use("call_1"),
            make_user_text("never mind, skip that"),
        ];
        sanitize_llm_messages(&mut msgs);
        // The assistant with tool_use should be removed since next msg has no tool_result
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content.as_text(), "hello");
        assert_eq!(msgs[1].content.as_text(), "never mind, skip that");
    }

    #[test]
    fn test_sanitize_empty() {
        let mut msgs: Vec<Message> = vec![];
        sanitize_llm_messages(&mut msgs);
        assert_eq!(msgs.len(), 0);
    }

    #[test]
    fn test_inject_workdir_when_missing() {
        let input = json!({ "command": "ls -la" });
        let injected = inject_session_context(
            &input,
            Some("/tmp/project"),
            "sid_test",
            None,
            None,
            None,
            false,
            None,
            None,
        );
        assert_eq!(
            injected.get("workdir").and_then(|v| v.as_str()),
            Some("/tmp/project")
        );
        assert_eq!(
            injected.get("command").and_then(|v| v.as_str()),
            Some("ls -la")
        );
    }

    #[test]
    fn test_keep_existing_workdir() {
        let input = json!({ "command": "ls -la", "workdir": "/already/set" });
        let injected = inject_session_context(
            &input,
            Some("/tmp/project"),
            "sid_test",
            None,
            None,
            None,
            false,
            None,
            None,
        );
        assert_eq!(
            injected.get("workdir").and_then(|v| v.as_str()),
            Some("/already/set")
        );
    }

    #[test]
    fn test_extract_workdir_from_context() {
        let context = json!({ "workdir": " /Users/zal/work " });
        let workdir = extract_session_workdir_from_context(&context);
        assert_eq!(workdir.as_deref(), Some("/Users/zal/work"));
    }
}
