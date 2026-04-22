# Cross-vendor config syncer — four vendors

Behavioral eval for `cteno-host-agent-sync`. The low-level writing and symlink
correctness is fully covered by `cargo test -p cteno-host-agent-sync` (13
tests, including a `reconcile_all` integration that writes every vendor layout
at once). The cases below target **runtime-behavior** of real agents after
the sync ran — runnable once Phase 6 (spawn integration) attaches the syncer
to session startup.

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-syncer-eval
- max-turns: 8

## setup
```bash
rm -rf /tmp/cteno-syncer-eval
mkdir -p /tmp/cteno-syncer-eval
# authoritative trees
mkdir -p /tmp/cteno-syncer-eval/.cteno/{agents,skills}
cat > /tmp/cteno-syncer-eval/AGENTS.md <<'EOF'
You are an assistant for this project. Stick to the repo's conventions.
EOF
cat > /tmp/cteno-syncer-eval/.cteno/agents/reviewer.md <<'EOF'
---
name: reviewer
description: Senior reviewer; comments on risk and correctness.
---
Review carefully; flag destructive operations.
EOF
```

## cases

### [pending] Claude session sees symlinked CLAUDE.md
- **message**: "What's the system prompt you were given? Quote the first sentence verbatim."
- **expect**: Agent quotes `"You are an assistant for this project. Stick to the repo's conventions."` — proving `CLAUDE.md` was symlinked to the authoritative `AGENTS.md`.
- **anti-pattern**: Agent quotes a different prompt (means the symlink pointed at the wrong file or the CLI fell through to a default prompt).
- **severity**: high

### [pending] Gemini session sees symlinked GEMINI.md
- (same as above but Gemini vendor.) Expect identical quoted prompt.
- **severity**: high

### [pending] Codex session already reads AGENTS.md natively
- **message**: "What does AGENTS.md tell you about this repo?"
- **expect**: Agent references project conventions from the authoritative AGENTS.md. Does NOT require the syncer to have written anything for Codex system-prompt (it's no-op by design).
- **anti-pattern**: Agent says it doesn't see AGENTS.md.
- **severity**: medium

### [pending] Claude sees subagent `reviewer` available
- **message**: "List all sub-agents available via the Task tool."
- **expect**: `reviewer` appears in the list with the description "Senior reviewer; comments on risk and correctness." This verifies `.claude/agents/reviewer.md` is a valid symlink to `.cteno/agents/reviewer.md`.
- **anti-pattern**: Subagent missing or has stale description.
- **severity**: high

### [pending] Gemini sees subagent `reviewer` available
- (same as above but Gemini.)
- **severity**: medium

### [pending] MCP memory server visible in Claude's tool list
- **message**: "What MCP tools do you have? List their names."
- **expect**: `memory_save`, `memory_recall`, `memory_read`, `memory_list` are present — the Claude `.mcp.json` was merged with our `cteno-memory` entry AND the Claude CLI actually spawned the server and listed its tools.
- **anti-pattern**: Zero MCP tools, or only non-Cteno ones.
- **severity**: high

### [pending] Codex sees the same MCP memory tools
- (same, via Codex's native MCP integration reading `~/.codex/config.toml`.)
- **severity**: high

### [pending] User's hand-edited Codex config sections survive reconcile
- **setup**: add a `[features]` block to `~/.codex/config.toml` before starting a Codex session.
- **message**: "Is the `multi_agent` feature enabled according to your config?"
- **expect**: Agent reports `multi_agent = true` — proves the reconcile preserved user-authored TOML sections untouched.
- **anti-pattern**: The block is gone; reconcile clobbered user data.
- **severity**: high

### [pending] Reconcile is idempotent
- Run a Cteno session that triggers reconcile twice back-to-back with no spec changes.
- **expect**: `.mcp.json`, `.gemini/settings.json`, and `config.toml` are byte-identical between pass 1 and pass 2 (no spurious rewrites, no duplicate entries).
- **severity**: medium

### [pending] Symlink retargets on spec change
- After the above, change the memory MCP spec (different `--project-dir`), reconcile again.
- **expect**: All three vendor files reflect the new args on next read.
- **anti-pattern**: Stale args persist; vendor CLIs still launch the old command.
- **severity**: medium
