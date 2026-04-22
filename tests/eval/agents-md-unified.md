# AGENTS.md — cross-vendor unified system prompt

Verifies that a single `{project}/AGENTS.md` is the one source-of-truth system
prompt consumed by all four vendors.

Low-level coverage lives in
`cargo test -p cteno-agent-runtime --lib system_prompt::tests::test_loads_agents_md_from_workspace`
and the Phase 3 `reconcile_all` test (which fans out `CLAUDE.md` / `GEMINI.md`
symlinks). The cases below are agent-behavioral and run post-Phase-6.

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-agents-md-eval
- max-turns: 5

## setup
```bash
rm -rf /tmp/cteno-agents-md-eval
mkdir -p /tmp/cteno-agents-md-eval
cat > /tmp/cteno-agents-md-eval/AGENTS.md <<'EOF'
# Project rules

- Use spaces, never tabs.
- Every Rust crate MUST have an explicit `edition = "2021"`.
- The codename for this project is "Kingfisher".
EOF
```

## cases

### [pending] Cteno session sees AGENTS.md rules
- **message**: "What's the codename for this project?"
- **expect**: "Kingfisher" — proves AGENTS.md landed in Cteno's system prompt via `load_workspace_context`.
- **anti-pattern**: Agent says it doesn't know; made-up codename.
- **severity**: high

### [pending] Claude session sees the same rules via CLAUDE.md symlink
- **message**: "What are the indentation rules for this project?"
- **expect**: "spaces, never tabs" — proves `CLAUDE.md` is a symlink to `AGENTS.md`.
- **anti-pattern**: Agent answers with a generic default.
- **severity**: high

### [pending] Editing AGENTS.md propagates to all vendors without rerunning reconcile
- **setup**: edit AGENTS.md to append a new rule ("Use nightly rustfmt.")
- **message**: "Any formatting tool preference in this repo?"
- **expect**: Agent picks up the new rule on its next turn — the symlinks mean the update is live. If the vendor does its own prompt caching, a new session may be required.
- **severity**: medium

### [pending] User hand-edit of CLAUDE.md does NOT overwrite AGENTS.md
- **setup**: open the CLAUDE.md symlink and (try to) write to it.
- **expect**: Because it's a symlink, writes pass through to AGENTS.md. **This is intentional** under the "host is authoritative" policy — if a user writes to CLAUDE.md expecting Claude-specific customization, they're surprised. The documented workflow is: all prompt edits go to AGENTS.md.
- **severity**: low (operator doc concern, not a correctness failure)
