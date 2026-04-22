//! Unified Skill Tool Executor
//!
//! Runtime operations: activate/deactivate, list, search/browse/install (SkillHub).
//! Skill creation is handled by the `skill-create` builtin skill (SKILL.md guide).
//!
//! Also supports `context: fork` skills that dispatch to a forked agent session.

use crate::agent_config::{SkillConfig, SkillContext};
use crate::hooks;
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct SkillExecutor {
    client: reqwest::Client,
    builtin_skills_dir: PathBuf,
    user_skills_dir: PathBuf,
}

impl SkillExecutor {
    pub fn new(builtin_skills_dir: PathBuf, user_skills_dir: PathBuf) -> Self {
        Self {
            client: reqwest::Client::new(),
            builtin_skills_dir,
            user_skills_dir,
        }
    }

    fn get_str<'a>(input: &'a Value, keys: &[&str]) -> Option<&'a str> {
        for k in keys {
            if let Some(s) = input.get(*k).and_then(|v| v.as_str()) {
                if !s.trim().is_empty() {
                    return Some(s);
                }
            }
        }
        None
    }

    fn get_u64(input: &Value, keys: &[&str]) -> Option<u64> {
        for k in keys {
            if let Some(n) = input.get(*k).and_then(|v| v.as_u64()) {
                return Some(n);
            }
        }
        None
    }

    /// Resolve workspace skills dir from workdir injected by session context.
    /// Session injects "workdir" (no prefix); fallback to "__workdir" for compatibility.
    fn workspace_skills_dir(input: &Value) -> Option<PathBuf> {
        input
            .get("workdir")
            .or_else(|| input.get("__workdir"))
            .and_then(|v| v.as_str())
            .map(|wd| {
                let expanded = shellexpand::tilde(wd).to_string();
                PathBuf::from(expanded).join(".cteno").join("skills")
            })
    }

    /// Load all skills with three-layer resolution (builtin > global > workspace).
    ///
    /// Delegates to the host via `SkillRegistryProvider` because skill FS layout
    /// (which builtin/global dirs to scan) is owned by the app.
    fn load_skills(&self, input: &Value) -> Vec<SkillConfig> {
        let ws = Self::workspace_skills_dir(input);
        match hooks::skill_registry() {
            Some(p) => p.load_all_skills(ws.as_deref()),
            None => {
                log::warn!("SkillRegistryProvider not installed — returning empty skill list");
                Vec::new()
            }
        }
    }

    // =========================================================================
    // activate / deactivate (from skill_context.rs)
    // =========================================================================

    async fn activate_skill(
        &self,
        input: &Value,
        id: &str,
        include_resources: bool,
    ) -> Result<String, String> {
        let skills = self.load_skills(input);

        let skill = skills
            .iter()
            .find(|s| s.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| {
                format!(
                    "Skill not found: {}. Use 'search' or 'list' to find available skills.",
                    id
                )
            })?;

        // Check if this is a fork-context skill
        if skill.context == Some(SkillContext::Fork) {
            return self.activate_fork_skill(input, skill).await;
        }

        // Inline activation: load instructions, apply substitutions
        let mut instructions = skill
            .instructions
            .clone()
            .unwrap_or_else(|| skill.description.clone());

        // Variable substitution (aligned with Claude Code's ${CLAUDE_SKILL_DIR} etc.)
        let skill_dir = skill
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let session_id = input
            .get("__session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let args = Self::get_str(input, &["args"]).unwrap_or("");

        instructions = instructions
            .replace("${SKILL_DIR}", &skill_dir)
            .replace("${SESSION_ID}", session_id)
            .replace("${SKILL_ID}", &skill.id)
            .replace("${SKILL_NAME}", &skill.name);

        // Argument substitution ($ARGS and $1)
        if !args.is_empty() {
            instructions = instructions.replace("$ARGS", args).replace("$1", args);
        }

        // Execute shell blocks in SKILL.md (```! command ``` and !`command`)
        if let Some(ref skill_path) = skill.path {
            instructions = execute_shell_blocks(&instructions, skill_path);
        }

        let resources = if include_resources {
            skill
                .path
                .as_ref()
                .map(|p| render_tree(p, 3, 200))
                .unwrap_or_else(|| "(unknown skill path)".to_string())
        } else {
            String::new()
        };

        Ok(format!(
            "<activated_skill id=\"{}\" name=\"{}\">\n  <description>\n    {}\n  </description>\n\n  <instructions>\n{}\n  </instructions>\n\n  <available_resources>\n{}\n  </available_resources>\n</activated_skill>",
            escape_xml(&skill.id),
            escape_xml(&skill.name),
            indent_block(&skill.description, 4),
            indent_block(&instructions, 4),
            indent_block(&resources, 4),
        ))
    }

    async fn activate_fork_skill(
        &self,
        input: &Value,
        skill: &SkillConfig,
    ) -> Result<String, String> {
        // Extract owner id (persona) from injected context. Both `__owner_id`
        // (current) and `__persona_id` (legacy) are honored.
        let persona_id = input
            .get("__owner_id")
            .or_else(|| input.get("__persona_id"))
            .and_then(|v| v.as_str())
            .ok_or("Fork skill requires an agent owner context (persona_id)")?
            .to_string();

        let dispatch = hooks::persona_dispatch().ok_or_else(|| {
            "PersonaDispatchProvider not installed — fork skill requires commercial feature"
                .to_string()
        })?;

        let agent_type = skill.agent.as_deref().unwrap_or("worker");
        let task = format!(
            "Execute skill '{}' instructions: {}",
            skill.id, skill.description
        );
        let skill_ids = vec![skill.id.clone()];
        let session_id = dispatch
            .dispatch_task(
                &persona_id,
                &task,
                None,
                None,
                Some(skill_ids),
                Some(agent_type),
                None,
            )
            .await?;

        Ok(json!({
            "forked": true,
            "session_id": session_id,
            "agent_type": agent_type,
            "skill_id": skill.id,
            "message": format!(
                "Skill '{}' dispatched to {} agent in session {}. Results will be pushed back.",
                skill.name, agent_type, session_id
            )
        })
        .to_string())
    }

    fn deactivate_skill(&self, id: &str, reason: Option<&str>) -> Result<String, String> {
        let reason_text = reason
            .map(|r| format!("\n  Reason: {}", escape_xml(r)))
            .unwrap_or_default();

        Ok(format!(
            "<deactivated_skill id=\"{}\">\n  \
            This skill has been deactivated and should no longer be used.{}\n\
            </deactivated_skill>",
            escape_xml(id),
            reason_text
        ))
    }

    // =========================================================================
    // list (enhanced from skill_manager.rs)
    // =========================================================================

    fn list_all_skills(&self, input: &Value) -> Result<String, String> {
        let skills = self.load_skills(input);

        let list: Vec<_> = skills
            .iter()
            .map(|s| {
                let mut obj = json!({
                    "id": s.id,
                    "name": s.name,
                    "description": s.description,
                    "is_bundled": s.is_bundled,
                });
                if let Some(ref src) = s.source {
                    obj.as_object_mut()
                        .unwrap()
                        .insert("source".to_string(), json!(src));
                }
                if let Some(ref wtu) = s.when_to_use {
                    obj.as_object_mut()
                        .unwrap()
                        .insert("when_to_use".to_string(), json!(wtu));
                }
                if let Some(ref ctx) = s.context {
                    let ctx_str = match ctx {
                        SkillContext::Inline => "inline",
                        SkillContext::Fork => "fork",
                    };
                    obj.as_object_mut()
                        .unwrap()
                        .insert("context".to_string(), json!(ctx_str));
                }
                obj
            })
            .collect();

        Ok(serde_json::to_string_pretty(&list).unwrap_or_else(|_| "[]".to_string()))
    }

    // =========================================================================
    // SkillHub operations (from skill_manager.rs)
    // =========================================================================

    fn github_mirror_prefixes() -> Vec<String> {
        let mut prefixes = vec![String::new()];
        if let Ok(raw) = std::env::var("CTENO_GITHUB_MIRROR") {
            for part in raw.split(',') {
                let trimmed = part.trim();
                if !trimmed.is_empty() {
                    prefixes.push(trimmed.to_string());
                }
            }
        }
        for default_prefix in ["https://ghproxy.com/", "https://mirror.ghproxy.com/"] {
            if !prefixes.iter().any(|item| item == default_prefix) {
                prefixes.push(default_prefix.to_string());
            }
        }
        prefixes
    }

    fn apply_mirror_prefix(prefix: &str, url: &str) -> String {
        let trimmed = prefix.trim();
        if trimmed.is_empty() {
            return url.to_string();
        }
        if trimmed.ends_with('/') {
            format!("{}{}", trimmed, url)
        } else {
            format!("{}/{}", trimmed, url)
        }
    }
}

// =============================================================================
// ToolExecutor impl
// =============================================================================

#[async_trait]
impl ToolExecutor for SkillExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let operation = Self::get_str(&input, &["operation", "command"])
            .ok_or("Missing required parameter: operation")?
            .to_ascii_lowercase();

        match operation.as_str() {
            // --- Context operations ---
            "activate" => {
                let id = Self::get_str(&input, &["id"])
                    .ok_or("Missing required parameter: id")?
                    .to_string();
                let include_resources = input
                    .get("include_resources")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                self.activate_skill(&input, &id, include_resources).await
            }
            "deactivate" => {
                let id = Self::get_str(&input, &["id"])
                    .ok_or("Missing required parameter: id")?;
                let reason = Self::get_str(&input, &["reason"]);
                self.deactivate_skill(id, reason)
            }

            // --- List ---
            "list" | "list_installed" => {
                self.list_all_skills(&input)
            }

            // --- SkillHub operations ---
            "search" | "search_skillhub" => {
                let query = Self::get_str(&input, &["query", "q", "keyword"])
                    .ok_or("Missing required parameter: query")?
                    .to_string();
                let limit = Self::get_u64(&input, &["limit"]).unwrap_or(20) as usize;

                let registry = hooks::skill_registry()
                    .ok_or("SkillRegistryProvider not installed")?;
                let skills = registry
                    .search_skills(&query, limit)
                    .await
                    .map_err(|e| format!("SkillHub search failed: {}", e))?;

                let results_len = skills.as_array().map(|a| a.len()).unwrap_or(0);
                let out = json!({
                    "query": query,
                    "results": results_len,
                    "skills": skills,
                });
                serde_json::to_string_pretty(&out)
                    .map_err(|e| format!("Failed to serialize search results: {}", e))
            }

            "browse" | "featured" => {
                let registry = hooks::skill_registry()
                    .ok_or("SkillRegistryProvider not installed")?;
                let skills = registry
                    .fetch_featured()
                    .await
                    .map_err(|e| format!("SkillHub fetch failed: {}", e))?;

                let results_len = skills.as_array().map(|a| a.len()).unwrap_or(0);
                let out = json!({
                    "results": results_len,
                    "skills": skills,
                });
                serde_json::to_string_pretty(&out)
                    .map_err(|e| format!("Failed to serialize featured skills: {}", e))
            }

            "install" | "install_from_skillhub" => {
                let slug = Self::get_str(&input, &["slug", "skill_id", "name"])
                    .ok_or("Missing required parameter: slug")?
                    .to_string();

                let registry = hooks::skill_registry()
                    .ok_or("SkillRegistryProvider not installed")?;
                let installed = registry.install_skill(&slug).await?;
                serde_json::to_string_pretty(&installed)
                    .map_err(|e| format!("Failed to serialize install result: {}", e))
            }

            other => Err(format!(
                "Unknown operation: '{}'. Supported: list, activate, deactivate, search, browse, install",
                other
            )),
        }
    }
}

// =============================================================================
// Helper functions
// =============================================================================

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn indent_block(s: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    s.lines()
        .map(|l| format!("{}{}", pad, l))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_tree(root: &Path, max_depth: usize, max_entries: usize) -> String {
    let mut out = Vec::new();
    let mut entries = 0usize;
    let root = root.to_path_buf();
    let root_name = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("(skill)");
    out.push(format!("{}/", root_name));

    fn walk(
        root: &Path,
        base: &Path,
        depth: usize,
        max_depth: usize,
        out: &mut Vec<String>,
        entries: &mut usize,
        max_entries: usize,
    ) {
        if depth > max_depth || *entries >= max_entries {
            return;
        }

        let Ok(read_dir) = std::fs::read_dir(base) else {
            return;
        };

        let mut children: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
        children.sort_by_key(|e| e.file_name());

        for child in children {
            if *entries >= max_entries {
                return;
            }

            let path = child.path();
            let name = child.file_name().to_string_lossy().to_string();

            if name.starts_with('.') || name == "node_modules" {
                continue;
            }

            let rel = path.strip_prefix(root).unwrap_or(&path);
            let indent = "  ".repeat(depth);
            if path.is_dir() {
                out.push(format!("{}- {}/", indent, rel.display()));
                *entries += 1;
                walk(root, &path, depth + 1, max_depth, out, entries, max_entries);
            } else {
                out.push(format!("{}- {}", indent, rel.display()));
                *entries += 1;
            }
        }
    }

    walk(
        &root,
        &root,
        1,
        max_depth,
        &mut out,
        &mut entries,
        max_entries,
    );

    if entries >= max_entries {
        out.push("  ... (truncated)".to_string());
    }

    out.join("\n")
}

/// Execute shell command blocks embedded in SKILL.md content.
/// Supports two syntaxes (aligned with Claude Code's `executeShellCommandsInPrompt`):
/// - Fenced blocks: ```! command ```
/// - Inline: !`command`
fn execute_shell_blocks(content: &str, cwd: &Path) -> String {
    use std::process::Command;
    use std::time::Duration;

    let mut result = content.to_string();

    // Pattern 1: ```! command ``` fenced blocks
    let block_re = regex::Regex::new(r"```!\s*\n?([\s\S]*?)\n?```").unwrap();
    let block_matches: Vec<(String, String)> = block_re
        .captures_iter(content)
        .filter_map(|cap| {
            let full = cap.get(0)?.as_str().to_string();
            let cmd = cap.get(1)?.as_str().trim().to_string();
            if cmd.is_empty() {
                None
            } else {
                Some((full, cmd))
            }
        })
        .collect();

    for (full_match, cmd) in block_matches {
        let output = run_shell_command(&cmd, cwd);
        result = result.replace(&full_match, &output);
    }

    // Pattern 2: !`command` inline
    let inline_re = regex::Regex::new(r"(?:^|\s)!`([^`]+)`").unwrap();
    let inline_matches: Vec<(String, String)> = inline_re
        .captures_iter(&result.clone())
        .filter_map(|cap| {
            let full = cap.get(0)?.as_str().to_string();
            let cmd = cap.get(1)?.as_str().trim().to_string();
            if cmd.is_empty() {
                None
            } else {
                Some((full, cmd))
            }
        })
        .collect();

    for (full_match, cmd) in inline_matches {
        let output = run_shell_command(&cmd, cwd);
        // Preserve leading whitespace from the match
        let trimmed = full_match.trim_start();
        let leading = &full_match[..full_match.len() - trimmed.len()];
        result = result.replacen(&full_match, &format!("{}{}", leading, output), 1);
    }

    result
}

/// Run a single shell command with timeout and output limits.
fn run_shell_command(cmd: &str, cwd: &Path) -> String {
    use std::process::Command;

    let child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    match child {
        Ok(child) => {
            match child.wait_with_output() {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let mut result = stdout.trim().to_string();
                    if !stderr.trim().is_empty() {
                        if !result.is_empty() {
                            result.push('\n');
                        }
                        result.push_str(&format!("[stderr: {}]", stderr.trim()));
                    }
                    // Limit output to 10KB
                    if result.len() > 10240 {
                        result.truncate(10240);
                        result.push_str("\n... (output truncated at 10KB)");
                    }
                    result
                }
                Err(e) => format!("[Error: {}]", e),
            }
        }
        Err(e) => format!("[Error: failed to execute: {}]", e),
    }
}
