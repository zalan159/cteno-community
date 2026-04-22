//! Tool Executors migrated from apps/client/desktop/src/tool_executors.
//!
//! These are session-internal executors (ReAct loop uses them directly).
//! Orchestration executors (ask_persona, dispatch_task, scheduler, etc.) stay
//! in the app crate because they depend on host-side types (PersonaManager,
//! SchedulerHandle, SessionRegistry) and are registered into the shared
//! ToolRegistry at boot time.

pub mod a2ui_render;
pub mod browser_action;
pub mod browser_adapter;
pub mod browser_cdp;
pub mod browser_manage;
pub mod browser_navigate;
pub mod browser_network;
pub mod builtin;
pub mod computer_use;
pub mod edit;
pub mod fetch;
pub mod file_tracker;
pub mod get_session_output;
pub mod glob;
pub mod grep;
pub mod image_generation;
pub mod memory;
pub mod oss_upload;
pub mod path_resolver;
pub mod query_subagent;
pub mod read;
pub mod run_manager;
pub mod sandbox;
pub mod screenshot;
pub mod shell;
pub mod skill;
pub mod start_subagent;
pub mod stop_subagent;
pub mod tool_search;
pub mod update_plan;
pub mod upload_artifact;
pub mod wait;
pub mod websearch;
pub mod write;

pub use a2ui_render::A2uiRenderExecutor;
pub use browser_action::BrowserActionExecutor;
pub use browser_adapter::BrowserAdapterExecutor;
pub use browser_cdp::BrowserCdpExecutor;
pub use browser_manage::BrowserManageExecutor;
pub use browser_navigate::BrowserNavigateExecutor;
pub use browser_network::BrowserNetworkExecutor;
pub use builtin::register_all_builtin_executors;
pub use computer_use::ComputerUseExecutor;
pub use edit::EditExecutor;
pub use fetch::FetchExecutor;
pub use get_session_output::GetSessionOutputExecutor;
pub use glob::GlobExecutor;
pub use grep::GrepExecutor;
pub use image_generation::ImageGenerationExecutor;
pub use memory::MemoryExecutor;
pub use query_subagent::QuerySubAgentExecutor;
pub use read::ReadExecutor;
pub use run_manager::RunManagerExecutor;
pub use sandbox::{SandboxCheckResult, SandboxContext, SandboxPolicy};
pub use screenshot::{CoordScale, ScreenshotExecutor};
pub use shell::ShellExecutor;
pub use skill::SkillExecutor;
pub use start_subagent::StartSubAgentExecutor;
pub use stop_subagent::StopSubAgentExecutor;
pub use tool_search::ToolSearchExecutor;
pub use update_plan::UpdatePlanExecutor;
pub use upload_artifact::UploadArtifactExecutor;
pub use wait::WaitExecutor;
pub use websearch::WebSearchExecutor;
pub use write::WriteExecutor;
