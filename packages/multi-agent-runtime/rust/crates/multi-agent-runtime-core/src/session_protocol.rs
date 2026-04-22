//! Unified session event protocol for the Cteno multi-agent runtime.
//!
//! All vendor-specific events (Cteno, Claude, Codex, Gemini, etc.) are
//! normalised into [`SessionEvent`] variants and wrapped in a
//! [`SessionEnvelope`] before being forwarded to the frontend or persisted.
//!
//! Design goals:
//! - **8 event variants** — keeps the wire format simple and extensible.
//! - **Vendor-agnostic** — no vendor-specific fields leak into the protocol.
//! - **Serde-friendly** — internally tagged (`"t"`) for compact JSON.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

/// Unified session event envelope — vendor-agnostic wire format.
///
/// Every event produced during a session (text deltas, tool calls, turn
/// boundaries, subagent lifecycle, ...) is wrapped in this envelope before
/// leaving the runtime layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEnvelope {
    /// Unique envelope ID (UUID v4).
    pub id: String,
    /// Timestamp in milliseconds since the Unix epoch.
    pub time: u64,
    /// Source of the event.
    pub role: SessionRole,
    /// Turn ID — groups all events from one agent iteration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    /// Subagent ID — for hierarchical agent nesting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<String>,
    /// The actual event payload.
    pub ev: SessionEvent,
}

impl SessionEnvelope {
    /// Create a new envelope with an auto-generated ID and the current
    /// wall-clock timestamp.
    pub fn new(role: SessionRole, ev: SessionEvent) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self {
            id: Uuid::new_v4().to_string(),
            time: now,
            role,
            turn: None,
            subagent: None,
            ev,
        }
    }

    /// Builder-style setter for `turn`.
    pub fn with_turn(mut self, turn: impl Into<String>) -> Self {
        self.turn = Some(turn.into());
        self
    }

    /// Builder-style setter for `subagent`.
    pub fn with_subagent(mut self, subagent: impl Into<String>) -> Self {
        self.subagent = Some(subagent.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Role
// ---------------------------------------------------------------------------

/// Source of a session event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionRole {
    User,
    Agent,
}

// ---------------------------------------------------------------------------
// Event (discriminated union)
// ---------------------------------------------------------------------------

/// Discriminated union of all possible session events.
///
/// Only 8 core variants — keeps the protocol simple while covering every
/// interaction pattern observed across Cteno, Claude CLI, Codex CLI, and
/// Gemini adapters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum SessionEvent {
    /// Text output (assistant response or thinking).
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "is_false")]
        thinking: bool,
    },

    /// System / service message (not part of the LLM conversation).
    #[serde(rename = "service")]
    Service { text: String },

    /// A tool call has started.
    #[serde(rename = "tool-call-start")]
    ToolCallStart {
        /// Correlation ID for this tool call (matches [`ToolCallEnd::call`]).
        call: String,
        /// Machine-readable tool name.
        name: String,
        /// Human-readable title (may be empty).
        #[serde(default)]
        title: String,
        /// Human-readable description (may be empty).
        #[serde(default)]
        description: String,
        /// Tool arguments (opaque JSON).
        #[serde(default)]
        args: serde_json::Value,
    },

    /// A tool call has ended.
    #[serde(rename = "tool-call-end")]
    ToolCallEnd {
        /// Correlation ID (matches [`ToolCallStart::call`]).
        call: String,
        /// Tool result text (if any).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        /// Whether the tool call errored.
        #[serde(default, skip_serializing_if = "is_false")]
        is_error: bool,
    },

    /// The agent has started a new turn (iteration).
    #[serde(rename = "turn-start")]
    TurnStart,

    /// The agent has finished a turn.
    #[serde(rename = "turn-end")]
    TurnEnd { status: TurnEndStatus },

    /// A subagent has been spawned.
    #[serde(rename = "start")]
    SubagentStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },

    /// A subagent has stopped.
    #[serde(rename = "stop")]
    SubagentStop,

    /// A file attachment.
    #[serde(rename = "file")]
    File {
        /// Opaque reference (path, URL, blob ID, ...).
        #[serde(rename = "ref")]
        file_ref: String,
        /// Display name.
        name: String,
        /// File size in bytes.
        size: u64,
        /// MIME type (if known).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// TurnEndStatus
// ---------------------------------------------------------------------------

/// Outcome of an agent turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnEndStatus {
    Completed,
    Failed,
    Cancelled,
}

// ---------------------------------------------------------------------------
// Helper constructors on SessionEvent
// ---------------------------------------------------------------------------

impl SessionEvent {
    /// Plain text output.
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text {
            text: s.into(),
            thinking: false,
        }
    }

    /// Thinking / reasoning text.
    pub fn thinking(s: impl Into<String>) -> Self {
        Self::Text {
            text: s.into(),
            thinking: true,
        }
    }

    /// Service message.
    pub fn service(s: impl Into<String>) -> Self {
        Self::Service { text: s.into() }
    }

    /// Tool call start.
    pub fn tool_start(
        call: impl Into<String>,
        name: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
        Self::ToolCallStart {
            call: call.into(),
            name: name.into(),
            title: String::new(),
            description: String::new(),
            args,
        }
    }

    /// Tool call end (success).
    pub fn tool_end(call: impl Into<String>, result: Option<String>) -> Self {
        Self::ToolCallEnd {
            call: call.into(),
            result,
            is_error: false,
        }
    }

    /// Tool call end (error).
    pub fn tool_error(call: impl Into<String>, result: Option<String>) -> Self {
        Self::ToolCallEnd {
            call: call.into(),
            result,
            is_error: true,
        }
    }

    /// Turn started.
    pub fn turn_start() -> Self {
        Self::TurnStart
    }

    /// Turn ended.
    pub fn turn_end(status: TurnEndStatus) -> Self {
        Self::TurnEnd { status }
    }

    /// Subagent started.
    pub fn subagent_start(title: Option<String>) -> Self {
        Self::SubagentStart { title }
    }

    /// Subagent stopped.
    pub fn subagent_stop() -> Self {
        Self::SubagentStop
    }

    /// File attachment.
    pub fn file(
        file_ref: impl Into<String>,
        name: impl Into<String>,
        size: u64,
        mime_type: Option<String>,
    ) -> Self {
        Self::File {
            file_ref: file_ref.into(),
            name: name.into(),
            size,
            mime_type,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_false(b: &bool) -> bool {
    !b
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_new_generates_id_and_time() {
        let env = SessionEnvelope::new(SessionRole::Agent, SessionEvent::text("hello"));
        assert!(!env.id.is_empty());
        assert!(env.time > 0);
        assert_eq!(env.role, SessionRole::Agent);
        assert!(env.turn.is_none());
        assert!(env.subagent.is_none());
    }

    #[test]
    fn envelope_builder_methods() {
        let env = SessionEnvelope::new(SessionRole::User, SessionEvent::turn_start())
            .with_turn("turn-1")
            .with_subagent("sub-a");
        assert_eq!(env.turn.as_deref(), Some("turn-1"));
        assert_eq!(env.subagent.as_deref(), Some("sub-a"));
    }

    #[test]
    fn text_event_serialises_correctly() {
        let ev = SessionEvent::text("hi");
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["t"], "text");
        assert_eq!(json["text"], "hi");
        // `thinking` should be omitted when false
        assert!(json.get("thinking").is_none());
    }

    #[test]
    fn thinking_event_includes_flag() {
        let ev = SessionEvent::thinking("hmm");
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["t"], "text");
        assert_eq!(json["thinking"], true);
    }

    #[test]
    fn tool_call_roundtrip() {
        let start = SessionEvent::tool_start("c1", "shell", serde_json::json!({"cmd": "ls"}));
        let json_str = serde_json::to_string(&start).unwrap();
        let deser: SessionEvent = serde_json::from_str(&json_str).unwrap();
        match deser {
            SessionEvent::ToolCallStart {
                call, name, args, ..
            } => {
                assert_eq!(call, "c1");
                assert_eq!(name, "shell");
                assert_eq!(args["cmd"], "ls");
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn tool_end_error_flag() {
        let ev = SessionEvent::tool_error("c1", Some("boom".into()));
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["is_error"], true);
        assert_eq!(json["result"], "boom");
    }

    #[test]
    fn turn_end_status_serialises_lowercase() {
        let ev = SessionEvent::turn_end(TurnEndStatus::Completed);
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["status"], "completed");

        let ev2 = SessionEvent::turn_end(TurnEndStatus::Failed);
        let json2 = serde_json::to_value(&ev2).unwrap();
        assert_eq!(json2["status"], "failed");
    }

    #[test]
    fn subagent_lifecycle_roundtrip() {
        let start = SessionEvent::subagent_start(Some("Research".into()));
        let json = serde_json::to_string(&start).unwrap();
        let deser: SessionEvent = serde_json::from_str(&json).unwrap();
        match deser {
            SessionEvent::SubagentStart { title } => {
                assert_eq!(title.as_deref(), Some("Research"));
            }
            other => panic!("unexpected variant: {:?}", other),
        }

        let stop = SessionEvent::subagent_stop();
        let json2 = serde_json::to_string(&stop).unwrap();
        assert!(json2.contains("\"t\":\"stop\""));
    }

    #[test]
    fn file_event_roundtrip() {
        let ev = SessionEvent::file(
            "/tmp/out.pdf",
            "out.pdf",
            4096,
            Some("application/pdf".into()),
        );
        let json_str = serde_json::to_string(&ev).unwrap();
        let deser: SessionEvent = serde_json::from_str(&json_str).unwrap();
        match deser {
            SessionEvent::File {
                file_ref,
                name,
                size,
                mime_type,
            } => {
                assert_eq!(file_ref, "/tmp/out.pdf");
                assert_eq!(name, "out.pdf");
                assert_eq!(size, 4096);
                assert_eq!(mime_type.as_deref(), Some("application/pdf"));
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn service_event() {
        let ev = SessionEvent::service("connected to server");
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["t"], "service");
        assert_eq!(json["text"], "connected to server");
    }

    #[test]
    fn envelope_full_roundtrip() {
        let env = SessionEnvelope::new(SessionRole::Agent, SessionEvent::text("hello"))
            .with_turn("t1")
            .with_subagent("sub-x");
        let json_str = serde_json::to_string(&env).unwrap();
        let deser: SessionEnvelope = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deser.id, env.id);
        assert_eq!(deser.time, env.time);
        assert_eq!(deser.role, SessionRole::Agent);
        assert_eq!(deser.turn.as_deref(), Some("t1"));
        assert_eq!(deser.subagent.as_deref(), Some("sub-x"));
    }

    #[test]
    fn role_serialises_lowercase() {
        let user_json = serde_json::to_value(SessionRole::User).unwrap();
        assert_eq!(user_json, "user");
        let agent_json = serde_json::to_value(SessionRole::Agent).unwrap();
        assert_eq!(agent_json, "agent");
    }

    #[test]
    fn unknown_fields_are_ignored_on_deserialise() {
        // Forward-compatibility: extra fields should not cause errors
        let json = r#"{"t":"text","text":"hi","future_field":42}"#;
        let ev: SessionEvent = serde_json::from_str(json).unwrap();
        match ev {
            SessionEvent::Text { text, thinking } => {
                assert_eq!(text, "hi");
                assert!(!thinking);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }
}
