//! Pending-request maps shared by the protocol.
//!
//! Two call patterns cross the stdio boundary with a request/response shape:
//!
//! 1. Permission checks: the ReAct loop asks the host to approve a tool. The
//!    agent emits a `permission_request` and awaits a `permission_response`
//!    matching `request_id`.
//! 2. Host-owned tool execution: an injected tool's `execute` method emits a
//!    `tool_execution_request` and awaits a `tool_execution_response`.
//!
//! Each map keys a pending `oneshot::Sender` by a globally-unique request id.
//! Request ids include a short type prefix (`perm_*`, `texec_*`) so stray
//! responses can be diagnosed from logs.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{oneshot, Mutex};

use cteno_agent_runtime::permission::PermissionDecision;

/// Result delivered by the host for a `tool_execution_request`. `Ok(output)`
/// on success, `Err(error)` on rejection or execution failure.
pub type ToolExecResult = Result<String, String>;

/// Shared pending-permission map. Keyed by `request_id`.
pub type PendingPermissions =
    Arc<Mutex<HashMap<String, oneshot::Sender<PermissionDecision>>>>;

/// Shared pending-tool-execution map. Keyed by `request_id`.
pub type PendingToolExecs =
    Arc<Mutex<HashMap<String, oneshot::Sender<ToolExecResult>>>>;

/// Construct a fresh, empty pending-permissions map.
pub fn new_pending_permissions() -> PendingPermissions {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Construct a fresh, empty pending-tool-execs map.
pub fn new_pending_tool_execs() -> PendingToolExecs {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Map a stringy inbound decision (`allow` / `deny` / `abort` / legacy
/// `approved` / `denied`) to the runtime's `PermissionDecision`. Unknown
/// values are treated as `Denied`.
pub fn parse_decision(s: &str) -> PermissionDecision {
    match s.to_ascii_lowercase().as_str() {
        "allow" | "approve" | "approved" => PermissionDecision::Approved,
        "allow_for_session" | "approved_for_session" => {
            PermissionDecision::ApprovedForSession
        }
        "abort" => PermissionDecision::Abort,
        _ => PermissionDecision::Denied,
    }
}

/// Generate a globally-unique permission request id.
pub fn new_permission_id() -> String {
    format!("perm_{}", uuid::Uuid::new_v4())
}

/// Generate a globally-unique tool-execution request id.
pub fn new_tool_exec_id() -> String {
    format!("texec_{}", uuid::Uuid::new_v4())
}

// ---------------------------------------------------------------------------
// Generic host-call pending map (host_call_request / host_call_response)
// ---------------------------------------------------------------------------

/// Result delivered by the host for a `host_call_request`. `Ok(value)` on
/// success (arbitrary JSON, `Value::Null` when the hook returns `()`),
/// `Err(error)` on rejection or execution failure.
pub type HostCallResult = Result<Value, String>;

/// Shared pending-host-call map. Keyed by `request_id`.
pub type PendingHostCalls =
    Arc<Mutex<HashMap<String, oneshot::Sender<HostCallResult>>>>;

/// Construct a fresh, empty pending-host-calls map.
pub fn new_pending_host_calls() -> PendingHostCalls {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Generate a globally-unique host-call request id.
pub fn new_host_call_id() -> String {
    format!("hcall_{}", uuid::Uuid::new_v4())
}
