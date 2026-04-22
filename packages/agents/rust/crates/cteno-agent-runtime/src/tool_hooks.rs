use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

#[derive(Debug, Clone, Deserialize)]
pub struct HookConfig {
    /// Tool name pattern (supports exact match or "*" for all tools)
    pub tool: String,
    /// When to run: "pre" or "post"
    pub event: String,
    /// Shell command to execute
    pub command: String,
    /// Timeout in seconds (default: 5 for pre, 10 for post)
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

fn default_timeout() -> u64 {
    5
}

#[derive(Debug, Clone, Deserialize)]
struct HooksFile {
    #[serde(default)]
    hooks: Vec<HookConfig>,
}

pub enum HookResult {
    /// Continue with (possibly modified) input
    Continue,
    /// Block execution with message
    Block(String),
}

pub struct HooksManager {
    hooks: Vec<HookConfig>,
}

impl HooksManager {
    /// Load hooks from .cteno/hooks.yaml in workspace directory
    pub fn load(workspace_dir: Option<&Path>) -> Self {
        let hooks = workspace_dir
            .and_then(|dir| {
                let path = dir.join(".cteno").join("hooks.yaml");
                let content = std::fs::read_to_string(&path).ok()?;
                let file: HooksFile = serde_yaml::from_str(&content).ok()?;
                log::info!("Loaded {} tool hooks from {:?}", file.hooks.len(), path);
                Some(file.hooks)
            })
            .unwrap_or_default();
        Self { hooks }
    }

    /// Run pre-execution hooks for a tool. Returns Block if any hook rejects.
    pub async fn run_pre_hooks(&self, tool_name: &str, input: &Value) -> HookResult {
        for hook in &self.hooks {
            if hook.event != "pre" || !Self::matches_tool(&hook.tool, tool_name) {
                continue;
            }
            match Self::execute_hook(hook, tool_name, input, None).await {
                Ok(output) => {
                    let trimmed = output.trim();
                    if trimmed.to_lowercase().starts_with("block:") {
                        // Strip the "block:" prefix (case-insensitive) to get the message
                        let msg = trimmed["block:".len()..].trim();
                        log::info!("Pre-hook blocked tool '{}': {}", tool_name, msg);
                        return HookResult::Block(format!("Blocked by pre-hook: {}", msg));
                    }
                }
                Err(e) => {
                    log::warn!("Pre-hook failed for '{}': {} (continuing)", tool_name, e);
                    // Pre-hook failure = continue (don't block on broken hooks)
                }
            }
        }
        HookResult::Continue
    }

    /// Run post-execution hooks. Returns replacement output if any hook provides one.
    pub async fn run_post_hooks(
        &self,
        tool_name: &str,
        input: &Value,
        output: &str,
    ) -> Option<String> {
        let mut final_output = None;
        for hook in &self.hooks {
            if hook.event != "post" || !Self::matches_tool(&hook.tool, tool_name) {
                continue;
            }
            match Self::execute_hook(hook, tool_name, input, Some(output)).await {
                Ok(hook_output) => {
                    if !hook_output.trim().is_empty() {
                        final_output = Some(hook_output);
                    }
                }
                Err(e) => {
                    log::warn!("Post-hook failed for '{}': {} (ignoring)", tool_name, e);
                }
            }
        }
        final_output
    }

    fn matches_tool(pattern: &str, tool_name: &str) -> bool {
        pattern == "*" || pattern == tool_name
    }

    async fn execute_hook(
        hook: &HookConfig,
        tool_name: &str,
        input: &Value,
        output: Option<&str>,
    ) -> Result<String, String> {
        let timeout_duration = Duration::from_secs(hook.timeout);

        // Build environment: TOOL_NAME, TOOL_INPUT, TOOL_OUTPUT
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(&hook.command);
        cmd.env("TOOL_NAME", tool_name);
        cmd.env(
            "TOOL_INPUT",
            serde_json::to_string(input).unwrap_or_default(),
        );
        if let Some(out) = output {
            cmd.env("TOOL_OUTPUT", out);
        }
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let result = timeout(timeout_duration, cmd.output())
            .await
            .map_err(|_| format!("Hook timed out after {}s", hook.timeout))?
            .map_err(|e| format!("Hook execution failed: {}", e))?;

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(format!("Hook exited with {}: {}", result.status, stderr));
        }

        Ok(String::from_utf8_lossy(&result.stdout).to_string())
    }
}
