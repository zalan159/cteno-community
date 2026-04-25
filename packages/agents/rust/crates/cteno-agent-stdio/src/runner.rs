//! Bridge between inbound protocol messages and the runtime's autonomous
//! agent loop.
//!
//! - A `user_message` triggers `execute_autonomous_agent_with_session`.
//! - Streaming content from the runtime (via the `AcpMessageSender` callback
//!   and `StreamCallback`) is forwarded as ACP payloads without semantic
//!   translation at the stdio boundary.
//! - Tool permission checks are turned into `permission_request` outbound
//!   messages; the matching `permission_response` resolves a per-turn
//!   `oneshot`.
//! - Missing configuration (api_key / model / base_url) produces a clear
//!   `error` outbound, not a panic.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use cteno_agent_runtime::agent_queue::AgentMessageQueue;
use cteno_agent_runtime::agent_session::AgentSessionManager;
use cteno_agent_runtime::autonomous_agent::{
    build_native_tool_surface_for_turn, execute_autonomous_agent_with_session, AcpMessageSender,
    PermissionChecker,
};
use cteno_agent_runtime::hooks;
use cteno_agent_runtime::llm::{ImageSource, StreamCallback};
use cteno_agent_runtime::llm_profile::{self, ApiFormat};
use cteno_agent_runtime::permission::{PermissionCheckResult, PermissionDecision};

use tokio::sync::oneshot;

use crate::io::OutboundWriter;
use crate::pending::{new_permission_id, PendingPermissions};
use crate::protocol::{AcpDelivery, Attachment, AttachmentKind, ContextUsage, Outbound, TurnUsage};
use crate::session::SessionState;

const DIRECT_API_KEY_ENV_KEYS: &[&str] =
    &["CTENO_AGENT_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY"];

fn env_string(env_keys: &[&str]) -> Option<String> {
    for k in env_keys {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Extract a direct string config key from `agent_config`.
fn cfg_string(cfg: &Value, key: &str) -> Option<String> {
    if let Some(v) = cfg.get(key).and_then(|v| v.as_str()) {
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    None
}

fn cfg_f32(cfg: &Value, key: &str, default: f32) -> f32 {
    cfg.get(key)
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(default)
}

fn cfg_u32(cfg: &Value, key: &str, default: u32) -> u32 {
    cfg.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(default)
}

fn cfg_optional_u32(cfg: &Value, key: &str) -> Option<u32> {
    cfg.get(key)
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v > 0)
}

fn cfg_profile_id(cfg: &Value) -> Option<String> {
    cfg_string(cfg, "profile_id")
}

fn direct_api_key_error() -> String {
    "请先登录 Cteno 账号以使用内置模型，或在环境变量中设置 CTENO_AGENT_API_KEY 直连第三方模型。"
        .to_string()
}

#[derive(Debug, Clone)]
struct ExecutionProfileSelection {
    profile_id: String,
    profile: llm_profile::LlmProfile,
    api_key: String,
    base_url: String,
    /// Happy Server proxy mode (Bearer + /v1/llm/chat). Mutually exclusive
    /// with `openrouter_direct`.
    use_proxy: bool,
    /// OpenRouter direct mode (Bearer + /messages on openrouter.ai). Set
    /// when a proxy profile resolves via the LlmKeyProvider hook. The actual
    /// client constructor branches on `base_url.starts_with("https://openrouter.ai")`
    /// in autonomous_agent, so this flag is currently only used for
    /// introspection / tests — `#[allow(dead_code)]` suppresses the warning.
    #[allow(dead_code)]
    openrouter_direct: bool,
}

fn build_direct_execution_profile(
    selection: llm_profile::ResolvedProfileSelection,
    global_api_key: &str,
) -> Result<ExecutionProfileSelection, String> {
    let api_key = if selection.profile.chat.api_key.is_empty() {
        global_api_key.to_string()
    } else {
        selection.profile.chat.api_key.clone()
    };
    if api_key.is_empty() {
        return Err(direct_api_key_error());
    }

    Ok(ExecutionProfileSelection {
        profile_id: selection.profile_id.clone(),
        base_url: selection.profile.chat.base_url.clone(),
        profile: selection.profile,
        api_key,
        use_proxy: false,
        openrouter_direct: false,
    })
}

fn resolve_execution_profile_selection(
    store: &llm_profile::ProfileStore,
    selection: llm_profile::ResolvedProfileSelection,
    server_url: &str,
    auth_token: Option<String>,
    global_api_key: &str,
) -> Result<ExecutionProfileSelection, String> {
    if llm_profile::is_proxy_profile(&selection.profile_id) {
        // Preferred path: direct-to-OpenRouter using a per-user subkey cached
        // by the host-side LlmKeyStore. Falls through to the legacy
        // happy-server Bearer-proxy path only if the subkey is not yet
        // provisioned (first boot, balance-depleted, etc.).
        if let Some(subkey) = cteno_agent_runtime::hooks::current_llm_key() {
            return Ok(ExecutionProfileSelection {
                profile_id: selection.profile_id.clone(),
                profile: selection.profile,
                api_key: subkey,
                base_url: "https://openrouter.ai/api/v1".to_string(),
                use_proxy: false,
                openrouter_direct: true,
            });
        }

        if let Some(auth_token) = auth_token {
            return Ok(ExecutionProfileSelection {
                profile_id: selection.profile_id.clone(),
                profile: selection.profile,
                api_key: auth_token,
                base_url: server_url.to_string(),
                use_proxy: true,
                openrouter_direct: false,
            });
        }

        let fallback = llm_profile::direct_fallback_selection(store);
        if llm_profile::is_proxy_profile(&fallback.profile_id) {
            return Err(format!(
                "profile '{}' requires an OpenRouter subkey or Happy proxy auth, but neither is \
                 available (not logged in and no direct-fallback profile).",
                selection.profile_id
            ));
        }
        return build_direct_execution_profile(fallback, global_api_key);
    }

    build_direct_execution_profile(selection, global_api_key)
}

fn nested_string(map: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| map.get(*key).and_then(|value| value.as_str()))
        .find(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn cfg_model(cfg: &Value) -> Option<String> {
    match cfg.get("model") {
        Some(Value::String(model)) if !model.is_empty() => Some(model.clone()),
        Some(Value::Object(model_cfg)) => nested_string(model_cfg, &["model", "model_id"]),
        _ => cfg_string(cfg, "model_id"),
    }
}

fn cfg_effort(cfg: &Value) -> Option<String> {
    cfg_string(cfg, "effort")
        .or_else(|| cfg_string(cfg, "reasoning_effort"))
        .or_else(|| {
            cfg.get("model")
                .and_then(|value| value.as_object())
                .and_then(|model_cfg| nested_string(model_cfg, &["effort", "reasoning_effort"]))
        })
}

fn has_explicit_base_url(cfg: &Value) -> bool {
    cfg_string(cfg, "base_url").is_some()
}

fn should_use_profile_resolution(cfg: &Value) -> bool {
    cfg_profile_id(cfg).is_some()
        || (!has_explicit_base_url(cfg) && cfg_string(cfg, "api_key").is_none())
}

fn ensure_object(cfg: &mut Value) -> &mut Map<String, Value> {
    if !cfg.is_object() {
        *cfg = Value::Object(Map::new());
    }
    cfg.as_object_mut().expect("agent_config must be an object")
}

/// Update the session's agent_config in response to a host-side `SetModel`
/// control frame.
///
/// The host executor's `ModelSpec.model_id` is a vendor-agnostic selector —
/// for Claude/Codex it's the CLI's native model name, but for Cteno sessions
/// it's whatever `SessionConnection::build_model_spec` returned, which is
/// currently the profile's `chat.model` (e.g. `"deepseek-reasoner"`). The
/// raw model name alone is ambiguous: multiple profiles can share a
/// `chat.model` (a user-local direct-API profile and a proxy profile via
/// happy-server will both carry `"deepseek-reasoner"`). If we just blindly
/// stripped `profile_id` and let the next turn's resolver rediscover a
/// profile by `chat.model` match, it would silently pick the wrong
/// api_format / base_url and blow up with either a 404 on the wrong
/// endpoint or — worse — fall all the way through to
/// `default_profile_id` (which the server-side community default is still
/// `proxy-minimax/minimax-m2.5:free`, an 8k-context free tier unusable for
/// agent work).
///
/// Instead: look up the previous `profile_id` to see whether the session
/// was running against a proxy profile or a user-local profile, then pick
/// the matching profile for the new `chat.model` from the same family.
/// Fall back gracefully if nothing matches.
///
/// `app_data_dir` is used only to read the cached proxy profile list + the
/// user profile store from disk — we intentionally do not make a network
/// call here since SetModel is on the hot path.
pub(crate) fn apply_model_control(
    cfg: &mut Value,
    model: String,
    effort: Option<String>,
    app_data_dir: &std::path::Path,
) {
    let cfg_obj = ensure_object(cfg);

    // Snapshot the existing profile_id so we can keep the family (proxy vs
    // user-local) stable across the model switch.
    let previous_profile_id = cfg_obj
        .get("profile_id")
        .and_then(Value::as_str)
        .map(str::to_string);

    let resolved_profile_id =
        resolve_profile_id_for_model(&model, previous_profile_id.as_deref(), app_data_dir);

    match &resolved_profile_id {
        Some(pid) => {
            cfg_obj.insert("profile_id".to_string(), Value::String(pid.clone()));
        }
        None => {
            // Couldn't map the new model back to any known profile. Strip
            // profile_id and let the runtime's full resolver (which does an
            // online fetch if needed) work it out on the next turn. This is
            // the degraded path — prior to this code everything took it
            // unconditionally.
            cfg_obj.remove("profile_id");
        }
    }

    let has_legacy_model_object = matches!(cfg_obj.get("model"), Some(Value::Object(_)));

    if let Some(Value::Object(model_cfg)) = cfg_obj.get_mut("model") {
        model_cfg.insert("model".to_string(), Value::String(model.clone()));
        model_cfg.insert("model_id".to_string(), Value::String(model.clone()));
        match effort.as_ref() {
            Some(value) => {
                model_cfg.insert("effort".to_string(), Value::String(value.clone()));
                model_cfg.insert("reasoning_effort".to_string(), Value::String(value.clone()));
            }
            None => {
                model_cfg.remove("effort");
                model_cfg.remove("reasoning_effort");
            }
        }
    }

    if !has_legacy_model_object {
        cfg_obj.insert("model".to_string(), Value::String(model));
    }

    match effort {
        Some(value) => {
            cfg_obj.insert("effort".to_string(), Value::String(value));
        }
        None => {
            cfg_obj.remove("effort");
        }
    }
}

/// Given a new model selector + the session's previous profile_id, pick the
/// best matching profile from disk. The selector may itself be a profile id
/// (e.g. `"proxy-deepseek-reasoner"` when the daemon later teaches
/// `build_model_spec` to pass it through, or when a dev manually sets it)
/// or a bare chat model (e.g. `"deepseek-reasoner"` — current daemon
/// behavior). Both shapes are accepted.
///
/// Family preference:
///   - If the previous profile was a proxy profile, prefer proxy matches
///     first. Fall back to user-local.
///   - Otherwise prefer user-local matches first, falling back to proxy.
///
/// Returns the matched profile's id, or None when nothing matches.
fn resolve_profile_id_for_model(
    selector: &str,
    previous_profile_id: Option<&str>,
    app_data_dir: &std::path::Path,
) -> Option<String> {
    let selector = selector.trim();
    if selector.is_empty() {
        return None;
    }

    let store = llm_profile::load_profiles(app_data_dir);
    let proxy_profiles = llm_profile::load_proxy_profiles_cache(app_data_dir);

    // 1. Exact profile-id match in either list — handles the case where the
    //    daemon (or a caller) passed the profile id directly.
    if let Some(profile) = proxy_profiles.iter().find(|p| p.id == selector) {
        return Some(profile.id.clone());
    }
    if let Some(profile) = store.profiles.iter().find(|p| p.id == selector) {
        return Some(profile.id.clone());
    }

    // 2. Match by chat.model. Preserve the previous family so
    //    `proxy-deepseek-reasoner` stays a proxy session even after a model
    //    switch, and a user-local `default` stays direct-API.
    let prefer_proxy = previous_profile_id
        .map(llm_profile::is_proxy_profile)
        // No prior profile → default to proxy, since logged-in sessions
        // almost always use the proxy family.
        .unwrap_or(true);

    let user_match = store
        .profiles
        .iter()
        .find(|p| p.chat.model == selector)
        .map(|p| p.id.clone());
    let proxy_match = proxy_profiles
        .iter()
        .find(|p| p.chat.model == selector)
        .map(|p| p.id.clone());

    if prefer_proxy {
        proxy_match.or(user_match)
    } else {
        user_match.or(proxy_match)
    }
}

pub(crate) fn apply_permission_mode_control(cfg: &mut Value, mode: String) {
    ensure_object(cfg).insert("permission_mode".to_string(), Value::String(mode));
}

pub(crate) async fn normalize_agent_config(cfg: &mut Value, app_data_dir: &std::path::Path) {
    if !should_use_profile_resolution(cfg) {
        return;
    }

    let requested_profile_id = cfg_profile_id(cfg);
    let requested_model = cfg_model(cfg);
    let requested_effort = cfg_effort(cfg);
    let selection = llm_profile::resolve_profile_request(
        app_data_dir,
        &hooks::resolved_happy_server_url(),
        requested_profile_id.as_deref(),
        requested_model.as_deref(),
        requested_effort.as_deref(),
    )
    .await;

    ensure_object(cfg).insert(
        "profile_id".to_string(),
        Value::String(selection.profile_id),
    );
}

async fn send_turn_error(writer: &OutboundWriter, session_id: &str, message: String) {
    writer
        .send(Outbound::Error {
            session_id: session_id.to_string(),
            message,
        })
        .await;
    writer
        .send(Outbound::TurnComplete {
            session_id: session_id.to_string(),
            final_text: String::new(),
            iteration_count: 0,
            usage: TurnUsage::default(),
            context_usage: None,
        })
        .await;
}

pub(crate) fn acp_outbound(session_id: &str, delivery: AcpDelivery, data: Value) -> Outbound {
    Outbound::Acp {
        session_id: session_id.to_string(),
        delivery,
        data,
    }
}

fn cfg_allowed_tools(cfg: &Value) -> Option<Vec<String>> {
    let value = cfg.get("allowed_tools")?;
    if let Some(items) = value.as_array() {
        let tools: Vec<String> = items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        return Some(tools);
    }
    value.as_str().map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    })
}

fn cfg_permission_mode(cfg: &Value) -> &str {
    cfg.get("permission_mode")
        .and_then(Value::as_str)
        .unwrap_or("default")
}

fn sandbox_policy_for_mode(
    mode: &str,
    additional_directories: &[String],
) -> cteno_agent_runtime::tool_executors::SandboxPolicy {
    use cteno_agent_runtime::tool_executors::SandboxPolicy;
    match mode {
        "bypass_permissions" | "danger_full_access" => SandboxPolicy::Unrestricted,
        "plan" | "read_only" => SandboxPolicy::ReadOnly,
        _ => SandboxPolicy::WorkspaceWrite {
            additional_writable_roots: additional_directories
                .iter()
                .map(std::path::PathBuf::from)
                .collect(),
        },
    }
}

fn permission_checker_for_mode(
    mode: &str,
    session_id: String,
    writer: OutboundWriter,
    pending_permissions: PendingPermissions,
) -> Option<PermissionChecker> {
    match mode {
        "bypass_permissions" | "danger_full_access" | "read_only" => None,
        "plan" => Some(Arc::new(
            move |tool_name: String, _call_id: String, _input: Value| {
                Box::pin(async move {
                    PermissionCheckResult::Denied(format!(
                        "Plan mode: tool execution is disabled ({tool_name})"
                    ))
                })
            },
        )),
        _ => Some(build_permission_checker(
            session_id,
            writer,
            pending_permissions,
        )),
    }
}

fn attachments_to_images(attachments: &[Attachment]) -> Option<Vec<ImageSource>> {
    let images: Vec<ImageSource> = attachments
        .iter()
        .filter(|attachment| matches!(attachment.kind, AttachmentKind::Image))
        .filter_map(|attachment| {
            let media_type = attachment
                .mime_type
                .clone()
                .unwrap_or_else(|| "image/png".to_string());
            if let Some(data) = attachment.data.clone().filter(|s| !s.is_empty()) {
                return Some(ImageSource {
                    source_type: "base64".to_string(),
                    media_type,
                    data,
                });
            }
            attachment
                .source
                .clone()
                .filter(|s| !s.is_empty())
                .map(|source| ImageSource {
                    source_type: "url".to_string(),
                    media_type,
                    data: source,
                })
        })
        .collect();
    if images.is_empty() {
        None
    } else {
        Some(images)
    }
}

/// Construct a `PermissionChecker` that round-trips each check through the
/// stdio protocol. For every tool call the runtime asks us about, we:
///
/// 1. Allocate a `request_id` and a fresh `oneshot`.
/// 2. Stash the sender in the shared pending-permissions map.
/// 3. Emit a `permission_request` outbound message.
/// 4. Await the matching `permission_response` (delivered by the main loop
///    via `pending.take(request_id).send(decision)`).
/// 5. Translate the `PermissionDecision` into a `PermissionCheckResult`.
fn build_permission_checker(
    session_id: String,
    writer: OutboundWriter,
    pending: PendingPermissions,
) -> PermissionChecker {
    Arc::new(move |tool_name: String, _call_id: String, input: Value| {
        let writer = writer.clone();
        let pending = pending.clone();
        let session_id = session_id.clone();
        Box::pin(async move {
            let request_id = new_permission_id();
            let (tx, rx) = oneshot::channel::<PermissionDecision>();

            {
                let mut guard = pending.lock().await;
                guard.insert(request_id.clone(), tx);
            }

            writer
                .send(Outbound::PermissionRequest {
                    session_id: session_id.clone(),
                    request_id: request_id.clone(),
                    tool_name: tool_name.clone(),
                    tool_input: input,
                })
                .await;

            match rx.await {
                Ok(PermissionDecision::Approved) | Ok(PermissionDecision::ApprovedForSession) => {
                    PermissionCheckResult::Allowed
                }
                Ok(PermissionDecision::Denied) => {
                    PermissionCheckResult::Denied("host denied tool".to_string())
                }
                Ok(PermissionDecision::Abort) => PermissionCheckResult::Aborted,
                Err(_) => {
                    // Sender dropped (host crash / shutdown) — clean up the
                    // map slot and fail closed.
                    let mut guard = pending.lock().await;
                    guard.remove(&request_id);
                    PermissionCheckResult::Denied(format!(
                        "host never answered permission_request {request_id} for {tool_name}"
                    ))
                }
            }
        })
    })
}

fn api_format_label(api_format: &ApiFormat) -> &'static str {
    match api_format {
        ApiFormat::Anthropic => "anthropic-compatible",
        ApiFormat::OpenAI => "openai-compatible",
        ApiFormat::Gemini => "gemini-compatible",
    }
}

/// Runtime identity context for the resolved Cteno profile.
///
/// The transport/API format is not the model provider identity. For example,
/// DeepSeek profiles can use an Anthropic-compatible endpoint; without this
/// explicit context, models sometimes infer "Claude" from the wire protocol.
fn build_model_identity_context(
    profile_id: Option<&str>,
    model: &str,
    api_format: &ApiFormat,
    supports_vision: bool,
    supports_computer_use: bool,
) -> String {
    let mut lines = Vec::new();
    lines.push("<model_identity>".to_string());
    lines.push("Agent: cteno-agent".to_string());
    lines.push(format!("Current model: {model}"));
    if let Some(profile_id) = profile_id {
        lines.push(format!("Current profile: {profile_id}"));
    }
    lines.push(format!(
        "API format: {}; transport compatibility, not model identity.",
        api_format_label(api_format)
    ));
    lines.push(
        "Do not identify as Claude/Anthropic unless Current model/profile says so.".to_string(),
    );
    lines.push(format!(
        "Vision: {}",
        if supports_vision { "yes" } else { "no" }
    ));
    lines.push(format!(
        "Computer-use: {}",
        if supports_computer_use { "yes" } else { "no" }
    ));
    lines.push("</model_identity>".to_string());
    lines.join("\n")
}

fn fallback_system_prompt(workdir: Option<&str>, supports_function_calling: bool) -> String {
    let workspace_path = workdir
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from);
    cteno_agent_runtime::system_prompt::build_system_prompt(
        &cteno_agent_runtime::system_prompt::PromptOptions {
            workspace_path,
            include_tool_style: supports_function_calling,
            ..Default::default()
        },
    )
}

/// Run a single user turn against the runtime.
pub async fn run_turn(
    state: &SessionState,
    user_message: String,
    task_id: Option<String>,
    attachments: Vec<Attachment>,
    writer: OutboundWriter,
    pending_permissions: PendingPermissions,
    message_queue: Option<Arc<AgentMessageQueue>>,
) {
    let session_id = state.session_id.clone();
    let cfg = &state.agent_config;
    let app_data_dir = state
        .db_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    // ---------------- Configuration resolution ----------------
    let using_profile_resolution = should_use_profile_resolution(cfg);
    let mut profile_id_for_tools: Option<String> = None;
    let (
        api_key,
        base_url,
        model,
        temperature,
        max_tokens,
        context_window_tokens,
        api_format,
        supports_vision,
        supports_computer_use,
        enable_thinking,
        reasoning_effort,
        supports_function_calling,
        supports_image_output,
        use_proxy,
    ) = if using_profile_resolution {
        let store = llm_profile::load_profiles(&app_data_dir);
        let selection = llm_profile::resolve_profile_request(
            &app_data_dir,
            &hooks::resolved_happy_server_url(),
            cfg_profile_id(cfg).as_deref(),
            cfg_model(cfg).as_deref(),
            cfg_effort(cfg).as_deref(),
        )
        .await;

        let global_api_key = cfg_string(cfg, "api_key")
            .or_else(|| env_string(DIRECT_API_KEY_ENV_KEYS))
            .unwrap_or_default();
        let auth_token = hooks::credentials().and_then(|provider| provider.access_token());
        let resolved = match resolve_execution_profile_selection(
            &store,
            selection,
            &hooks::resolved_happy_server_url(),
            auth_token,
            &global_api_key,
        ) {
            Ok(resolved) => resolved,
            Err(message) => {
                send_turn_error(&writer, &session_id, message).await;
                return;
            }
        };

        let profile = resolved.profile;
        let profile_id = resolved.profile_id;
        log::info!(
            "[run_turn {session_id}] resolved profile_id={profile_id} model={} api_format={:?} max_tokens={} use_proxy={} base_url={}",
            profile.chat.model,
            profile.api_format,
            profile.chat.max_tokens,
            resolved.use_proxy,
            resolved.base_url,
        );
        let temperature = cfg_f32(cfg, "temperature", profile.chat.temperature);
        // When a profile is resolved, take `max_tokens` from the profile itself
        // (authoritative: server-provided for proxy profiles, user-authored
        // for direct profiles) instead of the session's agent_config, which
        // snapshots the value at session spawn and goes stale across profile
        // switches. set_model only propagates the model name, not max_tokens,
        // so honouring cfg here would pin the session to whatever profile
        // it was born with — the exact bug that surfaced as "prompt contains
        // 6612 characters, you requested 8192 output tokens" errors from
        // OpenRouter when the active profile says 32000.
        let max_tokens = profile.chat.max_tokens;
        let context_window_tokens = profile.chat.context_window_tokens;
        let reasoning_effort = cfg_effort(cfg);
        profile_id_for_tools = Some(profile_id);
        (
            resolved.api_key,
            resolved.base_url,
            profile.chat.model.clone(),
            temperature,
            max_tokens,
            context_window_tokens,
            profile.api_format.clone(),
            profile.supports_vision,
            profile.supports_computer_use,
            profile.thinking,
            reasoning_effort,
            profile.supports_function_calling,
            profile.supports_image_output,
            resolved.use_proxy,
        )
    } else {
        let Some(api_key) =
            cfg_string(cfg, "api_key").or_else(|| env_string(DIRECT_API_KEY_ENV_KEYS))
        else {
            send_turn_error(&writer, &session_id, direct_api_key_error()).await;
            return;
        };

        let base_url = cfg_string(cfg, "base_url")
            .or_else(|| env_string(&["CTENO_AGENT_BASE_URL"]))
            .unwrap_or_else(|| "https://api.deepseek.com/anthropic".to_string());
        let model = cfg_model(cfg)
            .or_else(|| env_string(&["CTENO_AGENT_MODEL"]))
            .unwrap_or_else(|| "deepseek-chat".to_string());
        let temperature = cfg_f32(cfg, "temperature", 0.2);
        let max_tokens = cfg_u32(cfg, "max_tokens", 4096);
        let context_window_tokens = cfg_optional_u32(cfg, "context_window_tokens");
        let api_format = match cfg
            .get("api_format")
            .and_then(|v| v.as_str())
            .unwrap_or("anthropic")
        {
            "openai" => ApiFormat::OpenAI,
            "gemini" => ApiFormat::Gemini,
            _ => ApiFormat::Anthropic,
        };

        (
            api_key,
            base_url,
            model,
            temperature,
            max_tokens,
            context_window_tokens,
            api_format,
            false,
            false,
            false,
            None,
            true,
            false,
            false,
        )
    };
    let system_prompt = state.system_prompt.clone().unwrap_or_else(|| {
        fallback_system_prompt(state.workdir.as_deref(), supports_function_calling)
    });
    let model_identity_context = build_model_identity_context(
        profile_id_for_tools.as_deref(),
        &model,
        &api_format,
        supports_vision,
        supports_computer_use,
    );

    // ---------------- Callbacks ----------------
    let writer_for_acp = writer.clone();
    let session_for_acp = session_id.clone();
    let acp_sender: AcpMessageSender = Arc::new(move |payload: Value| {
        let writer = writer_for_acp.clone();
        let session_id = session_for_acp.clone();
        Box::pin(async move {
            writer
                .send(acp_outbound(&session_id, AcpDelivery::Persisted, payload))
                .await;
        })
    });

    let writer_for_stream = writer.clone();
    let session_for_stream = session_id.clone();
    let stream_cb: StreamCallback = Arc::new(move |payload: Value| {
        let writer = writer_for_stream.clone();
        let session_id = session_for_stream.clone();
        Box::pin(async move {
            writer
                .send(acp_outbound(&session_id, AcpDelivery::Transient, payload))
                .await;
        })
    });

    let permission_mode = cfg_permission_mode(cfg).to_string();
    let permission_checker = permission_checker_for_mode(
        &permission_mode,
        session_id.clone(),
        writer.clone(),
        pending_permissions,
    );

    // If a workdir was supplied at init time, persist it into the session's
    // context_data so executors pick it up via extract_session_workdir_from_context.
    if let Some(ref wd) = state.workdir {
        let mgr = AgentSessionManager::new(state.db_path.clone());
        // Best-effort: ensure the session row exists first. execute_* will
        // create it if missing, but update_context_field needs a row.
        let _ = mgr.create_session_with_id(&session_id, "cteno-agent-stdio", None, None);
        if let Err(err) = mgr.update_context_field(&session_id, "workdir", json!(wd)) {
            log::warn!("failed to persist workdir into session context: {err}");
        }
    }

    // Load native tools from the installed registry (builtin + any
    // host-injected tools registered so far).
    let allowed_tools = cfg_allowed_tools(cfg);
    let (tools, mut runtime_tool_context) =
        build_native_tool_surface_for_turn(supports_function_calling, allowed_tools.as_deref())
            .await;
    let sandbox_policy = sandbox_policy_for_mode(&permission_mode, &state.additional_directories);
    let user_images = attachments_to_images(&attachments);
    let mut runtime_context_messages = vec![model_identity_context];
    runtime_context_messages.append(&mut runtime_tool_context);

    // ---------------- Invoke runtime ----------------
    state.abort_flag.store(false, Ordering::SeqCst);

    let result = execute_autonomous_agent_with_session(
        state.db_path.clone(),
        "cteno-agent-stdio",
        &api_key,
        &base_url,
        &model,
        &system_prompt,
        &user_message,
        None,
        &tools,
        temperature,
        max_tokens,
        context_window_tokens,
        Some(&session_id),
        None,
        Some(runtime_context_messages),
        Some(acp_sender),
        None,
        permission_checker,
        None,
        profile_id_for_tools.as_deref(),
        Some(state.abort_flag.clone()),
        None,
        None,
        None,
        None,
        message_queue,
        use_proxy,
        Some(stream_cb),
        None,
        None,
        api_format,
        supports_vision,
        enable_thinking,
        reasoning_effort.as_deref(),
        supports_function_calling,
        supports_image_output,
        user_images,
        Some(&sandbox_policy),
    )
    .await;

    match result {
        Ok(res) => {
            if !res.response.is_empty() {
                writer
                    .send(acp_outbound(
                        &session_id,
                        AcpDelivery::Persisted,
                        json!({
                            "type": "message",
                            "message": res.response,
                        }),
                    ))
                    .await;
            }
            if let Some(task_id) = task_id.as_deref() {
                writer
                    .send(acp_outbound(
                        &session_id,
                        AcpDelivery::Persisted,
                        json!({
                            "type": "task_complete",
                            "id": task_id,
                        }),
                    ))
                    .await;
            }
            writer
                .send(Outbound::TurnComplete {
                    session_id,
                    final_text: String::new(),
                    iteration_count: res.iteration_count,
                    usage: TurnUsage {
                        input_tokens: res.total_usage.input_tokens,
                        output_tokens: res.total_usage.output_tokens,
                        cache_creation_input_tokens: res.total_usage.cache_creation_input_tokens,
                        cache_read_input_tokens: res.total_usage.cache_read_input_tokens,
                    },
                    context_usage: res
                        .context_usage
                        .as_ref()
                        .map(|context_usage| ContextUsage {
                            total_tokens: context_usage.total_tokens,
                            max_tokens: context_usage.max_tokens,
                            raw_max_tokens: context_usage.raw_max_tokens,
                            auto_compact_token_limit: context_usage.auto_compact_token_limit,
                        }),
                })
                .await;
        }
        Err(err) => {
            writer
                .send(Outbound::Error {
                    session_id: session_id.clone(),
                    message: err,
                })
                .await;
            if let Some(task_id) = task_id.as_deref() {
                writer
                    .send(acp_outbound(
                        &session_id,
                        AcpDelivery::Persisted,
                        json!({
                            "type": "task_complete",
                            "id": task_id,
                        }),
                    ))
                    .await;
            }
            writer
                .send(Outbound::TurnComplete {
                    session_id,
                    final_text: String::new(),
                    iteration_count: 0,
                    usage: TurnUsage::default(),
                    context_usage: None,
                })
                .await;
        }
    }
}

// Suppress unused warnings for `json!` macro in the future.
#[allow(dead_code)]
fn _keep_imports() -> Value {
    json!({})
}

#[cfg(test)]
mod tests {
    use super::*;
    use cteno_agent_runtime::llm_profile::{
        get_default_profile, LlmEndpoint, LlmProfile, ProfileStore,
    };

    fn build_profile(id: &str, base_url: &str, api_key: &str) -> LlmProfile {
        LlmProfile {
            id: id.to_string(),
            name: id.to_string(),
            chat: LlmEndpoint {
                api_key: api_key.to_string(),
                base_url: base_url.to_string(),
                model: format!("{id}-model"),
                temperature: 0.2,
                max_tokens: 4096,
                context_window_tokens: None,
            },
            compress: LlmEndpoint {
                api_key: String::new(),
                base_url: base_url.to_string(),
                model: format!("{id}-compress"),
                temperature: 0.1,
                max_tokens: 1024,
                context_window_tokens: None,
            },
            supports_vision: false,
            supports_computer_use: false,
            api_format: ApiFormat::Anthropic,
            thinking: false,
            is_free: false,
            supports_function_calling: true,
            supports_image_output: false,
        }
    }

    #[test]
    fn cfg_model_accepts_new_shape() {
        let cfg = json!({
            "model": "gpt-5.1",
            "effort": "medium"
        });
        assert_eq!(cfg_model(&cfg).as_deref(), Some("gpt-5.1"));
        assert_eq!(cfg_effort(&cfg).as_deref(), Some("medium"));
    }

    #[test]
    fn cfg_model_accepts_legacy_nested_shape() {
        let cfg = json!({
            "model": {
                "provider": "openai",
                "model_id": "gpt-5.1",
                "reasoning_effort": "high"
            }
        });
        assert_eq!(cfg_model(&cfg).as_deref(), Some("gpt-5.1"));
        assert_eq!(cfg_effort(&cfg).as_deref(), Some("high"));
    }

    #[test]
    fn permission_mode_selects_sandbox_policy() {
        let additional = vec!["/tmp/cteno-extra".to_string()];

        let default_policy = sandbox_policy_for_mode("default", &additional);
        match default_policy {
            cteno_agent_runtime::tool_executors::SandboxPolicy::WorkspaceWrite {
                additional_writable_roots,
            } => {
                assert_eq!(additional_writable_roots.len(), 1);
                assert_eq!(
                    additional_writable_roots[0],
                    std::path::PathBuf::from("/tmp/cteno-extra")
                );
            }
            other => panic!("expected workspace-write sandbox, got {other:?}"),
        }

        assert!(matches!(
            sandbox_policy_for_mode("plan", &additional),
            cteno_agent_runtime::tool_executors::SandboxPolicy::ReadOnly
        ));
        assert!(matches!(
            sandbox_policy_for_mode("read_only", &additional),
            cteno_agent_runtime::tool_executors::SandboxPolicy::ReadOnly
        ));
        assert!(matches!(
            sandbox_policy_for_mode("bypass_permissions", &additional),
            cteno_agent_runtime::tool_executors::SandboxPolicy::Unrestricted
        ));
    }

    #[test]
    fn image_attachments_become_runtime_images() {
        let attachments = vec![
            Attachment {
                kind: AttachmentKind::Text,
                mime_type: Some("text/plain".to_string()),
                source: None,
                data: Some("ignore me".to_string()),
            },
            Attachment {
                kind: AttachmentKind::Image,
                mime_type: Some("image/jpeg".to_string()),
                source: None,
                data: Some("base64-payload".to_string()),
            },
            Attachment {
                kind: AttachmentKind::Image,
                mime_type: None,
                source: Some("file:///tmp/photo.png".to_string()),
                data: None,
            },
        ];

        let images = attachments_to_images(&attachments).expect("images");
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].source_type, "base64");
        assert_eq!(images[0].media_type, "image/jpeg");
        assert_eq!(images[0].data, "base64-payload");
        assert_eq!(images[1].source_type, "url");
        assert_eq!(images[1].media_type, "image/png");
        assert_eq!(images[1].data, "file:///tmp/photo.png");
    }

    #[test]
    fn apply_model_control_updates_legacy_and_new_shapes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut legacy_cfg = json!({
            "profile_id": "proxy-old",
            "model": {
                "provider": "openai",
                "model_id": "gpt-4.1",
                "reasoning_effort": "low"
            },
            "resume_session_id": "resume-1"
        });
        apply_model_control(
            &mut legacy_cfg,
            "gpt-5.1".to_string(),
            Some("high".to_string()),
            dir.path(),
        );
        // No profile on disk matches "gpt-5.1" so profile_id is stripped.
        // The model + effort must still propagate cleanly through both the
        // nested-object and flat shapes, and unrelated fields must survive.
        assert_eq!(cfg_profile_id(&legacy_cfg), None);
        assert_eq!(cfg_model(&legacy_cfg).as_deref(), Some("gpt-5.1"));
        assert_eq!(cfg_effort(&legacy_cfg).as_deref(), Some("high"));
        assert_eq!(
            legacy_cfg.get("resume_session_id").and_then(Value::as_str),
            Some("resume-1")
        );

        let mut new_cfg = Value::Null;
        apply_model_control(
            &mut new_cfg,
            "claude-opus-4-1".to_string(),
            None,
            dir.path(),
        );
        assert_eq!(cfg_model(&new_cfg).as_deref(), Some("claude-opus-4-1"));
        assert_eq!(cfg_effort(&new_cfg), None);
    }

    #[test]
    fn model_identity_context_names_resolved_model_not_transport_provider() {
        let context = build_model_identity_context(
            Some("proxy-deepseek-reasoner"),
            "deepseek-reasoner",
            &ApiFormat::Anthropic,
            false,
            false,
        );

        assert!(context.contains("Agent: cteno-agent"));
        assert!(context.contains("Current model: deepseek-reasoner"));
        assert!(context.contains("Current profile: proxy-deepseek-reasoner"));
        assert!(context.contains("API format: anthropic-compatible"));
        assert!(context.contains("not model identity"));
        assert!(context.contains("Do not identify as Claude/Anthropic"));
        assert!(!context.contains("Current model: Claude"));
    }

    #[test]
    fn fallback_system_prompt_loads_project_agents_md() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "# Project rules\n\nAlways prefer project-local instructions.",
        )
        .expect("write AGENTS.md");

        let prompt = fallback_system_prompt(dir.path().to_str(), true);

        assert!(prompt.contains("AGENTS.md"));
        assert!(prompt.contains("Always prefer project-local instructions."));
    }

    /// When the previous profile was a proxy profile and the new model name
    /// has a proxy profile match, we must stay on the proxy side even
    /// though a user-local profile with the same chat.model also exists.
    /// This is the regression that caused DeepSeek R1 switches to silently
    /// fall back to the free OpenRouter default.
    #[test]
    fn apply_model_control_preserves_proxy_family() {
        let dir = tempfile::tempdir().expect("tempdir");

        // User profiles (local direct-API DeepSeek — api_format Anthropic).
        // The failure mode this regression guards against: matching on
        // chat.model="deepseek-reasoner" silently picks `default` (direct)
        // even when the session was running against the proxy.
        let user_store = json!({
            "profiles": [{
                "id": "default",
                "name": "Direct DeepSeek",
                "chat": {
                    "api_key": "",
                    "base_url": "https://api.deepseek.com/anthropic",
                    "model": "deepseek-reasoner",
                    "temperature": 0.2,
                    "max_tokens": 4096,
                    "context_window_tokens": null
                },
                "compress": {
                    "api_key": "",
                    "base_url": "https://api.deepseek.com/anthropic",
                    "model": "deepseek-chat",
                    "temperature": 0.1,
                    "max_tokens": 1024,
                    "context_window_tokens": null
                },
                "supports_vision": false,
                "supports_computer_use": false,
                "api_format": "anthropic",
                "thinking": false,
                "is_free": false,
                "supports_function_calling": true,
                "supports_image_output": false
            }],
            "default_profile_id": "default"
        });
        std::fs::write(
            dir.path().join("profiles.json"),
            serde_json::to_string_pretty(&user_store).unwrap(),
        )
        .expect("write profiles.json");

        let proxy_cache = json!([{
            "id": "proxy-deepseek-reasoner",
            "name": "DeepSeek R1 (proxy)",
            "chat": {
                "api_key": "",
                "base_url": "",
                "model": "deepseek-reasoner",
                "temperature": 0.2,
                "max_tokens": 4096,
                "context_window_tokens": 128000
            },
            "compress": {
                "api_key": "",
                "base_url": "",
                "model": "deepseek-chat",
                "temperature": 0.1,
                "max_tokens": 1024,
                "context_window_tokens": null
            },
            "supports_vision": false,
            "supports_computer_use": false,
            "api_format": "anthropic",
            "thinking": false,
            "is_free": false,
            "supports_function_calling": true,
            "supports_image_output": false
        }]);
        std::fs::write(
            dir.path().join("proxy_profiles_cache.json"),
            serde_json::to_string_pretty(&proxy_cache).unwrap(),
        )
        .expect("write proxy cache");

        let mut cfg = json!({ "profile_id": "proxy-deepseek-reasoner" });
        apply_model_control(&mut cfg, "deepseek-reasoner".to_string(), None, dir.path());
        assert_eq!(
            cfg_profile_id(&cfg).as_deref(),
            Some("proxy-deepseek-reasoner"),
            "previous proxy family must survive a same-model SetModel: the session was running on proxy-deepseek-reasoner and must not silently demote to user-local `default`",
        );

        // Dual: a user-local session switching model stays user-local.
        let mut cfg = json!({ "profile_id": "default" });
        apply_model_control(&mut cfg, "deepseek-reasoner".to_string(), None, dir.path());
        assert_eq!(
            cfg_profile_id(&cfg).as_deref(),
            Some("default"),
            "user-local family must also survive a same-model SetModel",
        );
    }

    #[test]
    fn profile_resolution_preserves_explicit_base_url_path() {
        let cfg = json!({
            "api_key": "key-123",
            "base_url": "https://example.com",
            "model": "custom-model"
        });
        assert!(!should_use_profile_resolution(&cfg));
    }

    #[test]
    fn profile_resolution_prefers_explicit_profile_id_over_conflicting_model() {
        let store = ProfileStore {
            profiles: vec![
                build_profile("user-direct", "https://direct.example", "direct-key"),
                build_profile("user-fast", "https://fast.example", "fast-key"),
            ],
            default_profile_id: "user-fast".to_string(),
        };

        let resolved = llm_profile::resolve_profile_selection(
            &store,
            &[],
            Some("user-direct"),
            Some("user-fast-model"),
            None,
        )
        .expect("resolved");

        assert_eq!(resolved.profile_id, "user-direct");
        assert_eq!(resolved.profile.chat.model, "user-direct-model");
    }

    #[test]
    fn profile_resolution_still_uses_model_fallback_without_profile_id() {
        let store = ProfileStore {
            profiles: vec![
                build_profile("user-direct", "https://direct.example", "direct-key"),
                build_profile("user-fast", "https://fast.example", "fast-key"),
            ],
            default_profile_id: "user-direct".to_string(),
        };

        let resolved = llm_profile::resolve_profile_selection(
            &store,
            &[],
            None,
            Some("user-fast-model"),
            None,
        )
        .expect("resolved");

        assert_eq!(resolved.profile_id, "user-fast");
        assert_eq!(resolved.profile.chat.model, "user-fast-model");
    }

    #[test]
    fn apply_permission_mode_control_updates_config_without_panicking() {
        let mut cfg = Value::Null;
        apply_permission_mode_control(&mut cfg, "accept_edits".to_string());
        assert_eq!(
            cfg.get("permission_mode").and_then(Value::as_str),
            Some("accept_edits")
        );
    }

    #[test]
    fn resolve_execution_profile_selection_falls_back_to_direct_without_auth() {
        let store = ProfileStore {
            profiles: vec![get_default_profile()],
            default_profile_id: llm_profile::DEFAULT_PROXY_PROFILE.to_string(),
        };
        let proxy_selection = llm_profile::ResolvedProfileSelection {
            profile_id: llm_profile::DEFAULT_PROXY_PROFILE.to_string(),
            profile: build_profile(llm_profile::DEFAULT_PROXY_PROFILE, "", ""),
        };

        let resolved = resolve_execution_profile_selection(
            &store,
            proxy_selection,
            "https://happy.example",
            None,
            "env-key-123",
        )
        .expect("resolved");

        assert_eq!(resolved.profile_id, llm_profile::DEFAULT_DIRECT_PROFILE);
        assert_eq!(resolved.base_url, "https://api.deepseek.com/anthropic");
        assert_eq!(resolved.api_key, "env-key-123");
        assert!(!resolved.use_proxy);
    }

    #[test]
    fn resolve_execution_profile_selection_reports_missing_direct_api_key() {
        let store = ProfileStore {
            profiles: vec![get_default_profile()],
            default_profile_id: llm_profile::DEFAULT_PROXY_PROFILE.to_string(),
        };
        let proxy_selection = llm_profile::ResolvedProfileSelection {
            profile_id: llm_profile::DEFAULT_PROXY_PROFILE.to_string(),
            profile: build_profile(llm_profile::DEFAULT_PROXY_PROFILE, "", ""),
        };

        let error = resolve_execution_profile_selection(
            &store,
            proxy_selection,
            "https://happy.example",
            None,
            "",
        )
        .expect_err("missing key error");

        assert_eq!(error, direct_api_key_error());
    }

    #[test]
    fn resolve_execution_profile_selection_prefers_store_direct_default_without_auth() {
        let store = ProfileStore {
            profiles: vec![
                build_profile("user-direct", "https://example.com", "user-key"),
                get_default_profile(),
            ],
            default_profile_id: "user-direct".to_string(),
        };
        let proxy_selection = llm_profile::ResolvedProfileSelection {
            profile_id: llm_profile::DEFAULT_PROXY_PROFILE.to_string(),
            profile: build_profile(llm_profile::DEFAULT_PROXY_PROFILE, "", ""),
        };

        let resolved = resolve_execution_profile_selection(
            &store,
            proxy_selection,
            "https://happy.example",
            None,
            "env-key-123",
        )
        .expect("resolved");

        assert_eq!(resolved.profile_id, "user-direct");
        assert_eq!(resolved.base_url, "https://example.com");
        assert_eq!(resolved.api_key, "user-key");
        assert!(!resolved.use_proxy);
    }

    #[test]
    fn resolve_execution_profile_selection_keeps_proxy_when_auth_exists() {
        let store = ProfileStore {
            profiles: vec![get_default_profile()],
            default_profile_id: llm_profile::DEFAULT_PROXY_PROFILE.to_string(),
        };
        let proxy_selection = llm_profile::ResolvedProfileSelection {
            profile_id: llm_profile::DEFAULT_PROXY_PROFILE.to_string(),
            profile: build_profile(llm_profile::DEFAULT_PROXY_PROFILE, "", ""),
        };

        let resolved = resolve_execution_profile_selection(
            &store,
            proxy_selection,
            "https://happy.example",
            Some("happy-token".to_string()),
            "",
        )
        .expect("resolved");

        assert_eq!(resolved.profile_id, llm_profile::DEFAULT_PROXY_PROFILE);
        assert_eq!(resolved.base_url, "https://happy.example");
        assert_eq!(resolved.api_key, "happy-token");
        assert!(resolved.use_proxy);
    }
}
