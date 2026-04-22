# Gemini connection-reuse adapter regression

## meta
- kind: host-runtime
- adapter: `multi-agent-runtime-gemini` (phase B connection reuse)
- cli: `gemini --acp` (0.38.2+)
- workdir: `/tmp`

## setup

Run tests against a mock CLI first, then the live binary if available.

```bash
cd packages/multi-agent-runtime/rust
cargo test -p multi-agent-runtime-gemini
# Optional live run:
GEMINI_PATH="$(command -v gemini)" cargo test -p multi-agent-runtime-gemini -- --ignored live_smoke
```

## cases

### [pending] initialize returns protocolVersion 1 + full capability bag
- **message**: n/a — integration test `open_connection_runs_initialize_and_parses_capabilities`
- **expect**: adapter spawns `gemini --acp`, sends `{jsonrpc:"2.0", id:1, method:"initialize", ...}`, receives `{result:{protocolVersion:1, authMethods:[...], agentCapabilities:{loadSession:true, ...}}}`. `AgentCapabilities::supports_multi_session_per_process` is now `true`.
- **anti-pattern**: sending the deprecated `--experimental-acp` flag, assuming protocolVersion=0.1, fabricating `{"type":"status","status":"running"}` frames.
- **severity**: high

### [pending] session/new works without explicit authenticate when OAuth cached
- **message**: n/a — covered by both mock and live smoke
- **expect**: adapter issues `session/new` after `initialize` without preemptively calling `authenticate`. On hosts with cached creds (`~/.gemini/oauth_creds.json`) the call succeeds and returns a UUIDv4 sessionId + modes + models.
- **anti-pattern**: always calling `authenticate` before `session/new` — will fail when only OAuth is available.
- **severity**: high

### [pending] session/new without credentials surfaces -32000 → AuthRequired
- **message**: mock scenario `auth_fail`
- **expect**: when `session/new` returns JSON-RPC error code `-32000 "Authentication required."`, adapter maps it into `AgentExecutorError::Vendor`. Caller can distinguish via the error message.
- **anti-pattern**: treating the error as a generic protocol failure or crashing the subprocess.
- **severity**: medium

### [pending] two sessions on one subprocess route events by sessionId
- **message**: n/a — `two_sessions_on_one_connection_route_independent_events`
- **expect**: two `spawn_session` calls reuse the same `gemini --acp` subprocess. `send_message` on session A only streams session A's `agent_message_chunk`s. Session B's stream is independent.
- **anti-pattern**: session cross-talk (deltas leaking between sessions), spawning a second subprocess.
- **severity**: high

### [pending] session/prompt response carries usage under _meta.quota.token_count
- **message**: n/a — `send_message_completes_with_turn_complete_and_usage`
- **expect**: final `ExecutorEvent::TurnComplete` carries `input_tokens` + `output_tokens` pulled from `response._meta.quota.token_count` using snake_case keys.
- **anti-pattern**: looking for `inputTokens` / `outputTokens` camelCase or a fabricated `status:"idle"` frame.
- **severity**: high

### [pending] session/request_permission correlates by inbound JSON-RPC id
- **message**: n/a — `respond_to_permission_correlates_by_inbound_request_id`
- **expect**: when gemini sends `{"id":100, "method":"session/request_permission", ...}`, adapter emits `ExecutorEvent::PermissionRequest { request_id: "100", ...}`. Caller invokes `respond_to_permission("100", Allow)` → adapter writes `{jsonrpc:"2.0", id:100, result:{outcome:{outcome:"selected", optionId:"proceed_once"}}}`. The turn resumes and streams the post-permission text.
- **anti-pattern**: flattening the nested `outcome.outcome` discriminator, using a fabricated `permission_response` frame, or correlating by `toolCallId` instead of the JSON-RPC request id.
- **severity**: high

### [pending] session/cancel is a notification (no reply)
- **message**: `executor.interrupt(&session).await`
- **expect**: adapter sends `{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"..."}}` with NO `id` field. The in-flight `session/prompt` resolves with `stopReason:"cancelled"` on its own.
- **anti-pattern**: sending cancel as a request + awaiting a response → deadlock.
- **severity**: medium

### [pending] check_connection reports Dead after subprocess exits
- **message**: n/a — `check_connection_reports_dead_after_child_exit`
- **expect**: after the gemini subprocess exits, `check_connection(&handle)` returns `ConnectionHealth::Dead { reason: "gemini --acp exited (code=...)" }`.
- **anti-pattern**: returning `Healthy` indefinitely, or silently re-spawning.
- **severity**: medium

### [pending] close_connection drains pendings + tears down
- **message**: n/a — `close_connection_kills_child_and_drains_pending`
- **expect**: `close_connection(handle)` kills the child, aborts demuxer / writer tasks, and any in-flight `call()` awaits receive `ConnectionClosed`-flavoured errors.
- **anti-pattern**: leaking the child process, hanging pendings.
- **severity**: medium

### [skip] `session/set_model` unsupported fallback — needs live -32601 fixture
- **reason**: requires a gemini build that has removed `session/set_model`; current 0.38.2 supports it so Unsupported fallback branch is unreachable without source-level mock.

## status

All cases marked `[pending]` have backing unit or integration tests and pass locally:

```
running 14 tests
test stream::tests::... (7 cases)     ok
test integration_connection_reuse::... (7 cases)     ok
+ 1 ignored (live_smoke_initialize_plus_prompt, gated by GEMINI_PATH)
```

QA agent: if live smoke is desired, set `GEMINI_PATH` (and optionally `GEMINI_API_KEY` — skipped when unset) and run `cargo test -p multi-agent-runtime-gemini -- --ignored live_smoke`.
