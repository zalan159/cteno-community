//! Line-delimited JSON protocol between the host process and cteno-agent.
//!
//! The protocol is intentionally narrow: one inbound enum for all host → agent
//! messages, one outbound enum for agent → host messages. Each message is a
//! single JSON object serialized on a single line (newline delimited). Unknown
//! fields are ignored to allow non-breaking evolution.
//!
//! Batch 2 extends the MVP protocol with:
//!
//! - Multi-session: every message carries `session_id`; a single agent process
//!   can manage multiple concurrent sessions.
//! - Permission closure: `permission_request` (out) / `permission_response`
//!   (in) form a pending-request/response loop keyed by `request_id`.
//! - Host tool injection: `tool_inject` (in) registers a host-owned tool (such
//!   as `dispatch_task` / `ask_persona`) whose execution is delegated back to
//!   the host via `tool_execution_request` (out) → `tool_execution_response`
//!   (in), again keyed by `request_id`.

#![allow(dead_code)] // Several Inbound fields are informational (e.g. `reason`)
                     // and captured by the protocol even when not consumed.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextUsage {
    pub total_tokens: u32,
    pub max_tokens: u32,
    pub raw_max_tokens: u32,
    pub auto_compact_token_limit: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpDelivery {
    Transient,
    Persisted,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    Text,
    File,
    Other,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Attachment {
    pub kind: AttachmentKind,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub data: Option<String>,
}

/// Metadata for a host-owned tool injected into the session's tool surface.
/// The agent registers an `InjectedToolExecutor` under `name` that, when
/// invoked, emits an outbound `tool_execution_request` and awaits a matching
/// `tool_execution_response`.
#[derive(Debug, Clone, Deserialize)]
pub struct InjectedTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// JSON Schema for the tool input. Forwarded verbatim to the LLM.
    #[serde(default)]
    pub input_schema: Value,
}

/// Messages the host writes to cteno-agent's stdin.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Inbound {
    /// Initialise a session. Sent once per session before any `user_message`.
    Init {
        session_id: String,
        #[serde(default)]
        workdir: Option<String>,
        /// Additional directories exposed to the session sandbox.
        #[serde(default)]
        additional_directories: Vec<String>,
        /// Optional free-form agent configuration (model, temperature, ...).
        /// The runner reads a small subset; unknown keys are ignored.
        #[serde(default)]
        agent_config: Value,
        /// Optional system prompt override. If None, a minimal default is used.
        #[serde(default)]
        system_prompt: Option<String>,

        /// Access token for Happy Server RPC / Socket.IO. Set when user is
        /// logged in; `None` for anonymous / local-only sessions.
        #[serde(default)]
        auth_token: Option<String>,

        /// Owning user id (Happy Server account).
        #[serde(default)]
        user_id: Option<String>,

        /// This machine's id. May be set even when auth_token is None.
        #[serde(default)]
        machine_id: Option<String>,
    },
    /// Send a user turn into the session.
    UserMessage {
        session_id: String,
        content: String,
        #[serde(default)]
        task_id: Option<String>,
        #[serde(default)]
        attachments: Vec<Attachment>,
    },
    /// Best-effort abort of the current turn.
    Abort {
        session_id: String,
        #[serde(default)]
        reason: Option<String>,
    },
    /// Close a session and release session-scoped resources (MCP registries,
    /// pending turn handle, etc.) without necessarily shutting down the stdio
    /// process.
    CloseSession { session_id: String },
    /// Update the session's active model selection for subsequent turns.
    SetModel {
        session_id: String,
        model: String,
        #[serde(default)]
        effort: Option<String>,
    },
    /// Update the session's active permission mode for subsequent turns.
    SetPermissionMode { session_id: String, mode: String },
    /// Reply to a pending `permission_request`.
    PermissionResponse {
        session_id: String,
        request_id: String,
        /// `allow`, `deny`, or `abort`.
        decision: String,
        #[serde(default)]
        reason: Option<String>,
    },
    /// Register a host-owned tool into the session's tool surface. Idempotent:
    /// injecting the same name twice replaces the previous definition.
    ToolInject {
        session_id: String,
        tool: InjectedTool,
    },
    /// Reply to a pending `tool_execution_request` emitted by the agent for a
    /// previously-injected host-owned tool.
    ToolExecutionResponse {
        session_id: String,
        request_id: String,
        /// Whether the host executed the tool successfully.
        ok: bool,
        /// On success, the tool output (string). On failure, unused.
        #[serde(default)]
        output: Option<String>,
        /// On failure, a short error description.
        #[serde(default)]
        error: Option<String>,
    },
    /// Reply to a pending `host_call_request` emitted by the agent for a
    /// generic runtime hook invocation. `output` carries the method return
    /// value as arbitrary JSON (`null` when the hook returns `()`).
    HostCallResponse {
        session_id: String,
        request_id: String,
        /// Whether the host executed the hook method successfully.
        ok: bool,
        /// On success, the method return value (arbitrary JSON). On failure, unused.
        #[serde(default)]
        output: Option<Value>,
        /// On failure, a short error description.
        #[serde(default)]
        error: Option<String>,
    },
    /// Host has rotated the access token. Applies globally to all sessions
    /// managed by this agent process — they share a single credentials slot.
    /// Emitted either proactively on host-side refresh, or in response to a
    /// `401` from the agent's Happy Server calls.
    TokenRefreshed { access_token: String },
    /// Unknown inbound message — forward-compat bucket. Dropped with a warning
    /// by the dispatcher so later protocol additions do not hard-fail older
    /// agent builds.
    #[serde(other)]
    Unknown,
}

/// Messages cteno-agent writes to stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outbound {
    /// Session initialised and ready for user messages.
    Ready { session_id: String },

    /// Runtime-native ACP payload. The stdio boundary transports it without
    /// translating into Cteno-specific semantic frames.
    Acp {
        session_id: String,
        delivery: AcpDelivery,
        data: Value,
    },

    /// Request the host to approve/deny a tool call. Host must reply with
    /// `permission_response` carrying the matching `request_id`.
    PermissionRequest {
        session_id: String,
        request_id: String,
        tool_name: String,
        tool_input: Value,
    },

    /// Request the host to execute a previously-injected host-owned tool.
    /// Host must reply with `tool_execution_response` carrying the matching
    /// `request_id`.
    ToolExecutionRequest {
        session_id: String,
        request_id: String,
        tool_name: String,
        tool_input: Value,
    },

    /// The current turn has finished.
    TurnComplete {
        session_id: String,
        final_text: String,
        iteration_count: usize,
        #[serde(default)]
        usage: TurnUsage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_usage: Option<ContextUsage>,
    },

    /// Fatal or non-fatal error surface.
    Error { session_id: String, message: String },

    /// Request the host to execute a runtime hook method on behalf of an
    /// in-agent `HostCallDispatcher` proxy. Host must reply with
    /// `host_call_response` carrying the matching `request_id`.
    ///
    /// - `hook_name` selects the logical hook family (e.g. `agent_owner`,
    ///   `skillhub`, `local_notification`).
    /// - `method` selects the method within that family (e.g. `session_owner`,
    ///   `list_skills`, `send_local_notification`).
    /// - `params` is arbitrary JSON payload defined per method; the host and
    ///   agent adapters agree on the shape.
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
        /// Synthetic user-message text that triggered this turn (e.g. the
        /// concatenated `[Task Complete] X\n\n result` blocks drained from
        /// the runtime queue when subagents completed). The host renders it
        /// as a user-bubble in the persona transcript so the user can see
        /// what fed into this turn — without it, the autonomous turn looks
        /// like the agent talking to itself.
        #[serde(default)]
        synthetic_user_message: Option<String>,
    },

    /// SubAgent lifecycle transition emitted by the agent's
    /// `SubAgentManager`. Routed by the cteno adapter's dispatcher to the
    /// host's `SessionEventSink::on_subagent_lifecycle`, which mirrors
    /// the SubAgent state into a desktop registry so the
    /// `BackgroundRunsModal` can render live progress without polling
    /// any RPC.
    SubAgentLifecycle {
        /// Parent persona session that owns this subagent.
        session_id: String,
        event: SubAgentLifecycleEvent,
    },
}

/// Wire payload for `Outbound::SubAgentLifecycle`. Mirrors the runtime's
/// `SubAgentLifecycleEventDto` (intentionally kept as a separate type so
/// the wire format can evolve without dragging the runtime DTO with it).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubAgentLifecycleEvent {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_init_with_auth_fields_round_trip() {
        let json = serde_json::json!({
            "type": "init",
            "session_id": "s-1",
            "additional_directories": ["/tmp/extra"],
            "auth_token": "acc-tok-xyz",
            "user_id": "u-42",
            "machine_id": "m-abc"
        });
        let parsed: Inbound = serde_json::from_value(json).expect("parse");
        match parsed {
            Inbound::Init {
                session_id,
                auth_token,
                user_id,
                machine_id,
                additional_directories,
                ..
            } => {
                assert_eq!(session_id, "s-1");
                assert_eq!(auth_token.as_deref(), Some("acc-tok-xyz"));
                assert_eq!(user_id.as_deref(), Some("u-42"));
                assert_eq!(machine_id.as_deref(), Some("m-abc"));
                assert_eq!(additional_directories, vec!["/tmp/extra"]);
            }
            _ => panic!("expected Init variant"),
        }
    }

    #[test]
    fn outbound_acp_round_trip() {
        let msg = Outbound::Acp {
            session_id: "s-acp".to_string(),
            delivery: AcpDelivery::Persisted,
            data: serde_json::json!({
                "type": "tool-call",
                "callId": "call-1",
                "name": "read",
                "input": {"path": "README.md"},
                "futureField": true
            }),
        };
        let json = serde_json::to_value(&msg).expect("serialize");
        assert_eq!(json["type"], "acp");
        assert_eq!(json["delivery"], "persisted");
        assert_eq!(json["data"]["futureField"], true);
    }

    #[test]
    fn outbound_subagent_lifecycle_round_trips() {
        let cases = vec![
            Outbound::SubAgentLifecycle {
                session_id: "p1".into(),
                event: SubAgentLifecycleEvent::Spawned {
                    subagent_id: "s1".into(),
                    agent_id: "worker".into(),
                    task: "do thing".into(),
                    label: Some("step 1".into()),
                    created_at_ms: 12345,
                },
            },
            Outbound::SubAgentLifecycle {
                session_id: "p1".into(),
                event: SubAgentLifecycleEvent::Started {
                    subagent_id: "s1".into(),
                    started_at_ms: 12346,
                },
            },
            Outbound::SubAgentLifecycle {
                session_id: "p1".into(),
                event: SubAgentLifecycleEvent::Completed {
                    subagent_id: "s1".into(),
                    result: Some("ok".into()),
                    completed_at_ms: 12500,
                },
            },
            Outbound::SubAgentLifecycle {
                session_id: "p1".into(),
                event: SubAgentLifecycleEvent::Failed {
                    subagent_id: "s2".into(),
                    error: "boom".into(),
                    completed_at_ms: 12500,
                },
            },
        ];
        for msg in cases {
            let json = serde_json::to_value(&msg).expect("serialize");
            let parsed: Outbound = serde_json::from_value(json).expect("deserialize");
            // Spot check a couple of fields per case via Debug
            assert!(
                format!("{:?}", parsed).contains("SubAgentLifecycle"),
                "expected SubAgentLifecycle variant, got {:?}",
                parsed
            );
        }
    }

    #[test]
    fn outbound_autonomous_turn_start_round_trip() {
        let msg = Outbound::AutonomousTurnStart {
            session_id: "sess-1".to_string(),
            reason: Some("subagent_handoff".to_string()),
            synthetic_user_message: Some("[Task Complete] greeting\n\n你好".to_string()),
        };
        let json = serde_json::to_value(&msg).expect("serialize");
        let parsed: Outbound = serde_json::from_value(json).expect("deserialize");
        match parsed {
            Outbound::AutonomousTurnStart {
                session_id,
                reason,
                synthetic_user_message,
            } => {
                assert_eq!(session_id, "sess-1");
                assert_eq!(reason.as_deref(), Some("subagent_handoff"));
                assert_eq!(
                    synthetic_user_message.as_deref(),
                    Some("[Task Complete] greeting\n\n你好")
                );
            }
            _ => panic!("expected AutonomousTurnStart variant"),
        }
    }

    #[test]
    fn inbound_close_session_round_trip() {
        let msg: Inbound =
            serde_json::from_value(serde_json::json!({"type":"close_session","session_id":"s-9"}))
                .expect("parse");
        match msg {
            Inbound::CloseSession { session_id } => assert_eq!(session_id, "s-9"),
            other => panic!("expected CloseSession, got {other:?}"),
        }
    }

    #[test]
    fn inbound_user_message_carries_task_id() {
        let msg: Inbound = serde_json::from_value(serde_json::json!({
            "type": "user_message",
            "session_id": "s-task",
            "content": "hello",
            "task_id": "task-1"
        }))
        .expect("parse");
        match msg {
            Inbound::UserMessage {
                session_id,
                content,
                task_id,
                attachments,
            } => {
                assert_eq!(session_id, "s-task");
                assert_eq!(content, "hello");
                assert_eq!(task_id.as_deref(), Some("task-1"));
                assert!(attachments.is_empty());
            }
            other => panic!("expected UserMessage, got {other:?}"),
        }
    }

    #[test]
    fn inbound_init_without_auth_fields_is_backward_compatible() {
        let json = serde_json::json!({
            "type": "init",
            "session_id": "s-2"
        });
        let parsed: Inbound = serde_json::from_value(json).expect("parse");
        match parsed {
            Inbound::Init {
                session_id,
                auth_token,
                user_id,
                machine_id,
                ..
            } => {
                assert_eq!(session_id, "s-2");
                assert!(auth_token.is_none());
                assert!(user_id.is_none());
                assert!(machine_id.is_none());
            }
            _ => panic!("expected Init variant"),
        }
    }

    #[test]
    fn inbound_token_refreshed_round_trip() {
        let json = serde_json::json!({
            "type": "token_refreshed",
            "access_token": "rotated-tok"
        });
        let parsed: Inbound = serde_json::from_value(json).expect("parse");
        match parsed {
            Inbound::TokenRefreshed { access_token } => {
                assert_eq!(access_token, "rotated-tok");
            }
            _ => panic!("expected TokenRefreshed variant"),
        }
    }

    #[test]
    fn inbound_set_model_accepts_runtime_control_shape() {
        let json = serde_json::json!({
            "type": "set_model",
            "session_id": "s-3",
            "model": "gpt-5.1",
            "effort": "high",
            "ignored_future_field": true
        });
        let parsed: Inbound = serde_json::from_value(json).expect("parse");
        match parsed {
            Inbound::SetModel {
                session_id,
                model,
                effort,
            } => {
                assert_eq!(session_id, "s-3");
                assert_eq!(model, "gpt-5.1");
                assert_eq!(effort.as_deref(), Some("high"));
            }
            _ => panic!("expected SetModel variant"),
        }
    }

    #[test]
    fn inbound_set_permission_mode_round_trip() {
        let json = serde_json::json!({
            "type": "set_permission_mode",
            "session_id": "s-4",
            "mode": "accept_edits"
        });
        let parsed: Inbound = serde_json::from_value(json).expect("parse");
        match parsed {
            Inbound::SetPermissionMode { session_id, mode } => {
                assert_eq!(session_id, "s-4");
                assert_eq!(mode, "accept_edits");
            }
            _ => panic!("expected SetPermissionMode variant"),
        }
    }

    #[test]
    fn inbound_abort_reason_is_optional() {
        let legacy: Inbound = serde_json::from_value(serde_json::json!({
            "type": "abort",
            "session_id": "s-legacy"
        }))
        .expect("parse legacy abort");
        match legacy {
            Inbound::Abort { session_id, reason } => {
                assert_eq!(session_id, "s-legacy");
                assert!(reason.is_none());
            }
            _ => panic!("expected Abort variant"),
        }

        let with_reason: Inbound = serde_json::from_value(serde_json::json!({
            "type": "abort",
            "session_id": "s-timeout",
            "reason": "timed out"
        }))
        .expect("parse abort with reason");
        match with_reason {
            Inbound::Abort { session_id, reason } => {
                assert_eq!(session_id, "s-timeout");
                assert_eq!(reason.as_deref(), Some("timed out"));
            }
            _ => panic!("expected Abort variant"),
        }
    }

    #[test]
    fn inbound_unknown_type_is_tolerated() {
        let json = serde_json::json!({
            "type": "future_protocol_message",
            "some_field": "some_value"
        });
        let parsed: Inbound = serde_json::from_value(json).expect("parse");
        match parsed {
            Inbound::Unknown => {}
            _ => panic!("expected Unknown variant for future message type"),
        }
    }
}
