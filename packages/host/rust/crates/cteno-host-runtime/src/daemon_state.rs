//! Daemon runtime helpers shared by app and standalone agent daemon.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(any(unix, windows))]
use std::process::Stdio;

const AGENTS_DIR_NAME: &str = ".agents";
const DAEMON_STATE_FILE: &str = "daemon.state.json";
const DAEMON_LOCK_FILE: &str = "daemon.lock";
const DAEMON_READY_FILE: &str = "daemon.ready";
const PENDING_MACHINE_AUTH_PUBLIC_KEY_FILE: &str = "pending-machine-auth-public-key.txt";
const MACHINE_AUTH_CACHE_FILE: &str = "machine_auth.json";
pub const DAEMON_ROOT_ENV: &str = "CTENO_DAEMON_ROOT";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonState {
    pub pid: u32,
    pub started_at: String,
    pub version: String,
    pub mode: String,
}

pub struct DaemonLockGuard {
    lock_path: PathBuf,
    state_path: PathBuf,
}

impl Drop for DaemonLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
        let _ = fs::remove_file(&self.state_path);
        let _ = clear_daemon_ready();
    }
}

pub fn default_agents_home_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "Failed to resolve home directory".to_string())?;
    Ok(home.join(AGENTS_DIR_NAME))
}

pub fn local_daemon_root(app_data_dir: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    app_data_dir.hash(&mut hasher);
    let suffix = format!("{:04x}", (hasher.finish() & 0xffff) as u16);
    app_data_dir
        .parent()
        .unwrap_or(app_data_dir)
        .join(format!(".d{}", suffix))
}

pub fn agents_home_dir() -> Result<PathBuf, String> {
    match std::env::var_os(DAEMON_ROOT_ENV) {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => default_agents_home_dir(),
    }
}

pub fn ensure_agents_home_dir() -> Result<PathBuf, String> {
    let dir = agents_home_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
    Ok(dir)
}

pub fn resolve_app_data_dir() -> PathBuf {
    crate::paths::default_headless_app_data_dir()
}

pub fn ensure_app_data_dir() -> Result<PathBuf, String> {
    let dir = resolve_app_data_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
    Ok(dir)
}

pub fn machine_auth_cache_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(MACHINE_AUTH_CACHE_FILE)
}

pub fn daemon_state_path() -> Result<PathBuf, String> {
    Ok(ensure_agents_home_dir()?.join(DAEMON_STATE_FILE))
}

pub fn daemon_lock_path() -> Result<PathBuf, String> {
    Ok(ensure_agents_home_dir()?.join(DAEMON_LOCK_FILE))
}

pub fn pending_machine_auth_public_key_path() -> Result<PathBuf, String> {
    Ok(ensure_agents_home_dir()?.join(PENDING_MACHINE_AUTH_PUBLIC_KEY_FILE))
}

pub fn daemon_ready_path() -> Result<PathBuf, String> {
    Ok(ensure_agents_home_dir()?.join(DAEMON_READY_FILE))
}

pub fn clear_daemon_ready() -> Result<(), String> {
    let path = daemon_ready_path()?;
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|e| format!("Failed to remove {}: {}", path.display(), e))?;
    }
    Ok(())
}

pub fn mark_daemon_ready() -> Result<(), String> {
    let path = daemon_ready_path()?;
    fs::write(&path, b"ready\n").map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

pub fn is_daemon_ready() -> bool {
    daemon_ready_path()
        .ok()
        .map(|path| path.exists())
        .unwrap_or(false)
}

pub fn acquire_daemon_lock(mode: &str) -> Result<DaemonLockGuard, String> {
    let lock_path = daemon_lock_path()?;
    let state_path = daemon_state_path()?;

    if lock_path.exists() {
        match fs::read_to_string(&lock_path) {
            Ok(content) => {
                let pid_str = content.trim();
                if let Ok(pid) = pid_str.parse::<u32>() {
                    if !is_process_running(pid) {
                        let _ = fs::remove_file(&lock_path);
                        let _ = fs::remove_file(&state_path);
                    }
                } else {
                    let _ = fs::remove_file(&lock_path);
                }
            }
            Err(_) => {
                let _ = fs::remove_file(&lock_path);
            }
        }
    }

    let mut lock_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|e| {
            format!(
                "Failed to acquire daemon lock at {}: {}",
                lock_path.display(),
                e
            )
        })?;

    let pid = std::process::id();
    let state = DaemonState {
        pid,
        started_at: Utc::now().to_rfc3339(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        mode: mode.to_string(),
    };
    let state_json = serde_json::to_string_pretty(&state)
        .map_err(|e| format!("Serialize daemon state: {}", e))?;

    lock_file
        .write_all(format!("{}\n", pid).as_bytes())
        .map_err(|e| format!("Write daemon lock file: {}", e))?;
    fs::write(&state_path, state_json)
        .map_err(|e| format!("Write daemon state file {}: {}", state_path.display(), e))?;
    clear_daemon_ready()?;

    Ok(DaemonLockGuard {
        lock_path,
        state_path,
    })
}

fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        Command::new("cmd")
            .args([
                "/C",
                &format!(
                    "tasklist /FI \"PID eq {}\" | findstr /R /C:\"[ ]{}[ ]\" >NUL",
                    pid, pid
                ),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

pub fn write_pending_machine_auth_public_key(public_key: Option<&str>) -> Result<(), String> {
    let path = pending_machine_auth_public_key_path()?;
    match public_key {
        Some(pk) if !pk.trim().is_empty() => {
            fs::write(&path, pk).map_err(|e| format!("Write {} failed: {}", path.display(), e))
        }
        _ => {
            if path.exists() {
                fs::remove_file(&path)
                    .map_err(|e| format!("Remove {} failed: {}", path.display(), e))?;
            }
            Ok(())
        }
    }
}

pub fn read_pending_machine_auth_public_key() -> Result<Option<String>, String> {
    let path = pending_machine_auth_public_key_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let content =
        fs::read_to_string(&path).map_err(|e| format!("Read {} failed: {}", path.display(), e))?;
    let trimmed = content.trim().to_string();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed))
    }
}

pub fn load_llm_api_key_from_config(config_path: &Path) -> Result<String, String> {
    if !config_path.exists() {
        return Ok(String::new());
    }

    let content = fs::read_to_string(config_path)
        .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;
    let parsed: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", config_path.display(), e))?;
    Ok(parsed
        .get("llm_api_key")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string())
}
