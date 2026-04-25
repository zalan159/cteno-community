//! Hook implementations for the stdio binary.
//!
//! We install the minimum set of hooks needed by the runtime's ReAct loop:
//!
//! - `ToolRegistryProvider` + `tool_registry_handle` — lets the loop list,
//!   describe and execute the built-in tools we register below, plus any
//!   host-injected tools registered dynamically via `tool_inject`.
//! - `ResolvedUrlProvider` — returns a happy-server URL from `HAPPY_SERVER_URL`
//!   (or empty string). stdio does not talk to happy-server directly, but some
//!   executors read the url hook defensively.
//! - `SubagentBootstrapProvider` — lets runtime-native `dispatch_task` spawn
//!   DAG nodes as SubAgents inside this stdio process.
//!
//! The actual tool inventory lives in
//! `cteno_agent_runtime::tool_executors::register_all_builtin_executors` so
//! that the stdio binary and the future Tauri host share one list.
//!
//! Every hook that requires host-side wiring (skill registry, persona
//! dispatch, A2UI store, subagent bootstrap, command interceptor, machine
//! socket, spawn config, agent owner, session waker) is intentionally not
//! installed; executors that require them are deliberately excluded from the
//! built-in set.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock as AsyncRwLock;

use cteno_agent_runtime::agent::executor::{AcpSenderFactory, SubAgentContext};
use cteno_agent_runtime::agent_config::{AgentConfig, AgentType};
use cteno_agent_runtime::autonomous_agent::{AcpMessageSender, PermissionChecker};
use cteno_agent_runtime::hooks::{
    ResolvedUrlProvider, SubAgentLifecycleEmitter, SubAgentLifecycleEventDto,
    SubagentBootstrapProvider, TaskGraphEventEmitter, ToolRegistryProvider,
};
use cteno_agent_runtime::llm_profile::ApiFormat;
use cteno_agent_runtime::mcp::MCPRegistry;
use cteno_agent_runtime::permission::{PermissionCheckResult, PermissionDecision};
use cteno_agent_runtime::runs::RunManager;
use cteno_agent_runtime::runtime_resources_dir;
use cteno_agent_runtime::tool::registry::ToolRegistry;
use cteno_agent_runtime::tool::ToolExecutor;
use cteno_agent_runtime::tool_executors::register_all_builtin_executors;
use cteno_agent_runtime::tool_executors::SandboxPolicy;
use tokio::sync::oneshot;

use crate::io::OutboundWriter;
use crate::pending::{new_permission_id, PendingPermissions};
use crate::protocol::{AcpDelivery, Outbound, SubAgentLifecycleEvent as WireSubAgentLifecycleEvent};

/// Bridges the runtime's `ToolRegistryProvider` trait onto a concrete
/// `ToolRegistry` we own. The `tool_registry_handle` hook is installed in
/// parallel so that executors which need direct registry access
/// (tool_search, concurrency_safe checks) keep working.
pub struct StdioToolRegistry {
    inner: Arc<AsyncRwLock<ToolRegistry>>,
}

impl StdioToolRegistry {
    pub fn new(inner: Arc<AsyncRwLock<ToolRegistry>>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ToolRegistryProvider for StdioToolRegistry {
    async fn execute(&self, tool_name: &str, input: Value) -> Result<String, String> {
        let guard = self.inner.read().await;
        guard.execute(tool_name, input).await
    }

    async fn list_tools(&self) -> Vec<String> {
        let guard = self.inner.read().await;
        guard.list_ids()
    }

    async fn describe(&self, tool_name: &str) -> Option<Value> {
        let guard = self.inner.read().await;
        guard
            .get_config(tool_name)
            .and_then(|cfg| serde_json::to_value(cfg.clone()).ok())
    }
}

pub struct StdioUrlProvider;

impl ResolvedUrlProvider for StdioUrlProvider {
    fn happy_server_url(&self) -> String {
        std::env::var("CTENO_HAPPY_SERVER_URL")
            .or_else(|_| std::env::var("HAPPY_SERVER_URL"))
            .unwrap_or_default()
    }
}

pub struct StdioTaskGraphEventEmitter {
    writer: OutboundWriter,
}

impl StdioTaskGraphEventEmitter {
    pub fn new(writer: OutboundWriter) -> Self {
        Self { writer }
    }
}

#[async_trait]
impl TaskGraphEventEmitter for StdioTaskGraphEventEmitter {
    async fn emit_task_graph_event(&self, session_id: &str, event: &str, payload: Value) {
        self.writer
            .send(Outbound::Acp {
                session_id: session_id.to_string(),
                delivery: AcpDelivery::Persisted,
                data: serde_json::json!({
                    "type": "native_event",
                    "kind": event,
                    "payload": payload,
                    "id": uuid::Uuid::new_v4().to_string(),
                }),
            })
            .await;
    }
}

/// Bridges the runtime's `SubAgentLifecycleEmitter` to a wire
/// `Outbound::SubAgentLifecycle` frame. Called from `SubAgentManager`
/// (sync, fire-and-forget) — we offload the async `writer.send` to a
/// detached task so the emitter never blocks the SubAgent state machine.
pub struct StdioSubAgentLifecycleEmitter {
    writer: OutboundWriter,
}

impl StdioSubAgentLifecycleEmitter {
    pub fn new(writer: OutboundWriter) -> Self {
        Self { writer }
    }
}

impl SubAgentLifecycleEmitter for StdioSubAgentLifecycleEmitter {
    fn emit(&self, parent_session_id: &str, event: SubAgentLifecycleEventDto) {
        let writer = self.writer.clone();
        let session_id = parent_session_id.to_string();
        let wire_event = lifecycle_dto_to_wire(event);
        let kind_label = wire_kind_label(&wire_event);
        log::info!(
            "[stdio subagent lifecycle] emitting {kind_label} for parent={session_id}"
        );
        tokio::spawn(async move {
            writer
                .send(Outbound::SubAgentLifecycle {
                    session_id,
                    event: wire_event,
                })
                .await;
        });
    }
}

fn wire_kind_label(event: &WireSubAgentLifecycleEvent) -> &'static str {
    match event {
        WireSubAgentLifecycleEvent::Spawned { .. } => "spawned",
        WireSubAgentLifecycleEvent::Started { .. } => "started",
        WireSubAgentLifecycleEvent::Updated { .. } => "updated",
        WireSubAgentLifecycleEvent::Completed { .. } => "completed",
        WireSubAgentLifecycleEvent::Failed { .. } => "failed",
        WireSubAgentLifecycleEvent::Stopped { .. } => "stopped",
    }
}

fn lifecycle_dto_to_wire(dto: SubAgentLifecycleEventDto) -> WireSubAgentLifecycleEvent {
    match dto {
        SubAgentLifecycleEventDto::Spawned {
            subagent_id,
            agent_id,
            task,
            label,
            created_at_ms,
        } => WireSubAgentLifecycleEvent::Spawned {
            subagent_id,
            agent_id,
            task,
            label,
            created_at_ms,
        },
        SubAgentLifecycleEventDto::Started {
            subagent_id,
            started_at_ms,
        } => WireSubAgentLifecycleEvent::Started {
            subagent_id,
            started_at_ms,
        },
        SubAgentLifecycleEventDto::Updated {
            subagent_id,
            iteration_count,
        } => WireSubAgentLifecycleEvent::Updated {
            subagent_id,
            iteration_count,
        },
        SubAgentLifecycleEventDto::Completed {
            subagent_id,
            result,
            completed_at_ms,
        } => WireSubAgentLifecycleEvent::Completed {
            subagent_id,
            result,
            completed_at_ms,
        },
        SubAgentLifecycleEventDto::Failed {
            subagent_id,
            error,
            completed_at_ms,
        } => WireSubAgentLifecycleEvent::Failed {
            subagent_id,
            error,
            completed_at_ms,
        },
        SubAgentLifecycleEventDto::Stopped {
            subagent_id,
            completed_at_ms,
        } => WireSubAgentLifecycleEvent::Stopped {
            subagent_id,
            completed_at_ms,
        },
    }
}

/// Return value of `install_default_registry`: the shared registry handle
/// (so callers can inject more tools dynamically) and the number of built-in
/// tools registered at boot.
pub struct InstalledRegistry {
    pub handle: Arc<AsyncRwLock<ToolRegistry>>,
    pub mcp_registries: SessionMcpRegistries,
    pub subagent_bootstrap: Arc<StdioSubagentBootstrap>,
    pub builtin_count: usize,
}

#[derive(Debug, Clone)]
struct StdioSessionBootstrap {
    workdir: Option<String>,
    additional_directories: Vec<String>,
    agent_config: Value,
}

pub struct StdioSubagentBootstrap {
    data_dir: PathBuf,
    writer: OutboundWriter,
    pending_permissions: PendingPermissions,
    sessions: AsyncRwLock<HashMap<String, StdioSessionBootstrap>>,
}

impl StdioSubagentBootstrap {
    pub fn new(
        data_dir: PathBuf,
        writer: OutboundWriter,
        pending_permissions: PendingPermissions,
    ) -> Self {
        Self {
            data_dir,
            writer,
            pending_permissions,
            sessions: AsyncRwLock::new(HashMap::new()),
        }
    }

    pub async fn register_session(
        &self,
        session_id: String,
        workdir: Option<String>,
        additional_directories: Vec<String>,
        agent_config: Value,
    ) {
        let mut sessions = self.sessions.write().await;
        sessions.insert(
            session_id,
            StdioSessionBootstrap {
                workdir,
                additional_directories,
                agent_config,
            },
        );
    }

    pub async fn unregister_session(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
    }
}

#[async_trait]
impl SubagentBootstrapProvider for StdioSubagentBootstrap {
    async fn build_subagent_context(
        &self,
        agent_id: &str,
        parent_session_id: &str,
        override_profile_id: Option<&str>,
    ) -> Result<(AgentConfig, SubAgentContext), String> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions
                .get(parent_session_id)
                .cloned()
                .ok_or_else(|| format!("No stdio session bootstrap for {parent_session_id}"))?
        };

        let profile_id = override_profile_id
            .map(ToString::to_string)
            .or_else(|| cfg_string(&session.agent_config, "profile_id"));
        let model = cfg_model(&session.agent_config);
        let server_url = cteno_agent_runtime::hooks::resolved_happy_server_url();
        let access_token =
            cteno_agent_runtime::hooks::credentials().and_then(|provider| provider.access_token());
        let direct_api_key =
            env_string(&["CTENO_AGENT_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY"]);
        let (global_api_key, default_base_url, use_proxy) = match (access_token, direct_api_key) {
            (Some(token), _) if !server_url.is_empty() => (token, server_url, true),
            (_, Some(api_key)) => (
                api_key,
                cfg_string(&session.agent_config, "base_url")
                    .unwrap_or_else(|| "https://api.anthropic.com".to_string()),
                false,
            ),
            _ => {
                return Err(
                    "SubAgent bootstrap has no auth token and no direct API key environment"
                        .to_string(),
                )
            }
        };

        let mut agent_config = default_agent_config(agent_id);
        if let Some(model) = model.clone() {
            agent_config.model = Some(model);
        }
        if let Some(workdir) = session.workdir.clone() {
            agent_config.instructions = Some(format!(
                "{}\n\nWorkspace: {}",
                agent_config.instructions.unwrap_or_default(),
                workdir
            ));
        }

        Ok((
            agent_config,
            SubAgentContext {
                db_path: self.data_dir.join("sessions.db"),
                builtin_skills_dir: runtime_resources_dir().join("skills"),
                user_skills_dir: self.data_dir.join("skills"),
                global_api_key,
                default_base_url,
                profile_id,
                use_proxy,
                profile_model: model,
                // SubAgents run as independent background sessions; their ACP
                // stream is tagged with the **subagent's own** session_id (not
                // the parent's), so it projects to a separate
                // `agent_sessions` row in the desktop's host DB rather than
                // bleeding into the parent persona's transcript. The runtime
                // calls this factory inside `execute_sub_agent_inner` once it
                // has resolved the subagent's session id (which equals
                // `SubAgent.id` thanks to `session_id_override`). Result:
                // clicking a SubAgent in BackgroundRunsModal navigates to
                // `/session/{subagent.id}` and `useSession(subagent.id)`
                // finds its full transcript.
                acp_sender_factory: Some(make_subagent_acp_sender_factory(self.writer.clone())),
                permission_checker: permission_checker_for_mode(
                    cfg_permission_mode(&session.agent_config),
                    parent_session_id.to_string(),
                    self.writer.clone(),
                    self.pending_permissions.clone(),
                ),
                abort_flag: None,
                thinking_flag: None,
                api_format: cfg_api_format(&session.agent_config),
                sandbox_policy: Some(sandbox_policy_for_mode(
                    cfg_permission_mode(&session.agent_config),
                    &session.additional_directories,
                )),
            },
        ))
    }
}

/// Build an `AcpSenderFactory` that, when called with a subagent's session
/// id, returns an `AcpMessageSender` that emits stdio `Outbound::Acp` frames
/// tagged with that id. The factory is what
/// [`SubAgentContext::acp_sender_factory`] expects — it lets the runtime
/// stamp each subagent's frames with its own id (equal to `SubAgent.id`)
/// rather than the parent's, so desktop projects them under
/// `agent_sessions.id = subagent.id`.
fn make_subagent_acp_sender_factory(writer: OutboundWriter) -> AcpSenderFactory {
    Arc::new(move |sub_session_id: String| {
        let writer = writer.clone();
        let sender: AcpMessageSender = Arc::new(move |payload: Value| {
            let writer = writer.clone();
            let sid = sub_session_id.clone();
            Box::pin(async move {
                writer
                    .send(crate::runner::acp_outbound(
                        &sid,
                        AcpDelivery::Persisted,
                        payload,
                    ))
                    .await;
            })
        });
        sender
    })
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
        _ => Some(Arc::new(
            move |tool_name: String, _call_id: String, input: Value| {
                let writer = writer.clone();
                let pending = pending_permissions.clone();
                let session_id = session_id.clone();
                Box::pin(async move {
                    let request_id = new_permission_id();
                    let (tx, rx) = oneshot::channel::<PermissionDecision>();
                    pending.lock().await.insert(request_id.clone(), tx);

                    writer
                        .send(Outbound::PermissionRequest {
                            session_id: session_id.clone(),
                            request_id: request_id.clone(),
                            tool_name: tool_name.clone(),
                            tool_input: input,
                        })
                        .await;

                    match rx.await {
                        Ok(PermissionDecision::Approved)
                        | Ok(PermissionDecision::ApprovedForSession) => {
                            PermissionCheckResult::Allowed
                        }
                        Ok(PermissionDecision::Denied) => {
                            PermissionCheckResult::Denied("host denied tool".to_string())
                        }
                        Ok(PermissionDecision::Abort) => PermissionCheckResult::Aborted,
                        Err(_) => {
                            pending.lock().await.remove(&request_id);
                            PermissionCheckResult::Denied(format!(
                            "host never answered permission_request {request_id} for {tool_name}"
                        ))
                        }
                    }
                })
            },
        )),
    }
}

fn cfg_permission_mode(cfg: &Value) -> &str {
    cfg.get("permission_mode")
        .and_then(Value::as_str)
        .unwrap_or("default")
}

fn sandbox_policy_for_mode(mode: &str, additional_directories: &[String]) -> SandboxPolicy {
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

fn default_agent_config(agent_id: &str) -> AgentConfig {
    let name = match agent_id {
        "browser" => "Browser Agent",
        "worker" => "Worker Agent",
        other => other,
    };
    let description = match agent_id {
        "browser" => "Autonomous browser-focused Cteno subagent.",
        _ => "Autonomous Cteno worker subagent.",
    };
    AgentConfig {
        id: agent_id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        version: "1.0.0".to_string(),
        agent_type: AgentType::Autonomous,
        instructions: Some(
            "You are a Cteno runtime SubAgent. Complete the assigned task independently and return a concise result for the parent agent.".to_string(),
        ),
        timeout_seconds: Some(300),
        ..Default::default()
    }
}

fn env_string(env_keys: &[&str]) -> Option<String> {
    env_keys
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
}

fn cfg_string(cfg: &Value, key: &str) -> Option<String> {
    cfg.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn cfg_model(cfg: &Value) -> Option<String> {
    match cfg.get("model") {
        Some(Value::String(model)) if !model.is_empty() => Some(model.clone()),
        Some(Value::Object(model_cfg)) => model_cfg
            .get("model")
            .or_else(|| model_cfg.get("model_id"))
            .and_then(Value::as_str)
            .filter(|model| !model.is_empty())
            .map(ToString::to_string),
        _ => cfg_string(cfg, "model_id"),
    }
}

fn cfg_api_format(cfg: &Value) -> ApiFormat {
    match cfg_string(cfg, "api_format").as_deref() {
        Some("openai") | Some("openai-compatible") => ApiFormat::OpenAI,
        Some("gemini") | Some("gemini-compatible") => ApiFormat::Gemini,
        _ => ApiFormat::Anthropic,
    }
}

pub type SessionMcpRegistries = Arc<AsyncRwLock<HashMap<String, SessionMcpState>>>;

#[derive(Clone)]
pub struct SessionMcpState {
    registry: Arc<AsyncRwLock<MCPRegistry>>,
    tool_ids: Vec<String>,
}

struct SessionMcpToolExecutor {
    registries: SessionMcpRegistries,
    server_id: String,
    tool_name: String,
}

impl SessionMcpToolExecutor {
    fn new(registries: SessionMcpRegistries, server_id: String, tool_name: String) -> Self {
        Self {
            registries,
            server_id,
            tool_name,
        }
    }
}

#[async_trait]
impl ToolExecutor for SessionMcpToolExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let session_id = input
            .get("__session_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "MCP tool call missing __session_id".to_string())?
            .to_string();

        let registry = {
            let guard = self.registries.read().await;
            guard.get(&session_id).map(|state| state.registry.clone())
        }
        .ok_or_else(|| format!("No MCP registry loaded for session {session_id}"))?;

        let registry = registry.read().await;
        registry
            .call_tool(&self.server_id, &self.tool_name, input)
            .await
    }

    fn supports_background(&self) -> bool {
        false
    }
}

/// Build a ToolRegistry populated with the runtime's built-in tool set and
/// register it globally (both via trait hook and via the concrete
/// `tool_registry_handle`). Returns the shared handle so the caller can
/// dynamically inject further tools.
pub fn install_default_registry(
    data_dir: PathBuf,
    writer: OutboundWriter,
    pending_permissions: PendingPermissions,
) -> InstalledRegistry {
    let mut registry = ToolRegistry::new();

    // RunManager is used by shell, run_manager, upload_artifact and
    // image_generation. Scratch logs live under `data_dir/runs/`.
    let run_manager = Arc::new(RunManager::new(data_dir.join("runs")));

    let count = register_all_builtin_executors(&mut registry, data_dir.clone(), run_manager);

    let arc_registry = Arc::new(AsyncRwLock::new(registry));
    let mcp_registries = Arc::new(AsyncRwLock::new(HashMap::new()));
    let subagent_bootstrap = Arc::new(StdioSubagentBootstrap::new(
        data_dir.clone(),
        writer.clone(),
        pending_permissions,
    ));

    // Install both hooks: the trait-based one for generic callers, and the
    // concrete handle for tool_search / concurrency-aware scheduling.
    let provider = Arc::new(StdioToolRegistry::new(arc_registry.clone()));
    cteno_agent_runtime::hooks::install_tool_registry(provider);
    cteno_agent_runtime::hooks::install_tool_registry_handle(arc_registry.clone());

    // URL provider (best-effort; empty string is fine for offline stdio).
    cteno_agent_runtime::hooks::install_url_provider(Arc::new(StdioUrlProvider));
    cteno_agent_runtime::hooks::install_subagent_bootstrap(subagent_bootstrap.clone());
    cteno_agent_runtime::hooks::install_task_graph_event_emitter(Arc::new(
        StdioTaskGraphEventEmitter::new(writer.clone()),
    ));
    cteno_agent_runtime::hooks::install_subagent_lifecycle_emitter(Arc::new(
        StdioSubAgentLifecycleEmitter::new(writer),
    ));

    InstalledRegistry {
        handle: arc_registry,
        mcp_registries,
        subagent_bootstrap,
        builtin_count: count,
    }
}

/// Load the MCP server set visible to one Cteno stdio session and register
/// its tools into the process-level ToolRegistry.
///
/// The registry hook is process-global, so MCP executors route by the
/// injected `__session_id` parameter added by the autonomous loop.
pub async fn install_session_mcp_tools(
    tool_registry: &Arc<AsyncRwLock<ToolRegistry>>,
    session_registries: &SessionMcpRegistries,
    session_id: &str,
    data_dir: &Path,
    workdir: Option<&str>,
) -> Result<usize, String> {
    cleanup_session_mcp_tools(tool_registry, session_registries, session_id).await;

    let global_config = data_dir.join("mcp_servers.yaml");
    let project_config = workdir
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(|w| PathBuf::from(shellexpand::tilde(w).to_string()))
        .map(|w| w.join(".cteno").join("mcp_servers.yaml"));

    let mut mcp_registry = MCPRegistry::new();
    mcp_registry
        .load_from_scoped_configs(&global_config, project_config.as_deref())
        .await?;
    let tool_entries = mcp_registry.get_all_tool_configs();
    let tool_ids: Vec<String> = tool_entries
        .iter()
        .map(|(_, _, config)| config.id.clone())
        .collect();
    let mcp_registry = Arc::new(AsyncRwLock::new(mcp_registry));

    {
        let mut sessions = session_registries.write().await;
        sessions.insert(
            session_id.to_string(),
            SessionMcpState {
                registry: mcp_registry,
                tool_ids,
            },
        );
    }

    let mut tools = tool_registry.write().await;
    let count = tool_entries.len();
    for (server_id, tool_name, tool_config) in tool_entries {
        if tools.has_tool(&tool_config.id) {
            log::debug!(
                "stdio: MCP tool '{}' already registered by another active session; keeping shared executor",
                tool_config.id
            );
            continue;
        }
        let executor = Arc::new(SessionMcpToolExecutor::new(
            session_registries.clone(),
            server_id,
            tool_name,
        ));
        tools.register(tool_config, executor);
    }

    Ok(count)
}

pub async fn cleanup_session_mcp_tools(
    tool_registry: &Arc<AsyncRwLock<ToolRegistry>>,
    session_registries: &SessionMcpRegistries,
    session_id: &str,
) -> usize {
    let (removed, still_active): (Option<SessionMcpState>, HashSet<String>) = {
        let mut sessions = session_registries.write().await;
        let removed = sessions.remove(session_id);
        let still_active = sessions
            .values()
            .flat_map(|state| state.tool_ids.iter().cloned())
            .collect();
        (removed, still_active)
    };

    let Some(removed) = removed else {
        return 0;
    };

    let mut cleaned = 0;
    let mut tools = tool_registry.write().await;
    for tool_id in removed.tool_ids {
        if !still_active.contains(&tool_id) && tools.unregister(&tool_id) {
            cleaned += 1;
        }
    }
    if cleaned > 0 {
        log::info!("stdio: cleaned {cleaned} MCP tools for closed session {session_id}");
    }
    cleaned
}
