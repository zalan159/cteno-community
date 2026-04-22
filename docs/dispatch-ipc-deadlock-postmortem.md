# Dispatch IPC Deadlock Postmortem (2026-04-07)

## Summary
- Symptom: after `dispatch_task`, the desktop app could enter a full stall where IPC events stopped progressing.
- Impact: `send_message_local` looked "hung", worker dispatch did not complete, and the session stayed in a long `thinking=true` state.
- Root cause category: async lock-scope bug (`lock + await`) on shared connection maps.

## Root Cause

The deadlock path was:

1. Tauri local IPC path called `send_message_local`.
2. It held `session_connections` mutex while awaiting full local message processing.
3. During that processing, the model executed `dispatch_task`.
4. Dispatch path needed to lock the same `session_connections` map to send to worker/persona sessions.
5. Outer call still held the lock while waiting on inner work, so the inner path blocked forever.

The critical behavior was not protocol/encryption related. It was lock lifetime crossing long async work.

## Why This Was Triggered

Desktop local mode uses a direct IPC path (`send_message_local` + channel streaming) that can run a full agent round in one async call. If that round re-enters session delivery (`dispatch_task` / `send_to_session` / `ask_persona`), any outer lock retained across await can self-block.

## Fixes Applied

### 1) Detached session message handle pattern
- Added `SessionConnectionHandle` and `message_handle()` to decouple long async message execution from map lock ownership.
- `SessionConnection::send_initial_user_message()` and `inject_local_message()` now forward to detached handle methods.

Key files:
- `apps/desktop/src-tauri/src/happy_client/session.rs`
- `apps/desktop/src-tauri/src/lib.rs`

### 2) Converted session delivery callsites to lock-short pattern
Pattern used everywhere:

```rust
let handle = {
  let conns = session_connections.lock().await;
  conns.get(id).map(|c| c.message_handle())
};
// lock released here
handle.send_initial_user_message(...).await
```

Updated areas include:
- persona dispatch/result delivery
- send_to_session / ask_persona / notification watcher
- local RPC send-message paths
- reconnect catch-up and kill/close paths where removal happened before slow awaits

### 3) Additional lock-scope hardening from full audit
Beyond dispatch path, also fixed:
- machine socket teardown/reconnect lock scope in manager/watchdog
- browser session manager and major browser executors (`browser_navigate`, `browser_manage`, `browser_action`, `browser_cdp`, `browser_adapter`, `browser_network`) to detach session from map before long CDP awaits

## CLI Compatibility

No CLI protocol or RPC contract was changed.
- `ctenoctl` still uses the same machine RPC methods.
- Changes are internal lock lifetime and execution ordering only.
- Result: behavior compatibility preserved while removing deadlock risk.

## Comprehensive Code Audit Results

Audit method:
- static scan for `lock().await` followed by downstream `.await`
- targeted review for shared global maps (`session_connections`, browser `sessions`, machine socket handles)

### Resolved high-risk classes
- `session_connections` re-entrant deadlock class (dispatch/local IPC)
- machine socket teardown/reconnect lock-held disconnect
- browser global session-map lock held across network/CDP awaits

### Remaining low-risk/intentional lock-await cases
- MCP registry write lock spans async server add/remove/toggle operations in RPC handlers (`happy_client/manager.rs`).
  - This is currently serialization-by-design, but worth future refactor if MCP operations become slow.
- run log writer mutex spans async file writes (`runs.rs`).
  - Intentional to preserve log line ordering.

## Validation

Build validation completed successfully:

```bash
CARGO_TARGET_DIR=/tmp/cteno-cargo-check cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml
```

Notes:
- only existing third-party warnings remained (rust-socketio `tarpaulin` cfg warnings).
- no new compile errors from this fix set.

## Guardrail Going Forward

For shared maps/sockets, enforce this rule:
- never hold global mutex guards across network/tool/agent awaits.
- extract cloneable handle or remove object from map first, then await.

Quick grep for review:

```bash
rg -nU "lock\(\)\.await[\s\S]{0,220}?\.await" apps/desktop/src-tauri/src
```
