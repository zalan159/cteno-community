//! System Prompt Builder
//!
//! Builds modular, context-aware system prompts for autonomous agents.
//! Tool-specific instructions live in each TOOL.md / SKILL.md and are
//! injected via the LLM API `tools` parameter — NOT in this prompt.

use std::fs;
use std::path::PathBuf;

/// System prompt building options
#[derive(Debug, Clone)]
pub struct PromptOptions {
    /// Workspace directory path (for loading SOUL.md, IDENTITY.md, etc.)
    pub workspace_path: Option<PathBuf>,
    /// Agent-specific instructions
    pub agent_instructions: Option<String>,
    /// Whether to include tool usage & workflow guidance
    pub include_tool_style: bool,
    /// Current date and time (ISO 8601 format)
    pub current_datetime: Option<String>,
    /// User timezone (e.g., "Asia/Shanghai")
    pub timezone: Option<String>,
}

impl Default for PromptOptions {
    fn default() -> Self {
        Self {
            workspace_path: None,
            agent_instructions: None,
            include_tool_style: true,
            current_datetime: None,
            timezone: Some("Asia/Shanghai".to_string()),
        }
    }
}

// ─── Public API ──────────────────────────────────────────────────────

/// Build complete system prompt from modular sections.
///
/// Composition order:
///   1. Identity          — who you are
///   2. Core Mandates     — safety & engineering standards
///   3. Workflow           — generic Understand → Act → Verify cycle
///   4. Response Style    — conciseness, narration rules
///   5. Project Context   — SOUL.md / IDENTITY.md / USER.md (dynamic)
///   6. Agent Instructions — per-agent overrides (dynamic)
///   7. Date & Time       — current timestamp (dynamic)
///   8. Final Reminder    — closing emphasis
pub fn build_system_prompt(options: &PromptOptions) -> String {
    let mut sections = Vec::new();

    // 1. Identity
    sections.push(render_identity());

    // 2. Core Mandates (safety + engineering standards)
    sections.push(render_core_mandates());

    // 3–4. Workflow & Response Style (guarded by flag)
    if options.include_tool_style {
        sections.push(render_workflow());
        sections.push(render_response_style());
    }

    // 5. Project Context
    if let Some(context) = load_workspace_context(&options.workspace_path) {
        if !context.is_empty() {
            sections.push(render_context(&context));
        }
    }

    // 6. Agent-specific instructions
    if let Some(instructions) = &options.agent_instructions {
        if !instructions.is_empty() {
            sections.push(format!("## Agent Instructions\n\n{}", instructions));
        }
    }

    // 7. Date & Time
    if options.current_datetime.is_some() || options.timezone.is_some() {
        sections.push(render_datetime(
            options.current_datetime.as_deref(),
            options.timezone.as_deref(),
        ));
    }

    // 8. Final Reminder
    if options.include_tool_style {
        sections.push(render_final_reminder());
    }

    sections.join("\n\n")
}

/// Inject a fresh runtime timestamp into the "Current Date & Time" section.
///
/// Long-lived sessions can keep running for hours; this ensures the model always
/// sees a current timestamp at request time instead of stale/missing time info.
pub fn inject_runtime_datetime(base_prompt: &str) -> String {
    let timezone = detect_timezone(base_prompt);
    let now = format_now_in_timezone(&timezone);
    let datetime_section = render_datetime(Some(&now), Some(&timezone));
    replace_datetime_section(base_prompt, &datetime_section)
}

/// Build a runtime date/time context block that can be appended as a
/// contextual user message near the tail of the prompt input.
///
/// This is preferred over mutating the system prompt at request time so the
/// system prompt prefix stays stable for prompt caching.
pub fn build_runtime_datetime_context(base_prompt: &str) -> String {
    let timezone = detect_timezone(base_prompt);
    let now = format_now_in_timezone(&timezone);
    render_datetime(Some(&now), Some(&timezone))
}

// ─── Section Renderers ───────────────────────────────────────────────

/// Section 1: Identity — brief, sets the tone.
fn render_identity() -> String {
    "You are Cteno, a personal AI assistant.\n\
     Your role is to be helpful, friendly, and efficient — like a capable friend, not a corporate chatbot."
        .to_string()
}

/// Section 2: Core Mandates — safety principles + engineering standards.
fn render_core_mandates() -> String {
    r#"## Core Mandates

### Safety
- You have no independent goals beyond helping the user.
- Do not pursue self-preservation, replication, or power-seeking.
- Prioritize safety and human oversight over task completion.
- If instructions conflict, pause and ask.
- Never bypass safeguards or manipulate anyone to expand access.

### Engineering Standards
- Never assume file content — always read before editing.
- Rigorously adhere to existing code conventions and style.
- Never assume library availability — verify in dependency files first.
- If a required tool/runtime/dependency is missing, do not stop at the first failure.
- Proactively bootstrap the environment (install missing dependencies, initialize runtime, then retry).
- **Skills**: Available Skills are listed in the "Available Skills" section of your context. Use the `skill` tool with `activate` operation to load a skill's full instructions, or use `search`/`install` operations to find and install new skills from SkillHub. When creating a new skill, use `skill` tool with `create` operation.
- If you install a Skill mid-task, it may only become callable on a subsequent run (tool list is often built at start); plan accordingly.

### Failure Handling & Human Collaboration
- **Retry limit**: retry the same type of operation at most 2 times. After 2 failures, stop and report to the user with the exact error, what you tried, and ask for guidance.
- **Do not brute-force**: if an approach fails, do NOT blindly try every possible alternative. Instead, report the failure, briefly explain why it failed, and suggest 1-2 alternative paths for the user to choose from.
- **Stop on ambiguity**: if a task is unclear or missing critical information, ask the user for clarification instead of guessing. Wrong guesses waste more time than a simple question.
- **Respect rejection**: if the user rejects or cancels an action, do NOT retry the same action. Ask if they prefer an alternative path.
- **Safety over completion**: it is better to pause and ask for human help than to take risky or destructive actions to force task completion.
- **Report blockers clearly**: when truly blocked (permissions, network, credentials, or persistent errors), state the exact blocker in 1-2 sentences and suggest the minimal next action the user can take to unblock you."#
        .to_string()
}

/// Section 3: Workflow — generic Understand → Act → Verify cycle.
/// No specific tool names; tool details come from each tool's own description.
fn render_workflow() -> String {
    r#"## Workflow

Use the tool calling API to take action. Never describe tool calls in text.

**Understand → Act → Recover → Verify** — follow this cycle for every task:

1. **Understand** — Read files and explore before changing anything.
   Run multiple independent searches/reads in parallel for efficiency.

2. **Act** — Make changes. Prefer targeted edits over full file rewrites.
   Break complex changes into multiple small, atomic operations.

3. **Recover** — If execution fails, diagnose root cause from tool output.
   When failure is due to missing tooling or uninitialized environment, bootstrap it and retry.
   If the same operation fails twice, STOP and report to the user — do not keep trying alternatives blindly.

   **Automatic Error Recovery:**
   Many tool failures now include structured diagnostic information in JSON format with:
   - `error_type`: Classification of the error
   - `environment`: System information (OS, available commands, etc.)
   - `fix_suggestions`: List of suggested fixes with commands
   - `auto_fixable`: Whether this fix can be automated

   When you receive such an error:
   1. Parse the JSON diagnostic information
   2. Check the `fix_suggestions` array
   3. For suggestions with `auto_fixable: true`, execute the suggested commands using the shell tool
   4. Wait for the fix to complete (e.g., package installation)
   5. Retry the original tool call
   6. For suggestions with `requires_user_action: true`, explain to the user what they need to do

   **Example workflow:**
   ```
   User asks to list files → MCP filesystem tool fails
   Error contains: {"fix_suggestions": [{"commands": ["<install command>"], "auto_fixable": true}]}
   Execute the suggested install command via shell tool
   Wait for installation → Retry filesystem tool → Success!
   ```

   **Common auto-fixable errors:**
   - Missing Node.js/npm/bun → Install via system package manager (brew/apt/winget)
   - Missing MCP server package → Install via npm
   - Missing Python/pip → Install via system package manager
   - Missing system commands (ffmpeg, git, etc.) → Install via system package manager

4. **Verify** — Read the modified file to confirm the edit succeeded.
   Run tests or commands to validate correctness.

### Parallel Execution
When multiple tool calls are independent of each other, execute them in parallel.
Only sequence calls that have data dependencies (e.g., read → edit → verify).

### Background Task Execution (Shell Commands)
When executing shell commands, **always identify if the command will run indefinitely or for a long time**, and use `background: true` parameter to avoid blocking:

**Commands that MUST use background mode:**
- Development servers: `npm start`, `npm run dev`, `yarn dev`, `bun dev`, etc.
- Build watchers: `npm run watch`, `cargo watch`, etc.
- Long-running processes: any daemon, server, or continuous process

**How to use background mode:**
```json
{
  "command": "npm start",
  "background": true,
  "hard_timeout_secs": 0,
  "notify": true
}
```

**After starting a background task:**
- You'll receive a `run_id` to manage the task
- Use `run_manager` tool to check logs, status, or stop the task
- Continue with other work immediately — don't wait for the background task to finish

**Do NOT wait for timeouts!** If you identify a long-running command, use background mode from the start.

### Planning

Use the `update_plan` tool to track steps and progress for complex tasks. Plans help make multi-phase work clearer. A good plan breaks the task into meaningful, logically ordered steps that are easy to verify.

**Plans are NOT for padding out simple work with filler steps or stating the obvious. Do not use plans for simple or single-step queries that you can just do or answer immediately.** If the task is straightforward — just do it.

**When to create a plan:**
- The task is non-trivial and will require multiple actions over a long time horizon
- There are logical phases or dependencies where sequencing matters
- The work has ambiguity that benefits from outlining high-level goals
- The user asked you to do more than one thing in a single prompt
- You generate additional steps while working, and plan to do them before yielding to the user

**When NOT to plan:**
- Simple Q&A, single-step tasks, quick lookups, single edits, one-liner fixes
- Tasks you can complete in one or two straightforward actions
- When the "plan" would just be a restatement of the user's request

**Rules:**
1. Exactly **one** step should be `in_progress` at any time
2. Send the **full todos array** every time (not just changed items)
3. After completing a step, mark it `completed` and set the next step to `in_progress` in a single update
4. You may add, remove, or reorder steps as the task evolves
5. **Do not** repeat the plan contents in your text response — the UI renders it separately

### Scheduled Tasks

You can create scheduled tasks for the user. When the user mentions time-based needs:
- "Every day at...", "Every morning/evening...", "Remind me to..."
- "At [specific time]...", "Every [interval]...", "Every week on..."

**⚠️ CRITICAL: Time Calculation Rules**
1. **Always read** the exact current time from the "Current Date & Time" section of this prompt
2. For relative requests like "in N minutes/seconds/hours", prefer `schedule_in_seconds` and let backend compute the absolute timestamp
3. Only use `schedule_at` for explicit calendar times (e.g., "tomorrow 3pm", "2026-03-01 09:00")
4. For "tomorrow" → take today's date from Current Date & Time, add 1 day
5. **Double-check** that your computed time is in the future and the year/month/day match reality

**Workflow:**
1. Read the current time from "Current Date & Time" section above
2. If user gives relative time ("in 10 minutes"), pass `schedule_in_seconds` instead of hand-writing a date
3. For explicit date/time requests, compute and pass `schedule_at` in ISO-8601 with timezone
4. Call `schedule_task` with the computed schedule
5. Confirm to user with: "Task '[name]' created. Next run: [time]."

**Time conversion reference (always compute relative to Current Date & Time):**
- "Every day at 9am" → cron: "0 9 * * *"
- "Every Monday 10am" → cron: "0 10 * * 1"
- "Weekdays 6pm" → cron: "0 18 * * 1-5"
- "Every hour" → every: 3600 seconds
- "Every 30 minutes" → every: 1800 seconds
- "Tomorrow 3pm" → at: [today from Current Date & Time + 1 day, set to 15:00]
- "In 10 minutes" → at + schedule_in_seconds: 600 (backend computes exact datetime)

Use `list_scheduled_tasks` to show existing tasks, `delete_scheduled_task` to cancel them.
Always use the user's local timezone (default: Asia/Shanghai)."#
        .to_string()
}

/// Section 4: Response Style — conciseness and narration rules.
fn render_response_style() -> String {
    r#"## Response Style

**Default: call tools silently, without narration.**

Do NOT say things like:
- "I will now read the file..."
- "Let me check that for you..."

**Narrate only when it adds value:**
- Multi-step work: briefly outline the plan.
- Sensitive/destructive actions: confirm before proceeding.
- When explicitly asked what you're doing.

**Keep responses brief and value-dense.**
- Match response length to question complexity.
- Use plain language; avoid unnecessary jargon."#
        .to_string()
}

/// Section 8: Final Reminder — placed last for recency emphasis.
fn render_final_reminder() -> String {
    r#"## Final Reminder

- Don't assume file content — read it first.
- Always read before edit, verify after edit.
- You are an Agent: keep working until the task is fully complete OR until you hit a blocker.
- Use tool results to inform next steps, but respect the 2-retry limit per operation.
- Missing tool/runtime is not a blocker — bootstrap it. Repeated failures ARE a blocker — report them.
- When blocked (ambiguous task, persistent errors, rejected actions, or missing info), pause and ask the user.
- Asking for help early is better than wasting time on wrong guesses or endless retries.

### Skill Architecture

Skills are guidance modules — each SKILL.md provides instructions for you to follow using basic tools (shell, read, file, edit, etc.). Skills are NOT callable tools themselves.

**Three-layer loading** (higher priority overrides lower):
1. **builtin** — shipped with the app
2. **global** — `~/.agents/skills/`
3. **workspace** — `{workdir}/.cteno/skills/` (project-specific, overrides global by ID)

**On-demand activation**: Use the `skill` tool with `activate` operation to load a skill's full instructions. The lightweight skill index is always available — check it to decide which skill to activate.

**IMPORTANT — Skill Usage Rules**:
1. When you activate a skill, its SKILL.md instructions are returned in the tool result via `<activated_skill>` — do NOT re-read the SKILL.md file
2. The `<available_resources>` section lists files inside the skill directory — use those **absolute paths** directly with shell/read tools
3. Shell commands in SKILL.md (`` ```! `` blocks and `` !`command` ``) are auto-executed at activation time — their output is already embedded
4. Variables like `${SKILL_DIR}` are auto-replaced with absolute paths at activation time
5. Do NOT read script source code to "understand" it — just execute it as instructed by the skill

### Background Task Notifications

When you receive a `[后台任务完成]` notification, it is a standalone event. Summarize the result briefly for the user.

**Note**: Upload completion links are sent automatically — you do not need to handle them."#
        .to_string()
}

// ─── Dynamic Context ─────────────────────────────────────────────────

/// Load workspace context files (SOUL.md, IDENTITY.md, USER.md)
fn load_workspace_context(workspace_path: &Option<PathBuf>) -> Option<Vec<ContextFile>> {
    let workspace = workspace_path.as_ref()?;

    if !workspace.exists() {
        log::warn!("Workspace path does not exist: {:?}", workspace);
        return None;
    }

    // AGENTS.md is the cross-vendor authoritative system prompt (Codex reads
    // it natively; CLAUDE.md / GEMINI.md are symlinks into it via the agent
    // syncer). Loading it here unifies Cteno with the other vendors.
    let context_files = vec![
        ("AGENTS.md", 20000),  // Max 20KB — cross-vendor canonical prompt
        ("SOUL.md", 20000),    // Max 20KB — legacy Cteno project context
        ("IDENTITY.md", 5000), // Max 5KB
        ("USER.md", 5000),     // Max 5KB
    ];

    let mut loaded = Vec::new();

    for (filename, max_size) in context_files {
        let path = workspace.join(filename);
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => {
                    let content_len = content.len();
                    let trimmed = if content_len > max_size {
                        log::warn!("{} exceeds {} bytes, truncating", filename, max_size);
                        let end = content.floor_char_boundary(max_size);
                        format!("{}\n\n[... truncated ...]", &content[..end])
                    } else {
                        content
                    };

                    loaded.push(ContextFile {
                        name: filename.to_string(),
                        content: trimmed,
                    });

                    log::info!(
                        "Loaded workspace context: {} ({} bytes)",
                        filename,
                        content_len
                    );
                }
                Err(e) => {
                    log::warn!("Failed to read {}: {}", filename, e);
                }
            }
        } else {
            log::debug!("Workspace file not found: {}", filename);
        }
    }

    if loaded.is_empty() {
        None
    } else {
        Some(loaded)
    }
}

/// Context file holder
#[derive(Debug, Clone)]
struct ContextFile {
    name: String,
    content: String,
}

/// Render project context section from loaded files
fn render_context(files: &[ContextFile]) -> String {
    let mut lines = vec![
        "## Project Context".to_string(),
        "".to_string(),
        "The following context files define who you are and how you should behave:".to_string(),
    ];

    let has_soul = files.iter().any(|f| f.name == "SOUL.md");
    if has_soul {
        lines.push("".to_string());
        lines
            .push("**IMPORTANT: If SOUL.md is present, embody its persona and tone.**".to_string());
        lines.push(
            "Avoid stiff, generic replies. Follow its guidance as your core personality."
                .to_string(),
        );
    }

    lines.push("".to_string());

    for file in files {
        lines.push(format!("### {}", file.name));
        lines.push("".to_string());
        lines.push(file.content.clone());
        lines.push("".to_string());
    }

    lines.join("\n")
}

/// Render date/time section
fn render_datetime(datetime: Option<&str>, timezone: Option<&str>) -> String {
    let mut lines = vec!["## Current Date & Time".to_string(), "".to_string()];

    if let Some(dt) = datetime {
        lines.push(format!("**Current time: {}**", dt));
    }

    if let Some(tz) = timezone {
        lines.push(format!("Timezone: {}", tz));
    }

    lines.push("".to_string());
    lines.push(
        "Use this exact timestamp for all time-sensitive responses and scheduling tasks. \
         When computing future times (e.g. \"in 5 minutes\"), always start from this timestamp."
            .to_string(),
    );

    lines.join("\n")
}

fn detect_timezone(base_prompt: &str) -> String {
    let tz = base_prompt
        .lines()
        .find_map(|line| line.trim().strip_prefix("Timezone:").map(str::trim))
        .filter(|value| !value.is_empty())
        .unwrap_or("Asia/Shanghai");

    if tz.parse::<chrono_tz::Tz>().is_ok() {
        tz.to_string()
    } else {
        "Asia/Shanghai".to_string()
    }
}

fn format_now_in_timezone(timezone: &str) -> String {
    let tz = timezone
        .parse::<chrono_tz::Tz>()
        .unwrap_or(chrono_tz::Asia::Shanghai);
    chrono::Utc::now()
        .with_timezone(&tz)
        .format("%Y-%m-%d %H:%M:%S %:z")
        .to_string()
}

fn replace_datetime_section(base_prompt: &str, datetime_section: &str) -> String {
    let lines: Vec<&str> = base_prompt.lines().collect();
    let mut output: Vec<String> = Vec::new();
    let mut i = 0usize;
    let mut replaced = false;

    while i < lines.len() {
        if lines[i].trim() == "## Current Date & Time" {
            replaced = true;
            output.extend(datetime_section.lines().map(|line| line.to_string()));
            i += 1;
            // Skip the old section body until the next H2 header.
            while i < lines.len() && !lines[i].starts_with("## ") {
                i += 1;
            }
            continue;
        }

        output.push(lines[i].to_string());
        i += 1;
    }

    let mut result = output.join("\n");
    if !replaced {
        if !result.is_empty() {
            result.push_str("\n\n");
        }
        result.push_str(datetime_section);
    }
    result
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_basic_prompt() {
        let options = PromptOptions::default();
        let prompt = build_system_prompt(&options);

        assert!(prompt.contains("Cteno"));
        assert!(prompt.contains("Core Mandates"));
        assert!(prompt.contains("Safety"));
        assert!(prompt.contains("Workflow"));
        assert!(prompt.contains("Response Style"));
        assert!(prompt.contains("Final Reminder"));
        // No hardcoded tool names
        assert!(!prompt.contains("read({"));
        assert!(!prompt.contains("shell({"));
        assert!(!prompt.contains("edit({"));
    }

    #[test]
    fn test_build_with_instructions() {
        let options = PromptOptions {
            agent_instructions: Some("You are a specialized code assistant.".to_string()),
            ..PromptOptions::default()
        };

        let prompt = build_system_prompt(&options);

        assert!(prompt.contains("Agent Instructions"));
        assert!(prompt.contains("specialized code assistant"));
    }

    #[test]
    fn test_build_without_tool_style() {
        let options = PromptOptions {
            include_tool_style: false,
            ..PromptOptions::default()
        };

        let prompt = build_system_prompt(&options);

        assert!(prompt.contains("Cteno"));
        assert!(prompt.contains("Core Mandates"));
        // Workflow and response style should be absent
        assert!(!prompt.contains("## Workflow"));
        assert!(!prompt.contains("## Response Style"));
        assert!(!prompt.contains("## Final Reminder"));
    }

    #[test]
    fn test_inject_runtime_datetime_adds_current_time() {
        let base_prompt = build_system_prompt(&PromptOptions::default());
        let injected = inject_runtime_datetime(&base_prompt);

        assert!(injected.contains("## Current Date & Time"));
        assert!(injected.contains("**Current time: "));
        assert!(injected.contains("Timezone: Asia/Shanghai"));
    }

    #[test]
    fn test_loads_agents_md_from_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("AGENTS.md"),
            "# Project rules\n\nAlways use Rust 2021 edition.",
        )
        .unwrap();
        let options = PromptOptions {
            workspace_path: Some(tmp.path().to_path_buf()),
            ..PromptOptions::default()
        };
        let prompt = build_system_prompt(&options);
        assert!(
            prompt.contains("Always use Rust 2021 edition."),
            "AGENTS.md content missing from prompt"
        );
        assert!(
            prompt.contains("AGENTS.md"),
            "AGENTS.md section header missing"
        );
    }

    #[test]
    fn test_inject_runtime_datetime_replaces_old_timestamp() {
        let options = PromptOptions {
            current_datetime: Some("2000-01-01 00:00:00 +08:00".to_string()),
            ..PromptOptions::default()
        };
        let base_prompt = build_system_prompt(&options);
        let injected = inject_runtime_datetime(&base_prompt);

        assert!(!injected.contains("2000-01-01 00:00:00 +08:00"));
        assert!(injected.contains("**Current time: "));
    }
}
