//! Normalised stream events emitted by `AgentExecutor::send_message`.
//!
//! `ExecutorEvent` is the vendor-agnostic contract that the app-level
//! `ExecutorNormalizer` consumes and translates into UI / ACP frames.
//! Unknown / vendor-specific payloads escape via
//! [`ExecutorEvent::NativeEvent`] so the contract never gates evolution.

use std::borrow::Cow;
use std::pin::Pin;

use futures_core::Stream;
use serde::{Deserialize, Serialize};

use super::error::AgentExecutorError;
use super::types::{NativeSessionId, TokenUsage};

/// Delta chunk classification for [`ExecutorEvent::StreamDelta`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaKind {
    /// Visible assistant text.
    Text,
    /// Thinking / scratch-pad content that should be visually distinct.
    Thinking,
    /// Reasoning content reported by providers that separate it from text.
    Reasoning,
}

/// Normalised stream event.
///
/// Variants follow the shape needed by the app-layer normalizer. Numeric /
/// byte-exact bookkeeping is intentionally excluded — consumers rebuild that
/// from sequences of `StreamDelta` + `ToolCallStart` + `ToolResult`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutorEvent {
    /// Session handshake completed; vendor produced a native session id.
    SessionReady {
        /// Vendor-native session id.
        native_session_id: NativeSessionId,
    },
    /// Incremental text / thinking / reasoning chunk.
    StreamDelta {
        /// Chunk classification.
        kind: DeltaKind,
        /// Raw chunk text.
        content: String,
    },
    /// A tool call was emitted by the agent.
    ToolCallStart {
        /// Unique id for this tool invocation (used to correlate results).
        tool_use_id: String,
        /// Tool name.
        name: String,
        /// Parsed input payload (may be partial — see `partial`).
        input: serde_json::Value,
        /// When `true`, `input` is the partial snapshot and more
        /// `ToolCallInputDelta` events may follow.
        partial: bool,
    },
    /// Incremental patch to an in-flight tool call's input.
    ToolCallInputDelta {
        /// Correlates with the initiating `ToolCallStart`.
        tool_use_id: String,
        /// JSON-patch / merge-patch chunk (vendor-defined shape).
        json_patch: serde_json::Value,
    },
    /// A tool call finished; result may be `Ok(output)` or `Err(message)`.
    ToolResult {
        /// Correlates with the initiating `ToolCallStart`.
        tool_use_id: String,
        /// Success payload or failure message.
        output: Result<String, String>,
    },
    /// The agent asks for user permission before running a sensitive tool.
    PermissionRequest {
        /// Identifier used by `respond_to_permission`.
        request_id: String,
        /// Tool that is asking.
        tool_name: String,
        /// Input payload that will be executed on approval.
        tool_input: serde_json::Value,
    },
    /// The agent invoked a caller-injected tool (see `UserMessage::injected_tools`).
    InjectedToolInvocation {
        /// Identifier used to reply to the injection with a result.
        request_id: String,
        /// Tool name.
        tool_name: String,
        /// Input payload.
        tool_input: serde_json::Value,
    },
    /// Cumulative usage snapshot surfaced mid-stream.
    UsageUpdate(TokenUsage),
    /// Turn finished; final text and aggregate usage reported.
    TurnComplete {
        /// Full concatenated assistant text, if reported by the vendor.
        #[serde(default)]
        final_text: Option<String>,
        /// Iteration count (e.g. LLM round-trips) consumed by the turn.
        iteration_count: u32,
        /// Aggregate token usage for this turn.
        usage: TokenUsage,
    },
    /// Recoverable or fatal error during the stream.
    Error {
        /// Human-readable diagnostic.
        message: String,
        /// When `true`, the session is still usable.
        recoverable: bool,
    },
    /// Escape hatch for vendor-specific events the normaliser does not recognise.
    NativeEvent {
        /// Provider identifier (`"cteno"`, `"claude"`, `"codex"`).
        ///
        /// `Cow<'static, str>` so adapters can pass a `&'static str` literal
        /// at zero cost while still supporting round-trip through `serde`.
        provider: Cow<'static, str>,
        /// Raw payload captured from the transport.
        payload: serde_json::Value,
    },
}

/// Boxed stream alias returned by `AgentExecutor::send_message`.
pub type EventStream =
    Pin<Box<dyn Stream<Item = Result<ExecutorEvent, AgentExecutorError>> + Send>>;
