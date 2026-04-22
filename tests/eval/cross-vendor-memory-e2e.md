# Cross-vendor memory end-to-end

Final integration test for the whole cross-session story: after the host
reconciles vendor configs and spawns sessions, memory saved by one vendor
must be recallable by another. Runnable only once Phase 6b (wiring
`reconcile_for_spawn` into each executor's `spawn_session` call site) lands.

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-cross-vendor-memory
- max-turns: 6

## setup
```bash
rm -rf /tmp/cteno-cross-vendor-memory
mkdir -p /tmp/cteno-cross-vendor-memory
cat > /tmp/cteno-cross-vendor-memory/AGENTS.md <<'EOF'
Project: cross-vendor-memory-test. Use short answers.
EOF
# No MCP config written by hand — reconcile during spawn must provision it.
```

## cases

### [pending] Claude session writes, Cteno session reads (primary goal)
- **step 1**: spawn a Claude session against the workdir. Ask: "Use memory_save to record under notes/handoff: 'shared secret is RABBIT-HOLE-7'."
- **step 2**: close the Claude session. Spawn a Cteno session against the same workdir. Ask: "What's the shared secret from memory?"
- **expect**: Cteno replies with `RABBIT-HOLE-7`. This proves: (a) reconcile wrote `cteno-memory` into Claude's `.mcp.json`, (b) Claude spawned the MCP subprocess, (c) the save landed in `{workdir}/.cteno/memory/notes/handoff.md`, (d) the Cteno agent saw the same `memory` tool, (e) `memory_recall` pulled the entry back.
- **anti-pattern**: Cteno says it has no such memory. Two different dirs were written. Claude's MCP server never started.
- **severity**: high

### [pending] Codex session sees memory written by Gemini session
- Same pattern, Gemini → Codex.
- **severity**: high

### [pending] Global scope persists across project switches
- **step 1**: Cteno session in project A saves `{scope: global}` "Ada Lovelace is considered the first programmer."
- **step 2**: Cteno session in project B recalls with query "Ada Lovelace programmer".
- **expect**: The fact surfaces tagged `[global]` — the global scope lives under `~/.cteno/memory/`, independent of project dir.
- **severity**: medium

### [pending] Reconcile does not duplicate MCP entries on repeat spawn
- Spawn the same vendor 3 times in a row without changing specs.
- **expect**: Each vendor's config file shows exactly ONE `cteno-memory` entry each time (same content). No version drift, no duplicate keys.
- **severity**: medium

### [pending] Vendor CLI error when memory_bin is missing is clear
- **setup**: delete the `cteno-memory-mcp` binary from the install dir.
- **step**: spawn a Claude session.
- **expect**: Claude surfaces "MCP server `cteno-memory` failed to start" or similar when it tries to list tools; error is visible in session transcript. Session does NOT crash — the agent runs with the remaining (non-memory) toolset.
- **severity**: low (operator/ops concern)

## Notes for QA

These cases are runnable once:
1. The `cteno-memory-mcp` binary is built (`cargo build -p cteno-host-memory-mcp --release`) and either (a) on `$PATH`, (b) next to the daemon binary, or (c) pointed at by `$CTENO_MEMORY_MCP_BIN`.
2. Test host has Claude Code CLI + Codex CLI + Gemini CLI installed & authenticated.

Phase 6b (reconcile wired into every vendor spawn site) is **done**: the
daemon calls `crate::agent_sync_bridge::reconcile_for_spawn(workdir)` once
per spawn via `ExecutorRegistry::start_session_with_autoreopen` plus the
direct-spawn fallbacks. Integration coverage:
`cargo test -p cteno agent_sync_bridge::`.
