//! stream-json wire types shared by [`crate::ClaudeWorkspace`] and the
//! new [`crate::ClaudeAgentExecutor`].
//!
//! `claude --output-format stream-json` emits one JSON object per line. The
//! shapes handled here are the stable union observed across recent Claude
//! Code CLI releases; anything else is surfaced as
//! [`ExecutorEvent::NativeEvent`](multi_agent_runtime_core::ExecutorEvent::NativeEvent)
//! without failing the stream.

use serde::Deserialize;
use serde_json::{Map, Value};

pub const REDACTED_THINKING_PLACEHOLDER: &str = "[redacted thinking]";

/// Top-level stream-json envelope emitted on each line of `claude` stdout.
///
/// The `type` tag partitions the event. `#[serde(other)]` on [`ClaudeContent`]
/// keeps the parser tolerant to unknown inner content kinds so new Claude
/// features don't break parsing — callers forward those to `NativeEvent`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ClaudeJsonEvent {
    /// System messages (session init, metadata).
    #[serde(rename = "system")]
    System {
        /// Sub-category, e.g. `"init"`.
        subtype: String,
        /// Vendor-native session id, emitted on `"init"`.
        #[serde(default)]
        session_id: Option<String>,
        /// Tool ids the session advertises, emitted on `"init"`.
        #[serde(default)]
        tools: Option<Vec<String>>,
        /// Session state for `session_state_changed` frames.
        #[serde(default)]
        state: Option<String>,
        /// Claude SDK task lifecycle id for background work.
        #[serde(default)]
        task_id: Option<String>,
        /// Human summary for task lifecycle events.
        #[serde(default)]
        description: Option<String>,
        /// Summary text for task_progress / task_notification.
        #[serde(default)]
        summary: Option<String>,
        /// Completion status for task_notification.
        #[serde(default)]
        status: Option<String>,
        /// Output artifact path surfaced on task_notification.
        #[serde(default)]
        output_file: Option<String>,
        /// Correlates the background task to the originating tool use.
        #[serde(default)]
        tool_use_id: Option<String>,
        /// Claude SDK task kind, e.g. background / agent / shell.
        #[serde(default)]
        task_type: Option<String>,
        /// AI-generated progress metrics.
        #[serde(default)]
        usage: Option<Value>,
        /// Tool most recently executed by the task.
        #[serde(default)]
        last_tool_name: Option<String>,
        /// Per-event uuid emitted by the SDK.
        #[serde(default)]
        uuid: Option<String>,
    },
    /// Assistant turn payload (text / thinking / tool_use blocks).
    #[serde(rename = "assistant")]
    Assistant {
        /// The assistant message body.
        message: ClaudeAssistantMessage,
        /// Native session id propagated on every assistant frame.
        #[serde(default)]
        session_id: Option<String>,
    },
    /// User-originating frames — the Claude CLI echoes tool results and
    /// plain user text through `user` envelopes. We parse the content so the
    /// session layer can fan `tool_result` blocks out as
    /// [`ExecutorEvent::ToolResult`](multi_agent_runtime_core::ExecutorEvent::ToolResult).
    #[serde(rename = "user")]
    User {
        /// The user message body.
        message: ClaudeUserMessage,
        /// Native session id propagated on every user frame.
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Rate-limit notices — transport-level, not a turn signal.
    #[serde(rename = "rate_limit_event")]
    RateLimitEvent {
        /// Detailed rate-limit state emitted by newer Claude CLI builds.
        #[serde(default)]
        rate_limit_info: Option<Value>,
        /// Event uuid emitted by Claude.
        #[serde(default)]
        uuid: Option<String>,
        /// Native session id.
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Predicted next-user-prompt suggestion emitted after a turn.
    #[serde(rename = "prompt_suggestion")]
    PromptSuggestion {
        /// Suggested follow-up prompt text.
        suggestion: String,
        /// Native session id.
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Claude SDK task lifecycle start for subagent/background work.
    #[serde(rename = "task_started")]
    TaskStarted {
        task_id: String,
        description: String,
        #[serde(default)]
        tool_use_id: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Claude SDK task lifecycle progress update.
    #[serde(rename = "task_progress")]
    TaskProgress {
        task_id: String,
        description: String,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        last_tool_name: Option<String>,
        #[serde(default)]
        tool_use_id: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Claude SDK task lifecycle terminal notification.
    #[serde(rename = "task_notification")]
    TaskNotification {
        task_id: String,
        status: String,
        summary: String,
        #[serde(default)]
        output_file: Option<String>,
        #[serde(default)]
        tool_use_id: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Claude SDK progress tick for a long-running tool invocation.
    #[serde(rename = "tool_progress")]
    ToolProgress {
        /// Preserve the vendor payload so callers can extract whatever fields
        /// the current SDK version emits.
        #[serde(flatten)]
        payload: Map<String, Value>,
    },
    /// Claude SDK compacted the running conversation and emitted a boundary marker.
    #[serde(rename = "compact_boundary")]
    CompactBoundary {
        #[serde(default)]
        trigger: Option<String>,
        #[serde(default)]
        pre_tokens: Option<u64>,
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Bidirectional control request emitted by the Claude SDK bridge.
    #[serde(rename = "control_request")]
    ControlRequest {
        request_id: String,
        request: ClaudeControlRequest,
    },
    /// Raw Anthropic API stream event emitted when `--include-partial-messages`
    /// is active. Carries `content_block_delta` (text_delta / thinking_delta),
    /// `content_block_start`, `content_block_stop`, `message_start`,
    /// `message_delta`, and `message_stop` frames.
    #[serde(rename = "stream_event")]
    StreamEvent {
        /// The raw Anthropic SSE event payload.
        #[serde(default)]
        event: Option<Value>,
    },
    /// Terminal frame for a turn.
    #[serde(rename = "result")]
    Result {
        /// `"success"` / error variants.
        subtype: String,
        /// Whether the turn ended in error.
        is_error: bool,
        /// Final result text (for success) or error message.
        result: String,
        /// Token usage summary emitted at end-of-turn.
        #[serde(default)]
        usage: Option<Value>,
        /// Some Claude builds surface extra model usage under a second key.
        #[serde(default, alias = "modelUsage", alias = "model_usage")]
        model_usage: Option<Value>,
        /// Native session id.
        #[serde(default)]
        session_id: Option<String>,
    },
}

/// Control requests routed over the same stdout stream as regular SDK messages.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "subtype")]
pub enum ClaudeControlRequest {
    /// MCP server asks the client to gather structured user input.
    #[serde(rename = "elicitation")]
    Elicitation {
        mcp_server_name: String,
        message: String,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        elicitation_id: Option<String>,
        #[serde(default)]
        requested_schema: Option<Value>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        display_name: Option<String>,
        #[serde(default)]
        description: Option<String>,
    },
    /// Any control request kind not yet modeled.
    #[serde(other)]
    Other,
}

/// Inner `message` body of an `assistant` envelope.
#[derive(Debug, Clone, Deserialize)]
pub struct ClaudeAssistantMessage {
    /// Ordered list of content blocks (text / thinking / tool_use).
    pub content: Vec<ClaudeContent>,
}

/// Inner `message` body of a `user` envelope. Claude occasionally emits a
/// plain string here (vanilla user prompt echo) or an array of content
/// blocks (tool_result piggyback after a tool execution).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ClaudeUserMessageContent {
    Text(String),
    Blocks(Vec<ClaudeContent>),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClaudeUserMessage {
    /// Ordered list of content blocks — either a raw string or tool_result
    /// / text blocks.
    pub content: ClaudeUserMessageContent,
}

/// Source payload for a Claude inline image block.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ClaudeImageSource {
    /// Inline base64 image data.
    #[serde(rename = "base64")]
    Base64 { media_type: String, data: String },
    /// Remote image reference.
    #[serde(rename = "url")]
    Url { url: String },
    /// Any source type not yet modeled.
    #[serde(other)]
    Other,
}

/// One block inside [`ClaudeAssistantMessage::content`].
///
/// Unknown block kinds fall through [`ClaudeContent::Other`] so the session
/// layer can translate them into `ExecutorEvent::NativeEvent` rather than
/// silently dropping data.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ClaudeContent {
    /// Function / tool invocation emitted by the LLM.
    #[serde(rename = "tool_use")]
    ToolUse {
        /// Unique id used to correlate with later `tool_result` frames.
        id: String,
        /// Tool name as declared in the tool catalogue.
        name: String,
        /// Raw input payload; may be partial mid-stream.
        #[serde(default)]
        input: Value,
    },
    /// Visible assistant text chunk.
    #[serde(rename = "text")]
    Text {
        /// The chunk body.
        text: String,
    },
    /// Inline assistant image block.
    #[serde(rename = "image")]
    Image {
        /// Claude usually nests image data under `source`.
        #[serde(default)]
        source: Option<ClaudeImageSource>,
        /// Fallback URL fields observed on some surfaces.
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        image_url: Option<String>,
    },
    /// Thinking / scratch-pad text chunk.
    #[serde(rename = "thinking")]
    Thinking {
        /// The chunk body.
        thinking: String,
    },
    /// Thinking block whose contents are intentionally omitted by Claude.
    #[serde(rename = "redacted_thinking")]
    RedactedThinking,
    /// Claude SDK summary emitted after multiple tool calls for context recovery.
    #[serde(rename = "tool_use_summary")]
    ToolUseSummary {
        /// Preserve the vendor payload so the executor can recover the best
        /// available summary string across SDK variants.
        #[serde(flatten)]
        payload: Map<String, Value>,
    },
    /// Server-side web search result block surfaced inline in assistant content.
    #[serde(rename = "web_search_result")]
    WebSearchResult {
        /// Result blocks may carry a correlating tool id on some SDK surfaces.
        #[serde(default)]
        tool_use_id: Option<String>,
        /// Preserve the vendor payload for downstream normalizers.
        #[serde(flatten)]
        payload: Map<String, Value>,
    },
    /// Server-side code execution result block surfaced inline in assistant content.
    #[serde(rename = "code_execution_result", alias = "bash_code_execution_result")]
    CodeExecutionResult {
        /// Result blocks may carry a correlating tool id on some SDK surfaces.
        #[serde(default)]
        tool_use_id: Option<String>,
        /// Preserve the vendor payload for downstream normalizers.
        #[serde(flatten)]
        payload: Map<String, Value>,
    },
    /// Server-side web fetch result block surfaced inline in assistant content.
    #[serde(rename = "web_fetch_result")]
    WebFetchResult {
        /// Result blocks may carry a correlating tool id on some SDK surfaces.
        #[serde(default)]
        tool_use_id: Option<String>,
        /// Preserve the vendor payload for downstream normalizers.
        #[serde(flatten)]
        payload: Map<String, Value>,
    },
    /// Some Claude surfaces wrap web fetch results in an outer tool-result block.
    #[serde(rename = "web_fetch_tool_result")]
    WebFetchToolResult {
        /// Correlating id for the underlying server-side tool use.
        #[serde(default)]
        tool_use_id: Option<String>,
        /// Nested web fetch result/error payload.
        content: Value,
    },
    /// Tool execution result piggybacked in a `user` envelope. The Claude
    /// CLI emits one of these blocks per completed tool_use, paired by
    /// `tool_use_id`.
    #[serde(rename = "tool_result")]
    ToolResult {
        /// Correlating id for the originating `tool_use` block.
        tool_use_id: String,
        /// Result payload. Claude emits either a raw string or a list of
        /// content blocks (text / image). Callers normalize downstream.
        #[serde(default)]
        content: Value,
        /// Whether the tool reported failure.
        #[serde(default)]
        is_error: bool,
    },
    /// Any content kind not yet enumerated — callers should forward verbatim.
    #[serde(other)]
    Other,
}

/// Attempt to parse a single line of `claude` stdout into a
/// [`ClaudeJsonEvent`]. Non-JSON noise (`null`, blanks, partial prefixes) is
/// returned as `None` so callers can skip it without raising an error.
pub fn parse_stream_line(line: &str) -> Option<Result<ClaudeJsonEvent, serde_json::Error>> {
    let trimmed = line.trim();
    if trimmed.is_empty() || !trimmed.starts_with('{') {
        return None;
    }
    Some(serde_json::from_str(trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_frame_with_tool_result_blocks_parses() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_01ABC","content":"hello","is_error":false}]},"session_id":"sess-1"}"#;
        let evt = parse_stream_line(line).unwrap().unwrap();
        match evt {
            ClaudeJsonEvent::User { message, .. } => match message.content {
                ClaudeUserMessageContent::Blocks(blocks) => {
                    assert_eq!(blocks.len(), 1);
                    match &blocks[0] {
                        ClaudeContent::ToolResult {
                            tool_use_id,
                            is_error,
                            ..
                        } => {
                            assert_eq!(tool_use_id, "toolu_01ABC");
                            assert!(!*is_error);
                        }
                        other => panic!("expected ToolResult block, got {other:?}"),
                    }
                }
                other => panic!("expected Blocks content, got {other:?}"),
            },
            other => panic!("expected User event, got {other:?}"),
        }
    }

    #[test]
    fn user_frame_with_error_tool_result_parses() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_02XYZ","content":[{"type":"text","text":"nope"}],"is_error":true}]}}"#;
        let evt = parse_stream_line(line).unwrap().unwrap();
        match evt {
            ClaudeJsonEvent::User { message, .. } => match message.content {
                ClaudeUserMessageContent::Blocks(blocks) => match &blocks[0] {
                    ClaudeContent::ToolResult {
                        tool_use_id,
                        is_error,
                        content,
                    } => {
                        assert_eq!(tool_use_id, "toolu_02XYZ");
                        assert!(*is_error);
                        assert!(content.is_array());
                    }
                    other => panic!("expected ToolResult, got {other:?}"),
                },
                other => panic!("expected Blocks, got {other:?}"),
            },
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn user_frame_with_plain_text_parses() {
        let line = r#"{"type":"user","message":{"role":"user","content":"hi there"}}"#;
        let evt = parse_stream_line(line).unwrap().unwrap();
        match evt {
            ClaudeJsonEvent::User { message, .. } => match message.content {
                ClaudeUserMessageContent::Text(s) => assert_eq!(s, "hi there"),
                other => panic!("expected Text content, got {other:?}"),
            },
            other => panic!("expected User event, got {other:?}"),
        }
    }
}
