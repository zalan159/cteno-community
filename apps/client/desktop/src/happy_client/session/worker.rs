use super::*;
use crate::session_message_codec::SessionMessageCodec;
use multi_agent_runtime_core::{AgentExecutor, SessionRef};

pub(super) struct BackgroundWorkerState {
    pub session_id: String,
    pub execution_state: ExecutionState,
    pub config: SessionAgentConfig,
    pub socket_for_response: Arc<HappySocket>,
    pub message_codec: SessionMessageCodec,
    pub perm_handler: Arc<PermissionHandler>,
    pub context_tokens: Arc<AtomicU32>,
    pub compression_threshold: Arc<AtomicU32>,
    /// Optional vendor executor for this session. The worker mirrors the live
    /// session connection so `handle_user_message` can drive every turn
    /// through `executor.send_message → ExecutorNormalizer`. Missing executor
    /// state is now treated as a turn error instead of triggering an
    /// in-process autonomous-agent fallback.
    pub executor: Option<Arc<dyn AgentExecutor>>,
    pub session_ref: Option<SessionRef>,
}

#[derive(Clone, Copy)]
pub(super) struct BackgroundWorkerOptions {
    pub worker_label: &'static str,
    pub auto_hibernate: bool,
    pub auto_rename_persona: bool,
}

#[derive(Clone)]
pub(super) enum WorkerExecutionMode {
    Background,
    LocalIpc {
        stream_callback: crate::llm::StreamCallback,
    },
}

impl WorkerExecutionMode {
    fn stream_callback(&self) -> Option<crate::llm::StreamCallback> {
        match self {
            Self::Background => None,
            Self::LocalIpc { stream_callback } => Some(stream_callback.clone()),
        }
    }
}

#[derive(Clone)]
pub(super) struct WorkerLoopOptions {
    pub worker_label: &'static str,
    pub auto_hibernate: bool,
    pub auto_rename_persona: bool,
    pub execution_mode: WorkerExecutionMode,
}

impl From<BackgroundWorkerOptions> for WorkerLoopOptions {
    fn from(options: BackgroundWorkerOptions) -> Self {
        Self {
            worker_label: options.worker_label,
            auto_hibernate: options.auto_hibernate,
            auto_rename_persona: options.auto_rename_persona,
            execution_mode: WorkerExecutionMode::Background,
        }
    }
}

pub(super) fn begin_queue_processing(session_id: &str, execution_state: &ExecutionState) -> bool {
    execution_state.try_begin_processing(session_id)
}

pub(super) fn finish_queue_processing(
    session_id: &str,
    execution_state: &ExecutionState,
    worker_label: &str,
    reason: &str,
) {
    execution_state.end_processing(session_id);
    log::info!(
        "[Agent] {} worker done for session {}, {}",
        worker_label,
        session_id,
        reason
    );
}

pub(super) fn finalize_worker_completion(
    session_id: &str,
    execution_state: &ExecutionState,
    last_success: bool,
    auto_hibernate: bool,
) {
    crate::agent_owner::notify_session_complete(session_id);
    maybe_schedule_continuous_browsing(session_id, execution_state, last_success);

    if auto_hibernate {
        maybe_auto_hibernate_worker(session_id, execution_state);
    }
}

pub(super) async fn run_worker_loop_if_idle(
    state: BackgroundWorkerState,
    options: WorkerLoopOptions,
) -> bool {
    if !begin_queue_processing(&state.session_id, &state.execution_state) {
        return false;
    }

    run_worker_loop(state, options).await;
    true
}

pub(super) fn spawn_worker_loop_if_idle(
    state: BackgroundWorkerState,
    options: WorkerLoopOptions,
) -> bool {
    if !begin_queue_processing(&state.session_id, &state.execution_state) {
        return false;
    }

    tokio::spawn(async move {
        run_worker_loop(state, options).await;
    });
    true
}

pub(super) fn spawn_background_queue_worker_if_idle(
    state: BackgroundWorkerState,
    options: BackgroundWorkerOptions,
) -> bool {
    spawn_worker_loop_if_idle(state, options.into())
}

pub(super) fn spawn_background_queue_worker(
    state: BackgroundWorkerState,
    options: BackgroundWorkerOptions,
) {
    tokio::spawn(async move {
        run_worker_loop(state, options.into()).await;
    });
}

async fn execute_worker_batch(
    session_id: &str,
    messages: &[AgentMessage],
    state: &BackgroundWorkerState,
    execution_mode: &WorkerExecutionMode,
) -> bool {
    let combined = messages
        .iter()
        .map(|message| message.content.clone())
        .collect::<Vec<_>>()
        .join("\n\n");

    let images =
        extract_images_from_messages(messages, &state.config.server_url, &state.config.auth_token)
            .await;
    let stream_callback = execution_mode.stream_callback();

    // Propagate the per-batch `local_id` when the batch is a single user
    // message. This lets the frontend's optimistic user bubble reconcile
    // against the DB row persisted by `ExecutorNormalizer::persist_user_message`
    // — merged multi-message batches fall back to `None` because the combined
    // text no longer maps to a single optimistic id.
    let combined_local_id = if messages.len() == 1 {
        messages
            .first()
            .and_then(|message| message.local_id.clone())
    } else {
        None
    };

    match execution_mode {
        WorkerExecutionMode::Background => {
            handle_user_message(
                session_id,
                &combined,
                &state.config,
                state.socket_for_response.clone(),
                &state.message_codec,
                state.perm_handler.clone(),
                state.execution_state.abort_flag.clone(),
                state.execution_state.thinking.clone(),
                state.context_tokens.clone(),
                state.compression_threshold.clone(),
                Some(state.execution_state.queue.clone()),
                images,
                combined_local_id.as_deref(),
                false,
                stream_callback,
                state.executor.clone(),
                state.session_ref.clone(),
            )
            .await
        }
        WorkerExecutionMode::LocalIpc { .. } => {
            handle_user_message_with_stream(
                session_id,
                &combined,
                &state.config,
                state.socket_for_response.clone(),
                &state.message_codec,
                state.perm_handler.clone(),
                state.execution_state.abort_flag.clone(),
                state.execution_state.thinking.clone(),
                state.context_tokens.clone(),
                state.compression_threshold.clone(),
                Some(state.execution_state.queue.clone()),
                images,
                combined_local_id.as_deref(),
                stream_callback,
                state.executor.clone(),
                state.session_ref.clone(),
            )
            .await
        }
    }
}

async fn run_worker_loop(state: BackgroundWorkerState, options: WorkerLoopOptions) {
    let mut last_success = true;
    loop {
        let messages = state.execution_state.queue.pop_all(&state.session_id);
        if messages.is_empty() {
            finish_queue_processing(
                &state.session_id,
                &state.execution_state,
                options.worker_label,
                "no more messages",
            );
            break;
        }

        let msg_count = messages.len();
        log::info!(
            "[Agent] Processing {} queued message(s) for {} session {}",
            msg_count,
            options.worker_label,
            state.session_id
        );

        last_success = execute_worker_batch(
            &state.session_id,
            &messages,
            &state,
            &options.execution_mode,
        )
        .await;

        if options.auto_rename_persona && last_success {
            let combined = messages
                .iter()
                .map(|message| message.content.clone())
                .collect::<Vec<_>>()
                .join("\n\n");
            maybe_auto_rename_persona_from_message(
                state.session_id.clone(),
                combined,
                state.config.global_api_key.clone(),
            );
        }

        if state.execution_state.queue.is_empty(&state.session_id) {
            finish_queue_processing(
                &state.session_id,
                &state.execution_state,
                options.worker_label,
                "queue empty after execution",
            );
            break;
        }

        state.execution_state.begin_processing(&state.session_id);
    }

    finalize_worker_completion(
        &state.session_id,
        &state.execution_state,
        last_success,
        options.auto_hibernate,
    );
}

pub(super) fn maybe_auto_rename_persona_from_message(
    session_id: String,
    message: String,
    api_key: String,
) {
    tokio::spawn(async move {
        if let Ok(pm) = crate::local_services::persona_manager() {
            if let Ok(Some(link)) = pm.store().get_persona_for_session(&session_id) {
                if link.session_type == crate::persona::models::PersonaSessionType::Chat {
                    if let Ok(Some(persona)) = pm.store().get_persona(&link.persona_id) {
                        if persona.name == "新对话" {
                            let msg_trimmed = message.trim();
                            if msg_trimmed.chars().count() <= 10 {
                                if let Err(e) =
                                    pm.store().update_name(&link.persona_id, msg_trimmed)
                                {
                                    log::warn!("[Persona] Auto-rename (short) failed: {}", e);
                                } else {
                                    log::info!(
                                        "[Persona] Auto-renamed persona {} to '{}' (short msg)",
                                        link.persona_id,
                                        msg_trimmed
                                    );
                                }
                            } else if let Err(e) = pm
                                .auto_rename_persona(&link.persona_id, msg_trimmed, &api_key)
                                .await
                            {
                                log::warn!("[Persona] Auto-rename failed: {}", e);
                            }
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_stream_callback() -> crate::llm::StreamCallback {
        Arc::new(|_| Box::pin(async {}))
    }

    #[test]
    fn background_execution_mode_has_no_stream_callback() {
        assert!(WorkerExecutionMode::Background.stream_callback().is_none());
    }

    #[test]
    fn local_ipc_execution_mode_clones_stream_callback() {
        let execution_mode = WorkerExecutionMode::LocalIpc {
            stream_callback: test_stream_callback(),
        };

        assert!(execution_mode.stream_callback().is_some());
    }
}
