//! Shell Executor
//!
//! Executes shell commands with safety checks and timeout controls.
//! Supports bash, zsh, and sh (auto-detected from $SHELL).
use crate::runs::RunManager;
use crate::tool::ToolExecutor;
use crate::tool_executors::sandbox::{self, SandboxCheckResult};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Result of a synchronous shell execution that may be moved to background.
pub enum RunSyncResult {
    Completed {
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    Backgrounded {
        run_id: String,
    },
}

/// Maximum output size in characters. Outputs exceeding this limit are truncated
/// with head/tail preservation (80% head, 20% tail).
const MAX_OUTPUT_CHARS: usize = 100_000;

/// Semantic interpretation of a command's non-zero exit code.
struct CommandSemantics {
    /// Whether this exit code represents a real error.
    is_error: bool,
    /// Optional human-readable explanation when not an error.
    message: Option<&'static str>,
}

/// Interpret the exit code of a command based on well-known semantics.
///
/// Many commands use exit code 1 to convey "no results" rather than "failure".
/// For example, `grep` returns 1 when no matches are found.
fn interpret_command_result(command: &str, exit_code: i32) -> CommandSemantics {
    if exit_code == 0 {
        return CommandSemantics {
            is_error: false,
            message: None,
        };
    }

    // For piped commands, the exit code comes from the last command in the pipeline.
    let effective_command = command.rsplit('|').next().unwrap_or(command).trim();

    // Extract the base command name (first token), stripping any leading path.
    let cmd_name = effective_command
        .split_whitespace()
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("");

    match cmd_name {
        // grep/rg/egrep/fgrep: 0=matches found, 1=no matches, 2+=error
        "grep" | "rg" | "egrep" | "fgrep" => CommandSemantics {
            is_error: exit_code >= 2,
            message: if exit_code == 1 {
                Some("No matches found")
            } else {
                None
            },
        },
        // diff: 0=no differences, 1=differences found, 2+=error
        "diff" | "colordiff" => CommandSemantics {
            is_error: exit_code >= 2,
            message: if exit_code == 1 {
                Some("Files differ")
            } else {
                None
            },
        },
        // find/fd: 0=success, 1=partial (some dirs inaccessible), 2+=error
        "find" | "fd" => CommandSemantics {
            is_error: exit_code >= 2,
            message: if exit_code == 1 {
                Some("Some directories were inaccessible")
            } else {
                None
            },
        },
        // test/[: 0=condition true, 1=condition false, 2+=error
        "test" | "[" => CommandSemantics {
            is_error: exit_code >= 2,
            message: if exit_code == 1 {
                Some("Condition is false")
            } else {
                None
            },
        },
        // All other commands: non-zero is an error.
        _ => CommandSemantics {
            is_error: true,
            message: None,
        },
    }
}

/// Truncate output that exceeds MAX_OUTPUT_CHARS, preserving head and tail.
/// Uses char_indices to avoid splitting in the middle of multi-byte UTF-8 characters.
fn truncate_output(output: &str) -> String {
    if output.len() <= MAX_OUTPUT_CHARS {
        // Fast path: byte length within limit means char count is also within limit.
        return output.to_string();
    }

    // Count actual characters to decide if truncation is needed.
    let char_count = output.chars().count();
    if char_count <= MAX_OUTPUT_CHARS {
        return output.to_string();
    }

    let head_chars = MAX_OUTPUT_CHARS * 4 / 5; // 80% head
    let tail_chars = MAX_OUTPUT_CHARS / 5; // 20% tail
    let omitted = char_count - head_chars - tail_chars;

    // Find byte offset for end of head portion.
    let head_end = output
        .char_indices()
        .nth(head_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(output.len());

    // Find byte offset for start of tail portion (char_count - tail_chars from the start).
    let tail_start_char = char_count - tail_chars;
    let tail_start = output
        .char_indices()
        .nth(tail_start_char)
        .map(|(idx, _)| idx)
        .unwrap_or(output.len());

    format!(
        "{}\n\n[... {} characters omitted ...]\n\n{}",
        &output[..head_end],
        omitted,
        &output[tail_start..]
    )
}

/// Shell command executor
pub struct ShellExecutor {
    /// Default timeout in seconds
    timeout_secs: u64,
    /// Dangerous command patterns to block
    blacklist: Vec<String>,
    /// Background runs manager (for background=true)
    run_manager: Arc<RunManager>,
}

impl ShellExecutor {
    /// Create a new ShellExecutor with default settings
    pub fn new(run_manager: Arc<RunManager>) -> Self {
        Self {
            timeout_secs: 30,
            blacklist: vec![
                "rm -rf /".to_string(),
                "rm -rf /*".to_string(),
                "curl | sh".to_string(),
                "curl|sh".to_string(),
                "wget | sh".to_string(),
                "wget|sh".to_string(),
                ":(){ :|:& };:".to_string(), // fork bomb
            ],
            run_manager,
        }
    }

    /// Get the system shell and its command-execution flag.
    /// Returns (shell_program, flag) where flag is "-c" on Unix and "-Command" on Windows.
    pub fn get_shell() -> (String, &'static str) {
        #[cfg(windows)]
        {
            ("powershell".to_string(), "-Command")
        }
        #[cfg(not(windows))]
        {
            (
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()),
                "-c",
            )
        }
    }

    /// On Windows, wrap a command with UTF-8 encoding settings so that
    /// PowerShell outputs UTF-8 instead of the system default (e.g. GBK on
    /// Chinese Windows). On other platforms, return the command unchanged.
    pub fn wrap_command_utf8(command: &str) -> String {
        #[cfg(windows)]
        {
            format!(
                "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; \
                 $OutputEncoding = [System.Text.Encoding]::UTF8; \
                 {}",
                command
            )
        }
        #[cfg(not(windows))]
        {
            command.to_string()
        }
    }

    /// Check if a command contains dangerous patterns
    fn is_dangerous(&self, command: &str) -> Option<&str> {
        self.blacklist
            .iter()
            .find(|pattern| command.contains(*pattern))
            .map(|pattern| pattern.as_str())
    }

    /// Expand tilde (~) in paths
    fn expand_tilde(path: &str) -> String {
        let home = Self::user_home_dir();
        if path == "~" {
            return home.to_string_lossy().to_string();
        }
        if path.starts_with("~/") || path.starts_with("~\\") {
            return home.join(&path[2..]).to_string_lossy().to_string();
        }
        path.to_string()
    }

    fn user_home_dir() -> PathBuf {
        dirs::home_dir()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn resolve_workdir(workdir: Option<&str>) -> String {
        let raw = workdir.unwrap_or("~");
        let expanded = Self::expand_tilde(raw);
        let path = PathBuf::from(&expanded);
        if path.is_relative() {
            Self::user_home_dir()
                .join(path)
                .to_string_lossy()
                .to_string()
        } else {
            expanded
        }
    }

    fn caller_session_id(input: &Value) -> Option<String> {
        input
            .get("__session_id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    async fn run_sync(
        &self,
        shell: &str,
        shell_flag: &str,
        command: &str,
        workdir: &str,
        timeout_secs: u64,
        bg_receiver: Option<tokio::sync::oneshot::Receiver<()>>,
        session_id: Option<&str>,
    ) -> Result<RunSyncResult, String> {
        let wrapped = Self::wrap_command_utf8(command);
        let mut cmd = Command::new(shell);
        cmd.arg(shell_flag)
            .arg(&wrapped)
            .current_dir(workdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // When background handoff is possible, we use kill_on_drop(false) so the child
            // survives being moved to RunManager. Otherwise, keep kill_on_drop(true) as safety net.
            .kill_on_drop(bg_receiver.is_none());

        // On Windows, prevent PowerShell from opening a visible console window
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child_opt: Option<tokio::process::Child> = Some(
            cmd.spawn()
                .map_err(|e| format!("Failed to spawn command: {}", e))?,
        );

        let mut stdout = child_opt
            .as_mut()
            .unwrap()
            .stdout
            .take()
            .ok_or_else(|| "Failed to capture stdout".to_string())?;
        let mut stderr = child_opt
            .as_mut()
            .unwrap()
            .stderr
            .take()
            .ok_or_else(|| "Failed to capture stderr".to_string())?;

        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf).await;
            buf
        });
        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf).await;
            buf
        });

        enum SyncOutcome {
            Completed(std::process::ExitStatus),
            WaitError(String),
            TimedOut,
            Backgrounded,
        }

        let outcome = {
            let child_ref = child_opt.as_mut().unwrap();
            match bg_receiver {
                Some(rx) => {
                    tokio::select! {
                        result = tokio::time::timeout(Duration::from_secs(timeout_secs), child_ref.wait()) => {
                            match result {
                                Ok(Ok(status)) => SyncOutcome::Completed(status),
                                Ok(Err(e)) => SyncOutcome::WaitError(format!("Failed to wait for command: {}", e)),
                                Err(_) => SyncOutcome::TimedOut,
                            }
                        }
                        _ = rx => SyncOutcome::Backgrounded,
                    }
                }
                None => {
                    match tokio::time::timeout(Duration::from_secs(timeout_secs), child_ref.wait())
                        .await
                    {
                        Ok(Ok(status)) => SyncOutcome::Completed(status),
                        Ok(Err(e)) => {
                            SyncOutcome::WaitError(format!("Failed to wait for command: {}", e))
                        }
                        Err(_) => SyncOutcome::TimedOut,
                    }
                }
            }
        };

        match outcome {
            SyncOutcome::Completed(status) => {
                let out = stdout_task.await.unwrap_or_default();
                let err = stderr_task.await.unwrap_or_default();
                // Explicitly clean up the child handle
                drop(child_opt.take());
                let stdout_s = String::from_utf8_lossy(&out).to_string();
                let stderr_s = String::from_utf8_lossy(&err).to_string();
                let code = status.code().unwrap_or(-1);
                Ok(RunSyncResult::Completed {
                    exit_code: code,
                    stdout: stdout_s,
                    stderr: stderr_s,
                })
            }
            SyncOutcome::WaitError(e) => {
                if let Some(mut child) = child_opt.take() {
                    let _ = child.kill().await;
                }
                Err(e)
            }
            SyncOutcome::TimedOut => {
                if let Some(mut child) = child_opt.take() {
                    let _ = child.kill().await;
                }
                Err(format!("Command timed out after {} seconds", timeout_secs))
            }
            SyncOutcome::Backgrounded => {
                let child = child_opt.take().unwrap();
                let sid = session_id.unwrap_or("unknown");
                let record = self
                    .run_manager
                    .adopt_process(sid, command, workdir, child, stdout_task, stderr_task)
                    .await?;
                Ok(RunSyncResult::Backgrounded {
                    run_id: record.run_id,
                })
            }
        }
    }
}

impl Default for ShellExecutor {
    fn default() -> Self {
        Self::new(Arc::new(RunManager::new(std::env::temp_dir())))
    }
}

#[async_trait]
impl ToolExecutor for ShellExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        // Extract parameters
        let command = input["command"]
            .as_str()
            .ok_or("Missing 'command' parameter")?;

        if command.trim().is_empty() {
            return Err("Command cannot be empty".to_string());
        }

        // Safety check
        if let Some(pattern) = self.is_dangerous(command) {
            return Err(format!(
                "Dangerous command blocked: contains '{}'\n\nCommand: {}",
                pattern, command
            ));
        }

        // Sandbox check: detect output redirections targeting outside workspace
        let sandbox_ctx = sandbox::SandboxContext::from_input(&input);
        if let SandboxCheckResult::Denied(reason) =
            sandbox::check_shell_command(command, &sandbox_ctx)
        {
            return Err(format!("SANDBOX_DENIED: {}", reason));
        }

        let workdir = Self::resolve_workdir(input["workdir"].as_str());

        let background = coerce_bool(input.get("background")).unwrap_or(false);

        // Get shell
        let (shell, shell_flag) = Self::get_shell();

        if !background {
            // Accept `timeout` as int, float, or numeric string — LLMs (especially
            // small proxy models) frequently emit numbers as strings despite the
            // JSON schema saying integer. Also tolerate the legacy `timeout_secs`
            // name that older schemas advertised.
            let timeout = input
                .get("timeout")
                .and_then(coerce_u64)
                .or_else(|| input.get("timeout_secs").and_then(coerce_u64))
                .ok_or_else(|| "Missing 'timeout' parameter. You must specify a timeout in seconds for synchronous shell commands (e.g. timeout: 30). For long-running tasks, use background: true instead.".to_string())?;

            // Register background signal if call_id is available
            let call_id = input
                .get("__call_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let bg_receiver = if let Some(ref cid) = call_id {
                Some(self.run_manager.register_background_signal(cid).await)
            } else {
                None
            };

            let session_id = Self::caller_session_id(&input);

            let result = self
                .run_sync(
                    &shell,
                    shell_flag,
                    command,
                    &workdir,
                    timeout,
                    bg_receiver,
                    session_id.as_deref(),
                )
                .await;

            // Cleanup background signal registration on normal completion
            if let Some(ref cid) = call_id {
                self.run_manager.unregister_background_signal(cid).await;
            }

            match result {
                Ok(RunSyncResult::Completed {
                    exit_code,
                    stdout,
                    stderr,
                }) => {
                    // Apply output truncation to prevent excessively large outputs.
                    let stdout = truncate_output(&stdout);
                    let stderr = truncate_output(&stderr);

                    if exit_code == 0 {
                        if stdout.is_empty() && stderr.is_empty() {
                            Ok("(Command executed successfully with no output)".to_string())
                        } else if stdout.is_empty() {
                            Ok(stderr)
                        } else {
                            Ok(stdout)
                        }
                    } else {
                        // Check if this non-zero exit code is semantically an error.
                        let semantics = interpret_command_result(command, exit_code);

                        if semantics.is_error {
                            let mut error_msg =
                                format!("Command failed with exit code {}", exit_code);
                            if !stdout.is_empty() {
                                error_msg.push_str(&format!("\n\nStdout:\n{}", stdout));
                            }
                            if !stderr.is_empty() {
                                error_msg.push_str(&format!("\n\nStderr:\n{}", stderr));
                            }
                            Err(error_msg)
                        } else {
                            // Non-zero but not a semantic error (e.g., grep with no matches).
                            let annotation = semantics
                                .message
                                .map(|msg| {
                                    format!(
                                        "(exit code {}: {})",
                                        exit_code, msg
                                    )
                                })
                                .unwrap_or_else(|| {
                                    format!("(exit code {})", exit_code)
                                });

                            if stdout.is_empty() && stderr.is_empty() {
                                Ok(annotation)
                            } else {
                                let output = if stdout.is_empty() {
                                    stderr
                                } else {
                                    stdout
                                };
                                Ok(format!("{}\n{}", output, annotation))
                            }
                        }
                    }
                }
                Ok(RunSyncResult::Backgrounded { run_id }) => Ok(format!(
                    "Command moved to background by user: run_id={}\nThe process continues running. Use the run_manager tool to check status/logs.",
                    run_id
                )),
                Err(e) => {
                    if e.contains("timed out") {
                        Err(format!(
                            "{}\n\nTip: For long-running tasks, re-run with background=true. Then call the run_manager tool to inspect logs/stop.",
                            e
                        ))
                    } else {
                        Err(e)
                    }
                }
            }
        } else {
            let session_id = Self::caller_session_id(&input)
                .ok_or_else(|| "background=true requires internal __session_id".to_string())?;
            let wait_timeout_secs = input
                .get("wait_timeout_secs")
                .and_then(coerce_u64)
                .unwrap_or(0);
            let notify = coerce_bool(input.get("notify")).unwrap_or(true);
            let hard_timeout_secs = input
                .get("hard_timeout_secs")
                .and_then(coerce_u64)
                .or(Some(0));

            let record = self
                .run_manager
                .start_shell_run(
                    &session_id,
                    command,
                    std::path::Path::new(&workdir),
                    notify,
                    hard_timeout_secs,
                )
                .await?;

            if wait_timeout_secs == 0 {
                return Ok(format!(
                    "Started in background: run_id={}\nTo check status/logs/stop, call the run_manager tool (not a shell command) with op and run_id parameters.",
                    record.run_id
                ));
            }

            let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_timeout_secs);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return Ok(format!(
                        "Still running in background: run_id={}\nTo check logs/stop, call the run_manager tool (not a shell command) with op and run_id parameters.",
                        record.run_id
                    ));
                }

                let latest = self.run_manager.get_run(&record.run_id).await;
                if let Some(r) = latest {
                    if !matches!(r.status, crate::runs::RunStatus::Running) {
                        let tail = self
                            .run_manager
                            .tail_log(&record.run_id, 20_000)
                            .await
                            .unwrap_or_default();
                        if matches!(r.status, crate::runs::RunStatus::Exited)
                            && r.exit.as_ref().map(|e| e.exit_code == 0).unwrap_or(false)
                        {
                            return Ok(tail);
                        }
                        return Err(format!(
                            "Background run finished (status={:?}).\n\nLog tail:\n{}",
                            r.status, tail
                        ));
                    }
                }

                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }

    fn supports_background(&self) -> bool {
        true
    }

    async fn execute_background(
        &self,
        input: Value,
        session_id: Option<String>,
    ) -> Result<String, String> {
        // Back-compat: delegate to execute() with background=true and injected session id.
        let mut input = input;
        if let Some(sid) = session_id {
            if let Some(obj) = input.as_object_mut() {
                obj.insert("__session_id".to_string(), Value::String(sid));
                obj.insert("background".to_string(), Value::Bool(true));
            }
        }
        self.execute(input).await
    }
}

/// Accept u64 from JSON integer, float (truncated), or numeric string.
/// Small LLMs often stringify numbers despite `{"type":"integer"}` in schema;
/// treat that as equivalent to the native integer so tool execution is robust
/// across proxy models.
fn coerce_u64(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        return Some(n);
    }
    if let Some(f) = v.as_f64() {
        if f.is_finite() && f >= 0.0 {
            return Some(f as u64);
        }
    }
    if let Some(s) = v.as_str() {
        let s = s.trim();
        if let Ok(n) = s.parse::<u64>() {
            return Some(n);
        }
        if let Ok(f) = s.parse::<f64>() {
            if f.is_finite() && f >= 0.0 {
                return Some(f as u64);
            }
        }
    }
    None
}

/// Accept bool from JSON bool or common string forms ("true"/"false", "1"/"0",
/// "yes"/"no", case-insensitive). Same rationale as `coerce_u64`.
fn coerce_bool(v: Option<&Value>) -> Option<bool> {
    let v = v?;
    if let Some(b) = v.as_bool() {
        return Some(b);
    }
    if let Some(s) = v.as_str() {
        match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "y" => return Some(true),
            "false" | "0" | "no" | "n" | "" => return Some(false),
            _ => {}
        }
    }
    if let Some(n) = v.as_i64() {
        return Some(n != 0);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_shell_executor_success() {
        let executor = ShellExecutor::new(Arc::new(RunManager::new(std::env::temp_dir())));
        let input = json!({
            "command": "echo 'Hello, World!'",
            "timeout": 10
        });

        let result = executor.execute(input).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Hello, World!"));
    }

    #[tokio::test]
    async fn test_shell_executor_with_workdir() {
        let executor = ShellExecutor::new(Arc::new(RunManager::new(std::env::temp_dir())));
        #[cfg(windows)]
        let input = json!({
            "command": "cd",
            "workdir": std::env::temp_dir().to_string_lossy().to_string(),
            "timeout": 10
        });
        #[cfg(not(windows))]
        let input = json!({
            "command": "pwd",
            "workdir": "/tmp",
            "timeout": 10
        });

        let result = executor.execute(input).await;
        assert!(result.is_ok());
        #[cfg(not(windows))]
        assert!(result.unwrap().contains("/tmp"));
    }

    #[tokio::test]
    async fn test_shell_executor_dangerous_command() {
        let executor = ShellExecutor::new(Arc::new(RunManager::new(std::env::temp_dir())));
        let input = json!({
            "command": "rm -rf /",
            "timeout": 10
        });

        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Dangerous command blocked"));
    }

    #[tokio::test]
    async fn test_shell_executor_timeout() {
        let executor = ShellExecutor::new(Arc::new(RunManager::new(std::env::temp_dir())));
        let input = json!({
            "command": "sleep 10",
            "timeout": 1
        });

        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timed out"));
    }

    #[tokio::test]
    async fn test_shell_executor_missing_timeout() {
        let executor = ShellExecutor::new(Arc::new(RunManager::new(std::env::temp_dir())));
        let input = json!({
            "command": "echo hello"
        });

        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing 'timeout'"));
    }

    #[tokio::test]
    async fn test_shell_executor_empty_command() {
        let executor = ShellExecutor::new(Arc::new(RunManager::new(std::env::temp_dir())));
        let input = json!({
            "command": ""
        });

        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot be empty"));
    }

    #[tokio::test]
    async fn test_shell_executor_missing_command() {
        let executor = ShellExecutor::new(Arc::new(RunManager::new(std::env::temp_dir())));
        let input = json!({});

        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing 'command'"));
    }

    #[test]
    fn test_expand_tilde() {
        let home = ShellExecutor::user_home_dir();
        let expanded = ShellExecutor::expand_tilde("~/test");
        assert_eq!(expanded, home.join("test").to_string_lossy());

        let no_tilde = ShellExecutor::expand_tilde("/tmp/test");
        assert_eq!(no_tilde, "/tmp/test");
    }

    #[test]
    fn test_get_shell() {
        let (shell, flag) = ShellExecutor::get_shell();
        #[cfg(windows)]
        {
            assert!(shell.contains("powershell"));
            assert_eq!(flag, "-Command");
        }
        #[cfg(not(windows))]
        {
            assert!(shell.contains("bash") || shell.contains("zsh") || shell.contains("sh"));
            assert_eq!(flag, "-c");
        }
    }

    // --- interpret_command_result tests ---

    #[test]
    fn test_grep_exit_code_1_is_not_error() {
        let s = interpret_command_result("grep foo bar.txt", 1);
        assert!(!s.is_error);
        assert_eq!(s.message, Some("No matches found"));
    }

    #[test]
    fn test_grep_exit_code_2_is_error() {
        let s = interpret_command_result("grep foo bar.txt", 2);
        assert!(s.is_error);
    }

    #[test]
    fn test_diff_exit_code_1_is_not_error() {
        let s = interpret_command_result("diff a.txt b.txt", 1);
        assert!(!s.is_error);
        assert_eq!(s.message, Some("Files differ"));
    }

    #[test]
    fn test_find_exit_code_1_is_not_error() {
        let s = interpret_command_result("find /some/path -name '*.rs'", 1);
        assert!(!s.is_error);
        assert_eq!(s.message, Some("Some directories were inaccessible"));
    }

    #[test]
    fn test_test_exit_code_1_is_not_error() {
        let s = interpret_command_result("test -f /nonexistent", 1);
        assert!(!s.is_error);
        assert_eq!(s.message, Some("Condition is false"));
    }

    #[test]
    fn test_unknown_command_exit_code_1_is_error() {
        let s = interpret_command_result("cargo build", 1);
        assert!(s.is_error);
    }

    #[test]
    fn test_piped_command_uses_last_segment() {
        // `grep foo | head` => last command is `head`, which uses default semantics
        let s = interpret_command_result("grep foo bar.txt | head", 1);
        assert!(s.is_error); // head exit code 1 is a real error

        // `cat foo | grep bar` => last command is `grep`
        let s2 = interpret_command_result("cat foo | grep bar", 1);
        assert!(!s2.is_error);
        assert_eq!(s2.message, Some("No matches found"));
    }

    #[test]
    fn test_command_with_path_prefix() {
        let s = interpret_command_result("/usr/bin/grep foo bar.txt", 1);
        assert!(!s.is_error);
    }

    #[test]
    fn test_exit_code_0_always_ok() {
        let s = interpret_command_result("anything", 0);
        assert!(!s.is_error);
        assert_eq!(s.message, None);
    }

    // --- truncate_output tests ---

    #[test]
    fn test_truncate_short_output() {
        let short = "hello world";
        assert_eq!(truncate_output(short), short);
    }

    #[test]
    fn test_truncate_exact_limit() {
        let exact: String = "a".repeat(MAX_OUTPUT_CHARS);
        assert_eq!(truncate_output(&exact), exact);
    }

    #[test]
    fn test_truncate_over_limit() {
        let long: String = "x".repeat(MAX_OUTPUT_CHARS + 1000);
        let result = truncate_output(&long);
        assert!(result.contains("[... 1000 characters omitted ...]"));
        assert!(result.len() < long.len());
    }

    #[test]
    fn test_truncate_utf8_multibyte() {
        // Each Chinese character is 3 bytes in UTF-8, so this ensures
        // we don't split on byte boundaries.
        let long: String = "\u{4e2d}".repeat(MAX_OUTPUT_CHARS + 500); // "中" repeated
        let result = truncate_output(&long);
        assert!(result.contains("[... 500 characters omitted ...]"));
        // Verify the result is valid UTF-8 (it is, since it compiles as String)
        assert!(result.is_char_boundary(0));
    }

    // --- Integration: grep returning exit code 1 is Ok ---

    #[tokio::test]
    async fn test_grep_no_match_returns_ok() {
        let executor = ShellExecutor::new(Arc::new(RunManager::new(std::env::temp_dir())));
        let input = json!({
            "command": "grep 'DEFINITELY_NOT_PRESENT_ZYXWV' /dev/null",
            "timeout": 10
        });

        let result = executor.execute(input).await;
        // grep with no match should be Ok, not Err
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("No matches found"));
    }
}
