//! Agent RPC Handler
//!
//! Provides RPC handler for `agent.execute` using the desktop executor
//! registry rather than the removed in-process autonomous-agent path.

use crate::agent_session::{AgentSession, AgentSessionManager};
use crate::executor_normalizer::user_visible_executor_error;
use crate::llm::Tool;
use futures_util::StreamExt;
use multi_agent_runtime_core::{
    DeltaKind, ModelSpec, NativeSessionId, PermissionMode, ResumeHints, SpawnSessionSpec,
    UserMessage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

const AGENT_RPC_VENDOR: &str = "cteno";
const AGENT_RPC_MODEL_PROVIDER: &str = "deepseek";

/// Agent execute request parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExecuteParams {
    /// User task/prompt to execute
    pub task: String,

    /// Optional session ID for conversation continuity
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Optional user ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,

    /// Optional agent ID (defaults to "worker")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

/// Agent execute response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExecuteResponse {
    /// Final agent response
    pub response: String,

    /// Session ID used
    pub session_id: String,

    /// Number of ReAct iterations
    pub iteration_count: usize,

    /// Intermediate messages (tool execution progress)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub intermediate_messages: Vec<String>,
}

/// Agent RPC Handler Configuration
pub struct AgentRpcConfig {
    pub db_path: PathBuf,
    pub api_key: String,
    pub model: String,
    pub system_prompt: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

impl AgentRpcConfig {
    /// Create agent RPC handler that can be registered with RpcRegistry
    pub fn create_handler(self) -> impl Fn(Value) -> Result<Value, String> + Send + Sync + 'static {
        move |params: Value| {
            let execute_params: AgentExecuteParams =
                serde_json::from_value(params).map_err(|e| format!("Invalid params: {e}"))?;

            log::info!(
                "🤖 Agent RPC: task={}, session_id={:?}",
                execute_params.task,
                execute_params.session_id
            );

            let tools: Vec<Tool> = Vec::new();
            let runtime_context_messages =
                vec![crate::system_prompt::build_runtime_datetime_context(
                    &self.system_prompt,
                )];

            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    execute_agent_rpc_via_executor(
                        &self,
                        &execute_params,
                        runtime_context_messages.clone(),
                        &tools,
                    )
                    .await
                })
            });

            match result {
                Ok(agent_result) => {
                    log::info!(
                        "✅ Agent RPC success: iterations={}, session={}",
                        agent_result.iteration_count,
                        agent_result.session_id
                    );
                    Ok(serde_json::to_value(agent_result).unwrap_or_default())
                }
                Err(e) => {
                    log::error!("❌ Agent RPC error: {e}");
                    Err(e)
                }
            }
        }
    }
}

fn restored_native_session_id(session: &AgentSession) -> Option<String> {
    session
        .context_data
        .as_ref()
        .and_then(|context| context.get("native_session_id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn restored_workdir(session: &AgentSession) -> Option<PathBuf> {
    session
        .context_data
        .as_ref()
        .and_then(|context| context.get("workdir"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn default_rpc_workdir() -> Result<PathBuf, String> {
    std::env::current_dir().map_err(|error| format!("Failed to resolve current directory: {error}"))
}

fn build_spawn_spec(config: &AgentRpcConfig, workdir: PathBuf) -> SpawnSessionSpec {
    SpawnSessionSpec {
        workdir,
        system_prompt: Some(config.system_prompt.clone()),
        model: Some(ModelSpec {
            provider: AGENT_RPC_MODEL_PROVIDER.to_string(),
            model_id: config.model.clone(),
            reasoning_effort: None,
            temperature: Some(config.temperature),
        }),
        permission_mode: PermissionMode::Default,
        allowed_tools: None,
        additional_directories: Vec::new(),
        env: BTreeMap::new(),
        agent_config: serde_json::json!({
            "max_tokens": config.max_tokens,
        }),
        resume_hint: None,
    }
}

async fn spawn_or_resume_session(
    config: &AgentRpcConfig,
    params: &AgentExecuteParams,
    session_id: &str,
) -> Result<
    (
        std::sync::Arc<dyn multi_agent_runtime_core::AgentExecutor>,
        multi_agent_runtime_core::SessionRef,
        PathBuf,
    ),
    String,
> {
    let registry = crate::local_services::executor_registry()?;
    let executor = registry.resolve(AGENT_RPC_VENDOR)?;

    let manager = AgentSessionManager::new(config.db_path.clone());
    let existing = manager.get_session(session_id)?;
    let workdir = existing
        .as_ref()
        .and_then(restored_workdir)
        .map(Ok)
        .unwrap_or_else(default_rpc_workdir)?;

    let session_ref = if let Some(native_session_id) =
        existing.as_ref().and_then(restored_native_session_id)
    {
        let mut metadata = BTreeMap::new();
        metadata.insert("happy_session_id".to_string(), session_id.to_string());
        metadata.insert(
            "agent_id".to_string(),
            params
                .agent_id
                .clone()
                .unwrap_or_else(|| "worker".to_string()),
        );

        executor
            .resume_session(
                NativeSessionId::new(native_session_id),
                ResumeHints {
                    vendor_cursor: None,
                    workdir: Some(workdir.clone()),
                    metadata,
                },
            )
            .await
            .map_err(|error| {
                format!(
                    "executor.resume_session({AGENT_RPC_VENDOR}) failed: {}",
                    user_visible_executor_error(&error)
                )
            })?
    } else {
        let spec = build_spawn_spec(config, workdir.clone());
        match registry
            .start_session_with_autoreopen(AGENT_RPC_VENDOR, spec.clone())
            .await
        {
            Ok(session) => session,
            Err(open_err) => {
                log::warn!(
                        "agent_rpc_handler: start_session_with_autoreopen({AGENT_RPC_VENDOR}) failed: {open_err} — falling back to spawn_session"
                    );
                executor.spawn_session(spec).await.map_err(|error| {
                    format!(
                        "executor.spawn_session({AGENT_RPC_VENDOR}) failed: {}",
                        user_visible_executor_error(&error)
                    )
                })?
            }
        }
    };

    crate::happy_client::session_helpers::upsert_agent_session_workdir_profile_and_vendor(
        &config.db_path,
        session_id,
        &workdir.to_string_lossy(),
        None,
        session_ref.vendor,
    )?;
    crate::happy_client::session_helpers::upsert_agent_session_native_session_id(
        &config.db_path,
        session_id,
        session_ref.vendor,
        session_ref.id.as_str(),
    )?;

    Ok((executor, session_ref, workdir))
}

async fn execute_agent_rpc_via_executor(
    config: &AgentRpcConfig,
    params: &AgentExecuteParams,
    runtime_context_messages: Vec<String>,
    _tools: &[Tool],
) -> Result<AgentExecuteResponse, String> {
    let session_id = params
        .session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let (executor, session_ref, _workdir) =
        spawn_or_resume_session(config, params, &session_id).await?;

    let mut prompt = params.task.clone();
    if !runtime_context_messages.is_empty() {
        prompt = format!("{}\n\n{}", runtime_context_messages.join("\n\n"), prompt);
    }

    // Persist the user turn locally before the vendor consumes it. The
    // executor-driven path here does not construct an ExecutorNormalizer,
    // so we call the module-level helper directly. See the P0 finding in
    // the session message persistence audit.
    crate::executor_normalizer::persist_local_user_message(
        &config.db_path,
        &session_id,
        session_ref.vendor,
        &prompt,
        None,
    )
    .map_err(|error| format!("persist user message failed: {error}"))?;

    let mut stream = executor
        .send_message(
            &session_ref,
            UserMessage {
                content: prompt,
                attachments: Vec::new(),
                parent_tool_use_id: None,
                injected_tools: Vec::new(),
            },
        )
        .await
        .map_err(|error| format!("executor.send_message({AGENT_RPC_VENDOR}) failed: {error}"))?;

    let mut streamed_text = String::new();
    let mut final_text = None;
    let mut iteration_count = 0usize;
    let mut intermediate_messages = Vec::new();
    let mut recoverable_error = None;

    while let Some(event) = stream.next().await {
        let event = event.map_err(|error| format!("executor stream error: {error}"))?;
        match event {
            multi_agent_runtime_core::ExecutorEvent::SessionReady { native_session_id } => {
                crate::happy_client::session_helpers::upsert_agent_session_native_session_id(
                    &config.db_path,
                    &session_id,
                    session_ref.vendor,
                    native_session_id.as_str(),
                )?;
            }
            multi_agent_runtime_core::ExecutorEvent::StreamDelta { kind, content } => {
                if kind == DeltaKind::Text {
                    streamed_text.push_str(&content);
                }
            }
            multi_agent_runtime_core::ExecutorEvent::ToolCallStart { name, .. } => {
                intermediate_messages.push(format!("tool:{name}"));
            }
            multi_agent_runtime_core::ExecutorEvent::Error {
                message,
                recoverable,
            } => {
                if recoverable {
                    recoverable_error = Some(message.clone());
                    intermediate_messages.push(message);
                } else {
                    let _ = executor.close_session(&session_ref).await;
                    return Err(format!("Agent execution failed: {message}"));
                }
            }
            multi_agent_runtime_core::ExecutorEvent::TurnComplete {
                final_text: turn_text,
                iteration_count: turns,
                ..
            } => {
                iteration_count = turns as usize;
                final_text =
                    turn_text.or_else(|| (!streamed_text.is_empty()).then_some(streamed_text));
                break;
            }
            _ => {}
        }
    }

    if let Err(error) = executor.close_session(&session_ref).await {
        log::warn!(
            "agent.execute: executor.close_session({}) failed: {}",
            session_ref.vendor,
            error
        );
    }

    let response = final_text
        .or(recoverable_error)
        .ok_or_else(|| "Agent execution failed: executor produced no completion".to_string())?;

    Ok(AgentExecuteResponse {
        response,
        session_id,
        iteration_count,
        intermediate_messages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_execute_params_parsing() {
        let json = serde_json::json!({
            "task": "测试任务",
            "session_id": "test-123",
            "user_id": "user-456"
        });

        let params: AgentExecuteParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.task, "测试任务");
        assert_eq!(params.session_id, Some("test-123".to_string()));
        assert_eq!(params.user_id, Some("user-456".to_string()));
    }
}
