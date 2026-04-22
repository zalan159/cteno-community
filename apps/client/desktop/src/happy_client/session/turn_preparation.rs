//! Host-side pre/post work for an executor-backed user-message turn.
//!
//! `TurnPreparation::build` keeps the small amount of host orchestration that
//! still happens before `AgentExecutor::send_message`: profile resolution,
//! balance pre-check, and `task_started` ACP emission.

use super::execution::resolve_execution_profile;
use super::*;
use crate::session_message_codec::SessionMessageCodec;

/// Outcome of `TurnPreparation::build`. Either we're ready to run the turn, or
/// we aborted early (for example because balance was depleted) and the caller
/// should return the supplied success flag without invoking the executor.
pub(super) enum TurnPreparationOutcome {
    Ready(TurnPreparation),
    Aborted { success: bool },
}

/// Host-side state required after pre-work completes.
pub(super) struct TurnPreparation {
    pub task_id: String,
}

impl TurnPreparation {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn build(
        session_id: &str,
        config: &SessionAgentConfig,
        socket: &Arc<HappySocket>,
        message_codec: &SessionMessageCodec,
        _permission_handler: Arc<PermissionHandler>,
        _abort_flag: Arc<AtomicBool>,
        thinking_flag: Arc<AtomicU8>,
        compression_threshold: &Arc<AtomicU32>,
        _override_stream_callback: Option<StreamCallback>,
        local_origin: bool,
    ) -> TurnPreparationOutcome {
        let resolution = crate::agent_kind::resolve_agent_kind(session_id);
        let profile =
            resolve_execution_profile(session_id, config, &resolution, compression_threshold).await;

        if profile.use_proxy && !profile.is_free_model {
            match check_proxy_balance(&config.server_url, &config.auth_token).await {
                Ok(balance) if balance <= 0.0 => {
                    let error_msg = "余额已用完，请前往设置页充值后继续使用。";
                    log::warn!(
                        "Session {} balance insufficient (¥{:.2}), aborting agent",
                        session_id,
                        balance
                    );

                    let task_id = uuid::Uuid::new_v4().to_string();
                    let _ = send_acp_message(
                        socket,
                        session_id,
                        json!({ "type": "task_started", "id": task_id }),
                        message_codec,
                    )
                    .await;
                    let _ = send_agent_response(
                        socket,
                        session_id,
                        error_msg,
                        message_codec,
                        local_origin,
                    )
                    .await;
                    let _ = send_acp_message(
                        socket,
                        session_id,
                        json!({ "type": "task_complete", "id": task_id }),
                        message_codec,
                    )
                    .await;
                    thinking_flag.store(0, Ordering::SeqCst);
                    return TurnPreparationOutcome::Aborted { success: false };
                }
                Err(e) => {
                    log::warn!(
                        "Session {} balance pre-check failed (continuing anyway): {}",
                        session_id,
                        e
                    );
                }
                _ => {}
            }
        }

        let task_id = uuid::Uuid::new_v4().to_string();
        if let Err(e) = send_acp_message(
            socket,
            session_id,
            json!({ "type": "task_started", "id": task_id }),
            message_codec,
        )
        .await
        {
            log::warn!("Failed to send task_started ACP: {}", e);
        } else {
            log::info!("ACP message sent: type=task_started, id={}", task_id);
        }

        TurnPreparationOutcome::Ready(TurnPreparation { task_id })
    }
}

/// Post-turn host work for executor-backed turns.
pub(super) struct TurnPostWork;

impl TurnPostWork {
    /// The executor path's normalizer emits its own `task_complete`,
    /// agent-response, and usage frames on each `TurnComplete` event, so the
    /// host only needs to:
    /// 1. Clear the thinking flag so the UI spinner stops.
    /// 2. Log the outcome.
    /// 3. On error, surface a user-visible error response and synthesize a
    ///    final `task_complete` ACP if the stream failed before completion.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn finalize_executor_path(
        session_id: &str,
        socket: &Arc<HappySocket>,
        message_codec: &SessionMessageCodec,
        local_origin: bool,
        task_id: &str,
        thinking_flag: Arc<AtomicU8>,
        _stream_callback: Option<StreamCallback>,
        outcome: Result<(), String>,
    ) -> bool {
        thinking_flag.store(0, Ordering::SeqCst);
        match outcome {
            Ok(()) => {
                log::info!(
                    "[Session {}] executor path turn complete (task_id={})",
                    session_id,
                    task_id
                );
                true
            }
            Err(e) => {
                log::error!("[Session {}] executor path turn failed: {}", session_id, e);
                let error_msg = if e.contains("余额不足") || e.contains("balance_insufficient")
                {
                    "余额已用完，请前往设置页充值后继续使用。".to_string()
                } else {
                    format!("Agent error: {}", e)
                };
                if let Err(send_err) =
                    send_agent_response(socket, session_id, &error_msg, message_codec, local_origin)
                        .await
                {
                    log::error!(
                        "[Session {}] Failed to send executor error response: {}",
                        session_id,
                        send_err
                    );
                }
                if let Err(err) = send_acp_message(
                    socket,
                    session_id,
                    json!({ "type": "task_complete", "id": task_id }),
                    message_codec,
                )
                .await
                {
                    log::warn!(
                        "[Session {}] Failed to send executor-path task_complete ACP: {}",
                        session_id,
                        err
                    );
                }
                false
            }
        }
    }
}
