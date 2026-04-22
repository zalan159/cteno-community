//! Tool Executors
//!
//! Native Rust implementations of tool execution logic.
//!
//! Session-internal executors live in `cteno-agent-runtime::tool_executors` and
//! are re-exported here so existing call sites (`crate::tool_executors::*`) keep
//! resolving.  Orchestration executors (`ask_persona`, `dispatch_task`,
//! scheduler, session CRUD, personality) stay in the app crate because they
//! depend on host-side types (PersonaManager, SchedulerHandle, SessionRegistry).

// ---------------------------------------------------------------------------
// Migrated executors (re-exported from cteno-agent-runtime)
// ---------------------------------------------------------------------------

pub use cteno_agent_runtime::tool_executors::{
    a2ui_render, browser_action, browser_adapter, browser_cdp, browser_manage, browser_navigate,
    browser_network, computer_use, edit, fetch, file_tracker, get_session_output, glob, grep,
    image_generation, memory, oss_upload, path_resolver, query_subagent, read, run_manager,
    sandbox, screenshot, shell, skill, start_subagent, stop_subagent, tool_search, update_plan,
    upload_artifact, wait, websearch, write,
};

pub use cteno_agent_runtime::tool_executors::{
    A2uiRenderExecutor, BrowserActionExecutor, BrowserAdapterExecutor, BrowserCdpExecutor,
    BrowserManageExecutor, BrowserNavigateExecutor, BrowserNetworkExecutor, ComputerUseExecutor,
    CoordScale, EditExecutor, FetchExecutor, GetSessionOutputExecutor, GlobExecutor, GrepExecutor,
    ImageGenerationExecutor, MemoryExecutor, QuerySubAgentExecutor, ReadExecutor,
    RunManagerExecutor, SandboxCheckResult, SandboxContext, SandboxPolicy, ScreenshotExecutor,
    ShellExecutor, SkillExecutor, StartSubAgentExecutor, StopSubAgentExecutor, ToolSearchExecutor,
    UpdatePlanExecutor, UploadArtifactExecutor, WaitExecutor, WebSearchExecutor, WriteExecutor,
};

// ---------------------------------------------------------------------------
// Orchestration executors (session-creation, cross-session messaging, scheduler)
// ---------------------------------------------------------------------------

pub mod ask_persona;
pub mod close_task_session;
pub mod delete_scheduled_task;
pub mod dispatch_task;
pub mod list_scheduled_tasks;
pub mod list_task_sessions;
pub mod schedule_task;
pub mod send_to_session;
pub mod update_personality;

pub use ask_persona::AskPersonaExecutor;
pub use close_task_session::CloseTaskSessionExecutor;
pub use delete_scheduled_task::DeleteScheduledTaskExecutor;
pub use dispatch_task::DispatchTaskExecutor;
pub use list_scheduled_tasks::ListScheduledTasksExecutor;
pub use list_task_sessions::ListTaskSessionsExecutor;
pub use schedule_task::ScheduleTaskExecutor;
pub use send_to_session::SendToSessionExecutor;
pub use update_personality::UpdatePersonalityExecutor;
