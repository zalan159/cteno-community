use super::*;
use crate::autonomous_agent::{
    build_deferred_tools_context, fetch_native_tools_split, AcpMessageSender, PermissionChecker,
};
use crate::executor_normalizer::{
    surface_executor_failure, user_visible_executor_error, ExecutorNormalizer,
};
use crate::llm::{LLMClient, Tool};
use crate::llm_profile::ApiFormat;
use crate::service_init::AgentConfig;
use crate::session_message_codec::SessionMessageCodec;
use multi_agent_runtime_core::{AgentExecutor, SessionRef};

type DeferredToolSummary = (String, String, Option<String>);

pub(super) struct ResolvedExecutionProfile {
    pub(super) profile_id: String,
    pub(super) api_key: String,
    pub(super) base_url: String,
    pub(super) model: String,
    pub(super) temperature: f32,
    pub(super) max_tokens: u32,
    #[allow(dead_code)]
    pub(super) context_window_tokens: Option<u32>,
    pub(super) use_proxy: bool,
    pub(super) api_format: ApiFormat,
    pub(super) supports_vision: bool,
    pub(super) supports_computer_use: bool,
    pub(super) enable_thinking: bool,
    pub(super) is_free_model: bool,
    pub(super) supports_function_calling: bool,
    pub(super) supports_image_output: bool,
    #[allow(dead_code)]
    pub(super) compression_threshold: u32,
    pub(super) compress_client: LLMClient,
}

pub(super) struct PreparedAgentRuntime {
    pub(super) tools: Vec<Tool>,
    pub(super) all_agents: Vec<AgentConfig>,
    pub(super) sub_agent_ctx: crate::agent::executor::SubAgentContext,
    pub(super) effective_system_prompt: String,
    pub(super) runtime_context_messages: Vec<String>,
    pub(super) detected_persona_id: Option<String>,
    pub(super) detected_persona_workdir: Option<String>,
}

/// Build a runtime context message describing the current model's identity and capabilities.
/// Prevents text-only models from hallucinating multimodal abilities.
fn build_model_identity_context(
    model: &str,
    supports_vision: bool,
    supports_computer_use: bool,
) -> String {
    let mut lines = Vec::new();
    lines.push("<model_identity>".to_string());
    lines.push(format!("当前模型: {}", model));

    let mut capabilities = Vec::new();
    if supports_vision {
        capabilities.push("视觉/图片理解");
    }
    if supports_computer_use {
        capabilities.push("计算机操作");
    }

    if capabilities.is_empty() {
        lines.push(
            "能力: 纯文本模型。你无法直接查看或理解图片/截图，不要假装能看到图片内容。".to_string(),
        );
        lines.push(
            "如果用户发送图片或需要视觉理解，请通过 dispatch_task 派发给标记了 [视觉] 的模型处理。"
                .to_string(),
        );
    } else {
        lines.push(format!("能力: {}", capabilities.join("、")));
    }

    lines.push("</model_identity>".to_string());
    lines.join("\n")
}

fn vendor_native_agent_for_resolution(
    resolution: &crate::agent_kind::AgentKindResolution,
) -> Option<&str> {
    resolution
        .persona
        .as_ref()
        .and_then(|persona| persona.agent.as_deref())
        .or_else(|| {
            resolution
                .persona_link
                .as_ref()
                .and_then(|link| link.agent_type.as_deref())
        })
        .map(str::trim)
        .filter(|vendor| matches!(*vendor, "claude" | "codex" | "gemini"))
}

fn vendor_native_agent_for_session(
    session_id: &str,
    db_path: &std::path::Path,
    resolution: &crate::agent_kind::AgentKindResolution,
) -> Option<String> {
    let persisted_vendor = crate::agent_session::AgentSessionManager::new(db_path.to_path_buf())
        .get_session(session_id)
        .ok()
        .flatten()
        .map(|session| session.vendor.trim().to_string())
        .filter(|vendor| matches!(vendor.as_str(), "claude" | "codex" | "gemini"));

    persisted_vendor.or_else(|| vendor_native_agent_for_resolution(resolution).map(str::to_string))
}

fn api_format_for_vendor(vendor: &str) -> Option<ApiFormat> {
    match vendor {
        "claude" => Some(ApiFormat::Anthropic),
        "codex" => Some(ApiFormat::OpenAI),
        "gemini" => Some(ApiFormat::Gemini),
        _ => None,
    }
}

fn default_context_window_for_vendor(vendor: &str) -> u32 {
    match vendor {
        "claude" => 200_000,
        "codex" => 128_000,
        "gemini" => 1_000_000,
        _ => 128_000,
    }
}

async fn is_vendor_native_model_id(vendor: &str, model_id: &str) -> bool {
    crate::commands::collect_vendor_models(vendor)
        .await
        .map(|models| models.into_iter().any(|model| model.id == model_id))
        .unwrap_or(false)
}

async fn default_vendor_model_id(vendor: &str) -> Option<String> {
    crate::commands::collect_vendor_models(vendor)
        .await
        .ok()
        .and_then(|models| {
            models
                .iter()
                .find(|model| model.is_default)
                .map(|model| model.id.clone())
                .or_else(|| models.first().map(|model| model.id.clone()))
        })
}

pub(super) async fn resolve_execution_profile(
    session_id: &str,
    config: &SessionAgentConfig,
    resolution: &crate::agent_kind::AgentKindResolution,
    compression_threshold: &Arc<AtomicU32>,
) -> ResolvedExecutionProfile {
    let mut profile_id = config.profile_id.read().await.clone();
    let original_profile_id = profile_id.clone();

    log::info!(
        "[Session {}] Agent kind: {:?}, profile: {}",
        session_id,
        resolution.kind,
        profile_id
    );

    let resolved = resolution.default_profile_id(&profile_id);
    if resolved != profile_id {
        log::info!(
            "[Session {}] {:?} agent: overriding profile {} → {}",
            session_id,
            resolution.kind,
            profile_id,
            resolved
        );
        profile_id = resolved;
    }

    if let Err(e) = upsert_agent_session_profile_id(&config.db_path, session_id, &profile_id) {
        log::warn!(
            "[Session {}] Failed to persist session profile_id '{}': {}",
            session_id,
            profile_id,
            e
        );
    }

    let vendor_native_override = if let Some(vendor) =
        vendor_native_agent_for_session(session_id, &config.db_path, resolution)
    {
        if is_vendor_native_model_id(&vendor, &profile_id).await {
            Some(vendor)
        } else if let Some(fallback_model_id) = default_vendor_model_id(&vendor).await {
            log::warn!(
                "[Session {}] Vendor-native model '{}' is not available for vendor={}; falling back to '{}'",
                session_id,
                profile_id,
                vendor,
                fallback_model_id
            );
            profile_id = fallback_model_id;
            Some(vendor)
        } else {
            None
        }
    } else {
        None
    };

    if let Some(vendor) = vendor_native_override.as_deref() {
        if let Some(api_format) = api_format_for_vendor(vendor) {
            let context_window_tokens = Some(default_context_window_for_vendor(vendor));
            let threshold =
                crate::chat_compression::CompressionService::for_model_with_context_window(
                    &profile_id,
                    context_window_tokens,
                )
                .token_threshold();
            compression_threshold.store(threshold, Ordering::SeqCst);

            log::info!(
                "[Session {}] Preserving vendor-native model selection '{}' for vendor={} ahead of profile-store resolution",
                session_id,
                profile_id,
                vendor
            );
            log::info!(
                "Session {} using vendor-native model '{}': vendor={}, compression_threshold={}, api_format={:?}",
                session_id,
                profile_id,
                vendor,
                threshold,
                api_format
            );

            return ResolvedExecutionProfile {
                profile_id,
                api_key: config.global_api_key.clone(),
                base_url: String::new(),
                model: original_profile_id,
                temperature: 0.0,
                max_tokens: 8192,
                context_window_tokens,
                use_proxy: false,
                api_format,
                supports_vision: false,
                supports_computer_use: false,
                enable_thinking: false,
                is_free_model: false,
                supports_function_calling: false,
                supports_image_output: false,
                compression_threshold: threshold,
                compress_client: LLMClient::with_proxy_and_machine_id(
                    config.auth_token.clone(),
                    config.server_url.clone(),
                ),
            };
        }
    }

    let proxy_profiles = config.proxy_profiles.read().await;
    let profile_exists_locally = {
        let store = config.profile_store.read().await;
        store
            .get_profile_or_proxy(&profile_id, &proxy_profiles)
            .is_some()
    };

    {
        let store = config.profile_store.read().await;
        if !profile_exists_locally {
            let fallback_profile_id = store.default_profile_id.clone();
            log::warn!(
                "[Session {}] Profile '{}' not found (local/proxy). Falling back to default '{}'",
                session_id,
                profile_id,
                fallback_profile_id
            );
            profile_id = fallback_profile_id;
        }
    }

    if profile_id != original_profile_id {
        if let Err(e) = upsert_agent_session_profile_id(&config.db_path, session_id, &profile_id) {
            log::warn!(
                "[Session {}] Failed to persist fallback profile_id '{}': {}",
                session_id,
                profile_id,
                e
            );
        }
    }

    let is_proxy = crate::llm_profile::is_proxy_profile(&profile_id);
    let (
        mut api_key,
        mut base_url,
        model,
        temperature,
        max_tokens,
        context_window_tokens,
        use_proxy,
        api_format,
        supports_vision,
        supports_computer_use,
        enable_thinking,
        is_free_model,
        supports_function_calling,
        supports_image_output,
        compress_client,
    ) = {
        let store = config.profile_store.read().await;
        let profile = store
            .get_profile_or_proxy(&profile_id, &proxy_profiles)
            .unwrap_or_else(|| store.get_default().clone());
        let api_key = if profile.chat.api_key.is_empty() {
            config.global_api_key.clone()
        } else {
            profile.chat.api_key.clone()
        };
        let use_proxy = is_proxy || api_key.is_empty();
        let compress_client = if use_proxy {
            LLMClient::with_proxy_and_machine_id(
                config.auth_token.clone(),
                config.server_url.clone(),
            )
        } else {
            let compress_key = profile.resolve_compress_api_key(&config.global_api_key);
            LLMClient::with_base_url(compress_key, profile.compress.base_url.clone())
        };
        (
            api_key,
            profile.chat.base_url.clone(),
            profile.chat.model.clone(),
            profile.chat.temperature,
            profile.chat.max_tokens,
            profile.chat.context_window_tokens,
            use_proxy,
            profile.api_format.clone(),
            profile.supports_vision,
            profile.supports_computer_use,
            profile.thinking,
            profile.is_free,
            profile.supports_function_calling,
            profile.supports_image_output,
            compress_client,
        )
    };
    drop(proxy_profiles);

    let threshold = crate::chat_compression::CompressionService::for_model_with_context_window(
        &model,
        context_window_tokens,
    )
    .token_threshold();
    compression_threshold.store(threshold, Ordering::SeqCst);

    if use_proxy {
        log::info!(
            "Session {} using LLM proxy: server_url={}, model={}",
            session_id,
            config.server_url,
            model
        );
        api_key = config.auth_token.clone();
        base_url = config.server_url.clone();
    }

    log::info!(
        "Session {} using profile '{}': model={}, base_url={}, context_window_tokens={:?}, compression_threshold={}, proxy={}, api_format={:?}",
        session_id,
        profile_id,
        model,
        base_url,
        context_window_tokens,
        threshold,
        use_proxy,
        api_format
    );

    ResolvedExecutionProfile {
        profile_id,
        api_key,
        base_url,
        model,
        temperature,
        max_tokens,
        context_window_tokens,
        use_proxy,
        api_format,
        supports_vision,
        supports_computer_use,
        enable_thinking,
        is_free_model,
        supports_function_calling,
        supports_image_output,
        compression_threshold: threshold,
        compress_client,
    }
}

pub(super) async fn prepare_agent_runtime(
    session_id: &str,
    config: &SessionAgentConfig,
    resolution: &crate::agent_kind::AgentKindResolution,
    profile: &ResolvedExecutionProfile,
    acp_sender: AcpMessageSender,
    permission_checker: PermissionChecker,
    abort_flag: Arc<AtomicBool>,
    thinking_flag: Arc<AtomicU8>,
) -> PreparedAgentRuntime {
    let workspace_skills_dir = resolution.workdir().map(|wd| {
        let expanded = shellexpand::tilde(wd).to_string();
        std::path::PathBuf::from(expanded)
            .join(".cteno")
            .join("skills")
    });
    let enabled_skills = crate::service_init::load_all_skills(
        &config.builtin_skills_dir,
        &config.user_skills_dir,
        workspace_skills_dir.as_deref(),
    );

    let (immediate_tools, deferred_summaries) = fetch_native_tools_split().await;
    let active_mcp_ids = config.session_mcp_server_ids.read().await;
    let filtered_native_tools: Vec<Tool> = immediate_tools
        .into_iter()
        .filter(|tool| {
            if tool.name.starts_with("mcp__") {
                let server_name = tool.name.split("__").nth(1).unwrap_or("");
                !active_mcp_ids.is_empty() && active_mcp_ids.contains(&server_name.to_string())
            } else {
                true
            }
        })
        .collect();
    let filtered_deferred_summaries: Vec<DeferredToolSummary> = deferred_summaries
        .into_iter()
        .filter(|(id, _, _)| {
            if id.starts_with("mcp__") {
                let server_name = id.split("__").nth(1).unwrap_or("");
                !active_mcp_ids.is_empty() && active_mcp_ids.contains(&server_name.to_string())
            } else {
                true
            }
        })
        .collect();
    drop(active_mcp_ids);

    let workspace_agents_dir = resolution.workdir().map(|wd| {
        let expanded = shellexpand::tilde(wd).to_string();
        std::path::PathBuf::from(expanded)
            .join(".cteno")
            .join("agents")
    });
    let all_agents = crate::service_init::load_all_agents(
        &config.builtin_agents_dir,
        &config.user_agents_dir,
        workspace_agents_dir.as_deref(),
    );
    let agent_tools = crate::autonomous_agent::build_agent_tools(&all_agents);
    log::info!(
        "Session {} loaded {} immediate tools + {} deferred tools + {} agent tools",
        session_id,
        filtered_native_tools.len(),
        filtered_deferred_summaries.len(),
        agent_tools.len()
    );

    let mut tools = filtered_native_tools;
    tools.extend(agent_tools);
    crate::agent_kind::apply_tool_filter(&mut tools, resolution);

    if !profile.supports_computer_use {
        let before = tools.len();
        tools.retain(|tool| tool.name != "computer_use");
        if before != tools.len() {
            log::info!(
                "Session {} excluded computer_use (profile '{}' does not support it)",
                session_id,
                profile.profile_id
            );
        }
    }

    if let Some(ref allowed_ids) = config.allowed_tool_ids {
        let before = tools.len();
        tools.retain(|tool| allowed_ids.iter().any(|id| tool.name == *id));
        log::info!(
            "Session {} filtered tools by allowed_tool_ids: {} -> {} tools",
            session_id,
            before,
            tools.len()
        );
    }

    // Legacy desktop-side host spawning path (pre-stdio). Build a factory
    // that ignores the subagent session id and reuses the parent's
    // pre-built sender as-is — mirrors the historical behavior at this
    // callsite. New flows (cteno-agent stdio bootstrap) build a real
    // session-tagged factory in `hooks_mvp::make_subagent_acp_sender_factory`.
    let acp_sender_for_factory = acp_sender.clone();
    let sub_agent_ctx = crate::agent::executor::SubAgentContext {
        db_path: config.db_path.clone(),
        builtin_skills_dir: config.builtin_skills_dir.clone(),
        user_skills_dir: config.user_skills_dir.clone(),
        global_api_key: profile.api_key.clone(),
        default_base_url: profile.base_url.clone(),
        profile_id: Some(profile.profile_id.clone()),
        use_proxy: profile.use_proxy,
        profile_model: Some(profile.model.clone()),
        acp_sender_factory: Some(std::sync::Arc::new(move |_sub_session_id: String| {
            acp_sender_for_factory.clone()
        })),
        permission_checker: Some(permission_checker.clone()),
        abort_flag: Some(abort_flag),
        thinking_flag: Some(thinking_flag),
        api_format: profile.api_format.clone(),
        sandbox_policy: None,
    };

    let base_prompt = if profile.supports_function_calling {
        config.system_prompt.clone()
    } else {
        log::info!(
            "[Session] Rebuilding system prompt without tool guidance (supportsFunctionCalling=false)"
        );
        crate::system_prompt::build_system_prompt(&crate::system_prompt::PromptOptions {
            include_tool_style: false,
            ..Default::default()
        })
    };
    let (effective_system_prompt, detected_persona_id, detected_persona_workdir) =
        crate::agent_kind::build_agent_prompt(resolution, &base_prompt);

    let mut runtime_context_messages = vec![crate::system_prompt::build_runtime_datetime_context(
        &effective_system_prompt,
    )];

    if let Some(skill_index) = crate::service_init::build_skill_index_message(
        &enabled_skills,
        profile.context_window_tokens.unwrap_or(128000),
    ) {
        runtime_context_messages.push(skill_index);
    }

    if let Some(ref pre_skill_ids) = config.pre_activated_skill_ids {
        for skill_id in pre_skill_ids {
            if let Some(skill) = enabled_skills
                .iter()
                .find(|skill| skill.id.eq_ignore_ascii_case(skill_id))
            {
                let instructions = skill
                    .instructions
                    .clone()
                    .unwrap_or_else(|| skill.description.clone());
                let activated = format!(
                    "<activated_skill id=\"{}\" name=\"{}\">\n  <description>\n    {}\n  </description>\n\n  <instructions>\n{}\n  </instructions>\n</activated_skill>",
                    skill.id, skill.name, skill.description, instructions,
                );
                runtime_context_messages.push(activated);
                log::info!(
                    "[Session] Pre-activated skill: {} for session {}",
                    skill.id,
                    session_id
                );
            } else {
                log::warn!(
                    "[Session] Pre-activated skill not found: {} for session {}",
                    skill_id,
                    session_id
                );
            }
        }
    }

    runtime_context_messages.push(build_model_identity_context(
        &profile.model,
        profile.supports_vision,
        profile.supports_computer_use,
    ));

    if profile.supports_function_calling {
        if let Some(deferred_ctx) = build_deferred_tools_context(&filtered_deferred_summaries) {
            runtime_context_messages.push(deferred_ctx);
        }
    } else {
        runtime_context_messages.push(
            "<system-reminder>\nIMPORTANT: You do NOT have any tools available in this session. \
             Do NOT attempt to call any tools or functions. Respond directly with text and/or images only.\n</system-reminder>".to_string()
        );
    }

    if detected_persona_id.is_some() {
        let persona_workspace_agents_dir = detected_persona_workdir.as_ref().map(|wd| {
            let expanded = shellexpand::tilde(wd).to_string();
            std::path::PathBuf::from(expanded)
                .join(".cteno")
                .join("agents")
        });
        let persona_agents = crate::service_init::load_all_agents(
            &config.builtin_agents_dir,
            &config.user_agents_dir,
            persona_workspace_agents_dir.as_deref(),
        );
        if let Some(agents_ctx) =
            crate::persona::prompt::build_agents_context_message(&persona_agents)
        {
            runtime_context_messages.push(agents_ctx);
        }

        let proxy_profiles = config.proxy_profiles.read().await;
        let store = config.profile_store.read().await;
        let mut profiles = Vec::new();
        for profile in proxy_profiles.iter() {
            profiles.push(crate::persona::prompt::ProfileInfo {
                id: profile.id.clone(),
                name: profile.name.clone(),
                is_proxy: true,
                supports_vision: profile.supports_vision,
                supports_computer_use: profile.supports_computer_use,
            });
        }
        for profile in store.profiles.iter() {
            profiles.push(crate::persona::prompt::ProfileInfo {
                id: profile.id.clone(),
                name: profile.name.clone(),
                is_proxy: false,
                supports_vision: profile.supports_vision,
                supports_computer_use: profile.supports_computer_use,
            });
        }
        if let Some(models_ctx) = crate::persona::prompt::build_models_context_message(&profiles) {
            runtime_context_messages.push(models_ctx);
        }
    }

    PreparedAgentRuntime {
        tools,
        all_agents,
        sub_agent_ctx,
        effective_system_prompt,
        runtime_context_messages,
        detected_persona_id,
        detected_persona_workdir,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_user_message_with_stream(
    session_id: &str,
    user_text: &str,
    config: &SessionAgentConfig,
    socket: Arc<HappySocket>,
    message_codec: &SessionMessageCodec,
    permission_handler: Arc<PermissionHandler>,
    abort_flag: Arc<AtomicBool>,
    thinking_flag: Arc<AtomicU8>,
    context_tokens: Arc<AtomicU32>,
    compression_threshold: Arc<AtomicU32>,
    queue: Option<Arc<crate::agent_queue::AgentMessageQueue>>,
    user_images: Option<Vec<crate::llm::ImageSource>>,
    user_local_id: Option<&str>,
    stream_callback: Option<StreamCallback>,
    executor: Option<Arc<dyn AgentExecutor>>,
    session_ref: Option<SessionRef>,
) -> bool {
    handle_user_message(
        session_id,
        user_text,
        config,
        socket,
        message_codec,
        permission_handler,
        abort_flag,
        thinking_flag,
        context_tokens,
        compression_threshold,
        queue,
        user_images,
        user_local_id,
        false,
        stream_callback,
        executor,
        session_ref,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_user_message(
    session_id: &str,
    user_text: &str,
    config: &SessionAgentConfig,
    socket: Arc<HappySocket>,
    message_codec: &SessionMessageCodec,
    permission_handler: Arc<PermissionHandler>,
    abort_flag: Arc<AtomicBool>,
    thinking_flag: Arc<AtomicU8>,
    context_tokens: Arc<AtomicU32>,
    compression_threshold: Arc<AtomicU32>,
    _queue: Option<Arc<crate::agent_queue::AgentMessageQueue>>,
    user_images: Option<Vec<crate::llm::ImageSource>>,
    user_local_id: Option<&str>,
    local_origin: bool,
    stream_callback: Option<StreamCallback>,
    executor: Option<Arc<dyn AgentExecutor>>,
    session_ref: Option<SessionRef>,
) -> bool {
    log::info!("Agent executing for session {}: {}", session_id, user_text);

    // Keep a handle to the permission handler for executor-path normalizer
    // construction (TurnPreparation::build consumes its own clone).
    let perm_handler_for_exec = permission_handler.clone();

    // Host-side pre-work (profile resolve, balance check, task_started,
    // closure construction, runtime prep). Extracted to `TurnPreparation`
    // (T11a) so the executor path (T11b) can re-use it.
    let prep = match super::turn_preparation::TurnPreparation::build(
        session_id,
        config,
        &socket,
        message_codec,
        permission_handler,
        abort_flag.clone(),
        thinking_flag.clone(),
        &compression_threshold,
        stream_callback.clone(),
        local_origin,
    )
    .await
    {
        super::turn_preparation::TurnPreparationOutcome::Ready(p) => p,
        super::turn_preparation::TurnPreparationOutcome::Aborted { success } => {
            return success;
        }
    };

    let Some(executor) = executor else {
        return super::turn_preparation::TurnPostWork::finalize_executor_path(
            session_id,
            &socket,
            message_codec,
            local_origin,
            &prep.task_id,
            thinking_flag,
            stream_callback.clone(),
            Err(format!(
                "Session {session_id} missing executor; in-process execution path removed"
            )),
        )
        .await;
    };
    let Some(session_ref) = session_ref else {
        return super::turn_preparation::TurnPostWork::finalize_executor_path(
            session_id,
            &socket,
            message_codec,
            local_origin,
            &prep.task_id,
            thinking_flag,
            stream_callback.clone(),
            Err(format!(
                "Session {session_id} missing session_ref; in-process execution path removed"
            )),
        )
        .await;
    };

    let finalize_stream_callback = stream_callback.clone();
    let outcome = run_executor_turn(
        session_id,
        user_text,
        user_images.as_deref(),
        user_local_id,
        socket.clone(),
        *message_codec,
        stream_callback,
        perm_handler_for_exec,
        prep.task_id.clone(),
        executor,
        session_ref,
        config.server_url.clone(),
        config.auth_token.clone(),
        config.db_path.clone(),
        context_tokens,
        compression_threshold,
    )
    .await;

    super::turn_preparation::TurnPostWork::finalize_executor_path(
        session_id,
        &socket,
        message_codec,
        local_origin,
        &prep.task_id,
        thinking_flag,
        finalize_stream_callback,
        outcome,
    )
    .await
}

/// Drive one turn through the executor path: build a `UserMessage`,
/// open the `EventStream`, and feed every event through the session's
/// normalizer until we see `TurnComplete` (or a fatal error).
///
/// Returns `Ok(())` on clean turn completion, `Err(msg)` otherwise — the
/// caller maps that into a user-visible error response through
/// `TurnPostWork::finalize_executor_path`.
async fn run_executor_turn(
    session_id: &str,
    user_text: &str,
    _user_images: Option<&[crate::llm::ImageSource]>,
    user_local_id: Option<&str>,
    socket: Arc<HappySocket>,
    message_codec: SessionMessageCodec,
    stream_callback: Option<StreamCallback>,
    permission_handler: Arc<PermissionHandler>,
    task_id: String,
    executor: Arc<dyn AgentExecutor>,
    session_ref: SessionRef,
    server_url: String,
    auth_token: String,
    db_path: std::path::PathBuf,
    context_tokens: Arc<AtomicU32>,
    compression_threshold: Arc<AtomicU32>,
) -> Result<(), String> {
    use futures_util::StreamExt;
    use multi_agent_runtime_core::UserMessage;

    // TODO(T11b/T11c): map `user_images` into `UserMessage.attachments`
    // once the executor surface supports image attachments end-to-end.
    let user_message = UserMessage {
        content: user_text.to_string(),
        task_id: Some(task_id.clone()),
        attachments: Vec::new(),
        parent_tool_use_id: None,
        injected_tools: Vec::new(),
    };

    let normalizer = ExecutorNormalizer::new(
        session_id.to_string(),
        socket,
        message_codec,
        stream_callback,
        permission_handler,
        task_id,
        executor.clone(),
        session_ref.clone(),
        server_url,
        auth_token,
        db_path,
        Some(context_tokens),
        Some(compression_threshold),
    );

    // Persist the user turn to the local `agent_sessions.messages` column
    // *before* handing off to the vendor. Vendor event streams only echo
    // assistant/tool output, so without this the local-mode DB ends up
    // with assistant-only history and reload after restart shows no user
    // messages (P0 in the persistence audit).
    //
    // Forward the caller-supplied `local_id` so the frontend's optimistic
    // user bubble (inserted at input time, tagged with the same id) can
    // reconcile against the server-side row instead of rendering twice.
    normalizer
        .persist_user_message(user_text, user_local_id)
        .map_err(|e| format!("persist user message failed: {e}"))?;

    let stream = match executor.send_message(&session_ref, user_message).await {
        Ok(stream) => stream,
        Err(error) => {
            surface_executor_failure(&normalizer, &error).await?;
            return Ok(());
        }
    };
    let mut stream = Box::pin(stream);

    log::info!(
        "[Session {}] executor path: stream opened (vendor={})",
        session_id,
        session_ref.vendor
    );

    let mut event_count: u32 = 0;
    while let Some(event) = stream.next().await {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                surface_executor_failure(&normalizer, &error).await?;
                return Ok(());
            }
        };
        event_count += 1;
        log::info!(
            "[Session {}] executor event #{}: {:?}",
            session_id,
            event_count,
            std::mem::discriminant(&event)
        );
        let done = normalizer.process_event(event).await?;
        if done {
            break;
        }
    }
    log::info!(
        "[Session {}] executor stream ended after {} events",
        session_id,
        event_count
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_session::AgentSessionManager;
    use crate::executor_registry::ExecutorRegistry;
    use crate::happy_client::permission::{
        PermissionHandler, PermissionMode as SessionPermissionMode,
    };
    use crate::session_store_impl::build_session_store;
    use cteno_host_session_wire::ConnectionType;
    use futures_util::StreamExt;
    use multi_agent_runtime_core::{
        DeltaKind, ModelSpec, PermissionMode, SpawnSessionSpec, UserMessage,
    };
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, Once};
    use tempfile::tempdir;
    use tokio::time::{timeout, Duration};
    use uuid::Uuid;

    const LIVE_LOCAL_STREAMING_ENV: &str = "CTENO_LIVE_LOCAL_EXECUTOR_STREAMING";
    const BANNED_ERROR_SUBSTRINGS: [&str; 2] = ["session not found", "stdout closed mid-turn"];
    static TEST_NO_PROXY: Once = Once::new();

    fn live_tests_enabled() -> bool {
        std::env::var(LIVE_LOCAL_STREAMING_ENV).ok().as_deref() == Some("1")
    }

    fn install_test_no_proxy() {
        TEST_NO_PROXY.call_once(|| {
            std::env::set_var("NO_PROXY", "*");
            std::env::set_var("no_proxy", "*");
        });
    }

    fn ensure_cteno_agent_path() {
        if std::env::var_os("CTENO_AGENT_PATH").is_some() {
            return;
        }

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidates = [
            manifest_dir
                .join("target")
                .join("debug")
                .join(if cfg!(windows) {
                    "cteno-agent.exe"
                } else {
                    "cteno-agent"
                }),
            manifest_dir
                .join("target")
                .join("release")
                .join(if cfg!(windows) {
                    "cteno-agent.exe"
                } else {
                    "cteno-agent"
                }),
            manifest_dir
                .join("..")
                .join("..")
                .join("..")
                .join("packages")
                .join("agents")
                .join("rust")
                .join("crates")
                .join("cteno-agent-stdio")
                .join("target")
                .join("release")
                .join(if cfg!(windows) {
                    "cteno-agent.exe"
                } else {
                    "cteno-agent"
                }),
        ];

        let Some(path) = candidates.into_iter().find(|path| path.is_file()) else {
            panic!("cteno-agent binary missing; set CTENO_AGENT_PATH or build the sidecar first");
        };

        std::env::set_var("CTENO_AGENT_PATH", path);
    }

    fn live_model_for_vendor(vendor: &str) -> Option<ModelSpec> {
        match vendor {
            "claude" => Some(ModelSpec {
                provider: "anthropic".to_string(),
                model_id: std::env::var("MULTI_AGENT_TEST_CLAUDE_MODEL")
                    .unwrap_or_else(|_| "claude-haiku-4-5".to_string()),
                reasoning_effort: None,
                temperature: None,
            }),
            "codex" => Some(ModelSpec {
                provider: "openai".to_string(),
                model_id: std::env::var("MULTI_AGENT_TEST_CODEX_MODEL")
                    .unwrap_or_else(|_| "gpt-5.4-mini".to_string()),
                reasoning_effort: None,
                temperature: None,
            }),
            _ => None,
        }
    }

    fn assert_no_banned_errors(label: &str, messages: &[String]) {
        let lowered: Vec<String> = messages.iter().map(|msg| msg.to_lowercase()).collect();
        for banned in BANNED_ERROR_SUBSTRINGS {
            assert!(
                lowered.iter().all(|msg| !msg.contains(banned)),
                "{label} emitted banned error substring '{banned}': {messages:?}",
            );
        }
    }

    async fn verify_vendor_local_streaming(
        vendor: &str,
        registry: Arc<ExecutorRegistry>,
        db_path: PathBuf,
        workdir: PathBuf,
    ) {
        let executor = registry
            .resolve(vendor)
            .unwrap_or_else(|err| panic!("resolve({vendor}) failed: {err}"));

        let session_id = format!("live-local-{vendor}-{}", Uuid::new_v4());
        let spec = SpawnSessionSpec {
            workdir,
            system_prompt: Some(
                "Reply directly in plain text. Do not use tools, bullets, or markdown.".to_string(),
            ),
            model: live_model_for_vendor(vendor),
            permission_mode: PermissionMode::BypassPermissions,
            allowed_tools: None,
            additional_directories: Vec::new(),
            env: BTreeMap::new(),
            agent_config: serde_json::json!({}),
            resume_hint: None,
        };

        let session_ref = timeout(Duration::from_secs(30), executor.spawn_session(spec))
            .await
            .unwrap_or_else(|_| panic!("spawn_session timed out for vendor {vendor}"))
            .unwrap_or_else(|err| panic!("spawn_session failed for vendor {vendor}: {err}"));

        let socket = Arc::new(HappySocket::local(ConnectionType::SessionScoped {
            session_id: session_id.clone(),
        }));
        let permission_handler = Arc::new(PermissionHandler::new(session_id.clone(), 0));
        permission_handler.set_mode(SessionPermissionMode::BypassPermissions);

        let callback_events = Arc::new(Mutex::new(Vec::<Value>::new()));
        let callback_sink = callback_events.clone();
        let stream_callback: crate::llm::StreamCallback = Arc::new(move |delta: Value| {
            callback_sink.lock().unwrap().push(delta);
            Box::pin(async {})
        });

        let task_id = format!("task-{vendor}");
        let normalizer = ExecutorNormalizer::new(
            session_id.clone(),
            socket,
            SessionMessageCodec::plaintext(),
            Some(stream_callback),
            permission_handler,
            task_id.clone(),
            executor.clone(),
            session_ref.clone(),
            "http://127.0.0.1:1".to_string(),
            "local-test".to_string(),
            db_path.clone(),
            None,
            None,
        );

        let user_message = UserMessage {
            content: "Write exactly two short sentences explaining why streaming replies feel responsive.".to_string(),
            task_id: Some(task_id.clone()),
            attachments: Vec::new(),
            parent_tool_use_id: None,
            injected_tools: Vec::new(),
        };

        let mut stream = timeout(
            Duration::from_secs(30),
            executor.send_message(&session_ref, user_message),
        )
        .await
        .unwrap_or_else(|_| panic!("send_message timed out for vendor {vendor}"))
        .unwrap_or_else(|err| panic!("send_message failed for vendor {vendor}: {err}"));

        let mut saw_text_delta = false;
        let mut turn_usage = None;
        let mut executor_errors = Vec::new();

        loop {
            let next_event = timeout(Duration::from_secs(180), stream.next())
                .await
                .unwrap_or_else(|_| panic!("event stream stalled for vendor {vendor}"));
            let Some(event) = next_event else {
                break;
            };
            let event = event
                .unwrap_or_else(|err| panic!("executor stream error for vendor {vendor}: {err}"));

            match &event {
                multi_agent_runtime_core::ExecutorEvent::StreamDelta { kind, content }
                    if *kind == DeltaKind::Text && !content.trim().is_empty() =>
                {
                    saw_text_delta = true;
                }
                multi_agent_runtime_core::ExecutorEvent::TurnComplete { usage, .. } => {
                    if matches!(vendor, "claude" | "codex" | "gemini") {
                        assert!(
                            usage.input_tokens > 0,
                            "{vendor} TurnComplete input_tokens should be non-zero: {usage:?}",
                        );
                        assert!(
                            usage.output_tokens > 0,
                            "{vendor} TurnComplete output_tokens should be non-zero: {usage:?}",
                        );
                    }
                    turn_usage = Some(usage.clone());
                }
                multi_agent_runtime_core::ExecutorEvent::Error { message, .. } => {
                    executor_errors.push(message.clone());
                }
                _ => {}
            }

            let done = normalizer
                .process_event(event)
                .await
                .unwrap_or_else(|err| panic!("normalizer failed for vendor {vendor}: {err}"));
            if done {
                break;
            }
        }

        executor
            .close_session(&session_ref)
            .await
            .unwrap_or_else(|err| panic!("close_session failed for vendor {vendor}: {err}"));

        assert!(saw_text_delta, "{vendor} never emitted a text delta");
        assert!(
            turn_usage.is_some(),
            "{vendor} never produced a TurnComplete usage snapshot",
        );
        assert_no_banned_errors(&format!("{vendor} executor"), &executor_errors);

        let callback_events = callback_events.lock().unwrap().clone();
        assert!(
            callback_events
                .iter()
                .any(|event| event.get("type").and_then(Value::as_str) == Some("text-delta")),
            "{vendor} stream callback never emitted text-delta: {callback_events:?}",
        );
        assert!(
            callback_events
                .iter()
                .any(|event| event.get("type").and_then(Value::as_str) == Some("stream-end")),
            "{vendor} stream callback never emitted stream-end: {callback_events:?}",
        );
        assert!(
            callback_events
                .iter()
                .any(|event| event.get("type").and_then(Value::as_str) == Some("finished")),
            "{vendor} stream callback never emitted finished: {callback_events:?}",
        );

        let callback_errors: Vec<String> = callback_events
            .iter()
            .filter(|event| event.get("type").and_then(Value::as_str) == Some("error"))
            .filter_map(|event| {
                event
                    .get("message")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .collect();
        assert_no_banned_errors(&format!("{vendor} callback"), &callback_errors);

        let manager = AgentSessionManager::new(db_path);
        let session = manager
            .get_session(&session_id)
            .unwrap_or_else(|err| {
                panic!("load persisted session failed for vendor {vendor}: {err}")
            })
            .unwrap_or_else(|| panic!("persisted session missing for vendor {vendor}"));
        assert_eq!(session.vendor, vendor);

        let persisted_blob = session
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            persisted_blob.contains("\"type\":\"task_complete\""),
            "{vendor} persisted messages missing task_complete: {persisted_blob}",
        );
    }

    #[tokio::test]
    async fn live_local_executor_streaming_regression() {
        if !live_tests_enabled() {
            eprintln!(
                "[skip] set {LIVE_LOCAL_STREAMING_ENV}=1 to run local executor streaming regression"
            );
            return;
        }

        install_test_no_proxy();
        ensure_cteno_agent_path();

        let temp = tempdir().expect("temp dir");
        crate::db::init_at_data_dir(temp.path()).expect("test db init");
        let db_path = temp.path().join("db").join("cteno.db");
        let registry = Arc::new(
            ExecutorRegistry::build(build_session_store(db_path.clone()))
                .await
                .expect("build executor registry"),
        );

        let available_vendors = registry.available_vendors();
        for vendor in ["cteno", "claude", "codex", "gemini"] {
            if !available_vendors.contains(&vendor) {
                continue;
            }
            let workdir = temp.path().join(format!("workdir-{vendor}"));
            std::fs::create_dir_all(&workdir).expect("vendor workdir");
            verify_vendor_local_streaming(vendor, registry.clone(), db_path.clone(), workdir).await;
        }
    }
}
