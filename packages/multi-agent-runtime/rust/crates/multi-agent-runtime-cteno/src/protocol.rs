//! Local copy of the cteno-agent stdio protocol DTOs.
//!
//! MUST stay byte-identical to
//! `packages/agents/rust/crates/cteno-agent-stdio/src/protocol.rs`. We can't
//! depend on that crate directly because it is a binary crate (`[[bin]]`
//! only) whose protocol types are not exposed as a library. Divergence
//! surfaces as a JSON-decode error at the adapter boundary.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Host-owned tool metadata injected into a session's tool surface.
#[derive(Debug, Clone, Serialize)]
pub struct InjectedToolWire {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
}

/// Messages the host writes to cteno-agent's stdin. Matches
/// `cteno-agent-stdio::protocol::Inbound`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Inbound {
    Init {
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        workdir: Option<String>,
        #[serde(default)]
        agent_config: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        system_prompt: Option<String>,
        /// Cteno 2.0 unified accessToken (30min TTL). `None` for anonymous /
        /// local-only sessions.
        #[serde(skip_serializing_if = "Option::is_none")]
        auth_token: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        user_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        machine_id: Option<String>,
    },
    UserMessage {
        session_id: String,
        content: String,
    },
    Abort {
        session_id: String,
    },
    SetModel {
        session_id: String,
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
    SetPermissionMode {
        session_id: String,
        mode: String,
    },
    PermissionResponse {
        session_id: String,
        request_id: String,
        decision: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    ToolInject {
        session_id: String,
        tool: InjectedToolWire,
    },
    #[allow(dead_code)]
    ToolExecutionResponse {
        session_id: String,
        request_id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    #[allow(dead_code)]
    HostCallResponse {
        session_id: String,
        request_id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Host-rotated access token; applies to the whole subprocess.
    TokenRefreshed {
        access_token: String,
    },
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UsageWire {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
}

/// Messages cteno-agent writes to stdout.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outbound {
    Ready {
        session_id: String,
    },
    Delta {
        session_id: String,
        kind: String,
        content: String,
    },
    ToolUse {
        session_id: String,
        tool_use_id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        session_id: String,
        tool_use_id: String,
        output: String,
        #[serde(default)]
        is_error: bool,
    },
    PermissionRequest {
        session_id: String,
        request_id: String,
        tool_name: String,
        tool_input: Value,
    },
    ToolExecutionRequest {
        session_id: String,
        request_id: String,
        tool_name: String,
        tool_input: Value,
    },
    TurnComplete {
        session_id: String,
        final_text: String,
        iteration_count: usize,
        #[serde(default)]
        usage: UsageWire,
    },
    Error {
        session_id: String,
        message: String,
    },
    HostCallRequest {
        session_id: String,
        request_id: String,
        hook_name: String,
        method: String,
        params: Value,
    },
}
