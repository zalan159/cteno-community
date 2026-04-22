//! Background runs (long-running tool executions)
//!
//! Design goals:
//! - Runs are in-memory only (no persistence across app restarts).
//! - Runs are owned by a Happy `session_id` (string).
//! - The only automatic cleanup trigger is session archive (killSession), plus process shutdown.
//! - Provide an HTTP API for listing/stopping/logs/notifications, and a tool (`run_manager`)
//!   for the agent to manage runs.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Exited,
    Failed,
    Killed,
    TimedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunExit {
    pub exit_code: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub session_id: String,
    pub tool_id: String,
    pub command: Option<String>,
    pub workdir: Option<String>,
    pub status: RunStatus,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub pid: Option<u32>,
    pub exit: Option<RunExit>,
    pub error: Option<String>,
    pub log_path: Option<String>,
    pub notify: bool,
    pub hard_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunNotification {
    pub run_id: String,
    pub created_at: i64,
    pub message: String,
}

struct RunState {
    record: RunRecord,
    handle: Option<RunHandle>,
    log_path: Option<PathBuf>,
}

enum RunHandle {
    Child(Child),
    Task(tokio::task::JoinHandle<()>),
}

#[derive(Clone)]
pub struct RunLogSink {
    file: Arc<Mutex<tokio::fs::File>>,
}

impl RunLogSink {
    pub async fn new(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(|e| format!("Failed to open log file {:?}: {}", path, e))?;
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
        })
    }

    pub async fn line(&self, line: &str) {
        let mut f = self.file.lock().await;
        let _ = f.write_all(line.as_bytes()).await;
        if !line.ends_with('\n') {
            let _ = f.write_all(b"\n").await;
        }
        let _ = f.flush().await;
    }
}

#[derive(Clone)]
pub struct RunManager {
    runs: Arc<Mutex<HashMap<String, RunState>>>,
    notifications: Arc<Mutex<HashMap<String, VecDeque<RunNotification>>>>,
    /// Signals for converting sync tool executions to background runs.
    /// Keyed by tool call_id. When triggered, the sync execution hands off the process.
    background_signals: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
    base_dir: PathBuf,
}

impl RunManager {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            notifications: Arc::new(Mutex::new(HashMap::new())),
            background_signals: Arc::new(Mutex::new(HashMap::new())),
            base_dir,
        }
    }

    /// Clean up old run log directories on startup.
    /// Removes session directories older than `max_age` and caps total runs directory size.
    pub fn cleanup_old_logs(&self) {
        let runs_dir = self.base_dir.join("runs");
        if !runs_dir.exists() {
            return;
        }

        let max_age = std::time::Duration::from_secs(7 * 24 * 3600); // 7 days
        let now = std::time::SystemTime::now();
        let mut removed_dirs = 0u32;
        let mut removed_files = 0u32;

        let entries = match std::fs::read_dir(&runs_dir) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("[RunManager] Failed to read runs dir {:?}: {}", runs_dir, e);
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            // Check modification time of the session directory
            let modified = match entry.metadata().and_then(|m| m.modified()) {
                Ok(t) => t,
                Err(_) => continue,
            };

            if now.duration_since(modified).unwrap_or_default() > max_age {
                // Remove all log files in this session directory
                if let Ok(files) = std::fs::read_dir(&path) {
                    for file in files.flatten() {
                        if file.path().extension().map(|e| e == "log").unwrap_or(false) {
                            if std::fs::remove_file(file.path()).is_ok() {
                                removed_files += 1;
                            }
                        }
                    }
                }
                // Remove the directory if empty
                if std::fs::remove_dir(&path).is_ok() {
                    removed_dirs += 1;
                }
            }
        }

        if removed_dirs > 0 || removed_files > 0 {
            log::info!(
                "[RunManager] Cleaned up old run logs: {} files, {} directories removed",
                removed_files,
                removed_dirs
            );
        }
    }

    /// Register a background signal for a sync tool execution.
    /// Returns a receiver that the executor should select! on.
    pub async fn register_background_signal(&self, call_id: &str) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        let mut signals = self.background_signals.lock().await;
        signals.insert(call_id.to_string(), tx);
        rx
    }

    /// Trigger the background signal for a given call_id.
    /// Returns true if the signal was sent (i.e., the call_id was registered and still pending).
    pub async fn trigger_background_signal(&self, call_id: &str) -> bool {
        let mut signals = self.background_signals.lock().await;
        if let Some(tx) = signals.remove(call_id) {
            tx.send(()).is_ok()
        } else {
            false
        }
    }

    /// Remove a background signal registration (cleanup after normal completion).
    pub async fn unregister_background_signal(&self, call_id: &str) {
        let mut signals = self.background_signals.lock().await;
        signals.remove(call_id);
    }

    /// Adopt an existing child process into the background run system.
    /// Used when a sync execution is moved to background mid-flight.
    /// The stdout/stderr JoinHandles are joined and their output written to a log file.
    pub async fn adopt_process(
        &self,
        session_id: &str,
        command: &str,
        workdir: &str,
        mut child: Child,
        stdout_task: tokio::task::JoinHandle<Vec<u8>>,
        stderr_task: tokio::task::JoinHandle<Vec<u8>>,
    ) -> Result<RunRecord, String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("Missing session_id for adopt_process".to_string());
        }

        let run_id = uuid::Uuid::new_v4().to_string();
        let started_at = Utc::now().timestamp();
        let pid = child.id();

        let session_dir = self.session_dir(session_id);
        Self::ensure_dir(&session_dir)?;
        let log_path = session_dir.join(format!("{}.log", run_id));

        let record = RunRecord {
            run_id: run_id.clone(),
            session_id: session_id.to_string(),
            tool_id: "shell".to_string(),
            command: Some(command.to_string()),
            workdir: Some(workdir.to_string()),
            status: RunStatus::Running,
            started_at,
            finished_at: None,
            pid,
            exit: None,
            error: None,
            log_path: Some(log_path.to_string_lossy().to_string()),
            notify: true,
            hard_timeout_secs: None,
        };

        {
            let mut runs = self.runs.lock().await;
            runs.insert(
                run_id.clone(),
                RunState {
                    record: record.clone(),
                    handle: None,
                    log_path: Some(log_path.clone()),
                },
            );
        }

        // Spawn completion watcher that joins the drain tasks, waits for exit, writes log
        let manager = self.clone();
        let run_id_clone = run_id.clone();
        let session_id_owned = session_id.to_string();
        let log_path_clone = log_path.clone();
        tokio::spawn(async move {
            // Wait for the child to exit
            let exit_status = child.wait().await.ok();

            // Join stdout/stderr drain tasks to get captured output
            let stdout_data = stdout_task.await.unwrap_or_default();
            let stderr_data = stderr_task.await.unwrap_or_default();

            // Write captured output to log file
            if let Ok(mut log_file) = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path_clone)
                .await
            {
                if !stdout_data.is_empty() {
                    let _ = log_file.write_all(b"[stdout] ").await;
                    let _ = log_file.write_all(&stdout_data).await;
                    if !stdout_data.ends_with(b"\n") {
                        let _ = log_file.write_all(b"\n").await;
                    }
                }
                if !stderr_data.is_empty() {
                    let _ = log_file.write_all(b"[stderr] ").await;
                    let _ = log_file.write_all(&stderr_data).await;
                    if !stderr_data.ends_with(b"\n") {
                        let _ = log_file.write_all(b"\n").await;
                    }
                }
                let _ = log_file.flush().await;
            }

            let finished_at = Utc::now().timestamp();

            // Update RunRecord
            {
                let mut runs = manager.runs.lock().await;
                if let Some(state) = runs.get_mut(&run_id_clone) {
                    if matches!(state.record.status, RunStatus::Running) {
                        match exit_status {
                            Some(es) => {
                                let code = es.code().unwrap_or(-1);
                                state.record.status = RunStatus::Exited;
                                state.record.exit = Some(RunExit { exit_code: code });
                            }
                            None => {
                                state.record.status = RunStatus::Failed;
                                state.record.error = Some("Process wait failed".to_string());
                            }
                        }
                        state.record.finished_at = Some(finished_at);
                    }
                }
            }

            // Push notification
            let tail = manager
                .tail_log(&run_id_clone, 8000)
                .await
                .unwrap_or_default();

            let (exit_code, status_str) = {
                let runs = manager.runs.lock().await;
                runs.get(&run_id_clone)
                    .map(|s| {
                        let code = s.record.exit.as_ref().map(|e| e.exit_code);
                        let status = format!("{:?}", s.record.status);
                        (code, status)
                    })
                    .unwrap_or((None, "Unknown".to_string()))
            };

            let status_label = match exit_code {
                Some(0) => "成功".to_string(),
                Some(code) => format!("失败 (exit_code={})", code),
                None => format!("异常 (status={})", status_str),
            };

            let notify_message = format!(
                "[后台任务完成] tool_id=shell run_id={} 状态={}\n\n日志尾部:\n{}",
                run_id_clone,
                status_label,
                tail.trim()
            );
            manager
                .push_notification(
                    &session_id_owned,
                    RunNotification {
                        run_id: run_id_clone.clone(),
                        created_at: finished_at,
                        message: notify_message,
                    },
                )
                .await;

            // Wake up the owning session so notification poll loop can consume this event.
            if let Some(waker) = crate::hooks::session_waker() {
                waker.wake_session(&session_id_owned, "bg-run-notify").await;
            }

            // Schedule delayed cleanup
            manager.schedule_cleanup(&run_id_clone);
        });

        Ok(record)
    }

    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.base_dir.join("runs").join(session_id)
    }

    fn ensure_dir(path: &Path) -> Result<(), String> {
        std::fs::create_dir_all(path).map_err(|e| format!("Failed to create dir {:?}: {}", path, e))
    }

    pub async fn list_runs(&self, session_id: Option<&str>) -> Vec<RunRecord> {
        let runs = self.runs.lock().await;
        let mut out: Vec<RunRecord> = runs
            .values()
            .map(|s| s.record.clone())
            .filter(|r| session_id.map(|sid| r.session_id == sid).unwrap_or(true))
            .collect();
        // Newest first
        out.sort_by_key(|r| -r.started_at);
        out
    }

    pub async fn get_run(&self, run_id: &str) -> Option<RunRecord> {
        let runs = self.runs.lock().await;
        runs.get(run_id).map(|s| s.record.clone())
    }

    /// Check if a session has any running background tasks.
    pub async fn has_running_tasks(&self, session_id: &str) -> bool {
        let runs = self.runs.lock().await;
        runs.values().any(|s| {
            s.record.session_id == session_id && matches!(s.record.status, RunStatus::Running)
        })
    }

    pub async fn pop_notifications(&self, session_id: &str) -> Vec<RunNotification> {
        let mut map = self.notifications.lock().await;
        let mut out = Vec::new();
        if let Some(q) = map.get_mut(session_id) {
            while let Some(n) = q.pop_front() {
                out.push(n);
            }
        }
        out
    }

    async fn push_notification(&self, session_id: &str, n: RunNotification) {
        let mut map = self.notifications.lock().await;
        let q = map
            .entry(session_id.to_string())
            .or_insert_with(VecDeque::new);
        q.push_back(n);
    }

    /// Remove a finished run from memory and delete its log file on disk.
    async fn remove_run(&self, run_id: &str) {
        let mut runs = self.runs.lock().await;
        if let Some(state) = runs.get(run_id) {
            if !matches!(state.record.status, RunStatus::Running) {
                // Delete the log file from disk
                if let Some(ref log_path) = state.log_path {
                    let path = log_path.clone();
                    // Remove from memory first, then delete file outside the lock
                    let session_dir = path.parent().map(|p| p.to_path_buf());
                    runs.remove(run_id);
                    // Drop lock before async I/O
                    drop(runs);
                    if let Err(e) = tokio::fs::remove_file(&path).await {
                        log::debug!("[RunManager] Failed to remove log file {:?}: {}", path, e);
                    }
                    // Try to remove the session directory if it's now empty
                    if let Some(dir) = session_dir {
                        let _ = tokio::fs::remove_dir(&dir).await; // only succeeds if empty
                    }
                    return;
                }
                runs.remove(run_id);
            }
        }
    }

    /// Schedule automatic removal of a finished run after 60 seconds.
    fn schedule_cleanup(&self, run_id: &str) {
        let manager = self.clone();
        let run_id = run_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            manager.remove_run(&run_id).await;
        });
    }

    pub async fn kill_by_session(&self, session_id: &str) -> usize {
        let run_ids: Vec<String> = {
            let runs = self.runs.lock().await;
            runs.values()
                .filter(|s| {
                    s.record.session_id == session_id
                        && matches!(s.record.status, RunStatus::Running)
                })
                .map(|s| s.record.run_id.clone())
                .collect()
        };
        let mut killed = 0usize;
        for id in run_ids {
            if self.kill_run(&id, "killed by session archive").await {
                killed += 1;
            }
        }
        killed
    }

    pub async fn kill_all(&self) -> usize {
        let run_ids: Vec<String> = {
            let runs = self.runs.lock().await;
            runs.values()
                .filter(|s| matches!(s.record.status, RunStatus::Running))
                .map(|s| s.record.run_id.clone())
                .collect()
        };
        let mut killed = 0usize;
        for id in run_ids {
            if self.kill_run(&id, "killed by shutdown").await {
                killed += 1;
            }
        }
        killed
    }

    pub async fn kill_run(&self, run_id: &str, reason: &str) -> bool {
        // Don't hold the runs lock across await.
        let (pid, handle_opt) = {
            let mut runs = self.runs.lock().await;
            let Some(state) = runs.get_mut(run_id) else {
                return false;
            };

            if !matches!(state.record.status, RunStatus::Running) {
                return false;
            }

            state.record.status = RunStatus::Killed;
            state.record.finished_at = Some(Utc::now().timestamp());
            state.record.error = Some(reason.to_string());
            state.record.exit = None;

            (state.record.pid, state.handle.take())
        };

        let mut killed = false;

        if let Some(handle) = handle_opt {
            match handle {
                RunHandle::Child(mut child) => {
                    let _ = child.kill().await;
                    // Best-effort reap.
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
                    killed = true;
                }
                RunHandle::Task(task) => {
                    task.abort();
                    killed = true;
                }
            }
        } else if let Some(pid) = pid {
            #[cfg(unix)]
            unsafe {
                // Ignore errors; worst case the process already exited.
                let _ = libc::killpg(pid as i32, libc::SIGKILL);
                let _ = libc::kill(pid as i32, libc::SIGKILL);
                killed = true;
            }
            #[cfg(not(unix))]
            {
                let _ = pid;
            }
        }

        if killed {
            self.schedule_cleanup(run_id);
        }

        killed
    }

    pub async fn tail_log(&self, run_id: &str, max_bytes: usize) -> Result<String, String> {
        let log_path = {
            let runs = self.runs.lock().await;
            runs.get(run_id)
                .and_then(|s| s.log_path.as_ref().cloned())
                .ok_or_else(|| "No log available for this run".to_string())?
        };

        let data = match tokio::fs::read(&log_path).await {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Log file may already be cleaned up; degrade gracefully instead of surfacing OS errors.
                let mut runs = self.runs.lock().await;
                if let Some(state) = runs.get_mut(run_id) {
                    state.record.log_path = None;
                    state.log_path = None;
                }
                return Ok(
                    "[log unavailable] Log file was not found. It may have been cleaned up after the run finished."
                        .to_string(),
                );
            }
            Err(e) => return Err(format!("Failed to read log: {}", e)),
        };

        if data.is_empty() {
            return Ok(String::new());
        }

        Ok(cteno_community_host::text_utils::tail_str_lossy(
            &data, max_bytes,
        ))
    }

    pub async fn start_shell_run(
        &self,
        session_id: &str,
        command: &str,
        workdir: &Path,
        notify: bool,
        hard_timeout_secs: Option<u64>,
    ) -> Result<RunRecord, String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("Missing session_id for background run".to_string());
        }
        if command.trim().is_empty() {
            return Err("Command cannot be empty".to_string());
        }

        let run_id = uuid::Uuid::new_v4().to_string();
        let started_at = Utc::now().timestamp();

        let session_dir = self.session_dir(session_id);
        Self::ensure_dir(&session_dir)?;
        let log_path = session_dir.join(format!("{}.log", run_id));

        #[cfg(windows)]
        let (shell_prog, shell_flag) = ("powershell".to_string(), "-Command");
        #[cfg(not(windows))]
        let (shell_prog, shell_flag) = (
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()),
            "-c",
        );

        let wrapped_command =
            crate::tool_executors::shell::ShellExecutor::wrap_command_utf8(command);
        let mut cmd = Command::new(shell_prog);
        cmd.arg(shell_flag)
            .arg(&wrapped_command)
            .current_dir(workdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // On Windows, prevent PowerShell from opening a visible console window
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        // Put the child in its own process group so we can kill it reliably.
        #[cfg(unix)]
        {
            unsafe {
                cmd.pre_exec(|| {
                    // Ignore failure; worst case we fall back to killing the child only.
                    let _ = libc::setpgid(0, 0);
                    Ok(())
                });
            }
        }

        let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn: {}", e))?;
        let pid = child.id();

        // Open log file (append mode).
        let mut log_file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
            .map_err(|e| format!("Failed to open log file: {}", e))?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to capture stdout".to_string())?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Failed to capture stderr".to_string())?;

        // Drain stdout/stderr to the combined log file.
        let mut log_file_err = log_file.try_clone().await.map_err(|e| e.to_string())?;
        let stdout_task = tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                let n = stdout.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                let _ = log_file.write_all(b"[stdout] ").await;
                let _ = log_file.write_all(&buf[..n]).await;
                let _ = log_file.flush().await;
            }
        });
        let stderr_task = tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                let n = stderr.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                let _ = log_file_err.write_all(b"[stderr] ").await;
                let _ = log_file_err.write_all(&buf[..n]).await;
                let _ = log_file_err.flush().await;
            }
        });

        let record = RunRecord {
            run_id: run_id.clone(),
            session_id: session_id.to_string(),
            tool_id: "shell".to_string(),
            command: Some(command.to_string()),
            workdir: Some(workdir.to_string_lossy().to_string()),
            status: RunStatus::Running,
            started_at,
            finished_at: None,
            pid,
            exit: None,
            error: None,
            log_path: Some(log_path.to_string_lossy().to_string()),
            notify,
            hard_timeout_secs,
        };

        {
            let mut runs = self.runs.lock().await;
            runs.insert(
                run_id.clone(),
                RunState {
                    record: record.clone(),
                    handle: Some(RunHandle::Child(child)),
                    log_path: Some(log_path.clone()),
                },
            );
        }

        // Completion watcher
        let manager = self.clone();
        let run_id_for_completion = run_id.clone();
        tokio::spawn(async move {
            // Take ownership of the Child so we can `.wait().await` without holding the runs lock.
            let mut child = {
                let mut runs = manager.runs.lock().await;
                runs.get_mut(&run_id_for_completion).and_then(|s| {
                    match s.handle.take() {
                        Some(RunHandle::Child(c)) => Some(c),
                        Some(other) => {
                            // Unexpected, but keep it.
                            s.handle = Some(other);
                            None
                        }
                        None => None,
                    }
                })
            };

            let exit_status = if let Some(ref mut c) = child {
                c.wait().await.ok()
            } else {
                None
            };

            // Ensure drain tasks complete.
            let _ = stdout_task.await;
            let _ = stderr_task.await;

            let finished_at = Utc::now().timestamp();
            let mut should_notify = false;
            let mut notify_session_id = String::new();

            {
                let mut runs = manager.runs.lock().await;
                if let Some(state) = runs.get_mut(&run_id_for_completion) {
                    // Child is finished; drop handle to avoid holding defunct process refs.
                    state.handle = None;
                    // If it was killed, keep that status.
                    if matches!(state.record.status, RunStatus::Running) {
                        match exit_status {
                            Some(es) => {
                                let code = es.code().unwrap_or(-1);
                                state.record.status = RunStatus::Exited;
                                state.record.exit = Some(RunExit { exit_code: code });
                            }
                            None => {
                                state.record.status = RunStatus::Failed;
                                state.record.error = Some("Process wait failed".to_string());
                            }
                        }
                        state.record.finished_at = Some(finished_at);
                    }
                    should_notify = state.record.notify;
                    notify_session_id = state.record.session_id.clone();
                }
            }

            if should_notify {
                let tail = manager
                    .tail_log(&run_id_for_completion, 8000)
                    .await
                    .unwrap_or_default();

                // Read exit code and status for the notification
                let (exit_code, status_str) = {
                    let runs = manager.runs.lock().await;
                    runs.get(&run_id_for_completion)
                        .map(|s| {
                            let code = s.record.exit.as_ref().map(|e| e.exit_code);
                            let status = format!("{:?}", s.record.status);
                            (code, status)
                        })
                        .unwrap_or((None, "Unknown".to_string()))
                };

                let status_label = match exit_code {
                    Some(0) => "成功".to_string(),
                    Some(code) => format!("失败 (exit_code={})", code),
                    None => format!("异常 (status={})", status_str),
                };

                let notify_message = format!(
                    "[后台任务完成] tool_id=shell run_id={} 状态={}\n\n日志尾部:\n{}",
                    run_id_for_completion,
                    status_label,
                    tail.trim()
                );
                log::info!(
                    "[RunManager] Pushing notification for session {}: run_id={}, status={}",
                    notify_session_id,
                    run_id_for_completion,
                    status_label
                );
                manager
                    .push_notification(
                        &notify_session_id,
                        RunNotification {
                            run_id: run_id_for_completion.clone(),
                            created_at: finished_at,
                            message: notify_message,
                        },
                    )
                    .await;

                // Wake up the owning session so notification poll loop can consume this event.
                if let Some(waker) = crate::hooks::session_waker() {
                    waker
                        .wake_session(&notify_session_id, "bg-run-notify")
                        .await;
                }
            }

            // Schedule delayed cleanup
            manager.schedule_cleanup(&run_id_for_completion);
        });

        // Hard timeout watcher (optional; 0 means "no hard timeout").
        if let Some(secs) = hard_timeout_secs {
            if secs > 0 {
                let manager = self.clone();
                let run_id_clone = run_id.clone();
                let session_id_owned = session_id.to_string();
                tokio::spawn(async move {
                    tokio::time::sleep(tokio::time::Duration::from_secs(secs)).await;
                    let did_kill = manager
                        .kill_run(&run_id_clone, "hard timeout reached")
                        .await;
                    if did_kill {
                        let _ = manager
                            .push_notification(
                                &session_id_owned,
                                RunNotification {
                                    run_id: run_id_clone.clone(),
                                    created_at: Utc::now().timestamp(),
                                    message: format!(
                                        "[后台任务超时并已停止] run_id={} (hard_timeout_secs={})",
                                        run_id_clone, secs
                                    ),
                                },
                            )
                            .await;

                        // Wake up the owning session so timeout notification can be delivered
                        // even if the session was hibernated/disconnected.
                        if let Some(waker) = crate::hooks::session_waker() {
                            waker
                                .wake_session(&session_id_owned, "bg-run-timeout")
                                .await;
                        }
                    }
                });
            }
        }

        Ok(record)
    }

    /// Start a background "task run" (in-process async job) with a log file and notifications.
    ///
    /// This is used for long-running tools that are better implemented in Rust rather than a shell
    /// child process. The task should write user-visible progress to the provided log sink.
    pub async fn start_task_run<F, Fut>(
        &self,
        session_id: &str,
        tool_id: &str,
        notify: bool,
        hard_timeout_secs: Option<u64>,
        task: F,
    ) -> Result<RunRecord, String>
    where
        F: FnOnce(RunLogSink, String) -> Fut + Send + 'static,
        Fut: Future<Output = Result<i32, String>> + Send + 'static,
    {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("Missing session_id for background run".to_string());
        }
        let tool_id = tool_id.trim();
        if tool_id.is_empty() {
            return Err("Missing tool_id for background run".to_string());
        }

        let run_id = uuid::Uuid::new_v4().to_string();
        let started_at = Utc::now().timestamp();

        let session_dir = self.session_dir(session_id);
        Self::ensure_dir(&session_dir)?;
        let log_path = session_dir.join(format!("{}.log", run_id));
        let log_sink = RunLogSink::new(&log_path).await?;

        let record = RunRecord {
            run_id: run_id.clone(),
            session_id: session_id.to_string(),
            tool_id: tool_id.to_string(),
            command: None,
            workdir: None,
            status: RunStatus::Running,
            started_at,
            finished_at: None,
            pid: None,
            exit: None,
            error: None,
            log_path: Some(log_path.to_string_lossy().to_string()),
            notify,
            hard_timeout_secs,
        };

        // Insert record before starting the task so it can be listed immediately.
        {
            let mut runs = self.runs.lock().await;
            runs.insert(
                run_id.clone(),
                RunState {
                    record: record.clone(),
                    handle: None,
                    log_path: Some(log_path.clone()),
                },
            );
        }

        // Completion watcher + status updates
        let manager = self.clone();
        let run_id_for_task = run_id.clone();
        let tool_id_owned = tool_id.to_string();
        let session_id_owned = session_id.to_string();
        let sink_for_task = log_sink.clone();

        let join = tokio::spawn(async move {
            sink_for_task
                .line(&format!(
                    "[run] started tool_id={} run_id={}",
                    tool_id_owned, run_id_for_task
                ))
                .await;

            let result = task(sink_for_task.clone(), run_id_for_task.clone()).await;
            let finished_at = Utc::now().timestamp();

            {
                let mut runs = manager.runs.lock().await;
                if let Some(state) = runs.get_mut(&run_id_for_task) {
                    // If it was killed/timeout, keep that status.
                    if matches!(state.record.status, RunStatus::Running) {
                        match result {
                            Ok(code) => {
                                state.record.status = RunStatus::Exited;
                                state.record.exit = Some(RunExit { exit_code: code });
                            }
                            Err(err) => {
                                state.record.status = RunStatus::Failed;
                                state.record.error = Some(err);
                                state.record.exit = Some(RunExit { exit_code: 1 });
                            }
                        }
                        state.record.finished_at = Some(finished_at);
                    }
                    // Drop handle reference after completion.
                    state.handle = None;
                }
            }

            // Notification (log tail is useful to drive agent follow-ups)
            let should_notify = {
                let runs = manager.runs.lock().await;
                runs.get(&run_id_for_task)
                    .map(|s| s.record.notify)
                    .unwrap_or(false)
            };

            if should_notify {
                let tail = manager
                    .tail_log(&run_id_for_task, 8000)
                    .await
                    .unwrap_or_default();
                let tail_trimmed = tail.trim();

                // For upload_artifact, extract file_id and filename and put them
                // prominently at the top so the LLM doesn't have to parse log lines.
                let notify_message = if tool_id_owned == "upload_artifact" {
                    let mut extracted_file_id = None;
                    let mut extracted_filename = None;
                    for line in tail_trimmed.lines() {
                        if line.contains("[artifact-upload-complete]") {
                            if let Some(pos) = line.find("file_id=") {
                                let rest = &line[pos + 8..];
                                extracted_file_id = Some(
                                    rest.split_whitespace().next().unwrap_or(rest).to_string(),
                                );
                            }
                        }
                        if line.contains("[artifact-upload] initiating") {
                            if let Some(pos) = line.find("filename='") {
                                let rest = &line[pos + 10..];
                                if let Some(end) = rest.find('\'') {
                                    extracted_filename = Some(rest[..end].to_string());
                                }
                            }
                        }
                    }
                    match (extracted_file_id, extracted_filename) {
                        (Some(fid), Some(fname)) => {
                            format!("[上传完成]\n✅ [{}](cteno-file://{})", fname, fid)
                        }
                        (Some(fid), None) => format!("[上传完成]\n✅ [file](cteno-file://{})", fid),
                        _ => format!(
                            "[后台任务完成] tool_id={} run_id={}\n\n日志尾部:\n{}",
                            tool_id_owned, run_id_for_task, tail_trimmed
                        ),
                    }
                } else {
                    format!(
                        "[后台任务完成] tool_id={} run_id={}\n\n日志尾部:\n{}",
                        tool_id_owned, run_id_for_task, tail_trimmed
                    )
                };
                manager
                    .push_notification(
                        &session_id_owned,
                        RunNotification {
                            run_id: run_id_for_task.clone(),
                            created_at: finished_at,
                            message: notify_message,
                        },
                    )
                    .await;

                // Wake up the session if it was auto-hibernated.
                // The poll loop (which consumes notifications) only runs while the
                // session connection is active. If the session was hibernated after
                // its last agent loop finished, we must reconnect it so the poll
                // loop restarts and picks up this notification.
                if let Some(waker) = crate::hooks::session_waker() {
                    waker.wake_session(&session_id_owned, "bg-run-notify").await;
                }
            }

            // Schedule delayed cleanup
            manager.schedule_cleanup(&run_id_for_task);
        });

        // Store handle after spawn (so kill_run can abort).
        {
            let mut runs = self.runs.lock().await;
            if let Some(state) = runs.get_mut(&run_id) {
                state.handle = Some(RunHandle::Task(join));
            }
        }

        // Hard timeout watcher (optional; 0 means "no hard timeout").
        if let Some(secs) = hard_timeout_secs {
            if secs > 0 {
                let manager = self.clone();
                let run_id_clone = run_id.clone();
                let session_id_owned = session_id.to_string();
                tokio::spawn(async move {
                    tokio::time::sleep(tokio::time::Duration::from_secs(secs)).await;
                    let did_kill = manager
                        .kill_run(&run_id_clone, "hard timeout reached")
                        .await;
                    if did_kill {
                        let _ = manager
                            .push_notification(
                                &session_id_owned,
                                RunNotification {
                                    run_id: run_id_clone.clone(),
                                    created_at: Utc::now().timestamp(),
                                    message: format!(
                                        "[后台任务超时并已停止] run_id={} (hard_timeout_secs={})",
                                        run_id_clone, secs
                                    ),
                                },
                            )
                            .await;

                        // Ensure timeout notification is consumable by reviving the session
                        // connection if it was hibernated/disconnected.
                        if let Some(waker) = crate::hooks::session_waker() {
                            waker
                                .wake_session(&session_id_owned, "bg-run-timeout")
                                .await;
                        }
                    }
                });
            }
        }

        Ok(record)
    }
}
