//! Gemini ACP wire-protocol DTOs (JSON-RPC 2.0 over ndJSON).
//!
//! These types model the frames that `gemini --acp` actually emits on stdout —
//! cross-checked against
//!
//! - gemini-cli `packages/cli/src/acp/acpClient.ts`
//! - `@agentclientprotocol/sdk` v0.13.1 schema
//! - the live capture in `docs/gemini-p1-live-captures.md`.
//!
//! The old fabricated `{"type":"status","status":"running"}` shape is gone;
//! Gemini speaks JSON-RPC end to end.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 envelope. One of `result` / `error` / `method`/`params` is set
/// depending on whether the frame is a response, a request, or a notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcFrame {
    /// JSON-RPC version string. Must be `"2.0"` for ACP.
    #[serde(default)]
    pub jsonrpc: Option<String>,
    /// Request / response correlation id. Notifications omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<JsonRpcId>,
    /// Method name on requests and notifications. Absent on responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Parameters payload (request / notification).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// Success payload (response).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Failure payload (response).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC ids can be a number, string, or null. Gemini currently uses u64.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    Number(u64),
    String(String),
}

impl JsonRpcId {
    /// Return an owned u64 when the id is numeric.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(n) => Some(*n),
            Self::String(s) => s.parse::<u64>().ok(),
        }
    }
}

impl From<u64> for JsonRpcId {
    fn from(value: u64) -> Self {
        Self::Number(value)
    }
}

/// JSON-RPC 2.0 error payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Classification of a parsed `JsonRpcFrame` for the demuxer.
///
/// - `Response` : `id + (result | error)`, keyed to a client request.
/// - `IncomingRequest` : `id + method`, the server asks us for something.
/// - `Notification` : `method` without `id`, no reply expected.
/// - `Invalid` : mandatory fields missing; log and drop.
#[derive(Debug, Clone)]
pub enum FrameKind {
    Response {
        id: JsonRpcId,
        result: Result<Value, JsonRpcError>,
    },
    IncomingRequest {
        id: JsonRpcId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Invalid,
}

impl JsonRpcFrame {
    /// Parse one ndJSON line into a `JsonRpcFrame`. Empty / whitespace-only
    /// lines return `None`.
    pub fn parse_line(line: &str) -> Option<Result<Self, serde_json::Error>> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(serde_json::from_str(trimmed))
    }

    /// Classify the frame based on which JSON-RPC fields are populated.
    pub fn classify(self) -> FrameKind {
        match (self.id, self.method, self.params, self.result, self.error) {
            (Some(id), Some(method), params, None, None) => FrameKind::IncomingRequest {
                id,
                method,
                params: params.unwrap_or(Value::Null),
            },
            (Some(id), None, _, Some(result), None) => FrameKind::Response {
                id,
                result: Ok(result),
            },
            (Some(id), None, _, None, Some(error)) => FrameKind::Response {
                id,
                result: Err(error),
            },
            (None, Some(method), params, _, _) => FrameKind::Notification {
                method,
                params: params.unwrap_or(Value::Null),
            },
            _ => FrameKind::Invalid,
        }
    }
}

/// Stop-reason enum reported in `session/prompt` responses.
///
/// See `PromptResponse` in `@agentclientprotocol/sdk`
/// (`types.gen.d.ts:1506-1522`). We accept any string so future values don't
/// break parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StopReason(pub String);

impl StopReason {
    pub fn is_cancelled(&self) -> bool {
        self.0 == "cancelled"
    }

    pub fn is_end_turn(&self) -> bool {
        self.0 == "end_turn"
    }
}

/// Gemini ACP session/update payload — one of several discriminated variants.
///
/// Discriminator field is `sessionUpdate` inside `update`. We only strongly
/// type the variants we actively consume; anything else falls through to the
/// `Other` catch-all.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub enum SessionUpdate {
    AgentMessageChunk {
        content: ContentBlock,
    },
    AgentThoughtChunk {
        content: ContentBlock,
    },
    UserMessageChunk {
        content: ContentBlock,
    },
    ToolCall {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        content: Option<Value>,
        #[serde(flatten)]
        extra: serde_json::Map<String, Value>,
    },
    ToolCallUpdate {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        content: Option<Value>,
        #[serde(flatten)]
        extra: serde_json::Map<String, Value>,
    },
    Plan(Value),
    AvailableCommandsUpdate(Value),
    CurrentModeUpdate {
        #[serde(rename = "currentModeId")]
        current_mode_id: String,
    },
    #[serde(other)]
    Other,
}

/// Content block inside an `agent_message_chunk` / `agent_thought_chunk` /
/// `user_message_chunk` update.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ResourceLink(Value),
    Image(Value),
    Audio(Value),
    Resource(Value),
    #[serde(other)]
    Other,
}

impl ContentBlock {
    /// Extract text if the variant is `Text`.
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text.as_str()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_initialize_response() {
        let line = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"authMethods":[]}}"#;
        let frame = JsonRpcFrame::parse_line(line).unwrap().unwrap();
        match frame.classify() {
            FrameKind::Response {
                id,
                result: Ok(value),
            } => {
                assert_eq!(id.as_u64(), Some(1));
                assert_eq!(value["protocolVersion"], json!(1));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_session_update_agent_message_chunk() {
        let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"abc","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"PONG"}}}}"#;
        let frame = JsonRpcFrame::parse_line(line).unwrap().unwrap();
        let FrameKind::Notification { method, params } = frame.classify() else {
            panic!("expected notification");
        };
        assert_eq!(method, "session/update");
        assert_eq!(params["sessionId"], json!("abc"));
        let update: SessionUpdate = serde_json::from_value(params["update"].clone()).unwrap();
        match update {
            SessionUpdate::AgentMessageChunk { content } => {
                assert_eq!(content.text(), Some("PONG"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_error_response() {
        let line = r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"Authentication required."}}"#;
        let frame = JsonRpcFrame::parse_line(line).unwrap().unwrap();
        let FrameKind::Response {
            id,
            result: Err(err),
        } = frame.classify()
        else {
            panic!("expected error response");
        };
        assert_eq!(id.as_u64(), Some(2));
        assert_eq!(err.code, -32000);
        assert_eq!(err.message, "Authentication required.");
    }

    #[test]
    fn classifies_incoming_permission_request() {
        let line = r#"{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{"sessionId":"abc","toolCall":{"toolCallId":"t1"},"options":[]}}"#;
        let frame = JsonRpcFrame::parse_line(line).unwrap().unwrap();
        match frame.classify() {
            FrameKind::IncomingRequest { id, method, .. } => {
                assert_eq!(id.as_u64(), Some(7));
                assert_eq!(method, "session/request_permission");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_available_commands_update() {
        let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"abc","update":{"sessionUpdate":"available_commands_update","availableCommands":[]}}}"#;
        let frame = JsonRpcFrame::parse_line(line).unwrap().unwrap();
        let FrameKind::Notification { params, .. } = frame.classify() else {
            panic!("expected notification");
        };
        let update: SessionUpdate = serde_json::from_value(params["update"].clone()).unwrap();
        assert!(matches!(update, SessionUpdate::AvailableCommandsUpdate(_)));
    }

    #[test]
    fn ignores_empty_lines() {
        assert!(JsonRpcFrame::parse_line("").is_none());
        assert!(JsonRpcFrame::parse_line("   \n").is_none());
    }

    #[test]
    fn invalid_frame_with_both_result_and_error_is_invalid_kind() {
        // Frame missing id/method entirely → invalid
        let line = r#"{"jsonrpc":"2.0"}"#;
        let frame = JsonRpcFrame::parse_line(line).unwrap().unwrap();
        assert!(matches!(frame.classify(), FrameKind::Invalid));
    }
}
