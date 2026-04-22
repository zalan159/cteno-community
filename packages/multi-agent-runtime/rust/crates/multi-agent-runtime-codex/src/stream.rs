//! Stream-JSON event types emitted by `codex exec --experimental-json`.
//!
//! These types are shared between:
//! - [`crate::workspace::CodexWorkspace`] — legacy multi-agent orchestrator.
//! - [`crate::agent_executor::CodexAgentExecutor`] — the new session-level
//!   [`multi_agent_runtime_core::AgentExecutor`] implementation.
//!
//! The shapes are copied verbatim from the Codex CLI's current JSON contract;
//! fields are decoded leniently (`#[serde(default)]`) so unexpected extra
//! properties do not break parsing.

use serde::Deserialize;
use serde_json::{Map, Value};

/// Top-level event envelope from `codex exec --experimental-json` stdout.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum CodexJsonEvent {
    /// A new Codex thread has been bound to this turn.
    #[serde(rename = "thread.started")]
    ThreadStarted {
        /// Vendor-native thread id.
        thread_id: String,
    },
    /// The current turn has started producing output.
    #[serde(rename = "turn.started")]
    TurnStarted,
    /// The current turn completed successfully.
    #[serde(rename = "turn.completed")]
    TurnCompleted {
        /// Optional usage block reported at end-of-turn.
        #[serde(default)]
        usage: Option<Value>,
    },
    /// The current turn failed with a vendor error.
    #[serde(rename = "turn.failed")]
    TurnFailed {
        /// Structured error payload.
        error: CodexTurnError,
    },
    /// The current turn's execution plan has been updated.
    #[serde(rename = "turn/plan/updated", alias = "turn.plan.updated")]
    TurnPlanUpdated {
        /// The plan-update payload. Codex has emitted both flattened and
        /// nested plan snapshots, so we deserialize both shapes here.
        #[serde(flatten)]
        update: CodexTurnPlanUpdate,
    },
    /// A new item (tool call, message, reasoning, …) has started streaming.
    #[serde(rename = "item.started")]
    ItemStarted {
        /// The item body.
        item: CodexItem,
    },
    /// An in-flight item received an incremental update.
    #[serde(rename = "item.updated")]
    ItemUpdated {
        /// The updated item body.
        item: CodexItem,
    },
    /// An item has finished and carries its terminal state.
    #[serde(rename = "item.completed")]
    ItemCompleted {
        /// The final item body.
        item: CodexItem,
    },
    /// Incremental progress text for an in-flight MCP tool call.
    #[serde(
        rename = "item/mcpToolCall/progress",
        alias = "item.mcpToolCall.progress",
        alias = "item/mcp_tool_call/progress",
        alias = "item.mcp_tool_call.progress",
        alias = "mcpToolCall/progress",
        alias = "mcpToolCall.progress",
        alias = "mcp_tool_call/progress",
        alias = "mcp_tool_call.progress"
    )]
    McpToolCallProgress {
        /// Raw vendor payload. Codex has emitted this in a few shapes, so
        /// we keep the map and normalize it later.
        #[serde(flatten)]
        progress: CodexMcpToolCallProgress,
    },
    /// Incremental stdout/stderr for an in-flight command execution.
    #[serde(
        rename = "command/exec/outputDelta",
        alias = "command.exec.outputDelta",
        alias = "command/exec/output_delta",
        alias = "command.exec.output_delta"
    )]
    CommandExecOutputDelta {
        /// Raw vendor payload. Codex has emitted this in a few shapes, so
        /// we keep the map and normalize it later.
        #[serde(flatten)]
        delta: CodexCommandExecOutputDelta,
    },
    /// Incremental plan text for an in-flight plan item.
    #[serde(
        rename = "item/plan/delta",
        alias = "item.plan.delta",
        alias = "plan/delta",
        alias = "plan.delta"
    )]
    PlanDelta {
        /// Raw vendor payload. Codex has emitted this in a few shapes, so
        /// we keep the map and normalize it later.
        #[serde(flatten)]
        delta: CodexPlanDelta,
    },
    /// A top-level error unrelated to a specific item.
    #[serde(rename = "error")]
    Error {
        /// Human-readable diagnostic.
        message: String,
    },
}

/// Structured turn-level failure payload.
#[derive(Debug, Clone, Deserialize)]
pub struct CodexTurnError {
    /// Human-readable diagnostic.
    pub message: String,
}

/// Discriminated item body payload.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum CodexItem {
    /// A shell command being or having been executed.
    #[serde(rename = "command_execution", alias = "commandExecution")]
    CommandExecution {
        /// Item id.
        id: String,
        /// The raw command-line string.
        command: String,
        /// Aggregated stdout+stderr snapshot (when Codex surfaces it).
        #[serde(default)]
        aggregated_output: Option<String>,
        /// Process exit code (when known).
        #[serde(default)]
        exit_code: Option<i32>,
        /// Execution status string (`"running"`, `"completed"`, …).
        #[serde(default)]
        status: Option<String>,
        /// Additional vendor fields such as `terminalInteraction`.
        #[serde(flatten)]
        data: Map<String, Value>,
    },
    /// Assistant text message.
    #[serde(rename = "agent_message", alias = "agentMessage")]
    AgentMessage {
        /// Item id.
        id: String,
        /// Rendered message text.
        text: String,
    },
    /// Assistant reasoning / thinking content.
    #[serde(rename = "reasoning")]
    Reasoning {
        /// Item id.
        id: String,
        /// Rendered reasoning text.
        text: String,
        /// Condensed reasoning summary blocks when Codex emits them.
        #[serde(default)]
        summary: Vec<Value>,
    },
    /// Workspace file mutation(s) produced by Codex.
    #[serde(rename = "file_change", alias = "fileChange")]
    FileChange {
        /// Item id.
        id: String,
        /// Applied file changes.
        changes: Vec<CodexFileChange>,
        /// Optional status hint.
        #[serde(default)]
        status: Option<String>,
    },
    /// External MCP tool call.
    #[serde(rename = "mcp_tool_call", alias = "mcpToolCall")]
    McpToolCall {
        /// Item id.
        id: String,
        /// MCP server identifier.
        server: String,
        /// Tool name within the server.
        tool: String,
        /// Status (`"completed"`, `"failed"`, …).
        #[serde(default)]
        status: Option<String>,
        /// Structured error payload (present on failure).
        #[serde(default)]
        error: Option<CodexItemError>,
    },
    /// A third-party dynamically registered tool call (non-MCP).
    #[serde(rename = "dynamicToolCall", alias = "dynamic_tool_call")]
    DynamicToolCall {
        /// Item id.
        id: String,
        /// Registered tool name.
        tool: String,
        /// Tool arguments payload.
        #[serde(default)]
        arguments: Option<Value>,
        /// Status (`"completed"`, `"failed"`, ...).
        #[serde(default)]
        status: Option<String>,
        /// Generic content blocks returned by the tool.
        #[serde(default, alias = "content_items", alias = "contentItems")]
        content_items: Vec<Value>,
        /// Structured error payload (present on failure).
        #[serde(default)]
        error: Option<CodexItemError>,
    },
    /// A collaborative agent tool call (spawn/send/wait/close).
    #[serde(rename = "collab_tool_call", alias = "collabAgentToolCall")]
    CollabAgentToolCall {
        /// Item id.
        id: String,
        /// Collab action name.
        tool: String,
        /// Status (`"in_progress"`, `"completed"`, `"failed"`, …).
        #[serde(default)]
        status: Option<String>,
        /// Prompt sent to the collab agent, when available.
        #[serde(default)]
        prompt: Option<String>,
    },
    /// A web search performed by the agent.
    #[serde(rename = "web_search")]
    WebSearch {
        /// Item id.
        id: String,
        /// Rendered search query.
        query: String,
    },
    /// An image generation tool call.
    #[serde(rename = "imageGeneration", alias = "image_generation")]
    ImageGeneration {
        /// Item id.
        id: String,
        /// Remaining payload fields emitted by Codex.
        #[serde(flatten)]
        data: Map<String, Value>,
    },
    /// An image/screenshot view item.
    #[serde(rename = "imageView", alias = "image_view")]
    ImageView {
        /// Item id.
        id: String,
        /// Remaining payload fields emitted by Codex.
        #[serde(flatten)]
        data: Map<String, Value>,
    },
    /// The agent's structured execution plan.
    #[serde(rename = "plan")]
    Plan {
        /// Item id.
        id: String,
        /// Optional explanation for why the plan changed.
        #[serde(default)]
        explanation: Option<String>,
        /// Plan entries. Codex has used both `steps` and `items`.
        #[serde(default, alias = "steps")]
        items: Vec<CodexPlanItem>,
    },
    /// The agent's todo-list snapshot.
    #[serde(rename = "todo_list", alias = "todoList")]
    TodoList {
        /// Item id.
        id: String,
        /// List of todo entries.
        items: Vec<CodexTodoItem>,
    },
    /// A Codex Guardian auto-approval review.
    #[serde(rename = "autoApprovalReview", alias = "auto_approval_review")]
    AutoApprovalReview {
        /// Item id.
        id: String,
        /// Remaining payload fields emitted by Codex.
        #[serde(flatten)]
        data: Map<String, Value>,
    },
    /// An item-scoped error.
    #[serde(rename = "error")]
    Error {
        /// Item id.
        id: String,
        /// Human-readable diagnostic.
        message: String,
    },
}

/// Single file mutation reported inside [`CodexItem::FileChange`].
#[derive(Debug, Clone, Deserialize)]
pub struct CodexFileChange {
    /// Relative path of the mutated file.
    pub path: String,
    /// Mutation kind (e.g. `"create"`, `"update"`, `"delete"`).
    pub kind: String,
    /// Unified diff for the change when Codex includes it.
    #[serde(default)]
    pub diff: Option<String>,
}

/// Structured error body for an item.
#[derive(Debug, Clone, Deserialize)]
pub struct CodexItemError {
    /// Human-readable message (absent when the vendor omits it).
    pub message: Option<String>,
}

/// Raw progress payload for an in-flight MCP tool call.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CodexMcpToolCallProgress {
    /// Codex has emitted several key layouts here, so keep the raw map and
    /// normalize common id/message fields with helper accessors.
    #[serde(flatten)]
    pub data: Map<String, Value>,
}

impl CodexMcpToolCallProgress {
    /// Correlates the progress payload with the MCP tool call item id.
    pub fn tool_use_id(&self) -> Option<&str> {
        string_field(
            &self.data,
            &[
                "id",
                "item_id",
                "itemId",
                "tool_use_id",
                "toolUseId",
                "call_id",
                "callId",
            ],
        )
        .or_else(|| {
            nested_object_field(
                &self.data,
                &[
                    "item",
                    "tool_call",
                    "toolCall",
                    "mcp_tool_call",
                    "mcpToolCall",
                ],
                &[
                    "id",
                    "item_id",
                    "itemId",
                    "tool_use_id",
                    "toolUseId",
                    "call_id",
                    "callId",
                ],
            )
        })
    }

    /// Human-readable progress message to surface in the UI.
    pub fn message(&self) -> Option<&str> {
        string_field(&self.data, &["message"]).or_else(|| {
            nested_object_field(
                &self.data,
                &["progress", "item", "mcp_tool_call", "mcpToolCall"],
                &["message"],
            )
        })
    }

    /// Lossless payload for native-event fallback.
    pub fn raw_payload(&self) -> Value {
        Value::Object(self.data.clone())
    }
}

/// Raw payload for an in-flight command stdout/stderr chunk.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CodexCommandExecOutputDelta {
    /// Codex has emitted several key layouts here, so keep the raw map and
    /// normalize common id/stream/chunk fields with helper accessors.
    #[serde(flatten)]
    pub data: Map<String, Value>,
}

impl CodexCommandExecOutputDelta {
    /// Correlates the output delta with the command execution item id.
    pub fn tool_use_id(&self) -> Option<&str> {
        string_field(
            &self.data,
            &[
                "id",
                "item_id",
                "itemId",
                "tool_use_id",
                "toolUseId",
                "call_id",
                "callId",
                "command_id",
                "commandId",
            ],
        )
        .or_else(|| {
            nested_object_field(
                &self.data,
                &[
                    "item",
                    "command",
                    "command_execution",
                    "commandExecution",
                    "output_delta",
                    "outputDelta",
                ],
                &[
                    "id",
                    "item_id",
                    "itemId",
                    "tool_use_id",
                    "toolUseId",
                    "call_id",
                    "callId",
                    "command_id",
                    "commandId",
                ],
            )
        })
    }

    /// Identifies whether the chunk belongs to stdout or stderr.
    pub fn stream(&self) -> Option<&str> {
        raw_string_field(
            &self.data,
            &[
                "stream",
                "channel",
                "source",
                "target",
                "fd",
                "output_stream",
                "outputStream",
            ],
        )
        .or_else(|| {
            nested_raw_string_field(
                &self.data,
                &["delta", "output_delta", "outputDelta", "chunk"],
                &[
                    "stream",
                    "channel",
                    "source",
                    "target",
                    "fd",
                    "output_stream",
                    "outputStream",
                ],
            )
        })
    }

    /// Base64-encoded chunk contents.
    pub fn chunk_base64(&self) -> Option<&str> {
        raw_string_field(
            &self.data,
            &["chunk", "data", "content", "output", "bytes", "base64"],
        )
        .or_else(|| {
            nested_raw_string_field(
                &self.data,
                &["delta", "output_delta", "outputDelta", "chunk"],
                &["chunk", "data", "content", "output", "bytes", "base64"],
            )
        })
    }

    /// Lossless payload for native-event fallback.
    pub fn raw_payload(&self) -> Value {
        Value::Object(self.data.clone())
    }
}

/// Raw payload for an in-flight plan-text delta.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CodexPlanDelta {
    /// Codex has emitted several key layouts here, so keep the raw map and
    /// normalize common id/delta fields with helper accessors.
    #[serde(flatten)]
    pub data: Map<String, Value>,
}

impl CodexPlanDelta {
    /// Correlates the delta payload with the plan item id.
    pub fn tool_use_id(&self) -> Option<&str> {
        string_field(
            &self.data,
            &["id", "item_id", "itemId", "tool_use_id", "toolUseId"],
        )
        .or_else(|| {
            nested_object_field(&self.data, &["item", "plan"], &["id", "item_id", "itemId"])
        })
    }

    /// Incremental plan text to surface in the UI.
    pub fn delta(&self) -> Option<&str> {
        raw_string_field(&self.data, &["delta", "text"]).or_else(|| {
            nested_raw_string_field(
                &self.data,
                &["delta", "plan", "item"],
                &["text", "delta", "content"],
            )
        })
    }

    /// Lossless payload for native-event fallback.
    pub fn raw_payload(&self) -> Value {
        Value::Object(self.data.clone())
    }
}

fn string_field<'a>(map: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| map.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn raw_string_field<'a>(map: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| map.get(*key).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
}

fn nested_object_field<'a>(
    map: &'a Map<String, Value>,
    container_keys: &[&str],
    value_keys: &[&str],
) -> Option<&'a str> {
    container_keys
        .iter()
        .filter_map(|key| map.get(*key).and_then(Value::as_object))
        .find_map(|object| string_field(object, value_keys))
}

fn nested_raw_string_field<'a>(
    map: &'a Map<String, Value>,
    container_keys: &[&str],
    value_keys: &[&str],
) -> Option<&'a str> {
    container_keys
        .iter()
        .filter_map(|key| map.get(*key).and_then(Value::as_object))
        .find_map(|object| raw_string_field(object, value_keys))
}

/// A single entry in a Codex plan snapshot.
#[derive(Debug, Clone, Deserialize)]
pub struct CodexPlanItem {
    /// Preferred frontend content field.
    #[serde(default)]
    pub content: Option<String>,
    /// Alternate text field used by some Codex payloads.
    #[serde(default)]
    pub text: Option<String>,
    /// Alternate label field used by some Codex payloads.
    #[serde(default)]
    pub step: Option<String>,
    /// Optional description field.
    #[serde(default)]
    pub description: Option<String>,
    /// Per-step status (`pending`, `inProgress`, `completed`, ...).
    #[serde(default)]
    pub status: Option<String>,
}

/// Top-level turn plan update emitted independently from item-scoped `plan`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CodexTurnPlanUpdate {
    /// Optional update id when Codex includes one.
    #[serde(default)]
    pub id: Option<String>,
    /// Optional explanation for why the plan changed.
    #[serde(default)]
    pub explanation: Option<String>,
    /// Top-level plan entries. Codex has used both `steps` and `items`.
    #[serde(default, alias = "steps")]
    pub items: Vec<CodexPlanItem>,
    /// Nested plan payload used by some Codex builds.
    #[serde(default)]
    pub plan: Option<CodexPlanPayload>,
}

impl CodexTurnPlanUpdate {
    /// Stable identifier for the update when Codex provides one.
    pub fn id(&self) -> Option<&str> {
        self.plan
            .as_ref()
            .and_then(CodexPlanPayload::id)
            .or(self.id.as_deref())
    }

    /// Explanation for the latest plan snapshot, when available.
    pub fn explanation(&self) -> Option<&str> {
        self.plan
            .as_ref()
            .and_then(CodexPlanPayload::explanation)
            .or(self.explanation.as_deref())
    }

    /// Step list for the latest plan snapshot.
    pub fn items(&self) -> &[CodexPlanItem] {
        self.plan
            .as_ref()
            .map(CodexPlanPayload::items)
            .unwrap_or(&self.items)
    }
}

/// Inner plan payload for [`CodexTurnPlanUpdate`].
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum CodexPlanPayload {
    /// Nested object payload carrying metadata plus steps/items.
    Snapshot(CodexPlanSnapshot),
    /// Bare array payload containing just the step list.
    Steps(Vec<CodexPlanItem>),
}

impl CodexPlanPayload {
    fn id(&self) -> Option<&str> {
        match self {
            Self::Snapshot(snapshot) => snapshot.id.as_deref(),
            Self::Steps(_) => None,
        }
    }

    fn explanation(&self) -> Option<&str> {
        match self {
            Self::Snapshot(snapshot) => snapshot.explanation.as_deref(),
            Self::Steps(_) => None,
        }
    }

    fn items(&self) -> &[CodexPlanItem] {
        match self {
            Self::Snapshot(snapshot) => &snapshot.items,
            Self::Steps(items) => items,
        }
    }
}

/// Structured plan snapshot reused by item and top-level plan updates.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CodexPlanSnapshot {
    /// Optional plan/update id when Codex includes one.
    #[serde(default)]
    pub id: Option<String>,
    /// Optional explanation for why the plan changed.
    #[serde(default)]
    pub explanation: Option<String>,
    /// Plan entries. Codex has used both `steps` and `items`.
    #[serde(default, alias = "steps")]
    pub items: Vec<CodexPlanItem>,
}

/// A single entry in a Codex todo-list snapshot.
#[derive(Debug, Clone, Deserialize)]
pub struct CodexTodoItem {
    /// Rendered todo item text.
    pub text: String,
    /// `true` if the item has been completed.
    pub completed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_collab_agent_tool_call_alias() {
        let event: CodexJsonEvent = serde_json::from_value(serde_json::json!({
            "type": "item.started",
            "item": {
                "type": "collabAgentToolCall",
                "id": "task-1",
                "tool": "spawnAgent",
                "status": "inProgress",
                "prompt": "Inspect the current mapping"
            }
        }))
        .expect("collabAgentToolCall payload should deserialize");

        match event {
            CodexJsonEvent::ItemStarted {
                item:
                    CodexItem::CollabAgentToolCall {
                        id,
                        tool,
                        status,
                        prompt,
                    },
            } => {
                assert_eq!(id, "task-1");
                assert_eq!(tool, "spawnAgent");
                assert_eq!(status.as_deref(), Some("inProgress"));
                assert_eq!(prompt.as_deref(), Some("Inspect the current mapping"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn deserializes_dynamic_tool_call_item() {
        let event: CodexJsonEvent = serde_json::from_value(serde_json::json!({
            "type": "item.completed",
            "item": {
                "type": "dynamicToolCall",
                "id": "dynamic-tool-1",
                "tool": "acme_lookup",
                "arguments": {
                    "query": "rpc parity"
                },
                "status": "completed",
                "contentItems": [
                    {
                        "type": "text",
                        "text": "Found a match"
                    }
                ]
            }
        }))
        .expect("dynamicToolCall payload should deserialize");

        match event {
            CodexJsonEvent::ItemCompleted {
                item:
                    CodexItem::DynamicToolCall {
                        id,
                        tool,
                        arguments,
                        status,
                        content_items,
                        error,
                    },
            } => {
                assert_eq!(id, "dynamic-tool-1");
                assert_eq!(tool, "acme_lookup");
                assert_eq!(
                    arguments,
                    Some(serde_json::json!({ "query": "rpc parity" }))
                );
                assert_eq!(status.as_deref(), Some("completed"));
                assert_eq!(
                    content_items,
                    vec![serde_json::json!({
                        "type": "text",
                        "text": "Found a match"
                    })]
                );
                assert!(error.is_none());
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn deserializes_image_generation_and_image_view_items() {
        let generation: CodexJsonEvent = serde_json::from_value(serde_json::json!({
            "type": "item.started",
            "item": {
                "type": "imageGeneration",
                "id": "img-gen-1",
                "prompt": "A parity gate robot",
                "image_url": "https://example.com/generated.png"
            }
        }))
        .expect("imageGeneration payload should deserialize");

        match generation {
            CodexJsonEvent::ItemStarted {
                item: CodexItem::ImageGeneration { id, data },
            } => {
                assert_eq!(id, "img-gen-1");
                assert_eq!(
                    data.get("prompt").and_then(Value::as_str),
                    Some("A parity gate robot")
                );
                assert_eq!(
                    data.get("image_url").and_then(Value::as_str),
                    Some("https://example.com/generated.png")
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let view: CodexJsonEvent = serde_json::from_value(serde_json::json!({
            "type": "item.completed",
            "item": {
                "type": "imageView",
                "id": "img-view-1",
                "image_url": "https://example.com/screenshot.png",
                "screen_size": [1440, 900]
            }
        }))
        .expect("imageView payload should deserialize");

        match view {
            CodexJsonEvent::ItemCompleted {
                item: CodexItem::ImageView { id, data },
            } => {
                assert_eq!(id, "img-view-1");
                assert_eq!(
                    data.get("image_url").and_then(Value::as_str),
                    Some("https://example.com/screenshot.png")
                );
                assert_eq!(
                    data.get("screen_size")
                        .and_then(Value::as_array)
                        .map(Vec::len),
                    Some(2)
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn deserializes_command_execution_terminal_interaction() {
        let event: CodexJsonEvent = serde_json::from_value(serde_json::json!({
            "type": "item.updated",
            "item": {
                "type": "command_execution",
                "id": "cmd-1",
                "command": "python3 -c 'input()'",
                "status": "running",
                "terminalInteraction": {
                    "prompt": "Enter stdin",
                    "placeholder": "stdin"
                }
            }
        }))
        .expect("command execution payload should deserialize");

        match event {
            CodexJsonEvent::ItemUpdated {
                item: CodexItem::CommandExecution { id, data, .. },
            } => {
                assert_eq!(id, "cmd-1");
                assert_eq!(
                    data.get("terminalInteraction"),
                    Some(&serde_json::json!({
                        "prompt": "Enter stdin",
                        "placeholder": "stdin"
                    }))
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn deserializes_mcp_tool_call_progress_aliases() {
        let event: CodexJsonEvent = serde_json::from_value(serde_json::json!({
            "type": "item/mcpToolCall/progress",
            "item": {
                "id": "mcp-1"
            },
            "progress": {
                "message": "Waiting for Linear response"
            }
        }))
        .expect("mcp tool progress payload should deserialize");

        match event {
            CodexJsonEvent::McpToolCallProgress { progress } => {
                assert_eq!(progress.tool_use_id(), Some("mcp-1"));
                assert_eq!(progress.message(), Some("Waiting for Linear response"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn deserializes_plan_delta_aliases() {
        let event: CodexJsonEvent = serde_json::from_value(serde_json::json!({
            "type": "item/plan/delta",
            "item": {
                "id": "plan-1"
            },
            "delta": "Break the task into checkpoints"
        }))
        .expect("plan delta payload should deserialize");

        match event {
            CodexJsonEvent::PlanDelta { delta } => {
                assert_eq!(delta.tool_use_id(), Some("plan-1"));
                assert_eq!(delta.delta(), Some("Break the task into checkpoints"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn deserializes_turn_plan_updated_with_top_level_steps() {
        let event: CodexJsonEvent = serde_json::from_value(serde_json::json!({
            "type": "turn/plan/updated",
            "explanation": "Break work into checkpoints",
            "steps": [
                { "content": "Inspect current mapping", "status": "completed" },
                { "text": "Forward plan steps to update_plan", "status": "inProgress" }
            ]
        }))
        .expect("turn/plan/updated payload should deserialize");

        match event {
            CodexJsonEvent::TurnPlanUpdated { update } => {
                assert_eq!(update.explanation(), Some("Break work into checkpoints"));
                assert_eq!(update.items().len(), 2);
                assert_eq!(
                    update.items()[0].content.as_deref(),
                    Some("Inspect current mapping")
                );
                assert_eq!(update.items()[1].status.as_deref(), Some("inProgress"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn deserializes_turn_plan_updated_with_nested_plan_snapshot() {
        let event: CodexJsonEvent = serde_json::from_value(serde_json::json!({
            "type": "turn.plan.updated",
            "plan": {
                "id": "plan-update-1",
                "items": [
                    { "step": "Inspect current mapping", "status": "completed" },
                    { "description": "Forward plan steps to update_plan", "status": "pending" }
                ]
            }
        }))
        .expect("nested turn plan payload should deserialize");

        match event {
            CodexJsonEvent::TurnPlanUpdated { update } => {
                assert_eq!(update.id(), Some("plan-update-1"));
                assert_eq!(update.items().len(), 2);
                assert_eq!(
                    update.items()[0].step.as_deref(),
                    Some("Inspect current mapping")
                );
                assert_eq!(
                    update.items()[1].description.as_deref(),
                    Some("Forward plan steps to update_plan")
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
