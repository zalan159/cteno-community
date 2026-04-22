//! Cteno agent runtime — local session execution kernel.
//!
//! Migrated from apps/client/desktop/src/ in repo refactor P0.
#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]

/// Returns the absolute path of this crate's `CARGO_MANIFEST_DIR` at build time.
///
/// Used by the host layer to locate bundled `tools/` and `skills/` resource
/// directories, which live inside this crate (the agent runtime) rather than
/// in the desktop app crate. Dev mode uses this path directly; release mode
/// falls back to the Tauri `resource_dir` where the bundler has copied them.
pub fn runtime_resources_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub mod agent;
pub mod agent_config;
pub mod agent_queue;
pub mod agent_session;
pub mod autonomous_agent;
pub mod browser;
pub mod chat_compression;
pub mod custom_agent_fs;
pub mod hooks;
pub mod llm;
pub mod llm_edit_fixer;
pub mod llm_profile;
pub mod mcp;
#[cfg(target_os = "macos")]
pub mod notification_watcher;
pub mod permission;
pub mod push_notification;
pub mod runs;
pub mod session_memory;
pub mod skillhub;
pub mod subagent;
pub mod system_prompt;
pub mod tool;
pub mod tool_executors;
pub mod tool_hooks;
pub mod tool_loader;

// Wave 3.3a — agent kind taxonomy and static tool-filter policy.
pub mod agent_kind;

// Wave 3.3b — SlashCommand parsing (host-free piece of command_interceptor).
pub mod command_interceptor;
