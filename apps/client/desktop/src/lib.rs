//! Cteno Desktop Application
#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_assignments)]
#![allow(private_interfaces)]
#![allow(deprecated)]

mod a2ui;
mod agent_hooks;
mod agent_kind;
mod agent_owner;
mod agent_rpc_handler;
mod agent_sync_bridge;
mod auth_anonymous;
mod auth_store_boot;
mod command_interceptor;
mod commands; // Tauri commands
mod db;
mod executor_normalizer;
mod executor_registry;
mod executor_session;
mod happy_client; // Happy Server client (cloud upload + machine register)
mod headless_auth;
mod host;
mod local_services;
mod multi_agent;
mod service_init;
mod session_delivery;
mod session_store_impl;
// mod native_host; // Removed — Extension replaced by CDP
mod orchestration;
mod persona;
mod scheduler;
mod session_message_codec;
mod session_relay;
mod session_sync_impl;
mod skill_store;
mod task_graph;
mod tool_executors;
mod tray;
mod usage_monitor;
mod webview_bridge;

// Re-export migrated agent-runtime modules under the previous crate:: paths so
// existing callers (use crate::llm::*, crate::tool::*, etc.) keep resolving.
pub use cteno_agent_runtime::{
    agent, agent_queue, agent_session, autonomous_agent, browser, chat_compression,
    custom_agent_fs, llm, llm_edit_fixer, llm_profile, mcp, push_notification, runs,
    session_memory, skillhub, subagent, system_prompt, tool, tool_hooks, tool_loader,
};

#[cfg(target_os = "macos")]
pub use cteno_agent_runtime::notification_watcher;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use tauri::Manager;

pub use commands::AgentStreamEvent;

/// Global Tauri AppHandle, set once during setup.
/// Used by session layer to emit Tauri events for local IPC (bypassing server).
pub static APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

const FALLBACK_HAPPY_SERVER_URL: &str = "https://cteno.frontfidelity.cn";

/// Shared session connections state, accessible from Tauri commands.
#[derive(Clone)]
pub struct SessionConnectionsState(pub happy_client::SessionRegistry);

pub use cteno_host_session_registry::{LocalHostInfoState, RpcRegistryState};

pub(crate) fn load_runtime_env() -> Result<(), String> {
    // Release builds: always use production URL, never read source-tree .env.
    // The .env file may point to dev server after `secrets:sync:dev`, which would
    // break the Release app since CARGO_MANIFEST_DIR is baked in at compile time
    // and still resolves to the source tree.
    if !cfg!(debug_assertions) {
        let server_url = resolved_happy_server_url();
        std::env::set_var("HAPPY_SERVER_URL", &server_url);
        log::info!(
            "Release build: using compiled HAPPY_SERVER_URL: {}",
            server_url
        );
        return Ok(());
    }

    // Dev builds: load .env from source tree (written by secrets:sync:dev)
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(custom_path) = std::env::var("CTENO_TAURI_ENV_FILE") {
        if !custom_path.trim().is_empty() {
            candidates.push(PathBuf::from(custom_path.trim()));
        }
    }

    candidates.push(manifest_dir.join(".env"));
    candidates.push(manifest_dir.join(".env.local"));

    let mut loaded_paths: Vec<PathBuf> = Vec::new();
    for path in candidates {
        if path.exists() {
            dotenvy::from_path_override(&path)
                .map_err(|e| format!("Failed to load env file {}: {}", path.display(), e))?;
            loaded_paths.push(path);
        }
    }

    if let Ok(value) = std::env::var("HAPPY_SERVER_URL") {
        if !value.trim().is_empty() {
            if !loaded_paths.is_empty() {
                log::info!(
                    "Loaded runtime env files: {}",
                    loaded_paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            return Ok(());
        }
    }

    // Fallback for dev builds without .env
    let server_url = resolved_happy_server_url();
    std::env::set_var("HAPPY_SERVER_URL", &server_url);
    log::info!(
        "No .env file found, using compiled HAPPY_SERVER_URL: {}",
        server_url
    );
    Ok(())
}

pub(crate) fn compiled_default_happy_server_url() -> &'static str {
    match option_env!("CTENO_DEFAULT_HAPPY_SERVER_URL") {
        Some(value) if !value.trim().is_empty() => value,
        _ => FALLBACK_HAPPY_SERVER_URL,
    }
}

pub(crate) fn resolved_happy_server_url() -> String {
    std::env::var("HAPPY_SERVER_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| compiled_default_happy_server_url().to_string())
}

pub(crate) fn load_headless_runtime_env() -> Result<(), String> {
    let server_url = resolved_happy_server_url();
    std::env::set_var("HAPPY_SERVER_URL", &server_url);
    log::info!(
        "Headless runtime: using compiled/default HAPPY_SERVER_URL: {}",
        server_url
    );
    Ok(())
}

/// Initialize and run the Tauri application
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    host::entrypoints::gui::run();
}

pub fn run_ctenoctl(argv0: Option<String>, args: Vec<String>) -> i32 {
    host::entrypoints::cli::run(argv0, args)
}

/// Run standalone agent daemon process (no Tauri UI window).
///
/// This keeps machine connection + extension server alive independently so
/// frontend processes can be restarted without dropping machine availability.
pub fn run_agent_daemon() -> Result<(), String> {
    host::entrypoints::daemon::run()
}

/// Application configuration
#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
pub struct Config {
    #[serde(alias = "openrouter_key")]
    pub llm_api_key: Option<String>,
    pub supabase_url: Option<String>,
    pub supabase_key: Option<String>,
}

/// Get current configuration
#[tauri::command]
fn get_config(app: tauri::AppHandle) -> Result<Config, String> {
    let config_path = get_config_path(&app)?;

    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read config: {}", e))?;
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse config: {}", e))
    } else {
        Ok(Config::default())
    }
}

/// Save configuration
#[tauri::command]
fn save_config(app: tauri::AppHandle, config: Config) -> Result<(), String> {
    let config_path = get_config_path(&app)?;
    let content = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;
    std::fs::write(&config_path, content).map_err(|e| format!("Failed to write config: {}", e))?;
    Ok(())
}

/// Application status
#[derive(serde::Serialize)]
pub struct Status {
    pub service_running: bool,
    pub service_message: String,
    pub api_configured: bool,
    pub api_message: String,
}

/// Get application status
#[tauri::command]
fn get_status(app: tauri::AppHandle) -> Result<Status, String> {
    let config = get_config(app.clone())?;

    // Services are initialized in-process, always available
    let service_running = true;
    let service_message = "服务运行中".to_string();

    Ok(Status {
        service_running,
        service_message,
        api_configured: config.llm_api_key.is_some(),
        api_message: if config.llm_api_key.is_some() {
            "已配置".to_string()
        } else {
            "未配置".to_string()
        },
    })
}

fn get_config_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let app_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;
    Ok(app_dir.join("config.json"))
}

/// Permission status for system capabilities
#[derive(serde::Serialize)]
pub struct PermissionStatus {
    pub full_disk_access: bool,
    pub automation_mail: bool,
}

/// Check automation permission for Mail app
fn check_mail_permission_sync() -> bool {
    #[cfg(not(target_os = "macos"))]
    {
        return false;
    }

    #[cfg(target_os = "macos")]
    {
        let script = r#"
            tell application "Mail"
                try
                    get unread count of inbox
                    return true
                on error
                    return false
                end try
            end tell
        "#;

        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                stdout.trim() == "true"
            }
            Err(_) => false,
        }
    }
}

/// Check system permissions
#[tauri::command]
fn check_permissions() -> Result<PermissionStatus, String> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        let full_disk_access =
            std::fs::File::open(std::path::PathBuf::from(&home).join("Library/Messages/chat.db"))
                .is_ok();
        let automation_mail = check_mail_permission_sync();
        Ok(PermissionStatus {
            full_disk_access,
            automation_mail,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(PermissionStatus {
            full_disk_access: true,
            automation_mail: false,
        })
    }
}

/// Open a URL (system preferences, etc.)
#[tauri::command]
fn open_url(window: tauri::WebviewWindow, url: String) -> Result<(), String> {
    if window.label() == "skill-store" {
        let parsed = reqwest::Url::parse(&url)
            .map_err(|e| format!("Invalid URL for in-webview open: {}", e))?;
        if parsed.scheme() == "http" || parsed.scheme() == "https" {
            window
                .navigate(parsed.clone())
                .map_err(|e| format!("Failed to navigate skill-store window: {}", e))?;
            let _ = window.set_focus();
            return Ok(());
        }
        return Err("Only http(s) URLs are allowed in skill store window".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&url)
            .spawn()
            .map_err(|e| format!("Failed to open URL: {}", e))?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &url])
            .spawn()
            .map_err(|e| format!("Failed to open URL: {}", e))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&url)
            .spawn()
            .map_err(|e| format!("Failed to open URL: {}", e))?;
    }
    Ok(())
}

/// Read a file and return its base64-encoded content (for drag-and-drop image support)
#[tauri::command]
fn read_file_base64(path: String) -> Result<String, String> {
    use base64::Engine;
    let data = std::fs::read(&path).map_err(|e| format!("Failed to read {}: {}", path, e))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&data))
}

/// Restart the application
#[tauri::command]
fn restart_app(app: tauri::AppHandle) {
    app.restart();
}

/// Log a message from the frontend JS to the Rust tracing output
#[tauri::command]
fn frontend_log(level: String, message: String) {
    match level.as_str() {
        "error" => log::error!("[Frontend] {}", message),
        "warn" => log::warn!("[Frontend] {}", message),
        "debug" => log::debug!("[Frontend] {}", message),
        _ => log::info!("[Frontend] {}", message),
    }
}
