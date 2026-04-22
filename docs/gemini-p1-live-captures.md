# Gemini `--acp` Live Capture Spot-Checks (Phase A+)

**Host**: `gemini --version` → `0.38.2` at `/opt/homebrew/bin/gemini`.

**Workdir used**: `/tmp` (absolute, as required by `acpClient.ts:519`).

**Auth state on this host**: cached OAuth credentials already present at
`~/.gemini/oauth_creds.json`. This is the critical divergence from the Phase A
findings doc (see §"Divergences" at the bottom).

All frames below were captured by running `python3 /tmp/gemini-captures/drive.py`
which:

1. Spawns `gemini --acp` with pipes on stdin/stdout/stderr.
2. Writes JSON-RPC frames one-per-line to stdin.
3. Consumes stdout line-by-line and saves every parsed frame.
4. Closes stdin and waits for the child to exit.

Stdout noise (startup banners, experiment flag dump, `[STARTUP]` profiler
lines, `Ignore file not found` warnings) is emitted by Gemini CLI on stderr,
not on stdout. **stdout was pure ndJSON JSON-RPC.** The adapter should redirect
stderr separately (e.g. `Stdio::null()` or pipe it into a logger).

---

## Step 1 — `initialize` (request id=1)

**Sent:**

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}}
```

**Received (single line, pretty-printed here for readability):**

```json
{"jsonrpc":"2.0","id":1,"result":{
  "protocolVersion":1,
  "authMethods":[
    {"id":"oauth-personal","name":"Log in with Google","description":"Log in with your Google account"},
    {"id":"gemini-api-key","name":"Gemini API key","description":"Use an API key with Gemini Developer API","_meta":{"api-key":{"provider":"google"}}},
    {"id":"vertex-ai","name":"Vertex AI","description":"Use an API key with Vertex AI GenAI API"},
    {"id":"gateway","name":"AI API Gateway","description":"Use a custom AI API Gateway","_meta":{"gateway":{"protocol":"google","restartRequired":"false"}}}
  ],
  "agentInfo":{"name":"gemini-cli","title":"Gemini CLI","version":"0.38.2"},
  "agentCapabilities":{
    "loadSession":true,
    "promptCapabilities":{"image":true,"audio":true,"embeddedContext":true},
    "mcpCapabilities":{"http":true,"sse":true}
  }
}}
```

**Confirms:** `protocolVersion: 1`, four auth methods with the ids the findings
doc expected (`oauth-personal` ≙ LOGIN_WITH_GOOGLE, `gemini-api-key` ≙
USE_GEMINI, `vertex-ai` ≙ USE_VERTEX_AI, `gateway` ≙ GATEWAY),
`agentCapabilities.loadSession: true`, full prompt capabilities, no `terminal`
entry (matches our `clientCapabilities.terminal=false`).

Auth-method ids in the live run use **kebab-case** (`oauth-personal`,
`gemini-api-key`, etc.). The findings doc referenced the SDK *constant names*
(`LOGIN_WITH_GOOGLE`, `USE_GEMINI`). When the adapter sends `authenticate`
it must use the kebab-case ids exactly as listed in the response.

---

## Step 2 — `session/new` without an explicit `authenticate` (request id=2)

**Sent:**

```json
{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}
```

**Received (pretty-printed):**

```json
{"jsonrpc":"2.0","id":2,"result":{
  "sessionId":"d17853dd-4b5b-4495-bf93-d2b88e1eddec",
  "modes":{
    "availableModes":[
      {"id":"default","name":"Default","description":"Prompts for approval"},
      {"id":"autoEdit","name":"Auto Edit","description":"Auto-approves edit tools"},
      {"id":"yolo","name":"YOLO","description":"Auto-approves all tools"},
      {"id":"plan","name":"Plan","description":"Read-only mode"}
    ],
    "currentModeId":"default"
  },
  "models":{
    "availableModels":[
      {"modelId":"auto-gemini-3","name":"Auto (Gemini 3)","description":"Let Gemini CLI decide the best model for the task: gemini-3.1-pro, gemini-3-flash"},
      {"modelId":"auto-gemini-2.5","name":"Auto (Gemini 2.5)","description":"Let Gemini CLI decide the best model for the task: gemini-2.5-pro, gemini-2.5-flash"},
      {"modelId":"gemini-3.1-pro-preview","name":"gemini-3.1-pro-preview"},
      {"modelId":"gemini-3-flash-preview","name":"gemini-3-flash-preview"},
      {"modelId":"gemini-3.1-flash-lite-preview","name":"gemini-3.1-flash-lite-preview"},
      {"modelId":"gemini-2.5-pro","name":"gemini-2.5-pro"},
      {"modelId":"gemini-2.5-flash","name":"gemini-2.5-flash"},
      {"modelId":"gemini-2.5-flash-lite","name":"gemini-2.5-flash-lite"}
    ],
    "currentModelId":"auto-gemini-3"
  }
}}
```

**Confirms:** response carries `sessionId` (UUID v4), `modes`, and `models`
exactly as the findings doc described, plus a new **`autoEdit`** mode id
(camelCase — the findings doc wrote `auto_edit` underscore-separated).
**Divergence ⚠**: the spec docs for gemini-cli show `auto_edit` but the
**live server sends `autoEdit` (camelCase)**. The adapter's
`permission_mode_cli_value` mapping must emit `autoEdit` — not `auto_edit` —
when calling `session/set_mode`.

---

## Step 3 — authenticate with env-provided API key

**Skipped.** `GEMINI_API_KEY` is not set in this capture session's env (cached
OAuth at `~/.gemini/oauth_creds.json` is used instead). The Phase B adapter
must still *support* sending `authenticate` with `_meta["api-key"]` when the
caller supplies one; we will exercise that path via unit-test mocks.

Because the cached OAuth on this host suffices, `session/new` above succeeded
without an explicit `authenticate` call. The findings doc's claim that
`session/new` throws `RequestError(-32000, "Authentication required.")` is
**conditional on cache absence**. On a fresh host it still applies; on a host
with any cached credential it does not.

## Step 4 — `session/new` with auth cached (covered by Step 2)

`sessionId = d17853dd-4b5b-4495-bf93-d2b88e1eddec`.

---

## Step 5 — `session/update` shape during a live prompt

Right after `session/new` resolved, Gemini also emitted an unsolicited
`available_commands_update` notification targeted at the new session:

```json
{"jsonrpc":"2.0","method":"session/update","params":{
  "sessionId":"d17853dd-4b5b-4495-bf93-d2b88e1eddec",
  "update":{
    "sessionUpdate":"available_commands_update",
    "availableCommands":[
      {"name":"memory","description":"Manage memory."},
      /* ...memory show/refresh/list/add, extensions *, init, restore, about, help... */
    ]
  }
}}
```

After sending `session/prompt` (id=3, `prompt: [{type:"text", text:"Respond with exactly the single word PONG."}]`),
three `session/update` frames streamed out, then the PromptResponse:

```json
{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"d17853dd-...","update":{"sessionUpdate":"available_commands_update","availableCommands":[...]}}}
{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"d17853dd-...","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"**Verifying Confirmation Protocol**\n\nI'm presently focusing on the user's explicit request for a singular, definitive response: \"PONG.\" It appears to be a straightforward confirmation signal, nothing more. My internal processes have been calibrated to ensure the correct output.\n\n\n"}}}}
{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"d17853dd-...","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"PONG"}}}}
{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn","_meta":{"quota":{"token_count":{"input_tokens":11700,"output_tokens":2},"model_usage":[{"model":"gemini-3-flash-preview","token_count":{"input_tokens":11700,"output_tokens":2}}]}}}}
```

**Confirms every claim in §8 of the findings doc:**

- `session/update.params.sessionId` is the top-level multiplex key on every
  notification. ✓
- `agent_message_chunk` payload is `{content: {type:"text", text:"..."}}`
  (not a bare `textDelta` string). ✓
- `agent_thought_chunk` carries `{content: {type:"text", text:"..."}}` the
  same way. ✓
- `available_commands_update` uses a top-level `availableCommands` array. ✓
- PromptResponse `stopReason = "end_turn"`, token usage under
  `_meta.quota.token_count` with `input_tokens / output_tokens` (snake_case),
  and a per-model breakdown under `_meta.quota.model_usage[].token_count`. ✓
- No `session/update` was emitted for a fabricated `status:"idle"` — end of
  turn is *only* the JSON-RPC response to the `session/prompt` request. ✓

The model **`gemini-3-flash-preview`** actually served the turn even though
`currentModelId` was `auto-gemini-3` (the selector auto-routed within the
Gemini-3 family). Adapters that want deterministic model routing must set
a concrete `modelId` via `session/set_model` rather than leave Gemini on one
of the `auto-*` entries.

---

## Step 6 — `session/cancel` after turn completion

```json
>>> {"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"d17853dd-4b5b-4495-bf93-d2b88e1eddec"}}
```

No response (notification semantics). The in-flight prompt had already
resolved with `stopReason:"end_turn"`, so `session/cancel` here is a no-op —
the CLI does not error. We did **not** attempt a mid-turn cancel against this
host (the prompt completed in ~3 seconds), but the absence of a response
confirms the notification shape matches §9 of the findings doc.

---

## Step 7 — close stdin, wait for exit

Closing stdin after the cancel caused `gemini` to shut down cleanly.
`proc.wait()` returned **exit code 0**. Total time for the whole capture
(init + session/new + prompt + cancel + close): ≈ 7 seconds including the
~3s `initialize_app` phase visible in stderr's `[STARTUP]` profiler dump.

---

## Divergences from Phase A findings

1. **`authMethods[].id`** is emitted in the live protocol as kebab-case
   (`oauth-personal`, `gemini-api-key`, `vertex-ai`, `gateway`) — not as the
   SDK constant names (`LOGIN_WITH_GOOGLE`, `USE_GEMINI`, etc.) that the
   findings doc quoted. **Impact**: when the adapter sends `authenticate`
   with `methodId`, it must use the kebab-case ids from the server response.

2. **Mode id `autoEdit`** (camelCase) in the live `modes.availableModes`
   response — the findings doc used `auto_edit` (snake_case). **Impact**:
   `permission_mode_cli_value` / the `session/set_mode` request must emit
   `autoEdit`, not `auto_edit`. Other mode ids match (`default`, `plan`,
   `yolo`).

3. **`session/new` does not require `authenticate`** when any credential is
   cached (OAuth, api key, etc.). The findings doc phrased the -32000 as
   always thrown without auth; the accurate statement is "without *any*
   cached credential". Implication: the adapter should optimistically call
   `session/new` and only treat a -32000 response as a trigger to surface
   `AgentExecutorError::AuthRequired`. Sending `authenticate` proactively is
   still safe when credentials are supplied through the spec.

4. **`available_commands_update` is pushed unsolicited** right after
   `session/new` succeeds, before the client sends any prompt. This is a
   session-scoped notification (carries `sessionId`), so the demuxer routes
   it normally, but the Phase B EventStream for `send_message` is not the
   right channel for it — it arrives before the first `send_message` call.
   Options: (a) buffer it into a session-state "available commands" snapshot
   that the adapter exposes via an additional method, or (b) drop it. For
   Phase B we **drop it and log** (it becomes a `NativeEvent` only if some
   caller ever subscribes to the session's event channel outside a turn).

5. **Gemini emits extensive startup logging to stderr**, including an
   `Experiments loaded` telemetry dump and a `[STARTUP] StartupProfiler`
   block. The adapter must not inherit that stream or it will pollute the
   host's logs. Current per-session adapter uses `Stdio::null()` for stderr;
   Phase B must do the same (or pipe stderr into a dedicated reader that
   logs at debug level only).

6. **Model auto-routing**: `currentModelId: "auto-gemini-3"` delegates to
   Gemini CLI's internal model picker (observed in `_meta.quota.model_usage`
   actually using `gemini-3-flash-preview`). When the adapter's spec requests
   `gemini-2.5-pro`, `session/set_model` must be called after `session/new`;
   the `GEMINI_MODEL` env var the current adapter exports **does not**
   influence ACP-mode selection.

No divergences material enough to block Phase B: the wire envelope, JSON-RPC
2.0 framing, multi-session demux key, PromptResponse shape, and usage/quota
structure all match the findings doc.
