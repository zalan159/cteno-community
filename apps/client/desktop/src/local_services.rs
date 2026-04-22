use std::sync::{Arc, OnceLock};

struct LocalServices {
    run_manager: Arc<crate::runs::RunManager>,
    scheduler: Arc<crate::scheduler::TaskScheduler>,
    persona_manager: Arc<crate::persona::PersonaManager>,
    usage_store: Arc<cteno_community_host::usage_store::UsageStore>,
    #[cfg(target_os = "macos")]
    notification_watcher: Arc<crate::notification_watcher::NotificationWatcher>,
    tool_registry: Arc<tokio::sync::RwLock<crate::tool::registry::ToolRegistry>>,
    mcp_registry: Arc<tokio::sync::RwLock<crate::mcp::MCPRegistry>>,
}

#[derive(Clone)]
pub struct AgentRuntimeContext {
    pub db_path: std::path::PathBuf,
    pub data_dir: std::path::PathBuf,
    pub config_path: std::path::PathBuf,
    pub builtin_skills_dir: std::path::PathBuf,
    pub user_skills_dir: std::path::PathBuf,
    pub builtin_agents_dir: std::path::PathBuf,
    pub user_agents_dir: std::path::PathBuf,
    pub profile_store: Arc<tokio::sync::RwLock<crate::llm_profile::ProfileStore>>,
    pub proxy_profiles: Arc<tokio::sync::RwLock<Vec<crate::llm_profile::LlmProfile>>>,
    pub global_api_key: Arc<tokio::sync::RwLock<String>>,
}

static LOCAL_SERVICES: OnceLock<LocalServices> = OnceLock::new();
static AGENT_RUNTIME_CONTEXT: OnceLock<AgentRuntimeContext> = OnceLock::new();

/// Spawn config is set later (after machine auth completes), so it uses a
/// separate OnceLock.
static SPAWN_CONFIG: OnceLock<Arc<crate::happy_client::manager::SpawnSessionConfig>> =
    OnceLock::new();

/// Shared machine RPC registry. Installed as soon as the machine host manager
/// constructs its registry so late-bound local RPC callers can resolve it.
static RPC_REGISTRY: OnceLock<Arc<crate::happy_client::RpcRegistry>> = OnceLock::new();

/// Shared vendor background task registry. Installed during service_init so
/// session lifecycle checks can observe vendor-owned work alongside host runs.
static BACKGROUND_TASK_REGISTRY: OnceLock<
    Arc<cteno_host_session_registry::BackgroundTaskRegistry>,
> = OnceLock::new();

/// BrowserManager is created during tool registration and shared across browser tools.
/// Separate OnceLock because it's created in service_init, not in install().
static BROWSER_MANAGER: OnceLock<Arc<crate::browser::BrowserManager>> = OnceLock::new();

/// A2UI in-memory store for declarative UI component trees (per-agent surfaces).
static A2UI_STORE: OnceLock<Arc<crate::a2ui::A2uiStore>> = OnceLock::new();

/// Orchestration flow store for visualizing orchestration script progress.
static ORCHESTRATION_STORE: OnceLock<Arc<crate::orchestration::OrchestrationStore>> =
    OnceLock::new();

/// Machine socket for emitting events to Happy Server (set after machine auth).
static MACHINE_SOCKET: OnceLock<Arc<crate::happy_client::socket::HappySocket>> = OnceLock::new();

/// Multi-vendor agent executor registry (cteno + claude + codex adapters),
/// sharing one SessionStoreProvider. Installed during service_init after the
/// local SQLite DB path is known.
static EXECUTOR_REGISTRY: OnceLock<Arc<crate::executor_registry::ExecutorRegistry>> =
    OnceLock::new();

/// Host-side subprocess supervisor. Tracks cteno-agent child pids in a
/// persistent pid file so the daemon can SIGTERM orphans on shutdown / crash
/// recovery. Installed during service_init; consumed by the daemon shutdown
/// hook to invoke `kill_all()` once before exit.
static SUBPROCESS_SUPERVISOR: OnceLock<Arc<cteno_host_runtime::SubprocessSupervisor>> =
    OnceLock::new();

pub struct LocalScheduledJobSource {
    scheduler: Arc<crate::scheduler::TaskScheduler>,
}

impl LocalScheduledJobSource {
    pub fn new(scheduler: Arc<crate::scheduler::TaskScheduler>) -> Self {
        Self { scheduler }
    }
}

impl cteno_host_session_registry::ScheduledJobSource for LocalScheduledJobSource {
    fn list_scheduled_jobs(&self) -> Vec<cteno_host_session_registry::BackgroundTaskRecord> {
        match self.scheduler.list_tasks(false) {
            Ok(tasks) => tasks
                .into_iter()
                .map(project_scheduled_job_record)
                .collect(),
            Err(error) => {
                log::warn!(
                    "[BackgroundTasks] Failed to list scheduled jobs from scheduler: {}",
                    error
                );
                Vec::new()
            }
        }
    }
}

fn project_scheduled_job_record(
    task: crate::scheduler::ScheduledTask,
) -> cteno_host_session_registry::BackgroundTaskRecord {
    let status = scheduled_job_status(&task);
    let summary = task.state.last_result_summary.clone();
    let completed_at = task.state.last_run_at;
    let next_run_at = task.state.next_run_at;
    let last_run_at = task.state.last_run_at;
    let last_status = task.state.last_status.clone();
    let task_execution_type = task.task_type.as_str().to_string();

    cteno_host_session_registry::BackgroundTaskRecord {
        task_id: task.id,
        session_id: task.session_id,
        vendor: "cteno".to_string(),
        category: cteno_host_session_registry::BackgroundTaskCategory::ScheduledJob,
        task_type: "scheduled_job".to_string(),
        description: Some(task.name),
        summary,
        status,
        started_at: task.created_at,
        completed_at,
        tool_use_id: None,
        output_file: None,
        vendor_extra: serde_json::json!({
            "enabled": task.enabled,
            "deleteAfterRun": task.delete_after_run,
            "schedule": task.schedule,
            "timezone": task.timezone,
            "personaId": task.persona_id,
            "nextRunAt": next_run_at,
            "lastRunAt": last_run_at,
            "lastStatus": last_status,
            "taskExecutionType": task_execution_type,
        }),
    }
}

fn scheduled_job_status(
    task: &crate::scheduler::ScheduledTask,
) -> cteno_host_session_registry::BackgroundTaskStatus {
    if !task.enabled {
        return cteno_host_session_registry::BackgroundTaskStatus::Paused;
    }

    match task.state.last_status.as_ref() {
        Some(crate::scheduler::TaskRunStatus::Success) => {
            cteno_host_session_registry::BackgroundTaskStatus::Completed
        }
        Some(
            crate::scheduler::TaskRunStatus::Failed | crate::scheduler::TaskRunStatus::TimedOut,
        ) => cteno_host_session_registry::BackgroundTaskStatus::Failed,
        Some(crate::scheduler::TaskRunStatus::Skipped) => {
            cteno_host_session_registry::BackgroundTaskStatus::Cancelled
        }
        None => cteno_host_session_registry::BackgroundTaskStatus::Unknown,
    }
}

#[cfg(target_os = "macos")]
pub fn install(
    run_manager: Arc<crate::runs::RunManager>,
    scheduler: Arc<crate::scheduler::TaskScheduler>,
    persona_manager: Arc<crate::persona::PersonaManager>,
    usage_store: Arc<cteno_community_host::usage_store::UsageStore>,
    notification_watcher: Arc<crate::notification_watcher::NotificationWatcher>,
    tool_registry: Arc<tokio::sync::RwLock<crate::tool::registry::ToolRegistry>>,
    mcp_registry: Arc<tokio::sync::RwLock<crate::mcp::MCPRegistry>>,
) {
    let _ = LOCAL_SERVICES.set(LocalServices {
        run_manager,
        scheduler,
        persona_manager,
        usage_store,
        notification_watcher,
        tool_registry,
        mcp_registry,
    });

    crate::session_sync_impl::install();
}

#[cfg(not(target_os = "macos"))]
pub fn install(
    run_manager: Arc<crate::runs::RunManager>,
    scheduler: Arc<crate::scheduler::TaskScheduler>,
    persona_manager: Arc<crate::persona::PersonaManager>,
    usage_store: Arc<cteno_community_host::usage_store::UsageStore>,
    tool_registry: Arc<tokio::sync::RwLock<crate::tool::registry::ToolRegistry>>,
    mcp_registry: Arc<tokio::sync::RwLock<crate::mcp::MCPRegistry>>,
) {
    let _ = LOCAL_SERVICES.set(LocalServices {
        run_manager,
        scheduler,
        persona_manager,
        usage_store,
        tool_registry,
        mcp_registry,
    });

    crate::session_sync_impl::install();
}

/// Store the spawn session config (called once after machine auth succeeds).
pub fn install_spawn_config(config: Arc<crate::happy_client::manager::SpawnSessionConfig>) {
    let _ = SPAWN_CONFIG.set(config);
}

/// Store the shared machine RPC registry (called once during machine host
/// startup when the HappyClientManager is constructed).
pub fn install_rpc_registry(registry: Arc<crate::happy_client::RpcRegistry>) {
    let _ = RPC_REGISTRY.set(registry);
}

pub fn install_agent_runtime_context(context: AgentRuntimeContext) {
    let _ = AGENT_RUNTIME_CONTEXT.set(context);
}

pub fn run_manager() -> Result<Arc<crate::runs::RunManager>, String> {
    LOCAL_SERVICES
        .get()
        .map(|services| services.run_manager.clone())
        .ok_or_else(|| "Local services are not initialized".to_string())
}

pub fn scheduler() -> Result<Arc<crate::scheduler::TaskScheduler>, String> {
    LOCAL_SERVICES
        .get()
        .map(|services| services.scheduler.clone())
        .ok_or_else(|| "Local services are not initialized".to_string())
}

pub fn persona_manager() -> Result<Arc<crate::persona::PersonaManager>, String> {
    LOCAL_SERVICES
        .get()
        .map(|services| services.persona_manager.clone())
        .ok_or_else(|| "Local services are not initialized".to_string())
}

pub fn usage_store() -> Result<Arc<cteno_community_host::usage_store::UsageStore>, String> {
    LOCAL_SERVICES
        .get()
        .map(|services| services.usage_store.clone())
        .ok_or_else(|| "Local services are not initialized".to_string())
}

#[cfg(target_os = "macos")]
pub fn notification_watcher(
) -> Result<Arc<crate::notification_watcher::NotificationWatcher>, String> {
    LOCAL_SERVICES
        .get()
        .map(|services| services.notification_watcher.clone())
        .ok_or_else(|| "Local services are not initialized".to_string())
}

pub fn tool_registry(
) -> Result<Arc<tokio::sync::RwLock<crate::tool::registry::ToolRegistry>>, String> {
    LOCAL_SERVICES
        .get()
        .map(|services| services.tool_registry.clone())
        .ok_or_else(|| "Local services are not initialized".to_string())
}

pub fn mcp_registry() -> Result<Arc<tokio::sync::RwLock<crate::mcp::MCPRegistry>>, String> {
    LOCAL_SERVICES
        .get()
        .map(|services| services.mcp_registry.clone())
        .ok_or_else(|| "Local services are not initialized".to_string())
}

pub fn spawn_config() -> Result<Arc<crate::happy_client::manager::SpawnSessionConfig>, String> {
    SPAWN_CONFIG
        .get()
        .cloned()
        .ok_or_else(|| "Spawn config not initialized (machine auth not complete)".to_string())
}

pub fn rpc_registry() -> Result<Arc<crate::happy_client::RpcRegistry>, String> {
    RPC_REGISTRY
        .get()
        .cloned()
        .ok_or_else(|| "RPC registry not initialized".to_string())
}

/// Store the shared vendor background task registry (called once during
/// service initialization).
pub fn install_background_task_registry(
    registry: Arc<cteno_host_session_registry::BackgroundTaskRegistry>,
) {
    let _ = BACKGROUND_TASK_REGISTRY.set(registry);
}

pub fn background_task_registry(
) -> Result<Arc<cteno_host_session_registry::BackgroundTaskRegistry>, String> {
    BACKGROUND_TASK_REGISTRY
        .get()
        .cloned()
        .ok_or_else(|| "Background task registry not initialized".to_string())
}

pub fn agent_runtime_context() -> Result<AgentRuntimeContext, String> {
    AGENT_RUNTIME_CONTEXT
        .get()
        .cloned()
        .ok_or_else(|| "Agent runtime context not initialized".to_string())
}

/// Store the BrowserManager (called once during service_init tool registration).
pub fn install_browser_manager(manager: Arc<crate::browser::BrowserManager>) {
    let _ = BROWSER_MANAGER.set(manager);
}

pub fn browser_manager() -> Result<Arc<crate::browser::BrowserManager>, String> {
    BROWSER_MANAGER
        .get()
        .cloned()
        .ok_or_else(|| "BrowserManager not initialized".to_string())
}

/// Store the machine socket (called after machine socket connects).
pub fn install_machine_socket(socket: Arc<crate::happy_client::socket::HappySocket>) {
    let _ = MACHINE_SOCKET.set(socket);
}

pub fn machine_socket() -> Result<Arc<crate::happy_client::socket::HappySocket>, String> {
    MACHINE_SOCKET
        .get()
        .cloned()
        .ok_or_else(|| "Machine socket not initialized".to_string())
}

/// Install the A2UI store (called once during service initialization).
pub fn install_a2ui_store(store: Arc<crate::a2ui::A2uiStore>) {
    let _ = A2UI_STORE.set(store);
}

pub fn a2ui_store() -> Result<Arc<crate::a2ui::A2uiStore>, String> {
    A2UI_STORE
        .get()
        .cloned()
        .ok_or_else(|| "A2UI store not initialized".to_string())
}

/// Install the orchestration store (called once during service initialization).
pub fn install_orchestration_store(store: Arc<crate::orchestration::OrchestrationStore>) {
    let _ = ORCHESTRATION_STORE.set(store);
}

pub fn orchestration_store() -> Result<Arc<crate::orchestration::OrchestrationStore>, String> {
    ORCHESTRATION_STORE
        .get()
        .cloned()
        .ok_or_else(|| "Orchestration store not initialized".to_string())
}

/// Install the multi-vendor executor registry (called once during service init).
pub fn install_executor_registry(registry: Arc<crate::executor_registry::ExecutorRegistry>) {
    let _ = EXECUTOR_REGISTRY.set(registry);
}

/// Fetch the executor registry. Returns Err when service init has not yet
/// run or the registry failed to build (missing cteno-agent binary).
pub fn executor_registry() -> Result<Arc<crate::executor_registry::ExecutorRegistry>, String> {
    EXECUTOR_REGISTRY
        .get()
        .cloned()
        .ok_or_else(|| "Executor registry not initialized".to_string())
}

/// Install the host-side subprocess supervisor (called once during service
/// init before the executor registry is built).
pub fn install_subprocess_supervisor(supervisor: Arc<cteno_host_runtime::SubprocessSupervisor>) {
    let _ = SUBPROCESS_SUPERVISOR.set(supervisor);
}

/// Fetch the subprocess supervisor. Returns `None` when the daemon boot
/// path did not install one (e.g. Windows stub, construction failure).
pub fn subprocess_supervisor() -> Option<Arc<cteno_host_runtime::SubprocessSupervisor>> {
    SUBPROCESS_SUPERVISOR.get().cloned()
}
