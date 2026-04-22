# Claude CLI stream-json Multi-Session Protocol — Empirical Findings

**Date**: 2026-04-20
**CLI**: `/Users/zal/.local/bin/claude`, version `2.1.114 (Claude Code)`
**Purpose**: Phase-A empirical validation for the OR-P1 Claude-vendor connection-reuse refactor in `packages/multi-agent-runtime/rust/crates/multi-agent-runtime-claude/`.

Everything below is captured verbatim from live `claude` subprocess transcripts. Raw logs are at `/tmp/claude-p1-probe/s{1,2,3,4}.log`. The harness that drove the CLI is `/tmp/claude-p1-probe/probe.py`.

---

## TL;DR — ARCHITECTURE-BREAKING FINDING

**The CLI ignores the inbound `session_id` field on `type:"user"` frames.** Every subprocess is tied to **exactly one** CLI-generated session id (emitted in `system:init`) for its whole lifetime. Sending two user messages with different `session_id` tags results in both being appended to the SAME CLI-internal conversation. There is no per-session demux.

This invalidates the group-pooled-by-(workdir, system_prompt, allowed_tools, additional_directories) design that was the assumed basis for Phase B. **One Claude session = one subprocess, period.** No connection reuse across distinct sessions is possible with this CLI version.

Phase B must therefore treat "connection" and "session" as 1:1. The `open_connection` / `start_session_on` seam can still be wired — but `open_connection` becomes a lazy no-op (or a version probe), and `start_session_on` unconditionally spawns a fresh subprocess. We do NOT flip `supports_multi_session_per_process` to `true`.

## CLI invocation used

Unless otherwise noted, every probe launched:

```
claude \
  --input-format stream-json \
  --output-format stream-json \
  --permission-prompt-tool stdio \
  --include-partial-messages \
  --verbose \
  --dangerously-skip-permissions \
  --permission-mode default
```

with `env` cleared of `CLAUDECODE` and `CLAUDE_CODE_ENTRYPOINT=sdk-py`, matching the Python SDK and the existing adapter.

## Q1 — Outbound frame `session_id` stamping

Scenario 1 sent one user message with `"session_id": "sessA-2be05679-dd4f-456d-926b-24b54abeae38"` and observed what the CLI emitted.

**Finding**: The CLI generated its own session id `6d516679-cdaf-47b2-bfdd-899c986446ec` and stamped it on every outbound frame. The inbound tag was NOT echoed anywhere.

### Stamping table (all outbound frames seen in scenario 1)

| Frame `type` | Carries `session_id`? | Location | Notes |
|---|---|---|---|
| `control_response` | no | — | `response.request_id` only |
| `system` subtype `hook_started` | yes | outer | CLI-internal hook |
| `system` subtype `hook_response` | yes | outer | CLI-internal hook |
| `system` subtype `init` | yes | outer | **authoritative session id** |
| `system` subtype `status` | yes | outer | |
| `stream_event` (message_start/content_block_*/message_delta/message_stop) | yes | outer | partial-message deltas |
| `assistant` | yes | outer (not inside `message`) | |
| `user` (tool_result echo) | yes | outer | tool-result envelope |
| `rate_limit_event` | yes | outer | |
| `result` | yes | outer | terminal frame |

Every `session_id` appears at the **outer JSON level**, never inside `message`/`event`/`response`.

## Q2 — Concurrent interleaving

Scenario 2 sent two user messages 300ms apart, each tagged with a distinct `session_id`, then waited for both `result` frames.

**Finding**: Both messages were processed against the **same** CLI-generated id (`1585fdba-ab56-48ec-a6e6-4bfbaf0a7317`). They were processed **serially** — message A completed (`result` at t=7.699s) before message B started (`system:init` for its turn at t=7.693s, finishing at t=9.535s). No interleaving of deltas between A and B.

Excerpt proving both turns share the same `session_id`:

```
[7.699] result ... "session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317" ... "result":"1\n2\n...\n10"
[7.693] system init ... "session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317"
[9.320] content_block_delta ... "text":"B-PING" ... "session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317"
[9.535] result ... "session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317" ... "result":"B-PING"
```

**Conclusion**: The CLI is a serial FIFO over a single session. Subsequent user frames queue and run against the same conversation. The `session_id` tag on inbound user frames has no routing effect.

## Q3 — `can_use_tool` control_request body

Scenario 3 attempted to force the CLI to emit a `control_request can_use_tool` for a write outside cwd. With `--dangerously-skip-permissions`, the CLI did **not** route through `can_use_tool` — it executed `Write` immediately and the tool succeeded (file was created at `/tmp/probe-outside-cwd/marker.txt`).

Empirical outcome without `--dangerously-skip-permissions` was not re-probed at length because the existing adapter uses the dangerous flag and depends on `can_use_tool` routing via a separate configuration path. Reading the Python SDK source at `/Users/zal/Cteno/tmp/claude-agent-sdk-python/src/claude_agent_sdk/_internal/query.py` (handler for `subtype == "can_use_tool"`, lines 273-316):

```
permission_request: SDKControlPermissionRequest = request_data
original_input = permission_request["input"]
...
tool_use_id=permission_request.get("tool_use_id"),
agent_id=permission_request.get("agent_id"),
...
self.can_use_tool(
    permission_request["tool_name"],
    permission_request["input"],
    context,
)
```

**The SDK reads `tool_name`, `input`, `tool_use_id`, `agent_id` — there is no `session_id` read from the request body.** Combined with Q1+Q2's finding (the subprocess is mono-session), the absence of `session_id` in the request body is consistent: the subprocess already knows which session is the only session.

**Conclusion for adapter design**: `respond_to_permission` does not need session-id routing — `request_id` is globally unique inside one subprocess and every subprocess owns exactly one session. The current adapter's behavior (match `request_id` → reply on the session's stdin) is correct.

## Q4 — Close + interrupt behavior

Scenario 4 sent two user messages A (long) and B (short), then an `interrupt` control_request 1s after.

**Finding (interrupt)**: The CLI emitted a synthetic user frame `"[Request interrupted by user]"` and a `result` with `subtype:"error_during_execution"` for the *current* turn (turn A). The second queued turn B then ran to completion normally on the SAME session. So interrupt = "cancel the currently running turn on this subprocess's sole session". Since every subprocess hosts exactly one session, this is equivalent to "cancel the in-flight turn for this session". There is no multi-session ambiguity because there is no multi-session.

**Finding (close)**: After sending `stdin.close()`, the subprocess exited cleanly (`returncode=0`) in **1.92 seconds**. No force-kill required.

## ≥20 lines of raw captured stdout (scenario 2, verbatim from `s2.log`)

```
5.517	out	{"type":"system","subtype":"hook_started","hook_id":"1861070d-ecce-48ae-8859-2fcfd2478d18","hook_name":"SessionStart:startup","hook_event":"SessionStart","uuid":"55d86552-a765-4cb8-b233-c93e497065fd","session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317"}
5.518	out	{"type":"system","subtype":"hook_response","hook_id":"1861070d-ecce-48ae-8859-2fcfd2478d18","hook_name":"SessionStart:startup","hook_event":"SessionStart","output":"","stdout":"","stderr":"","exit_code":0,"outcome":"success","uuid":"caafd9d4-219b-4563-9cee-77250eac007f","session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317"}
5.524	out	{"type":"control_response","response":{"subtype":"success","request_id":"req_init","response":{...}}}
5.532	out	{"type":"system","subtype":"init","cwd":"/private/tmp/claude-p1-probe/ws","session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317","tools":[...],"model":"claude-opus-4-7[1m]","permissionMode":"bypassPermissions",...}
5.532	out	{"type":"system","subtype":"status","status":"requesting","uuid":"f01a1c3a-affb-4bd8-a8fc-b26c15ee2110","session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317"}
7.521	out	{"type":"stream_event","event":{"type":"message_start","message":{"model":"claude-opus-4-7",...}},"session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317","parent_tool_use_id":null,"uuid":"57be0bc6-4b5a-4517-8b35-a4c09bb5b0f2"}
7.522	out	{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}},"session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317","parent_tool_use_id":null,"uuid":"131e9aa1-1013-45ae-9173-80e34f2b6327"}
7.522	out	{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"1\n2\n3\n4\n5\n6\n7\n8\n9\n10"}},"session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317","parent_tool_use_id":null,"uuid":"dcf6b33c-e45b-46f6-a8d7-a20067c8755c"}
7.559	out	{"type":"assistant","message":{"model":"claude-opus-4-7","content":[{"type":"text","text":"1\n2\n3\n4\n5\n6\n7\n8\n9\n10"}],...},"parent_tool_use_id":null,"session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317","uuid":"210a1f3a-d1aa-491e-a093-06484f4bb25b"}
7.559	out	{"type":"stream_event","event":{"type":"content_block_stop","index":0},"session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317",...}
7.591	out	{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{...}},"session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317",...}
7.591	out	{"type":"stream_event","event":{"type":"message_stop"},"session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317",...}
7.592	out	{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1776628800,...},"uuid":"683af43c-4571-4f82-91b7-a28b57ae063c","session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317"}
7.699	out	{"type":"result","subtype":"success","is_error":false,"duration_ms":2168,"num_turns":1,"result":"1\n2\n3\n4\n5\n6\n7\n8\n9\n10","stop_reason":"end_turn","session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317",...}
7.693	out	{"type":"system","subtype":"init","cwd":"/private/tmp/claude-p1-probe/ws","session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317",...}
7.693	out	{"type":"system","subtype":"status","status":"requesting","uuid":"01ddb97a-45cc-40a6-b4ff-c0678c08a529","session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317"}
9.319	out	{"type":"stream_event","event":{"type":"message_start","message":{"model":"claude-opus-4-7",...}},"session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317","parent_tool_use_id":null,"uuid":"a271bfed-f260-40f3-bb8e-6fb38e8021d2"}
9.320	out	{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}},"session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317",...}
9.320	out	{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"B-PING"}},"session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317",...}
9.452	out	{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"},...},"session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317",...}
9.535	out	{"type":"result","subtype":"success","is_error":false,"duration_ms":1843,"num_turns":1,"result":"B-PING","stop_reason":"end_turn","session_id":"1585fdba-ab56-48ec-a6e6-4bfbaf0a7317",...}
```

Note: `1585fdba-ab56-48ec-a6e6-4bfbaf0a7317` is repeated on every frame. The inbound tags `sessA-0946687b-...` and `sessB-369bfdc5-...` appear **nowhere** in the outbound stream.

## Phase-B design implications (for the adapter refactor)

Because Q1 + Q2 establish the CLI enforces one session per subprocess:

1. **Do not implement a ConfigKey-keyed subprocess pool**. Each logical Cteno session must own its own `claude` subprocess. The existing 1-session-per-subprocess registry in `ClaudeAgentExecutor::sessions` is correct and should stay.

2. **`open_connection` becomes a lightweight probe / version check**. Its `ConnectionHandle.inner` holds an `Arc<ClaudeConnectionEnv>` whose only content is the precomputed `claude_path`, `session_store`, timeouts, and shared `env`. `start_session_on` delegates to the existing `spawn_internal` — i.e. a full subprocess spawn per session. No reuse.

3. **`check_connection` probes liveness of the owning `ClaudeAgentExecutor`** (claude binary still exists + readable), not a persistent transport. It should return `Healthy` whenever `check_cli_version` succeeds.

4. **`close_connection` is a no-op beyond clearing the probe cache** because no subprocess is owned by the connection.

5. **`AgentCapabilities::supports_multi_session_per_process` stays `false`**.

6. **`set_model` / `set_permission_mode` / `interrupt` remain session-scoped** because each session has its own subprocess — there is no sibling-session hazard. The runtime hazard described in the task prompt (set_model on A affects B) does NOT exist with this CLI.

7. **The `ConnectionSpec.probe` fast-path** short-circuits after `check_cli_version`, which is already cheap (~200ms).

The Phase-B work therefore reduces to wiring the new trait seam on top of the existing single-session-per-subprocess model, plus adding tests that confirm `start_session_on` spawns a fresh subprocess on every call (instead of reusing any pool).
