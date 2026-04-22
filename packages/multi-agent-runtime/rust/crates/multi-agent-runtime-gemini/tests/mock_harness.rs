//! Shared helper — spawns a mock `gemini --acp` written as a shell script
//! that speaks real JSON-RPC 2.0 / ndJSON. Used by both connection-level unit
//! tests and the connection-reuse integration test.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub struct MockHarness {
    pub root: PathBuf,
    pub script_path: PathBuf,
}

impl MockHarness {
    pub fn new(scenario: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("gemini-mock-{scenario}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let script_path = root.join("mock-gemini");
        write_script(&script_path, scenario);
        Self { root, script_path }
    }
}

impl Drop for MockHarness {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Write one of several canned protocol traces as an executable shell script.
///
/// Scenarios:
/// - `"basic"`      — initialize → ack; session/new → sessionId; session/prompt → stream + response
/// - `"two_sessions"` — initialize ack, two session/new calls, two interleaved prompts
/// - `"auth_fail"`  — initialize ack; session/new returns -32000
/// - `"permission"` — initialize ack; session/new; session/prompt issues an inbound permission request
/// - `"model_gate"` — session/new reports a real `availableModels` list and
///                    every `session/set_model` call appends its modelId to
///                    `<root>/set_model.log` so a test can assert whether the
///                    adapter forwarded it. session/prompt replies with a
///                    canned 500 "Requested entity was not found." iff
///                    `<root>/poison` exists — simulating the real Gemini
///                    backend behaviour after a bad `set_model` succeeds but
///                    the model id doesn't resolve.
fn write_script(path: &Path, scenario: &str) {
    let body = match scenario {
        "basic" => BASIC_SCRIPT,
        "two_sessions" => TWO_SESSIONS_SCRIPT,
        "auth_fail" => AUTH_FAIL_SCRIPT,
        "permission" => PERMISSION_SCRIPT,
        "exit_after_init" => EXIT_AFTER_INIT_SCRIPT,
        "model_gate" => MODEL_GATE_SCRIPT,
        other => panic!("unknown scenario {other}"),
    };
    // Inject the test-scoped root directory so scripts can write side-files
    // (e.g. `set_model.log` for `model_gate`) without polluting tmpdir roots.
    let root = path.parent().unwrap().to_string_lossy().to_string();
    let materialised = body.replace("{{ROOT}}", &root);
    fs::write(path, materialised).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

/// Basic scenario: initialize → one session → one prompt.
const BASIC_SCRIPT: &str = r#"#!/bin/bash
set -u
while IFS= read -r line; do
  # Parse id + method out of the incoming frame.
  id=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('id',''))")
  method=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('method',''))")
  case "$method" in
    initialize)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"protocolVersion\":1,\"authMethods\":[{\"id\":\"gemini-api-key\",\"name\":\"Gemini API key\"}],\"agentInfo\":{\"name\":\"mock\",\"version\":\"0.0.0\"},\"agentCapabilities\":{\"loadSession\":true,\"promptCapabilities\":{\"image\":false,\"audio\":false,\"embeddedContext\":false},\"mcpCapabilities\":{}}}}"
      ;;
    session/new)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"sessionId\":\"mock-session-1\",\"modes\":{\"availableModes\":[],\"currentModeId\":\"default\"},\"models\":{\"availableModels\":[],\"currentModelId\":\"mock\"}}}"
      ;;
    session/prompt)
      # Stream a delta + respond.
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"mock-session-1\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"hi\"}}}}"
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"stopReason\":\"end_turn\",\"_meta\":{\"quota\":{\"token_count\":{\"input_tokens\":3,\"output_tokens\":1}}}}}"
      ;;
    session/set_mode|session/set_model)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{}}"
      ;;
    session/cancel)
      # notification — no reply
      :
      ;;
    authenticate)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{}}"
      ;;
    *)
      if [ -n "$id" ]; then
        printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"error\":{\"code\":-32601,\"message\":\"method not found\"}}"
      fi
      ;;
  esac
done
"#;

/// Two concurrent sessions: first session/new returns session-A, second returns
/// session-B. Prompts stream different content.
const TWO_SESSIONS_SCRIPT: &str = r#"#!/bin/bash
set -u
session_count=0
while IFS= read -r line; do
  id=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('id',''))")
  method=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('method',''))")
  sid=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print((d.get('params') or {}).get('sessionId',''))")
  case "$method" in
    initialize)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"protocolVersion\":1,\"authMethods\":[],\"agentInfo\":{\"name\":\"mock\",\"version\":\"0\"},\"agentCapabilities\":{\"loadSession\":true,\"promptCapabilities\":{\"image\":false,\"audio\":false,\"embeddedContext\":false},\"mcpCapabilities\":{}}}}"
      ;;
    session/new)
      session_count=$((session_count + 1))
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"sessionId\":\"session-${session_count}\",\"modes\":{\"availableModes\":[],\"currentModeId\":\"default\"},\"models\":{\"availableModels\":[],\"currentModelId\":\"mock\"}}}"
      ;;
    session/prompt)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"${sid}\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"from-${sid}\"}}}}"
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"stopReason\":\"end_turn\",\"_meta\":{\"quota\":{\"token_count\":{\"input_tokens\":1,\"output_tokens\":1}}}}}"
      ;;
    session/cancel)
      :
      ;;
    *)
      if [ -n "$id" ]; then
        printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{}}"
      fi
      ;;
  esac
done
"#;

/// session/new returns -32000. Authenticate also fails.
const AUTH_FAIL_SCRIPT: &str = r#"#!/bin/bash
set -u
while IFS= read -r line; do
  id=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('id',''))")
  method=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('method',''))")
  case "$method" in
    initialize)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"protocolVersion\":1,\"authMethods\":[],\"agentInfo\":{\"name\":\"mock\",\"version\":\"0\"},\"agentCapabilities\":{\"loadSession\":true,\"promptCapabilities\":{\"image\":false,\"audio\":false,\"embeddedContext\":false},\"mcpCapabilities\":{}}}}"
      ;;
    session/new)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"error\":{\"code\":-32000,\"message\":\"Authentication required.\"}}"
      ;;
    authenticate)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{}}"
      ;;
    *)
      if [ -n "$id" ]; then
        printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"error\":{\"code\":-32601,\"message\":\"nope\"}}"
      fi
      ;;
  esac
done
"#;

/// Permission request — server issues a session/request_permission request
/// during a prompt.
const PERMISSION_SCRIPT: &str = r#"#!/bin/bash
set -u
server_req_id=100
perm_pending=0
while IFS= read -r line; do
  id=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('id',''))")
  method=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('method',''))")
  has_result=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print('1' if 'result' in d else '')")
  case "$method" in
    initialize)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"protocolVersion\":1,\"authMethods\":[],\"agentInfo\":{\"name\":\"mock\",\"version\":\"0\"},\"agentCapabilities\":{\"loadSession\":true,\"promptCapabilities\":{\"image\":false,\"audio\":false,\"embeddedContext\":false},\"mcpCapabilities\":{}}}}"
      ;;
    session/new)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"sessionId\":\"mock-session-1\",\"modes\":{\"availableModes\":[],\"currentModeId\":\"default\"},\"models\":{\"availableModels\":[],\"currentModelId\":\"mock\"}}}"
      ;;
    session/prompt)
      # Issue an inbound permission request.
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${server_req_id},\"method\":\"session/request_permission\",\"params\":{\"sessionId\":\"mock-session-1\",\"toolCall\":{\"toolCallId\":\"t1\",\"title\":\"Bash\"},\"options\":[{\"optionId\":\"proceed_once\",\"name\":\"Allow\",\"kind\":\"allow_once\"},{\"optionId\":\"reject_once\",\"name\":\"Deny\",\"kind\":\"reject_once\"}]}}"
      perm_pending=1
      prompt_id="${id}"
      # Wait for client response by reading the next line — but we need to
      # multiplex; the bash while-loop already does this. We finish the prompt
      # only after we've seen the client's outcome reply. We set a guard and
      # continue the outer loop.
      ;;
    "")
      # Likely a response (has `result`). If we were waiting on permission,
      # finalize the prompt response.
      if [ "$has_result" = "1" ] && [ -n "$id" ] && [ "$perm_pending" = "1" ]; then
        # Parse outcome
        outcome=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());o=d.get('result',{}).get('outcome',{});print(o.get('outcome',''))")
        printf '%s\n' "{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"mock-session-1\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"perm=${outcome}\"}}}}"
        printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${prompt_id},\"result\":{\"stopReason\":\"end_turn\",\"_meta\":{\"quota\":{\"token_count\":{\"input_tokens\":1,\"output_tokens\":1}}}}}"
        perm_pending=0
      fi
      ;;
    *)
      if [ -n "$id" ]; then
        printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{}}"
      fi
      ;;
  esac
done
"#;

/// Respond to initialize then exit, simulating subprocess death mid-flight.
const EXIT_AFTER_INIT_SCRIPT: &str = r#"#!/bin/bash
set -u
IFS= read -r line
id=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('id',''))")
printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"protocolVersion\":1,\"authMethods\":[],\"agentInfo\":{\"name\":\"mock\",\"version\":\"0\"},\"agentCapabilities\":{\"loadSession\":true,\"promptCapabilities\":{\"image\":false,\"audio\":false,\"embeddedContext\":false},\"mcpCapabilities\":{}}}}"
exit 0
"#;

/// Model-gate scenario: session/new returns a non-empty availableModels list.
/// Every session/set_model call appends its modelId + newline to
/// `$GEMINI_MOCK_ROOT/set_model.log` so the test can assert exactly what was
/// forwarded. session/prompt behaves like the real backend: if a poison file
/// exists (`$GEMINI_MOCK_ROOT/poison`) it errors with `[500] Requested entity
/// was not found.`, otherwise it streams + ends normally.
const MODEL_GATE_SCRIPT: &str = r#"#!/bin/bash
set -u
LOG="{{ROOT}}/set_model.log"
POISON="{{ROOT}}/poison"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('id',''))")
  method=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('method',''))")
  case "$method" in
    initialize)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"protocolVersion\":1,\"authMethods\":[],\"agentInfo\":{\"name\":\"mock\",\"version\":\"0\"},\"agentCapabilities\":{\"loadSession\":true,\"promptCapabilities\":{\"image\":false,\"audio\":false,\"embeddedContext\":false},\"mcpCapabilities\":{}}}}"
      ;;
    session/new)
      # Report a real availableModels list — matching the live 0.38.x shape.
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"sessionId\":\"mock-session-gate\",\"modes\":{\"availableModes\":[],\"currentModeId\":\"default\"},\"models\":{\"availableModels\":[{\"modelId\":\"auto-gemini-3\",\"name\":\"Auto (Gemini 3)\"},{\"modelId\":\"gemini-2.5-pro\",\"name\":\"gemini-2.5-pro\"},{\"modelId\":\"gemini-2.5-flash\",\"name\":\"gemini-2.5-flash\"}],\"currentModelId\":\"auto-gemini-3\"}}}"
      ;;
    session/set_model)
      mid=$(printf '%s' "$line" | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print((d.get('params') or {}).get('modelId',''))")
      printf '%s\n' "${mid}" >> "$LOG"
      # Any modelId that isn't one of the advertised ones "poisons" the
      # session — next session/prompt will 500.
      case "$mid" in
        auto-gemini-3|gemini-2.5-pro|gemini-2.5-flash)
          :
          ;;
        *)
          : > "$POISON"
          ;;
      esac
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{}}"
      ;;
    session/prompt)
      if [ -e "$POISON" ]; then
        printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"error\":{\"code\":500,\"message\":\"Requested entity was not found.\"}}"
      else
        printf '%s\n' "{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"mock-session-gate\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"ok\"}}}}"
        printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"stopReason\":\"end_turn\",\"_meta\":{\"quota\":{\"token_count\":{\"input_tokens\":1,\"output_tokens\":1}}}}}"
      fi
      ;;
    session/cancel)
      :
      ;;
    *)
      if [ -n "$id" ]; then
        printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{}}"
      fi
      ;;
  esac
done
"#;
