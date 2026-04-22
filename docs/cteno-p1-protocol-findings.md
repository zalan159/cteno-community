# cteno-agent stdio protocol — connection-reuse findings (Phase 1)

Empirical validation of the multi-session capability in the `cteno-agent`
binary (`packages/agents/rust/crates/cteno-agent-stdio`). These notes are the
contract the `multi-agent-runtime-cteno` adapter will rely on when it stops
spawning one subprocess per session.

All observations below were captured by driving the debug build
`packages/agents/rust/crates/cteno-agent-stdio/target/debug/cteno-agent`
directly with scripted stdin writes; stdout was JSON-parsed per line and
stderr was forwarded unchanged. Harness source: `/tmp/cteno-probe.py`.

## 1. Inbound / Outbound frame schemas (observed)

All frames are line-delimited JSON. Every frame carries a `type` discriminant
(snake_case) and, for all session-scoped messages, a `session_id: string`.

### Inbound (host → agent) — fields verified by decode behaviour

| type                     | required               | optional                                             |
|--------------------------|------------------------|------------------------------------------------------|
| `init`                   | `session_id`           | `workdir`, `agent_config`, `system_prompt`, `auth_token`, `user_id`, `machine_id` |
| `user_message`           | `session_id`, `content`| –                                                    |
| `abort`                  | `session_id`           | –                                                    |
| `set_model`              | `session_id`, `model`  | `effort`                                             |
| `set_permission_mode`    | `session_id`, `mode`   | –                                                    |
| `permission_response`    | `session_id`, `request_id`, `decision` (`allow`\|`deny`\|`abort`) | `reason` |
| `tool_inject`            | `session_id`, `tool`   | –                                                    |
| `tool_execution_response`| `session_id`, `request_id`, `ok: bool` | `output: string`, `error: string`   |
| `host_call_response`     | `session_id`, `request_id`, `ok: bool` | `output: Value`, `error: string`    |
| `token_refreshed`        | `access_token`         | (process-wide, does not carry session_id)            |
| unknown `type`           | —                      | dropped with stderr warning; never fatal             |

### Outbound (agent → host) — fields captured from live frames

Every outbound frame carries `session_id` (observed in every test). Frames:

| type                     | fields                                                                                |
|--------------------------|---------------------------------------------------------------------------------------|
| `ready`                  | `session_id`                                                                          |
| `delta`                  | `session_id`, `kind` (`text`\|`thinking`), `content`                                  |
| `tool_use`               | `session_id`, `tool_use_id`, `name`, `input`                                          |
| `tool_result`            | `session_id`, `tool_use_id`, `output`, `is_error`                                     |
| `permission_request`     | `session_id`, `request_id`, `tool_name`, `tool_input`                                 |
| `tool_execution_request` | `session_id`, `request_id`, `tool_name`, `tool_input`                                 |
| `host_call_request`      | `session_id`, `request_id`, `hook_name`, `method`, `params`                           |
| `turn_complete`          | `session_id`, `final_text`, `iteration_count`, `usage`                                |
| `error`                  | `session_id`, `message`                                                               |

Important: error frames produced for unrecognized message routing include the
offending `session_id` as given by the host (including the literal string
`"ghost"` we sent below — the agent does NOT rewrite it to empty/null). Parse
errors on inbound lines emit `error` with `session_id: ""`.

## 2. Handshake / timing

Observed for a single Init:

```
host → agent : {"type":"init","session_id":"sessA","workdir":"/tmp",...}
agent → host : {"type":"ready","session_id":"sessA"}      [<1 ms after enqueue]
```

Observed for two back-to-back Inits on one process (test 2 log excerpt,
verbatim):

```
[t2][STDIN]   {"type": "init", "session_id": "sessA", "workdir": "/tmp", "agent_config": {}}
[t2][STDIN]   {"type": "init", "session_id": "sessB", "workdir": "/tmp", "agent_config": {}}
[t2][STDERR]  [...INFO cteno_agent] cteno-agent stdio bootstrap complete: 24 builtin tools registered
[t2][STDOUT]  {"type": "ready", "session_id": "sessA"}
[t2][STDOUT]  {"type": "ready", "session_id": "sessB"}
```

Every new `init` for a fresh `session_id` yields exactly one matching `ready`
frame. Init for an existing `session_id` is a **replace-on-reinit**: the prior
`SessionHandle` is dropped (its in-flight turn keeps running but will no longer
receive new input) and a second `ready{session_id}` is emitted (test 8).

There is no batch bootstrap frame; the runtime logs "bootstrap complete" once
on stderr but **does not** emit a stdout frame before the first Ready. The
host must not wait for a non-existent protocol-level hello.

## 3. Multi-session routing rules

- **Key = `session_id` string.** The routing table is a
  `HashMap<String, SessionHandle>` (see `main.rs:100`). No session namespacing,
  no hierarchical ids — duplicate ids collide and replace.
- **Every inbound session-scoped message is dispatched by `session_id`.**
  `Abort`, `SetModel`, `SetPermissionMode`, `ToolInject`, and `UserMessage`
  each look up the session in the map. Unknown session yields behaviour below.
- **Every outbound frame we observed carries `session_id`.** The
  `Outbound::Error` variant emitted for parse failures on stdin uses an empty
  string for `session_id` (harness test 3 shows it populated when the frame
  routes through `main.rs` with the requested id).
- **Pending request maps are keyed by `request_id`, not `session_id`.**
  `permission_response` / `tool_execution_response` / `host_call_response` all
  succeed by `request_id` alone; `session_id` on the response is only used for
  the error-on-miss frame. Implication: the adapter can safely use a single
  global pending-request map per `CtenoConnection`, not per session.
- **Permission closure and tool injection are per-session in intent.** The
  host is expected to tool-inject the same orchestration tools once per
  session; the tool executor emits `tool_execution_request` carrying the
  caller session_id, so the host can route results back to the right session
  state even though the pending map is global.

## 4. Error modes

Captured verbatim:

- Unknown session on `user_message` (test 3):
  `{"type": "error", "session_id": "ghost", "message": "unknown session_id; init must be sent first"}`
- Unknown session on `tool_inject` (test 6):
  `{"type": "error", "session_id": "ghost", "message": "tool_inject: unknown session_id; init must be sent first"}`
- Unknown session on `abort` (test 4): **does NOT emit error**, only stderr
  warn: `WARN cteno_agent] abort for unknown session ghost (dropping)`.
- Unknown `request_id` on `permission_response` (test 7):
  `{"type": "error", "session_id": "sessA", "message": "permission_response: no pending request for request_id=does-not-exist"}`
- Unknown `type`: silently dropped with a stderr warn line; no outbound frame.
- Subprocess exit on stdin EOF: clean, exit code 0 (verified in tests 1, 4, 5
  with and without live sessions). In-flight turns are awaited before exit
  (`main.rs:362-370`), so closing stdin is a graceful-shutdown signal — not
  an abort.

### Subprocess death mid-session

Not observed in this probe (no way to cleanly kill the child from inside
the agent with our harness short of SIGKILL). For the adapter, the operative
assumption is:
- `Child::try_wait()` going `Some(status)` → connection dead, **all**
  sessions on that connection are terminal. The adapter must fan out a
  synthetic `error{recoverable: false}` to each session's event channel and
  fail any awaiting pending-request receiver.
- This is why `check_connection` must exist and is mandatory before
  `start_session_on` on a reused handle — a session on a dead process would
  silently hang on `Init` otherwise.

## 5. The head-of-line-blocking question

`OutboundWriter` is `Arc<Mutex<tokio::io::Stdout>>` (see
`cteno-agent-stdio/src/io.rs:12-46`). All stdout writes serialize through this
single mutex. If the reader on the host side stops draining stdin (no, stdout
— same principle: the OS pipe buffer fills) every session writer inside the
agent blocks at the mutex.

**Implication for the adapter's shared outbound writer:**

The adapter's `ChildStdin` Mutex has the symmetric problem: if the subprocess
stops reading stdin fast enough, a host task holding the mutex to write (say,
a large `tool_execution_response` payload) stalls every other session's
writes. For Phase B we must:

1. Wrap the shared writer with an `mpsc::Sender<Inbound>` (bounded, e.g.
   cap 256) and a single dedicated writer task that owns `ChildStdin`.
   Producers push frames and never hold a lock across an await longer than
   the channel's capacity-exceeded backpressure.
2. This means **one slow consumer (the child) still backpressures all
   producers**, but it does so uniformly via channel send pressure rather
   than unfairly via mutex ordering — all sessions see the same latency
   budget, and a sender timeout can surface as a recoverable error.
3. On the stdout side the adapter already has the right shape: a single
   demultiplexer task reads lines, routes by `session_id` to per-session
   `mpsc::Sender<ExecutorEvent>` channels. Slow per-session consumers
   can block only their own channel; other sessions are unaffected.

## 6. Verbatim captured stdout excerpt

From test 2 (two sessions on one process, multiplexed):

```
[t2][STDIN] {"type": "init", "session_id": "sessA", "workdir": "/tmp", "agent_config": {}}
[t2][STDIN] {"type": "init", "session_id": "sessB", "workdir": "/tmp", "agent_config": {}}
[t2][STDERR] [2026-04-19T19:21:12.283Z INFO  cteno_agent] cteno-agent stdio bootstrap complete: 24 builtin tools registered
[t2][STDOUT][1776626472.283] {"type": "ready", "session_id": "sessA"}
[t2][STDOUT][1776626472.283] {"type": "ready", "session_id": "sessB"}
```

## 7. Contract for Phase B

Baking the findings above into the adapter rewrite:

1. `open_connection(spec)` spawns the subprocess and **does not** send Init.
   It starts a demux task on stdout and a writer task on stdin, then returns.
   No probe is required — the child is considered healthy as soon as
   `Child::try_wait()` reports `None`.
2. `start_session_on(handle, spec)` sends Init through the shared writer and
   waits for a `ready{session_id: spec.session_id}` frame routed back via the
   per-session channel registered in the connection's HashMap. Timeout:
   current adapter uses 30 s for Ready; preserve that.
3. `close_session` sends Abort, removes the per-session state, closes the
   event channel. **Does not** terminate the subprocess.
4. `close_connection` closes stdin (graceful shutdown), waits up to a bounded
   time for the child to exit, then kills on timeout. Clears the HashMap.
5. `check_connection` returns `Healthy` while `Child::try_wait()` is `None`
   and the writer channel is open; otherwise `Dead{reason}`.

## 8. Protocol changes required on the agent side

**None.** Everything Phase B needs is already implemented by
`cteno-agent-stdio`. The refactor is strictly adapter-side.
