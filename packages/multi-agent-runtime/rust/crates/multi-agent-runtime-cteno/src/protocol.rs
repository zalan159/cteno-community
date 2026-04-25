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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpDeliveryWire {
    Transient,
    Persisted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKindWire {
    Image,
    Text,
    File,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachmentWire {
    pub kind: AttachmentKindWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        additional_directories: Vec<String>,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<AttachmentWire>,
    },
    Abort {
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    CloseSession {
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

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ContextUsageWire {
    #[serde(default)]
    pub total_tokens: u32,
    #[serde(default)]
    pub max_tokens: u32,
    #[serde(default)]
    pub raw_max_tokens: u32,
    #[serde(default)]
    pub auto_compact_token_limit: u32,
}

/// Messages cteno-agent writes to stdout.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outbound {
    Ready {
        session_id: String,
    },
    Acp {
        session_id: String,
        delivery: AcpDeliveryWire,
        data: Value,
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
        #[serde(default)]
        context_usage: Option<ContextUsageWire>,
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
    AutonomousTurnStart {
        session_id: String,
        #[serde(default)]
        reason: Option<String>,
        /// Synthetic user-message text the agent will feed into the new turn
        /// (e.g. concatenated `[Task Complete] X\n\n result` blocks for
        /// queued subagent handoffs). Mirrors the field added on the stdio
        /// crate's wire enum; carried through the dispatcher into the
        /// host-side autonomous_turn_handler so the host can render it in
        /// the persona transcript before the turn's assistant frames begin.
        #[serde(default)]
        synthetic_user_message: Option<String>,
    },
    /// SubAgent lifecycle transition emitted by the agent's
    /// `SubAgentManager`. Mirror of the stdio crate's variant; routed by
    /// the dispatcher to `SessionEventSink::on_subagent_lifecycle` so the
    /// host can update its SubAgent registry mirror and trigger a UI
    /// refresh (BackgroundRunsModal).
    SubAgentLifecycle {
        session_id: String,
        event: SubAgentLifecycleEventWire,
    },
}

/// Wire representation of subagent lifecycle events. Mirror of the stdio
/// crate's `SubAgentLifecycleEvent` — kept here so the adapter doesn't
/// need to depend on the stdio crate just for the types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubAgentLifecycleEventWire {
    Spawned {
        subagent_id: String,
        agent_id: String,
        task: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        created_at_ms: i64,
    },
    Started {
        subagent_id: String,
        started_at_ms: i64,
    },
    Updated {
        subagent_id: String,
        iteration_count: u32,
    },
    Completed {
        subagent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        completed_at_ms: i64,
    },
    Failed {
        subagent_id: String,
        error: String,
        completed_at_ms: i64,
    },
    Stopped {
        subagent_id: String,
        completed_at_ms: i64,
    },
}
