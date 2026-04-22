# Claude connection-reuse seam (OR-P1 Phase 1)

## meta
- kind: claude-adapter
- profile: claude-vendor
- workdir: /tmp/cteno-claude-conn-reuse
- max-turns: 1

## context

This eval exercises the Phase-1 connection-reuse seam
(`open_connection` / `close_connection` / `check_connection` /
`start_session_on`) implemented in
`packages/multi-agent-runtime/rust/crates/multi-agent-runtime-claude/`.

Phase A empirical findings (`docs/claude-p1-protocol-findings.md`) proved the
claude CLI enforces a single session per subprocess — the inbound `session_id`
on user frames is ignored. The seam therefore **shims** the trait: each
logical Cteno session owns its own subprocess, and reuse across sessions is
NOT supported for Claude. The eval cases below verify the invariant holds
and that the hazards that were feared in the original design (sibling-session
set_model leakage, interrupt leak, etc.) do not manifest because every
session has its own private subprocess.

Rust gate: `cargo test -p multi-agent-runtime-claude` must be green on the
working tree. The integration test
`tests/integration_connection_reuse.rs::connection_reuse_end_to_end` is
`#[ignore]` and becomes runnable with `CLAUDE_PATH=/path/to/claude cargo
test -p multi-agent-runtime-claude -- --ignored`.

## cases

### [pending] two sessions opened on same handle spawn two subprocesses
- **message**: open one connection via `ClaudeAgentExecutor::open_connection(ConnectionSpec::default())`, then call `start_session_on` twice with identical `SpawnSessionSpec` (same workdir, same permission mode, same model = None). Record both returned `SessionRef.process_handle` tokens.
- **expect**: tokens differ. The executor's internal `sessions` map holds two live entries. The fake-CLI argv log contains exactly two `ARGS:…` lines.
- **anti-pattern**: the second session reuses the first session's subprocess (tokens equal, log shows one argv). This would violate the CLI's one-session-per-subprocess contract and cause message-routing drift in production.
- **severity**: high

### [pending] three sessions across two distinct workdirs each get their own subprocess
- **message**: open one connection, then `start_session_on` with workdir `/tmp/ws-a`, then `/tmp/ws-a` again, then `/tmp/ws-b`. Record all three `process_handle` tokens.
- **expect**: three distinct tokens, three subprocesses. Workdir-based pooling must NOT apply.
- **anti-pattern**: the adapter silently groups sessions by workdir and reuses the subprocess for the first `ws-a` session when the second arrives — producing a phantom "shared transport" that the CLI cannot actually honour. Caller messages would cross-contaminate.
- **severity**: high

### [pending] set_permission_mode on session A does NOT affect session B
- **message**: open one connection, start sessions A and B. Call `set_permission_mode(A, AcceptEdits)`. Inspect the fake-CLI log for each subprocess. Probe session B's effective mode via a subsequent `set_permission_mode(B, Plan)` and confirm the argv logs show only the matching mode per subprocess.
- **expect**: subprocess A's stdin receives `{"subtype":"set_permission_mode","mode":"acceptEdits"}`, subprocess B's stdin receives `{"subtype":"set_permission_mode","mode":"plan"}`. No cross-subprocess leak. Since every session has its own subprocess, the "set_model/set_permission_mode leaks to siblings" hazard flagged in the original Phase-B design prompt does NOT manifest here — the test locks in that non-regression.
- **anti-pattern**: the adapter writes both control_requests to a single shared stdin pipe (no such pipe exists in the 1:1 model, but a later refactor might introduce one incorrectly).
- **severity**: high

### [pending] killing one subprocess does not kill siblings in the pool
- **message**: open one connection, start sessions A and B. Externally kill session A's subprocess (send `SIGKILL` via the `Child` handle). Observe session B's next `send_message` completes normally and its subprocess is still alive.
- **expect**: session B keeps working. `close_session(A)` is idempotent (the child was already dead). `check_connection` returns `Healthy` because no connection-owned subprocess exists in the 1:1 model.
- **anti-pattern**: killing one subprocess deadlocks or errors out other sessions. This would happen if the adapter incorrectly implemented a shared-subprocess pool.
- **severity**: medium

### [pending] close_connection on a handle from a different vendor is rejected
- **message**: build a `ConnectionHandle { vendor: "codex", inner: Arc::new(()), ... }` and pass it to `ClaudeAgentExecutor::close_connection`.
- **expect**: returns `AgentExecutorError::Protocol(_)` containing "non-claude". No panics, no silent success.
- **anti-pattern**: returns `Ok(())` (silent wrong-vendor acceptance), or panics on downcast (no downcast-unwrap in production paths).
- **severity**: medium

### [pending] check_connection on a killed CLI path returns Dead
- **message**: open a connection, then delete the fake-CLI binary on disk. Call `check_connection` again.
- **expect**: returns `Ok(ConnectionHealth::Dead { reason })` — note: the real `check_cli_version` is lenient and returns `Ok(())` when it cannot spawn, so this case documents that `check_connection` only reports `Dead` when the CLI explicitly reports an unsupported version. The eval should confirm the adapter does not panic when the binary is missing.
- **anti-pattern**: panics on missing binary, or reports `Healthy` then lets `start_session_on` fail with a cryptic IO error.
- **severity**: low

### [pending] AgentCapabilities must not advertise multi-session-per-process
- **message**: call `ClaudeAgentExecutor::capabilities()`. Inspect the `supports_multi_session_per_process` bit.
- **expect**: `false`. Callers gate shared-subprocess code on this bit; flipping it to `true` without underlying CLI support would break downstream assumptions about message routing and resource cleanup.
- **anti-pattern**: returns `true` (would trigger sharer code paths in the registry that the CLI cannot actually support). This is the single biggest regression risk for Phase 2; lock it in with a test.
- **severity**: high
