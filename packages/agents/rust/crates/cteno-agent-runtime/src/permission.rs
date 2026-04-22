//! Permission model shared across the agent runtime.
//!
//! Pure data types that describe the outcome of a permission check for a tool
//! call.  The host (app crate) is responsible for producing these values — this
//! module only owns the schema so runtime modules (ReAct loop, tool executors)
//! can reason about the result without depending on `happy_client`.

use serde::Deserialize;

/// Permission mode controlling how tool calls are approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    /// All mutating tools need confirmation.
    Default,
    /// File/edit auto-approved, shell still needs confirmation.
    AcceptEdits,
    /// All tools auto-approved.
    BypassPermissions,
    /// Read-only mode: reject all mutations.
    Plan,
}

/// Decision from user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Approved,
    ApprovedForSession,
    Denied,
    Abort,
}

/// Result of a permission check.
#[derive(Debug, Clone)]
pub enum PermissionCheckResult {
    Allowed,
    Denied(String),
    Aborted,
}

/// RPC response from the frontend for a permission request.
/// Matches happy-cli protocol: `{ id, approved, decision?, mode?, allowTools?, vendorOption? }`.
///
/// `vendor_option` is used by vendors like gemini whose server dictates a
/// list of option ids (`proceed_once / proceed_always / cancel / ...`); the
/// frontend surfaces them as buttons and echoes the chosen id back here.
/// When set, consumers should prefer it over the generic `approved` /
/// `decision` fields.
#[derive(Debug, Clone, Deserialize)]
pub struct PermissionRpcResponse {
    pub id: String,
    pub approved: bool,
    pub decision: Option<String>,
    pub mode: Option<String>,
    #[serde(rename = "allowTools")]
    pub allow_tools: Option<Vec<String>>,
    #[serde(rename = "vendorOption", default)]
    pub vendor_option: Option<String>,
}
