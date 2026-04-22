#![cfg_attr(not(feature = "tauri-commands"), allow(dead_code))]

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(feature = "tauri-commands")]
pub mod commands;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    AutomationAppleEvents,
    FullDiskAccess,
    Accessibility,
    ScreenRecording,
}

impl PermissionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutomationAppleEvents => "automation_apple_events",
            Self::FullDiskAccess => "full_disk_access",
            Self::Accessibility => "accessibility",
            Self::ScreenRecording => "screen_recording",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Denied,
    NotDetermined,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionSnapshot {
    pub automation_apple_events: PermissionState,
    pub full_disk_access: PermissionState,
    pub accessibility: PermissionState,
    pub screen_recording: PermissionState,
}

impl PermissionSnapshot {
    pub fn state_for(&self, kind: PermissionKind) -> PermissionState {
        match kind {
            PermissionKind::AutomationAppleEvents => self.automation_apple_events,
            PermissionKind::FullDiskAccess => self.full_disk_access,
            PermissionKind::Accessibility => self.accessibility,
            PermissionKind::ScreenRecording => self.screen_recording,
        }
    }
}

pub fn current_snapshot() -> PermissionSnapshot {
    PermissionSnapshot {
        automation_apple_events: check_automation_state(),
        full_disk_access: check_full_disk_access_state(),
        accessibility: check_accessibility_state(),
        screen_recording: check_screen_recording_state(),
    }
}

pub fn required_permissions_for_shell_command(command: &str) -> Vec<PermissionKind> {
    let lower = command.to_lowercase();
    let mut required = Vec::new();

    if lower.contains("osascript") {
        required.push(PermissionKind::AutomationAppleEvents);
    }
    if lower.contains("system events") {
        required.push(PermissionKind::Accessibility);
    }
    if lower.contains("screencapture")
        || lower.contains("screenrecord")
        || lower.contains("capture screen")
    {
        required.push(PermissionKind::ScreenRecording);
    }
    if lower.contains("imsg")
        || lower.contains("library/messages/chat.db")
        || lower.contains("messages/chat.db")
    {
        required.push(PermissionKind::FullDiskAccess);
    }

    let mut seen = HashSet::new();
    required
        .into_iter()
        .filter(|kind| seen.insert(*kind))
        .collect()
}

pub fn ensure_shell_command_permissions(command: &str) -> Result<(), String> {
    let required = required_permissions_for_shell_command(command);
    if required.is_empty() {
        return Ok(());
    }

    let snapshot = current_snapshot();
    let missing: Vec<PermissionKind> = required
        .into_iter()
        .filter(|kind| snapshot.state_for(*kind) != PermissionState::Granted)
        .collect();

    if missing.is_empty() {
        return Ok(());
    }

    let kinds = missing
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    Err(format!(
        "PERMISSION_MISSING: {}\nOpen Settings > Privacy & Security and grant the required permission(s), then retry.",
        kinds
    ))
}

/// Install/update `ctenoctl` symlink in user bin directory.
/// Creates a symlink from `~/.local/bin/ctenoctl` → current executable.
/// Skips if the existing symlink already points to the same binary (same version).
pub fn install_ctenoctl_symlink_if_needed() {
    let target_exe = match ctenoctl_target_path() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("[ctenoctl] Failed to resolve target exe path: {}", e);
            return;
        }
    };

    let bin_dir = if cfg!(windows) {
        dirs::data_local_dir().map(|d| d.join("bin"))
    } else {
        dirs::home_dir().map(|d| d.join(".local").join("bin"))
    };

    let bin_dir = match bin_dir {
        Some(d) => d,
        None => {
            log::warn!("[ctenoctl] Failed to determine user bin directory");
            return;
        }
    };

    let link_name = if cfg!(windows) {
        "ctenoctl.exe"
    } else {
        "ctenoctl"
    };
    let link_path = bin_dir.join(link_name);

    if link_path.is_symlink() {
        if let Ok(existing_target) = std::fs::read_link(&link_path) {
            if existing_target == target_exe {
                log::debug!("[ctenoctl] Symlink already up-to-date: {:?}", link_path);
                return;
            }
        }
    }

    if let Err(e) = std::fs::create_dir_all(&bin_dir) {
        log::warn!("[ctenoctl] Failed to create bin dir {:?}: {}", bin_dir, e);
        return;
    }

    if link_path.exists() || link_path.is_symlink() {
        if let Err(e) = std::fs::remove_file(&link_path) {
            log::warn!("[ctenoctl] Failed to remove old {:?}: {}", link_path, e);
            return;
        }
    }

    #[cfg(unix)]
    {
        if let Err(e) = std::os::unix::fs::symlink(&target_exe, &link_path) {
            log::warn!("[ctenoctl] Failed to create symlink: {}", e);
            return;
        }
    }
    #[cfg(windows)]
    {
        if let Err(e) = std::os::windows::fs::symlink_file(&target_exe, &link_path) {
            log::info!("[ctenoctl] Symlink failed, copying binary instead: {}", e);
            if let Err(e2) = std::fs::copy(&target_exe, &link_path) {
                log::warn!("[ctenoctl] Failed to copy binary: {}", e2);
                return;
            }
        }
    }

    log::info!("[ctenoctl] Installed: {:?} -> {:?}", link_path, target_exe);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CtenoCliInstallStatus {
    pub supported: bool,
    pub installed: bool,
    pub symlink_path: String,
    pub target_path: String,
    pub in_path: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_hint: Option<String>,
}

fn install_ctenoctl_impl() -> Result<(), String> {
    #[cfg(unix)]
    {
        let symlink_path = ctenoctl_symlink_path()?;
        let target_path = ctenoctl_target_path()?;

        if let Some(parent) = symlink_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
        }

        if symlink_path.exists() || symlink_path.symlink_metadata().is_ok() {
            let metadata = symlink_path
                .symlink_metadata()
                .map_err(|e| format!("Failed to read {}: {}", symlink_path.display(), e))?;
            if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                return Err(format!(
                    "{} exists and is a directory",
                    symlink_path.display()
                ));
            }
            fs::remove_file(&symlink_path).map_err(|e| {
                format!(
                    "Failed to remove existing {}: {}",
                    symlink_path.display(),
                    e
                )
            })?;
        }

        std::os::unix::fs::symlink(&target_path, &symlink_path).map_err(|e| {
            format!(
                "Failed to create symlink {} -> {}: {}",
                symlink_path.display(),
                target_path.display(),
                e
            )
        })?;

        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err("ctenoctl installation is only supported on Unix-like systems".to_string())
    }
}

fn get_ctenoctl_install_status_impl() -> Result<CtenoCliInstallStatus, String> {
    let symlink_path = ctenoctl_symlink_path()?;
    let target_path = ctenoctl_target_path()?;

    #[cfg(unix)]
    let installed = match symlink_path.symlink_metadata() {
        Ok(meta) => {
            if !meta.file_type().is_symlink() {
                false
            } else {
                let actual = fs::read_link(&symlink_path).ok();
                match actual {
                    Some(actual_target) => {
                        let actual_abs = absolutize_path(actual_target, symlink_path.parent());
                        let expected_abs = canonicalize_or_keep(target_path.clone());
                        actual_abs == expected_abs
                    }
                    None => false,
                }
            }
        }
        Err(_) => false,
    };

    #[cfg(not(unix))]
    let installed = false;

    let in_path = path_contains_dir(
        symlink_path
            .parent()
            .ok_or_else(|| "Invalid ctenoctl symlink path".to_string())?,
    );

    let path_hint = if in_path {
        None
    } else {
        Some(format!(
            "Add to your shell profile: export PATH=\"{}:$PATH\"",
            symlink_path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "~/.local/bin".to_string())
        ))
    };

    Ok(CtenoCliInstallStatus {
        supported: cfg!(unix),
        installed,
        symlink_path: symlink_path.display().to_string(),
        target_path: target_path.display().to_string(),
        in_path,
        path_hint,
    })
}

fn ctenoctl_target_path() -> Result<PathBuf, String> {
    let current = std::env::current_exe()
        .map_err(|e| format!("Failed to resolve app binary path: {}", e))?;
    let current_name = current
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let daemon_name = if cfg!(windows) {
        "cteno-agentd.exe"
    } else {
        "cteno-agentd"
    };

    if current_name == daemon_name {
        let cli_name = if cfg!(windows) { "cteno.exe" } else { "cteno" };
        if let Some(parent) = current.parent() {
            let cli = parent.join(cli_name);
            if cli.is_file() {
                return Ok(cli);
            }
        }
    }

    Ok(current)
}

fn ctenoctl_symlink_path() -> Result<PathBuf, String> {
    let install_dir = match std::env::var("CTENO_CLI_INSTALL_DIR") {
        Ok(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => {
            let home =
                dirs::home_dir().ok_or_else(|| "Failed to resolve home directory".to_string())?;
            home.join(".local").join("bin")
        }
    };
    Ok(install_dir.join("ctenoctl"))
}

fn canonicalize_or_keep(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn absolutize_path(path: PathBuf, base_dir: Option<&Path>) -> PathBuf {
    let absolute = if path.is_absolute() {
        path
    } else if let Some(base) = base_dir {
        base.join(path)
    } else {
        path
    };
    canonicalize_or_keep(absolute)
}

fn path_contains_dir(dir: &Path) -> bool {
    let dir_text = dir.display().to_string();
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|entry| entry == dir_text)
}

fn request_permission_impl(kind: PermissionKind) -> Result<(), String> {
    match kind {
        PermissionKind::AutomationAppleEvents => {
            trigger_automation_prompt();
            open_permission_settings_impl(kind)
        }
        PermissionKind::ScreenRecording => {
            trigger_screen_recording_prompt();
            Ok(())
        }
        PermissionKind::FullDiskAccess | PermissionKind::Accessibility => {
            open_permission_settings_impl(kind)
        }
    }
}

fn open_permission_settings_impl(kind: PermissionKind) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let urls = match kind {
            PermissionKind::AutomationAppleEvents => vec![
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation",
                "x-apple.systempreferences:com.apple.preference.security",
            ],
            PermissionKind::FullDiskAccess => vec![
                "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
                "x-apple.systempreferences:com.apple.preference.security",
            ],
            PermissionKind::Accessibility => vec![
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                "x-apple.systempreferences:com.apple.preference.security",
            ],
            PermissionKind::ScreenRecording => vec![
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
                "x-apple.systempreferences:com.apple.preference.security",
            ],
        };
        for url in urls {
            if let Ok(status) = Command::new("open").arg(url).status() {
                if status.success() {
                    return Ok(());
                }
            }
        }
        Err("Failed to open System Settings".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = kind;
        Err("Permission settings are only supported on macOS".to_string())
    }
}

fn check_automation_state() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        let script = r#"
            tell application "System Events"
                return name of first process
            end tell
        "#;
        match Command::new("osascript").arg("-e").arg(script).output() {
            Ok(out) => {
                if out.status.success() {
                    PermissionState::Granted
                } else {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    if stderr.contains("-1743") || stdout.contains("-1743") {
                        PermissionState::Denied
                    } else {
                        PermissionState::Denied
                    }
                }
            }
            Err(_) => PermissionState::Unavailable,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::Unavailable
    }
}

fn trigger_automation_prompt() {
    #[cfg(target_os = "macos")]
    {
        let script = r#"
            tell application "System Events"
                return name of first process
            end tell
        "#;
        let _ = Command::new("osascript").arg("-e").arg(script).status();
    }
}

fn check_full_disk_access_state() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        let Some(home) = dirs::home_dir() else {
            return PermissionState::Unavailable;
        };

        let probes = [
            home.join("Library/Messages/chat.db"),
            home.join("Library/Application Support/com.apple.TCC/TCC.db"),
        ];
        let mut saw_permission_denied = false;

        for probe in probes {
            match std::fs::File::open(&probe) {
                Ok(_) => return PermissionState::Granted,
                Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                    saw_permission_denied = true;
                }
                Err(_) => {}
            }
        }

        if saw_permission_denied {
            PermissionState::Denied
        } else {
            PermissionState::NotDetermined
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::Unavailable
    }
}

fn check_accessibility_state() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        let trusted = unsafe { AXIsProcessTrusted() };
        if trusted {
            PermissionState::Granted
        } else {
            PermissionState::Denied
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::Unavailable
    }
}

fn check_screen_recording_state() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        let granted = unsafe { CGPreflightScreenCaptureAccess() };
        if granted {
            PermissionState::Granted
        } else {
            PermissionState::Denied
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::Unavailable
    }
}

fn trigger_screen_recording_prompt() {
    #[cfg(target_os = "macos")]
    {
        let _ = unsafe { CGRequestScreenCaptureAccess() };
    }
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}
