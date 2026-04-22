# Gemini `--acp` Protocol Findings — Phase A (or-p1-gemini)

**Status:** source-derived; live-verified = NO (gemini binary not installed on this
host and cannot be installed in the current sandbox — no network, no npm access).

**Sources of truth:**

1. `/Users/zal/Cteno/tmp/gemini-cli/` at commit `857365025` (`feat(config): split
   memoryManager flag into autoMemory (#25601)`) — upstream main.
2. `@agentclientprotocol/sdk` **v0.13.1** (the SDK gemini-cli `packages/cli` imports).
   Reference copy at
   `/opt/homebrew/lib/node_modules/clawdbot/node_modules/@agentclientprotocol/sdk/`.
3. `packages/cli/src/acp/acpClient.ts` in gemini-cli — the agent-side handler
   implementation (defines what the CLI accepts, how it replies, how it emits).

Every wire-shape claim below is backed by a file+line citation. No guessing.
When Phase B is executed on a machine with `gemini` installed, this doc should
be cross-checked against a live stdin/stdout capture before freezing the
adapter.

---

## 1. CLI flag

- **Current:** `gemini --acp`
  (`packages/cli/src/config/config.ts:341` — `.option('acp', {...})`)
- **Deprecated alias (still accepted):** `gemini --experimental-acp`
  (`packages/cli/src/config/config.ts:345-349` — description explicitly says
  *"deprecated, use --acp instead"*)
- Detection branch that picks the mode:
  `const isAcpMode = !!argv.acp || !!argv.experimentalAcp;`
  (`config.ts:751`).
- The current Rust adapter in `agent_executor.rs:162` passes
  `--experimental-acp`. That still works but will print a deprecation warning
  in future versions. The Phase B refactor should switch to `--acp` and keep
  `--experimental-acp` as a fallback if the binary rejects `--acp` with exit
  code != 0.

---

## 2. Wire envelope

**ACP is JSON-RPC 2.0 carried over newline-delimited JSON (ndJSON).** One JSON
object per line. There is no 8-byte Content-Length header (that's LSP, not
ACP), there is no batching.

Evidence:

- `acpClient.ts:104` —
  `const stream = acp.ndJsonStream(stdout, stdin);`
- ACP SDK `dist/stream.js` — `ndJsonStream()`: reads lines from `stdin`,
  `JSON.parse(trimmedLine)` each one, emits parsed objects; on write,
  `JSON.stringify(message) + "\n"` to `stdout`.
- ACP SDK `dist/acp.js:745-873` — every outbound message the SDK builds carries
  `jsonrpc: "2.0"`:
  - requests: `{ jsonrpc:"2.0", id, method, params }`
  - notifications: `{ jsonrpc:"2.0", method, params }` (no `id`)
  - responses: `{ jsonrpc:"2.0", id, result }` or `{ jsonrpc:"2.0", id, error:{code,message,data?} }`

Direction on the message routing (`acp.js:762-790`):

- message has both `method` + `id` → incoming request (must respond with
  `result` or `error` keyed to that id)
- message has `method` but no `id` → incoming notification (no response)
- message has `id` but no `method` → response to one of our earlier requests

**Implication for the Rust adapter:** the demuxer needs three maps /
dispatchers:

1. `pending_requests: HashMap<u64, oneshot::Sender<Result<Value, JsonRpcError>>>`
   keyed by outbound request id.
2. inbound-request dispatcher keyed by method name (`fs/read_text_file`,
   `fs/write_text_file`, `session/request_permission`,
   `terminal/*`) — these are server→client requests; the adapter must
   reply with a JSON-RPC response envelope carrying the same `id`.
3. inbound-notification dispatcher keyed by method name
   (`session/update` + future extensions) — routed to the per-session event
   channel via `params.sessionId`.

---

## 3. Protocol version + capability negotiation

- `PROTOCOL_VERSION = 1` (ACP SDK `schema/index.js:27`).
- Client sends `initialize` **first**, before anything else. Method name
  `"initialize"` (not `session/initialize`). `AGENT_METHODS.initialize`
  in `schema/index.js:4`.
- Request params (`InitializeRequest`, `types.gen.d.ts:834-859`):
  ```json
  {
    "protocolVersion": 1,
    "clientInfo":        { "name":"cteno", "version":"..." },
    "clientCapabilities": { "fs": { ... }, "terminal": boolean }
  }
  ```
- Response (`InitializeResponse`, `types.gen.d.ts:867-900`):
  ```json
  {
    "protocolVersion": 1,
    "agentCapabilities": {
      "loadSession": true,
      "promptCapabilities": { "image":true, "audio":true, "embeddedContext":true },
      "mcpCapabilities":    { "http":true, "sse":true }
    },
    "agentInfo":  { "name":"gemini-cli", "title":"Gemini CLI", "version":"..." },
    "authMethods": [ AuthMethod, ... ]
  }
  ```
  (exact shape hard-coded in `acpClient.ts:177-197`).

Nothing about initialize is session-scoped — it's per-connection, once per
subprocess lifetime.

---

## 4. Authentication

Method: `"authenticate"` (`AGENT_METHODS.authenticate`, `schema/index.js:3`).

`AuthenticateRequest` (`types.gen.d.ts:126-143`):
```json
{ "methodId": "<one-of-authMethods[].id>",
  "_meta":   { "api-key": "<string>", "gateway": { "baseUrl":"...", "headers":{...} } } }
```

- `methodId` **must** be one of the `id`s listed in
  `InitializeResponse.authMethods`. Gemini offers (from `acpClient.ts:141-172`):
  - `LOGIN_WITH_GOOGLE` — OAuth flow, inherits cached credentials
  - `USE_GEMINI` — Gemini Developer API key. The request passes the key via
    `_meta["api-key"]` as a plain string.
  - `USE_VERTEX_AI` — Vertex AI
  - `GATEWAY` — custom HTTP gateway via `_meta.gateway`.

- **Scope:** per-connection, persisted in the `GeminiAgent` instance
  (`this.apiKey`, `this.baseUrl`, `this.customHeaders` fields set in
  `acpClient.ts:219-245`). Subsequent `newSession` calls reuse them.
- **Required?** If the user already has cached credentials
  (`~/.config/gemini/...`), `initialize` can suffice and `authenticate` is
  optional. If no cache, `newSession` will throw `RequestError(-32000,
  "Authentication required.")` per `acpClient.ts:307-312`.
- **Safe default for our adapter:** if `OpenConnectionSpec` carries a
  Gemini API key via env or credentials, send `authenticate` once right
  after `initialize`; otherwise skip. On a subsequent `session/new` failure
  with code -32000, surface as `AgentExecutorError::AuthRequired` and let
  the host prompt.

---

## 5. `session/new`

Method: `"session/new"`. Params
(`NewSessionRequest`, `types.gen.d.ts:1240-1260`):
```json
{ "cwd": "/abs/path",  "mcpServers": [ McpServer, ... ] }
```

- `cwd` **must** be absolute (not enforced server-side but see `acpClient.ts:519`
  `loadCliConfig({..., cwd})`).
- `mcpServers` array is mandatory but may be empty `[]`. Entries are stdio
  or http/sse MCP configs; we pass `[]`.

Response (`NewSessionResponse`, `types.gen.d.ts:1264-1309`):
```json
{
  "sessionId": "<uuid v4>",
  "modes":     { "availableModes":[...], "currentModeId":"<default|auto_edit|plan|yolo>" },
  "models":    { "availableModels":[...], "currentModelId":"..." },
  "configOptions": [ ... ]   // optional
}
```

Gemini generates the sessionId via `randomUUID()` (`acpClient.ts:267`). We
must **capture and key every later request for this session on that returned
UUID**. Do not reuse anything from `initialize`.

**Multi-session confirmed** — `this.sessions: Map<string, Session>` at
`acpClient.ts:123`, keyed by `sessionId`, with no upper bound. All
`session/prompt` / `session/cancel` / `session/set_mode` / `session/set_model`
handlers do `this.sessions.get(params.sessionId)` first
(`acpClient.ts:531, 539, 549, 557`).

---

## 6. `session/load` (resume a persisted session)

Method: `"session/load"`. Params (`LoadSessionRequest`, `types.gen.d.ts:1006-1032`):
```json
{ "sessionId":"<prev-uuid>", "cwd":"/abs/path", "mcpServers":[...] }
```

Response (`LoadSessionResponse`, `types.gen.d.ts:1033-1072`): same shape as
`NewSessionResponse` **without** `sessionId` (the caller supplied it).
Additionally the agent streams the prior conversation back as
`session/update` notifications (each user+assistant msg replayed, see
`acpClient.ts:397-399` / `streamHistory()` at `:622-690`).

- Gated by `agentCapabilities.loadSession = true`. Gemini always advertises
  that true (`acpClient.ts:186`), so unconditional.
- **Implication for `resume_session`:** if the session store has a prior
  gemini sessionId, call `session/load`. If not, fall back to `session/new`
  on the shared connection. This is better than the current adapter's
  cold-restart+transcript-replay approach because the CLI does the replay
  itself.

---

## 7. `session/prompt`

Method: `"session/prompt"`. Params (`PromptRequest`,
`types.gen.d.ts:1469-1500`):
```json
{ "sessionId": "<uuid>",
  "prompt": [ ContentBlock, ... ] }
```

`ContentBlock` variants (from `types.gen.d.ts`, discriminated union via
`type` field):
- `{ "type":"text", "text":"..." }`  *(always supported)*
- `{ "type":"resource_link", "uri":"file:///...", "name":"..", "mimeType":".." }`  *(always)*
- `{ "type":"image", "mimeType":"..", "data":"<base64>" }`  *(if `promptCapabilities.image`)*
- `{ "type":"audio", ... }`  *(if `promptCapabilities.audio`)*
- `{ "type":"resource", "resource": EmbeddedResource }`  *(if `promptCapabilities.embeddedContext`)*

For our Phase B text-only path we emit a single `{type:"text", text:content}`
element and keep `sessionId` the session's uuid.

**Response** (`PromptResponse`, `types.gen.d.ts:1506-1522`):
```json
{ "stopReason": "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" | "cancelled",
  "_meta": { "quota": { "token_count": {...}, "model_usage": [...] } } }
```

- The response does **not** carry content. All content is streamed via
  `session/update` **notifications** during the turn and terminated by the
  `PromptResponse` coming back.
- `stopReason` maps to our `TurnComplete` event; `"cancelled"` maps to
  turn-aborted.
- Gemini's `_meta.quota` shape (`acpClient.ts:875-892`) is how token usage
  comes back — a stable place to pull `input_tokens` / `output_tokens` /
  per-model breakdown. Replaces the ad-hoc `usage` probe in the current
  stream parser.

---

## 8. `session/update` (notification, agent → client)

Method: `"session/update"` (`CLIENT_METHODS.session_update`, `schema/index.js:20`).
Params (`SessionNotification`, `types.gen.d.ts:2239-2258`):
```json
{ "sessionId": "<uuid>",
  "update": SessionUpdate }
```

**sessionId is a top-level field on every outbound event.** That's the
multiplex key. Demuxer routes `params.sessionId → session.event_channel`.

`SessionUpdate` is a discriminated union keyed by `sessionUpdate`
(`types.gen.d.ts:2289-2317`). The variants Gemini emits:

| `sessionUpdate`             | Payload shape                                                         | ExecutorEvent mapping |
|-----------------------------|-----------------------------------------------------------------------|-----------------------|
| `user_message_chunk`        | `{content: ContentBlock}`                                             | ignore (replayed on resume only; we already have local copy) |
| `agent_message_chunk`       | `{content: ContentBlock}` — `content.type="text"` stream              | `StreamDelta{kind:Text, content: content.text}` |
| `agent_thought_chunk`       | `{content: ContentBlock}`                                             | `StreamDelta{kind:Thinking, content: content.text}` |
| `tool_call`                 | `ToolCall` — `{toolCallId, title, status, kind, content:[], locations:[]}` | `ToolCallStart{tool_use_id: toolCallId, name: title, input: {}, partial:false}` |
| `tool_call_update`          | `ToolCallUpdate` — `{toolCallId, status, content, ...}`               | `status=="completed"` → `ToolResult{tool_use_id, output}`; `status=="failed"` → `ToolResult{output:Err}` |
| `plan`                      | Plan                                                                  | NativeEvent |
| `available_commands_update` | `{availableCommands:[...]}`                                           | NativeEvent |
| `current_mode_update`       | `{currentModeId}`                                                     | NativeEvent (also update local state) |
| `config_option_update`      | ConfigOptionUpdate                                                    | NativeEvent |
| `session_info_update`       | SessionInfoUpdate                                                     | NativeEvent |

`ContentBlock` for `agent_message_chunk` is not a standalone string — it's
`{type:"text", text:"..."}`. The current adapter's fabricated `textDelta`
field does not exist in real ACP.

---

## 9. `session/cancel` (notification, client → agent)

Method: `"session/cancel"` (`AGENT_METHODS.session_cancel`,
`schema/index.js:5`). **It's a notification, not a request** — we don't get a
JSON-RPC response. (Verified in `acp.js:612` —
`sendNotification(session_cancel, params)`.)

Params (`CancelNotification`, `types.gen.d.ts:233-250`):
```json
{ "sessionId": "<uuid>" }
```

Semantics per spec (from `acp.d.ts:388` and gemini
`acpClient.ts:579-586`): the agent aborts the pending `session/prompt`, then
responds to the **in-flight** `PromptResponse` with `stopReason: "cancelled"`.
Our adapter already has that in-flight request tracked in `pending_requests`
— we just need to let it resolve normally, **not** remove it ourselves.

**Independence invariant:** cancelling session A's `pendingPrompt` does not
touch session B — each `Session` owns its own `AbortController`
(`acpClient.ts:568, 693-695`). Confirmed.

---

## 10. `session/request_permission` (request, agent → client)

Method: `"session/request_permission"` (`CLIENT_METHODS.session_request_permission`,
`schema/index.js:19`). **This is server→client** — the agent asks *us* for
permission, and we must reply with a JSON-RPC response keyed to the
server's request id.

Request params (`RequestPermissionRequest`, `types.gen.d.ts:1643-1667`):
```json
{ "sessionId": "<uuid>",
  "toolCall":  { "toolCallId":"...", "status":"pending", "title":"...", "content":[...], "kind":"edit|exec|..." },
  "options":   [ PermissionOption, ... ] }
```

`PermissionOption` (`types.gen.d.ts:1055-1090`):
```json
{ "optionId":"proceed_once|cancel|proceed_always|...",
  "name":"Allow",
  "kind":"allow_once|allow_always|reject_once|reject_always" }
```

Response we must send back (`RequestPermissionResponse`,
`types.gen.d.ts:1670-1691`):
```json
{ "outcome":
     { "outcome":"selected", "optionId":"proceed_once" }
   OR
     { "outcome":"cancelled" } }
```

Note the nested `outcome` field — the discriminator literal `"selected"` vs
`"cancelled"` lives one level deeper than the envelope. `acpClient.ts:82-90`
and `:1078` parse it with a zod discriminatedUnion; don't flatten.

**Pairing:** we correlate by the **JSON-RPC request id** on the server→client
`session/request_permission` message (that's the id we must echo in our
response). `toolCallId` and `sessionId` are payload fields; they identify
*which* tool call needs permission, but the reply is keyed to the
request id. Our adapter's `PermissionRequest` ExecutorEvent must expose
both: a public `request_id` derived from toolCallId for UI, and an
internal correlation id = jsonrpc request id.

---

## 11. `session/set_mode`, `session/set_model`, `session/set_config_option`

| Method                       | Request params                              | Response         | Scope       |
|------------------------------|---------------------------------------------|------------------|-------------|
| `session/set_mode`           | `{ sessionId, modeId: "default"\|"auto_edit"\|"plan"\|"yolo" }` | `{}` (empty)     | per-session |
| `session/set_model`          | `{ sessionId, modelId:"gemini-2.5-pro"\|... }`                  | `{}` (empty)     | per-session |
| `session/set_config_option`  | `{ sessionId, optionId, value }`                                 | `{}`             | per-session |

All three apply **to the existing session without restarting**
(`acpClient.ts:546-564`, `Session.setMode` / `Session.setModel`). This
invalidates the current adapter's restart-and-replay strategy — the refactor
should remove `maybe_restart_session` entirely once on the shared connection.

- **`set_model`** should now return `ModelChangeOutcome::Applied` without
  cold-restart. But note `set_model` is declared on the
  *unstable* namespace in the SDK: the method constant is
  `AGENT_METHODS.session_set_model = "session/set_model"` and the TS
  handler is `unstable_setSessionModel` (`acpClient.ts:556`). If Gemini
  decides to remove it, we should fall back to
  `ModelChangeOutcome::Unsupported`.
- **`set_permission_mode`** → maps `PermissionMode::*` → Gemini's
  `"default"|"auto_edit"|"plan"|"yolo"` mode id. Same mapping as the
  current `permission_mode_cli_value`. Just send via
  `session/set_mode` instead of cold restart.

---

## 12. Unsupported / out-of-scope for Phase B

- `session/list`, `session/fork`, `session/resume` — all `@experimental` in
  the SDK. Skip, leave `list_sessions` reading our local
  `SessionStoreProvider` (unchanged from current adapter).
- `terminal/*` client methods — we don't advertise terminal capability
  (`clientCapabilities.terminal = false` or omitted), so the agent won't
  emit these.
- `fs/read_text_file` / `fs/write_text_file` client methods — only if we
  advertise `clientCapabilities.fs`. For Phase B we advertise `fs.readTextFile
  = false, fs.writeTextFile = false` (we have our own fs tools on the host
  side); Gemini's `AcpFileSystemService` will not be installed
  (`acpClient.ts:314-323` — guarded by `this.clientCapabilities?.fs`).

---

## 13. Subprocess death behaviour

- stdin close → gemini's ndJsonStream sees EOF → `connection.closed` resolves
  → `runExitCleanup()` fires → process exits (`acpClient.ts:110-113`).
- Signal kill (SIGKILL / SIGTERM) → stdin/stdout pipes break → our
  `BufReader::read_line` returns `Ok(0)`. The demuxer must propagate that as
  a terminal error on every in-flight pending request and every session's
  event channel (`AgentExecutorError::Protocol("gemini process died")`),
  then mark the connection `unhealthy`. `check_connection` after that point
  returns `ConnectionUnhealthy`.
- A single session's prompt throwing (e.g. auth revoked mid-turn) → Gemini
  replies with `error: { code:-32000, message:"..." }` to that
  `session/prompt` only; other sessions are unaffected. Confirmed by
  `acpClient.ts:847-899` — rate-limit / InvalidStreamError paths return a
  per-request error, not a connection-level crash.

---

## 14. Protocol vs current Rust adapter — delta list for Phase B

The current adapter (`packages/multi-agent-runtime/rust/crates/multi-agent-runtime-gemini/src/agent_executor.rs`)
and its stream parser (`stream.rs`) use a **fabricated, non-ACP protocol**:

| Current (wrong)                                               | Real ACP                                                       |
|---------------------------------------------------------------|----------------------------------------------------------------|
| `{"type":"status","status":"running","sessionId":"..."}`      | No such frame. Ready-state = `initialize` response arriving.  |
| `{"type":"user_message","message":{"role":"user",...}}`       | `{"jsonrpc":"2.0","id":N,"method":"session/prompt","params":{"sessionId":"...","prompt":[{"type":"text","text":"..."}]}}` |
| `{"type":"model-output","textDelta":"..."}`                   | `{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"...","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"..."}}}}` |
| `{"type":"tool-call","toolCallId":"...","toolName":"...",...}`| `session/update` with `sessionUpdate:"tool_call"`              |
| `{"type":"tool-result","toolCallId":"...","output":"..."}`    | `session/update` with `sessionUpdate:"tool_call_update"`, `status:"completed"` |
| `{"type":"permission_request","request_id":"..."}`            | Server→client JSON-RPC request, `method:"session/request_permission"`, response keyed to the request id, not `request_id` |
| `{"type":"status","status":"idle","usage":{...}}`             | `session/prompt` response returns with `stopReason:"end_turn"` + `_meta.quota` |
| `{"type":"permission_response","request_id":"...","decision":"..."}` | Response envelope `{jsonrpc:"2.0",id:<serverRequestId>,result:{outcome:{outcome:"selected",optionId:"..."}}}` |
| `--approval-mode <mode>` CLI flag at spawn                    | `session/set_mode` request after newSession                    |
| Per-session subprocess + cold-restart on mode/model change   | One subprocess for the connection; `session/new` per session; `session/set_mode` / `session/set_model` are hot |

The delta is large enough that the Phase B refactor is effectively a
from-scratch rewrite of `agent_executor.rs` + `stream.rs`, plus
replacement of every test in the `#[cfg(test)] mod tests` block (the mock
shell script speaks the wrong protocol).

---

## 15. Deliverable — what Phase B starts with

**Contracts the adapter must implement per `trait_def.rs`:**

| Trait method               | ACP mapping                                                   |
|---------------------------|---------------------------------------------------------------|
| `open_connection`         | spawn `gemini --acp`; `initialize`; optional `authenticate`   |
| `close_connection`        | drop ndJSON writer (closes stdin) → CLI exits; wait on child  |
| `check_connection`        | `Child::try_wait()` + demuxer-task-alive check                |
| `start_session_on`        | `session/new` on shared connection, capture returned sessionId|
| `spawn_session` (legacy)  | `open_connection` → `start_session_on`                        |
| `resume_session`          | `session/load` on shared connection (agent replays history)   |
| `send_message`            | `session/prompt`; stream `session/update` notifications; await `PromptResponse` |
| `respond_to_permission`   | Send JSON-RPC response `{id:<serverReqId>, result:{outcome:{outcome:"selected", optionId:"proceed_once|cancel|..."}}}` |
| `interrupt`               | `session/cancel` notification + await pending `PromptResponse` resolving with `stopReason:"cancelled"` |
| `close_session`           | No ACP method; just drop local session entry. (Gemini keeps it in Map until subprocess exit — minor leak, acceptable.) |
| `set_permission_mode`     | `session/set_mode` request                                    |
| `set_model`               | `session/set_model` request (unstable — fall back to Unsupported if agent returns method-not-found) |
| `list_sessions`           | Keep going through SessionStoreProvider (Gemini `session/list` is experimental) |

**`AgentCapabilities` bits to flip:**

- `supports_multi_session_per_process: true`
- `supports_resume: true` (already true)
- `supports_runtime_set_model: true` (already true) but the backing
  mechanism changes from "adapter-owned restart" to "native ACP request"
- `permission_mode_kind: Dynamic` (unchanged, but now truly dynamic without restart)

---

## 16. Validation note

The above is derived from static source reading only. Before landing the
Phase B adapter, a live capture from a machine with `gemini` installed
**must** be run against at least:

- `initialize` request/response
- `authenticate` request/response (with USE_GEMINI + meta api-key)
- two concurrent `session/new` → confirm distinct UUIDs
- interleaved `session/prompt` on two sessions → confirm
  `session/update.params.sessionId` demuxes correctly
- `session/cancel` on session A mid-turn while session B's prompt is in flight
- a `session/request_permission` round-trip for an edit tool
- `session/load` on a prior sessionId → confirm history replay via
  `session/update` with `user_message_chunk` + `agent_message_chunk`
- subprocess kill -9 mid-turn → observe how in-flight pending requests and
  both sessions' event channels see the termination.

Any deviation from the source-derived schemas above becomes a bug in the
adapter; re-run this doc as a checklist when validating.
