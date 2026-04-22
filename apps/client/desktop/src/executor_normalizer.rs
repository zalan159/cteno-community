//! Translate vendor-agnostic [`ExecutorEvent`]s into the session-layer ACP
//! shapes consumed by the Happy Server clients.
//!
//! ## Why a normalizer?
//!
//! The `AgentExecutor` trait emits a narrow cross-vendor event enum. The
//! session layer (both local Tauri IPC and remote Socket.IO) speaks ACP —
//! `{type, callId, ...}` payloads wrapped in encrypted ACP envelopes. This
//! module is the single translation point; every vendor adapter uses the
//! same normalizer, which means adding a new vendor only needs
//! `AgentExecutor::send_message` to return well-formed `ExecutorEvent`s — no
//! separate ACP glue per vendor.
//!
//! ## Translation coverage
//!
//! | `ExecutorEvent`             | ACP output                                  |
//! |-----------------------------|---------------------------------------------|
//! | `SessionReady`              | persist `native_session_id` locally         |
//! | `StreamDelta { Text }`      | transient `{type: "text-delta", text}`      |
//! | `StreamDelta { Thinking }`  | transient delta; persisted on `TurnComplete`|
//! | `StreamDelta { Reasoning }` | transient delta; persisted on `TurnComplete`|
//! | `ToolCallStart`             | persisted `{type: "tool-call", callId, …}`  |
//! | `ToolCallInputDelta`        | transient `{type: "tool-call-delta", …}`    |
//! | `ToolResult`                | persisted `{type: "tool-result", callId, …}`|
//! | `PermissionRequest`         | delegated to [`PermissionHandler`]          |
//! | `InjectedToolInvocation`    | persisted host-owned `tool-call` annotation |
//! | `UsageUpdate`               | persisted `{type: "token_count", ...}`     |
//! | `TurnComplete`              | persisted `{type: "task_complete", id}`     |
//! | `Error`                     | transient/persisted `{type: "error", …}`    |
//! | `NativeEvent`               | logged, vendor-specific ACP side-effects     |
//!
//! ## Streaming vs persisted
//!
//! - **Transient** events (deltas) are forwarded via
//!   [`send_transient_acp_message`] for immediate UI paint.
//! - **Persisted** events (completed thinking, final text, tool-call,
//!   tool-result, task lifecycle, error) go through [`send_acp_message`] so
//!   clients can replay them on reconnect.
//!
//! The normalizer is intentionally stateless across turns: the session layer
//! owns `task_started` / `task_complete` bookkeeping. This module emits
//! `task_complete` on `TurnComplete` and on fatal `Error` events — the
//! session already emits `task_started` before invoking `send_message`.

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use cteno_agent_runtime::hooks as runtime_hooks;
use cteno_host_session_registry::{
    BackgroundTaskCategory, BackgroundTaskRecord, BackgroundTaskStatus,
};
use multi_agent_runtime_core::{
    AgentExecutor, AgentExecutorError, DeltaKind, ExecutorEvent, PermissionDecision, SessionRef,
};
use serde_json::json;
use uuid::Uuid;

use crate::agent_session::{AgentSessionManager, SessionMessage};
use crate::happy_client::permission::{PermissionCheckResult, PermissionHandler};
use crate::happy_client::session::encode_session_payload;
use crate::happy_client::socket::HappySocket;
use crate::session_message_codec::SessionMessageCodec;

const HOST_OWNED_TOOL_METADATA_KEY: &str = "__cteno_host";

#[derive(Debug, Clone, PartialEq)]
struct RecentToolCall {
    call_id: String,
    name: String,
    input: serde_json::Value,
}

fn acp_error_payload(message: impl Into<String>, recoverable: bool) -> serde_json::Value {
    json!({
        "type": "error",
        "message": message.into(),
        "recoverable": recoverable,
    })
}

fn acp_task_complete_payload(task_id: &str) -> serde_json::Value {
    json!({
        "type": "task_complete",
        "id": task_id,
    })
}

fn acp_thinking_payload(text: impl Into<String>) -> serde_json::Value {
    json!({
        "type": "thinking",
        "text": text.into(),
    })
}

fn acp_token_count_payload(usage: &multi_agent_runtime_core::TokenUsage) -> serde_json::Value {
    json!({
        "type": "token_count",
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "cache_creation_input_tokens": usage.cache_creation_tokens,
        "cache_read_input_tokens": usage.cache_read_tokens,
        "reasoning_tokens": usage.reasoning_tokens,
    })
}

fn acp_tool_call_payload(
    call_id: impl Into<String>,
    name: impl Into<String>,
    input: serde_json::Value,
) -> serde_json::Value {
    json!({
        "type": "tool-call",
        "callId": call_id.into(),
        "name": name.into(),
        "input": input,
        "id": Uuid::new_v4().to_string()
    })
}

fn acp_tool_result_payload(
    call_id: impl Into<String>,
    output: Result<String, String>,
) -> serde_json::Value {
    let (text, is_error) = match output {
        Ok(s) => (s, false),
        Err(s) => (s, true),
    };

    json!({
        "type": "tool-result",
        "callId": call_id.into(),
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
        "id": Uuid::new_v4().to_string(),
    })
}

fn host_owned_tool_metadata(request_id: &str) -> serde_json::Value {
    json!({
        "owned": true,
        "requestId": request_id,
        "source": "injected_tool",
    })
}

fn with_host_owned_tool_metadata(input: serde_json::Value, request_id: &str) -> serde_json::Value {
    let metadata = host_owned_tool_metadata(request_id);
    match input {
        serde_json::Value::Object(mut map) => {
            map.insert(HOST_OWNED_TOOL_METADATA_KEY.to_string(), metadata);
            serde_json::Value::Object(map)
        }
        other => {
            let mut map = serde_json::Map::new();
            map.insert("value".to_string(), other);
            map.insert(HOST_OWNED_TOOL_METADATA_KEY.to_string(), metadata);
            serde_json::Value::Object(map)
        }
    }
}

fn remember_recent_tool_call(
    recent_tool_calls: &mut Vec<RecentToolCall>,
    call_id: &str,
    name: &str,
    input: &serde_json::Value,
) {
    recent_tool_calls.push(RecentToolCall {
        call_id: call_id.to_string(),
        name: name.to_string(),
        input: input.clone(),
    });
    if recent_tool_calls.len() > 32 {
        let drop_count = recent_tool_calls.len() - 32;
        recent_tool_calls.drain(..drop_count);
    }
}

fn take_matching_recent_tool_call_id(
    recent_tool_calls: &mut Vec<RecentToolCall>,
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> Option<String> {
    let idx = recent_tool_calls
        .iter()
        .rposition(|entry| entry.name == tool_name && entry.input == *tool_input)?;
    Some(recent_tool_calls.remove(idx).call_id)
}

fn native_event_error_payload(
    provider: &str,
    payload: &serde_json::Value,
) -> Option<serde_json::Value> {
    match payload.get("kind").and_then(serde_json::Value::as_str) {
        Some("rate_limit_event") => {
            let message = match provider {
                "claude" => "Claude API rate limit reached. Retrying automatically.",
                _ => "API rate limit reached. Retrying automatically.",
            };
            Some(acp_error_payload(message, true))
        }
        _ => None,
    }
}

fn native_event_prompt_suggestion_payload(
    payload: &serde_json::Value,
) -> Option<serde_json::Value> {
    let suggestion = payload
        .get("suggestion")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    match payload.get("kind").and_then(serde_json::Value::as_str) {
        Some("prompt_suggestion") => Some(json!({
            "type": "prompt-suggestion",
            "suggestions": [suggestion],
        })),
        _ => None,
    }
}

fn native_event_image_payload(payload: &serde_json::Value) -> Option<serde_json::Value> {
    match payload.get("kind").and_then(serde_json::Value::as_str) {
        Some("assistant_image") => {
            if let Some(image_url) = payload
                .get("image_url")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return Some(json!({
                    "type": "image",
                    "image_url": image_url,
                }));
            }

            let source = payload.get("source")?;
            let source_type = source
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)?;
            if source_type != "base64" {
                return None;
            }
            let media_type = source
                .get("media_type")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let data = source
                .get("data")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;

            Some(json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": media_type,
                    "data": data,
                },
            }))
        }
        _ => None,
    }
}

fn session_state_from_native_payload(payload: &serde_json::Value) -> Option<String> {
    match payload {
        serde_json::Value::String(raw) => serde_json::from_str::<serde_json::Value>(raw)
            .ok()
            .as_ref()
            .and_then(session_state_from_native_payload),
        serde_json::Value::Object(map) => {
            let state = map
                .get("state")
                .or_else(|| map.get("status"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            let kind = map.get("kind").and_then(serde_json::Value::as_str);
            let ty = map.get("type").and_then(serde_json::Value::as_str);
            let subtype = map.get("subtype").and_then(serde_json::Value::as_str);

            match (kind, ty, subtype) {
                (Some("session_state_changed"), _, _)
                | (Some("thread_status_changed"), _, _)
                | (_, Some("thread/status/changed"), _)
                | (_, Some("thread.status.changed"), _)
                | (_, Some("system"), Some("session_state_changed")) => state,
                _ => None,
            }
        }
        _ => None,
    }
}

fn native_event_session_state_payload(payload: &serde_json::Value) -> Option<serde_json::Value> {
    session_state_from_native_payload(payload).map(|state| {
        json!({
            "type": "session-state",
            "state": state,
        })
    })
}

fn native_event_compact_boundary_payload(payload: &serde_json::Value) -> Option<serde_json::Value> {
    match payload.get("kind").and_then(serde_json::Value::as_str) {
        Some("compact_boundary") => Some(json!({
            "type": "message",
            "message": "Compaction completed",
        })),
        _ => None,
    }
}

fn native_event_context_usage_payload(payload: &serde_json::Value) -> Option<serde_json::Value> {
    if payload.get("kind").and_then(serde_json::Value::as_str) != Some("context_usage") {
        return None;
    }

    let total_tokens = payload
        .get("total_tokens")
        .and_then(serde_json::Value::as_u64)?;

    let mut data = serde_json::Map::new();
    data.insert(
        "type".to_string(),
        serde_json::Value::String("context_usage".to_string()),
    );
    data.insert(
        "total_tokens".to_string(),
        serde_json::Value::from(total_tokens),
    );

    if let Some(max_tokens) = payload
        .get("max_tokens")
        .and_then(serde_json::Value::as_u64)
    {
        data.insert(
            "max_tokens".to_string(),
            serde_json::Value::from(max_tokens),
        );
    }
    if let Some(raw_max_tokens) = payload
        .get("raw_max_tokens")
        .and_then(serde_json::Value::as_u64)
    {
        data.insert(
            "raw_max_tokens".to_string(),
            serde_json::Value::from(raw_max_tokens),
        );
    }
    if let Some(percentage) = payload
        .get("percentage")
        .and_then(serde_json::Value::as_f64)
    {
        data.insert(
            "percentage".to_string(),
            serde_json::Value::from(percentage),
        );
    }
    if let Some(model) = payload
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        data.insert(
            "model".to_string(),
            serde_json::Value::String(model.to_string()),
        );
    }

    Some(serde_json::Value::Object(data))
}

fn find_f64_by_keys(value: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    match value {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(found) = map.get(*key).and_then(|candidate| {
                    candidate
                        .as_f64()
                        .or_else(|| candidate.as_u64().map(|n| n as f64))
                        .or_else(|| candidate.as_i64().map(|n| n as f64))
                }) {
                    return Some(found);
                }
            }
            map.values()
                .find_map(|nested| find_f64_by_keys(nested, keys))
        }
        serde_json::Value::Array(items) => items
            .iter()
            .find_map(|nested| find_f64_by_keys(nested, keys)),
        _ => None,
    }
}

fn find_u64_by_keys(value: &serde_json::Value, keys: &[&str]) -> Option<u64> {
    match value {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(found) = map.get(*key).and_then(serde_json::Value::as_u64) {
                    return Some(found);
                }
            }
            map.values()
                .find_map(|nested| find_u64_by_keys(nested, keys))
        }
        serde_json::Value::Array(items) => items
            .iter()
            .find_map(|nested| find_u64_by_keys(nested, keys)),
        _ => None,
    }
}

fn find_bool_by_keys(value: &serde_json::Value, keys: &[&str]) -> Option<bool> {
    match value {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(found) = map.get(*key).and_then(serde_json::Value::as_bool) {
                    return Some(found);
                }
            }
            map.values()
                .find_map(|nested| find_bool_by_keys(nested, keys))
        }
        serde_json::Value::Array(items) => items
            .iter()
            .find_map(|nested| find_bool_by_keys(nested, keys)),
        _ => None,
    }
}

fn find_string_by_keys(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(found) = map
                    .get(*key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|candidate| !candidate.is_empty())
                    .map(str::to_string)
                {
                    return Some(found);
                }
            }
            map.values()
                .find_map(|nested| find_string_by_keys(nested, keys))
        }
        serde_json::Value::Array(items) => items
            .iter()
            .find_map(|nested| find_string_by_keys(nested, keys)),
        _ => None,
    }
}

fn claude_task_call_id(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("tool_use_id")
        .or_else(|| payload.get("task_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn claude_task_description(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("description")
        .or_else(|| payload.get("summary"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn claude_task_started_payload(payload: &serde_json::Value) -> Option<serde_json::Value> {
    if payload.get("kind").and_then(serde_json::Value::as_str) != Some("task_started") {
        return None;
    }

    let task_id = payload.get("task_id")?.as_str()?.trim();
    if task_id.is_empty() {
        return None;
    }

    let mut data = serde_json::Map::new();
    data.insert(
        "type".to_string(),
        serde_json::Value::String("task_started".to_string()),
    );
    data.insert(
        "id".to_string(),
        serde_json::Value::String(task_id.to_string()),
    );

    if let Some(description) = claude_task_description(payload) {
        data.insert(
            "description".to_string(),
            serde_json::Value::String(description),
        );
    }

    if let Some(task_type) = payload
        .get("task_type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        data.insert(
            "taskType".to_string(),
            serde_json::Value::String(task_type.to_string()),
        );
    }

    Some(serde_json::Value::Object(data))
}

fn claude_task_tool_call_payload(payload: &serde_json::Value) -> Option<serde_json::Value> {
    if payload.get("kind").and_then(serde_json::Value::as_str) != Some("task_started") {
        return None;
    }

    let call_id = claude_task_call_id(payload)?;
    let description = claude_task_description(payload)?;
    let task_id = payload
        .get("task_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    let mut input = serde_json::Map::new();
    input.insert(
        "description".to_string(),
        serde_json::Value::String(description.clone()),
    );
    input.insert(
        "taskId".to_string(),
        serde_json::Value::String(task_id.to_string()),
    );
    input.insert("prompt".to_string(), serde_json::Value::String(description));
    if let Some(task_type) = payload
        .get("task_type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        input.insert(
            "taskType".to_string(),
            serde_json::Value::String(task_type.to_string()),
        );
    }
    if let Some(uuid) = payload
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        input.insert(
            "uuid".to_string(),
            serde_json::Value::String(uuid.to_string()),
        );
    }
    if let Some(session_id) = payload
        .get("session_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        input.insert(
            "sessionId".to_string(),
            serde_json::Value::String(session_id.to_string()),
        );
    }

    Some(json!({
        "type": "tool-call",
        "callId": call_id,
        "name": "Task",
        "input": serde_json::Value::Object(input),
        "id": Uuid::new_v4().to_string(),
    }))
}

fn claude_task_progress_delta_payload(payload: &serde_json::Value) -> Option<serde_json::Value> {
    if payload.get("kind").and_then(serde_json::Value::as_str) != Some("task_progress") {
        return None;
    }

    let call_id = claude_task_call_id(payload)?;
    let task_id = payload
        .get("task_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let summary = payload
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let description = summary
        .clone()
        .or_else(|| claude_task_description(payload))
        .filter(|value| !value.is_empty())?;

    let mut patch = serde_json::Map::new();
    patch.insert(
        "description".to_string(),
        serde_json::Value::String(description.clone()),
    );
    patch.insert(
        "taskId".to_string(),
        serde_json::Value::String(task_id.to_string()),
    );
    if let Some(summary) = summary {
        patch.insert("summary".to_string(), serde_json::Value::String(summary));
    }
    if let Some(last_tool_name) = payload
        .get("last_tool_name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        patch.insert(
            "lastToolName".to_string(),
            serde_json::Value::String(last_tool_name.to_string()),
        );
    }
    if let Some(task_type) = payload
        .get("task_type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        patch.insert(
            "taskType".to_string(),
            serde_json::Value::String(task_type.to_string()),
        );
    }
    if let Some(usage) = payload.get("usage").cloned() {
        patch.insert("usage".to_string(), usage);
    }

    Some(json!({
        "type": "tool-call-delta",
        "callId": call_id,
        "patch": serde_json::Value::Object(patch),
    }))
}

fn claude_task_notification_tool_result_payload(
    payload: &serde_json::Value,
) -> Option<serde_json::Value> {
    if payload.get("kind").and_then(serde_json::Value::as_str) != Some("task_notification") {
        return None;
    }

    let call_id = claude_task_call_id(payload)?;
    let status = payload
        .get("status")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("completed");
    let summary = payload
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(status);

    let mut content = vec![json!({ "type": "text", "text": summary })];
    if let Some(output_file) = payload
        .get("output_file")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        content.push(json!({
            "type": "text",
            "text": format!("output_file: {output_file}")
        }));
    }

    Some(json!({
        "type": "tool-result",
        "callId": call_id,
        "content": content,
        "isError": status != "completed",
        "id": Uuid::new_v4().to_string(),
    }))
}

fn claude_task_complete_payload(payload: &serde_json::Value) -> Option<serde_json::Value> {
    if payload.get("kind").and_then(serde_json::Value::as_str) != Some("task_notification") {
        return None;
    }

    let task_id = payload
        .get("task_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let summary = payload
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());

    Some(match summary {
        Some(summary) => json!({
            "type": "task_complete",
            "id": task_id,
            "summary": summary,
            "status": payload
                .get("status")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("completed"),
        }),
        None => json!({
            "type": "task_complete",
            "id": task_id,
            "status": payload
                .get("status")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("completed"),
        }),
    })
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn claude_background_task_started_record(
    session_id: &str,
    payload: &serde_json::Value,
    started_at: i64,
) -> Option<BackgroundTaskRecord> {
    if payload.get("kind").and_then(serde_json::Value::as_str) != Some("task_started") {
        return None;
    }

    let task_id = payload
        .get("task_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let task_type = payload
        .get("task_type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("other");

    Some(BackgroundTaskRecord {
        task_id: task_id.to_string(),
        session_id: session_id.to_string(),
        vendor: "claude".to_string(),
        category: BackgroundTaskCategory::ExecutionTask,
        task_type: task_type.to_string(),
        description: claude_task_description(payload),
        summary: payload
            .get("summary")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        status: BackgroundTaskStatus::Running,
        started_at,
        completed_at: None,
        tool_use_id: payload
            .get("tool_use_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        output_file: payload
            .get("output_file")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        vendor_extra: payload.clone(),
    })
}

fn update_claude_background_task_registry(
    session_id: &str,
    payload: &serde_json::Value,
    timestamp_millis: i64,
) -> Option<BackgroundTaskRecord> {
    let registry = match crate::local_services::background_task_registry() {
        Ok(registry) => registry,
        Err(err) => {
            log::debug!(
                "[Normalizer {}] Claude background task registry unavailable: {}",
                session_id,
                err
            );
            return None;
        }
    };

    match payload.get("kind").and_then(serde_json::Value::as_str) {
        Some("task_started") => {
            let record =
                claude_background_task_started_record(session_id, payload, timestamp_millis)?;
            registry.upsert(record.clone());
            registry.get(&record.task_id)
        }
        Some("task_progress") => {
            let task_id = payload
                .get("task_id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let summary = payload
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);

            registry.update_status(task_id, BackgroundTaskStatus::Running, None, summary);
            registry.get(task_id)
        }
        Some("task_notification") => {
            let task_id = payload
                .get("task_id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let status = payload
                .get("status")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("completed");
            let summary = payload
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or_else(|| Some(status.to_string()));

            registry.update_status(
                task_id,
                BackgroundTaskStatus::from_str(status).unwrap_or(BackgroundTaskStatus::Unknown),
                Some(timestamp_millis),
                summary,
            );
            registry.get(task_id)
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexGuardianReviewCompletion {
    request_id: String,
    status: &'static str,
    risk_level: Option<String>,
}

fn is_codex_guardian_permission_request(tool_input: &serde_json::Value) -> bool {
    tool_input
        .get("__codex_guardian_review")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn codex_guardian_permission_description(tool_input: &serde_json::Value) -> Option<String> {
    codex_guardian_risk_level(tool_input).map(|risk_level| format!("Risk level: {risk_level}"))
}

fn codex_guardian_risk_level(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("riskLevel")
        .or_else(|| payload.get("risk_level"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn codex_guardian_review_status(payload: &serde_json::Value) -> &'static str {
    let explicit = payload
        .get("result")
        .or_else(|| payload.get("decision"))
        .or_else(|| payload.get("outcome"))
        .or_else(|| payload.get("status"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase);

    match explicit.as_deref() {
        Some("approved" | "approve" | "allowed" | "allow" | "accepted" | "auto_approved") => {
            "approved"
        }
        Some("denied" | "deny" | "rejected" | "reject" | "blocked") => "denied",
        Some("canceled" | "cancelled" | "aborted" | "abort") => "canceled",
        _ => {
            if payload
                .get("autoApproved")
                .or_else(|| payload.get("auto_approved"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
                || payload
                    .get("approved")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            {
                "approved"
            } else {
                // Codex only emits the completed review item once the guardian
                // decision is known. In the absence of an explicit terminal
                // field, treat it as approved so the frontend does not keep a
                // stale pending permission card open forever.
                "approved"
            }
        }
    }
}

fn codex_guardian_completion(payload: &serde_json::Value) -> Option<CodexGuardianReviewCompletion> {
    let map = payload.as_object()?;
    if map.get("kind").and_then(serde_json::Value::as_str)? != "codex_guardian_review_completed" {
        return None;
    }

    let request_id = map.get("request_id")?.as_str()?.to_string();
    let review = map.get("review")?;
    Some(CodexGuardianReviewCompletion {
        request_id,
        status: codex_guardian_review_status(review),
        risk_level: codex_guardian_risk_level(review),
    })
}

fn codex_guardian_tool_result_payload(
    completion: &CodexGuardianReviewCompletion,
) -> serde_json::Value {
    let approved = completion.status == "approved";
    let status_text = if approved {
        "auto-approved"
    } else {
        completion.status
    };
    let message = match completion.risk_level.as_deref() {
        Some(risk_level) => format!("Codex Guardian {status_text} ({risk_level} risk)"),
        None => format!("Codex Guardian {status_text}"),
    };

    json!({
        "type": "tool-result",
        "callId": completion.request_id,
        "content": [{ "type": "text", "text": message }],
        "isError": !approved,
        "permissions": {
            "date": chrono::Utc::now().timestamp_millis(),
            "result": if approved { "approved" } else { "denied" },
            "decision": if approved { "approved" } else { "denied" },
        },
        "id": Uuid::new_v4().to_string(),
    })
}

/// Stateful adapter driving one turn's worth of `ExecutorEvent`s into ACP
/// messages on a session-scoped Socket.IO connection.
#[derive(Clone)]
pub struct ExecutorNormalizer {
    session_id: String,
    socket: Arc<HappySocket>,
    message_codec: SessionMessageCodec,
    stream_callback: Option<crate::llm::StreamCallback>,
    permission_handler: Arc<PermissionHandler>,
    /// Task id emitted by the session layer when it started the turn. The
    /// normalizer reuses it on `TurnComplete` so client-side task state
    /// toggles cleanly.
    task_id: String,
    /// Executor that produced the event stream — used to reply to
    /// `PermissionRequest` events via `respond_to_permission` once the
    /// host-side `PermissionHandler` has resolved the user decision.
    executor: Arc<dyn AgentExecutor>,
    /// Session this turn belongs to (needed by `respond_to_permission`).
    session_ref: SessionRef,
    /// Happy server URL + auth token, forwarded to `PermissionHandler` so it
    /// can push notify mobile clients about pending prompts. Kept alongside
    /// the handler because the normalizer owns the per-turn socket state.
    server_url: String,
    auth_token: String,
    /// Local SQLite store path used to persist executor resume metadata.
    db_path: PathBuf,
    /// Shared session context token counter used by heartbeat updates.
    context_tokens: Option<Arc<AtomicU32>>,
    /// Shared compression threshold used by compaction UI.
    compression_threshold: Option<Arc<AtomicU32>>,
    /// Whether we've sent `stream-start` callback (sent on first StreamDelta).
    stream_started: Arc<std::sync::atomic::AtomicBool>,
    /// Thinking/reasoning deltas are transient while streaming. Accumulate
    /// them so the completed turn can replay as a regular chat block.
    accumulated_thinking: Arc<std::sync::Mutex<String>>,
    /// Recent tool-call starts so InjectedToolInvocation can annotate the
    /// matching persisted tool card instead of creating an unclosable duplicate.
    recent_tool_calls: Arc<std::sync::Mutex<Vec<RecentToolCall>>>,
}

pub(crate) async fn surface_terminal_executor_error(
    normalizer: &ExecutorNormalizer,
    message: impl Into<String>,
) -> Result<(), String> {
    let should_stop = normalizer
        .process_event(ExecutorEvent::Error {
            message: message.into(),
            recoverable: false,
        })
        .await?;
    if !should_stop {
        log::warn!(
            "[Normalizer {}] fatal executor error did not stop the turn",
            normalizer.session_id
        );
    }
    Ok(())
}

pub(crate) async fn surface_executor_failure(
    normalizer: &ExecutorNormalizer,
    error: &AgentExecutorError,
) -> Result<(), String> {
    surface_terminal_executor_error(normalizer, user_visible_executor_error(error)).await
}

fn cteno_auth_guidance(message: &str) -> Option<String> {
    let lower = message.to_ascii_lowercase();
    let is_auth_gate = lower.contains("not logged in")
        || lower.contains("requires happy proxy auth")
        || lower.contains("no cteno api key configured")
        || lower.contains("set cteno_agent_api_key");
    if !is_auth_gate {
        return None;
    }
    Some(format!(
        "{message}。请先登录，或为 Cteno 配置 API key 后再试。"
    ))
}

pub(crate) fn user_visible_executor_error(error: &AgentExecutorError) -> String {
    let base = match error {
        AgentExecutorError::Io(message)
        | AgentExecutorError::Protocol(message)
        | AgentExecutorError::Vendor { message, .. } => message.clone(),
        AgentExecutorError::Timeout { operation, seconds } => {
            format!("timeout after {seconds}s: {operation}")
        }
        AgentExecutorError::SubprocessExited { code, stderr } => {
            match (code, stderr.trim().is_empty()) {
                (Some(code), true) => format!("subprocess exited unexpectedly (code {code})."),
                (None, true) => "subprocess exited unexpectedly.".to_string(),
                (Some(code), false) => {
                    format!("subprocess exited unexpectedly (code {code}). Last stderr: {stderr}")
                }
                (None, false) => {
                    format!("subprocess exited unexpectedly. Last stderr: {stderr}")
                }
            }
        }
        _ => error.to_string(),
    };
    cteno_auth_guidance(&base).unwrap_or(base)
}

impl ExecutorNormalizer {
    /// Build a normalizer for one turn. `task_id` should match the id the
    /// session layer used in its `task_started` ACP message.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: String,
        socket: Arc<HappySocket>,
        message_codec: SessionMessageCodec,
        stream_callback: Option<crate::llm::StreamCallback>,
        permission_handler: Arc<PermissionHandler>,
        task_id: String,
        executor: Arc<dyn AgentExecutor>,
        session_ref: SessionRef,
        server_url: String,
        auth_token: String,
        db_path: PathBuf,
        context_tokens: Option<Arc<AtomicU32>>,
        compression_threshold: Option<Arc<AtomicU32>>,
    ) -> Self {
        Self {
            session_id,
            socket,
            message_codec,
            stream_callback,
            permission_handler,
            task_id,
            executor,
            session_ref,
            server_url,
            auth_token,
            db_path,
            context_tokens,
            compression_threshold,
            stream_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            accumulated_thinking: Arc::new(std::sync::Mutex::new(String::new())),
            recent_tool_calls: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn update_context_tokens(&self, usage: &multi_agent_runtime_core::TokenUsage) {
        let Some(context_tokens) = self.context_tokens.as_ref() else {
            return;
        };

        let total_input_tokens = usage
            .input_tokens
            .saturating_add(usage.cache_creation_tokens)
            .saturating_add(usage.cache_read_tokens);
        if total_input_tokens == 0 {
            return;
        }

        context_tokens.store(
            total_input_tokens.min(u32::MAX as u64) as u32,
            Ordering::SeqCst,
        );
    }

    fn update_context_tokens_total(&self, total_tokens: u64) {
        let Some(context_tokens) = self.context_tokens.as_ref() else {
            return;
        };
        context_tokens.store(total_tokens.min(u32::MAX as u64) as u32, Ordering::SeqCst);
    }

    fn remember_tool_call(&self, call_id: &str, name: &str, input: &serde_json::Value) {
        let Ok(mut guard) = self.recent_tool_calls.lock() else {
            return;
        };
        remember_recent_tool_call(&mut guard, call_id, name, input);
    }

    fn take_matching_tool_call_id(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
    ) -> Option<String> {
        let Ok(mut guard) = self.recent_tool_calls.lock() else {
            return None;
        };
        take_matching_recent_tool_call_id(&mut guard, tool_name, tool_input)
    }

    fn forget_tool_call(&self, call_id: &str) {
        let Ok(mut guard) = self.recent_tool_calls.lock() else {
            return;
        };
        if let Some(idx) = guard.iter().rposition(|entry| entry.call_id == call_id) {
            guard.remove(idx);
        }
    }

    fn append_thinking_delta(&self, content: &str) {
        let Ok(mut guard) = self.accumulated_thinking.lock() else {
            log::warn!(
                "[Normalizer {}] thinking accumulator lock poisoned; dropping delta",
                self.session_id
            );
            return;
        };
        guard.push_str(content);
    }

    async fn flush_accumulated_thinking(&self) -> Result<(), String> {
        let thinking = {
            let Ok(mut guard) = self.accumulated_thinking.lock() else {
                log::warn!(
                    "[Normalizer {}] thinking accumulator lock poisoned; cannot persist thinking",
                    self.session_id
                );
                return Ok(());
            };
            if guard.trim().is_empty() {
                return Ok(());
            }
            std::mem::take(&mut *guard)
        };

        self.send_persisted(acp_thinking_payload(thinking)).await
    }

    /// Dispatch one event. Returns `Ok(true)` when the turn is finished
    /// (`TurnComplete`), `Ok(false)` to keep consuming, and `Err(_)` for
    /// fatal transport failures — the caller should propagate.
    pub async fn process_event(&self, event: ExecutorEvent) -> Result<bool, String> {
        match event {
            ExecutorEvent::SessionReady { native_session_id } => {
                if let Err(e) =
                    crate::happy_client::session_helpers::upsert_agent_session_native_session_id(
                        &self.db_path,
                        &self.session_id,
                        &self.session_ref.vendor,
                        native_session_id.as_str(),
                    )
                {
                    log::warn!(
                        "[Normalizer {}] Failed to persist native_session_id for vendor {}: {}",
                        self.session_id,
                        self.session_ref.vendor,
                        e
                    );
                }
                log::debug!(
                    "[Normalizer {}] SessionReady (native_id={})",
                    self.session_id,
                    native_session_id
                );
                Ok(false)
            }

            ExecutorEvent::StreamDelta { kind, content } => {
                // Emit stream-start on the first delta so frontend sets thinking=true.
                if !self
                    .stream_started
                    .swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    self.emit_stream_callback(json!({ "type": "stream-start" }))
                        .await;
                }
                let ty = match kind {
                    DeltaKind::Text => "text-delta",
                    DeltaKind::Thinking | DeltaKind::Reasoning => {
                        self.append_thinking_delta(&content);
                        "thinking-delta"
                    }
                };
                let payload = json!({ "type": ty, "text": content });
                let (send_result, ()) = tokio::join!(
                    self.send_transient(payload.clone()),
                    self.emit_stream_callback(payload),
                );
                send_result?;
                Ok(false)
            }

            ExecutorEvent::ToolCallStart {
                tool_use_id,
                name,
                input,
                partial: _,
            } => {
                // `partial` is currently ignored — adapters either emit a
                // single complete ToolCallStart (cteno, codex) or stream
                // partials via ToolCallInputDelta (claude). UI consumers
                // already debounce partials.
                self.remember_tool_call(&tool_use_id, &name, &input);
                let acp_data = acp_tool_call_payload(tool_use_id, name, input);
                self.send_persisted(acp_data).await?;
                Ok(false)
            }

            ExecutorEvent::ToolCallInputDelta {
                tool_use_id,
                json_patch,
            } => {
                // Transient — UI merges client-side.
                self.send_transient(json!({
                    "type": "tool-call-delta",
                    "callId": tool_use_id,
                    "patch": json_patch,
                }))
                .await?;
                Ok(false)
            }

            ExecutorEvent::ToolResult {
                tool_use_id,
                output,
            } => {
                // ACP tool-result payloads from desktop use `content` as a
                // text-block array (`[{ type: "text", text: "..." }]`).
                // The frontend normalizer flattens that array back to the
                // reducer's tool.result value, so keep this shape stable.
                self.forget_tool_call(&tool_use_id);
                let acp_data = acp_tool_result_payload(tool_use_id, output);
                self.send_persisted(acp_data).await?;
                Ok(false)
            }

            ExecutorEvent::PermissionRequest {
                request_id,
                tool_name,
                tool_input,
            } => {
                if is_codex_guardian_permission_request(&tool_input) {
                    let description = codex_guardian_permission_description(&tool_input);
                    self.permission_handler
                        .publish_permission_request(
                            &self.socket,
                            &self.message_codec,
                            &request_id,
                            &tool_name,
                            &tool_input,
                            description.as_deref(),
                        )
                        .await;
                    return Ok(false);
                }

                log::info!(
                    "[Normalizer {}] PermissionRequest tool={} id={}",
                    self.session_id,
                    tool_name,
                    request_id
                );

                // Evaluate fast-path pre-approval (read-only, session-allowed,
                // bypass / plan mode) without involving the user. If the tool
                // can be decided immediately we reply to the executor inline
                // and keep the stream draining.
                if let Some(pre) = self
                    .permission_handler
                    .evaluate_pre_approval(&tool_name, &tool_input)
                {
                    let decision = match pre {
                        PermissionCheckResult::Allowed => PermissionDecision::Allow,
                        PermissionCheckResult::Denied(_) => PermissionDecision::Deny,
                        PermissionCheckResult::Aborted => PermissionDecision::Abort,
                    };
                    log::info!(
                        "[Normalizer {}] Permission pre-approval for {}: {:?}",
                        self.session_id,
                        request_id,
                        decision
                    );
                    let is_abort = matches!(decision, PermissionDecision::Abort);
                    self.executor
                        .respond_to_permission(&self.session_ref, request_id, decision)
                        .await
                        .map_err(|e| format!("respond_to_permission failed: {e}"))?;
                    return Ok(is_abort);
                }

                // User approval required. Split the flow:
                //   1. Register a pending oneshot so the session RPC callback
                //      has somewhere to deliver the reply.
                //   2. Publish the ACP `permission-request` + agent-state
                //      update so the UI shows the approval card.
                //   3. Spawn a detached task that awaits the reply (with a
                //      120s timeout), calls `executor.respond_to_permission`,
                //      and closes the pending `agentState.completedRequests`
                //      entry.
                // Crucially, `process_event` returns immediately (Ok(false))
                // so the stdout reader keeps draining further events from the
                // executor. This matches the Happy Coder design where the
                // permission handler returns a Promise and the RPC handler
                // just resolves the shared map.
                let rx = self
                    .permission_handler
                    .register_pending_request(&request_id, &tool_name);

                self.permission_handler
                    .publish_permission_request(
                        &self.socket,
                        &self.message_codec,
                        &request_id,
                        &tool_name,
                        &tool_input,
                        None,
                    )
                    .await;
                // Local-mode persistence + Tauri event fan-out happens inside
                // `HappySocket::send_message` → `LocalEventSink::on_message`
                // (see `happy_client::local_sink`). Same path for ACP
                // update-state (agent state) via `on_state_update`.

                // Fire-and-forget push notification so mobile clients wake up.
                {
                    let push_server_url = self.server_url.clone();
                    let push_auth_token = self.auth_token.clone();
                    let push_session_id = self.session_id.clone();
                    let push_call_id = request_id.clone();
                    let push_tool_name = tool_name.clone();
                    tokio::spawn(async move {
                        PermissionHandler::send_push_notification_public(
                            &push_server_url,
                            &push_auth_token,
                            &push_session_id,
                            &push_call_id,
                            &push_tool_name,
                        )
                        .await;
                    });
                }

                // Detached resolver task. Owns:
                // - the oneshot receiver (→ timeout + user reply)
                // - a clone of executor + session_ref (→ respond_to_permission)
                // - a clone of permission_handler + socket + codec (→ agent-state cleanup)
                let permission_handler = self.permission_handler.clone();
                let socket = self.socket.clone();
                let message_codec = self.message_codec;
                let executor = self.executor.clone();
                let session_ref = self.session_ref.clone();
                let tool_name_for_task = tool_name.clone();
                let request_id_for_task = request_id.clone();
                let normalizer_id = self.session_id.clone();
                tokio::spawn(async move {
                    // Capture the RPC-response shape so we can both apply the
                    // side effects (session-allowed tools, mode change) AND
                    // echo the fields back into completedRequests for the
                    // frontend reducer. Without the echo the UI shows just
                    // "approved" even when the user picked "approved-for-session".
                    let mut response_decision: Option<String> = None;
                    let mut response_mode: Option<String> = None;
                    let mut response_allow_tools: Option<Vec<String>> = None;
                    let mut response_reason: Option<String> = None;
                    // Vendor-chosen option id (gemini-style option buttons).
                    // When set, we build a `PermissionDecision::SelectedOption`
                    // instead of the Allow/Deny/Abort 3-way mapping so the
                    // adapter can echo it back verbatim.
                    let mut response_vendor_option: Option<String> = None;

                    let result = match tokio::time::timeout(
                        tokio::time::Duration::from_secs(120),
                        rx,
                    )
                    .await
                    {
                        Ok(Ok(response)) => {
                            log::info!(
                                "[Normalizer {}] Permission reply for {}: approved={} decision={:?} mode={:?} allow_tools={:?} vendor_option={:?}",
                                normalizer_id,
                                request_id_for_task,
                                response.approved,
                                response.decision,
                                response.mode,
                                response.allow_tools,
                                response.vendor_option,
                            );
                            response_decision = response.decision.clone();
                            response_mode = response.mode.clone();
                            response_allow_tools = response.allow_tools.clone();
                            response_vendor_option = response.vendor_option.clone();
                            permission_handler.apply_response(
                                response,
                                &request_id_for_task,
                                &tool_name_for_task,
                            )
                        }
                        Ok(Err(_)) => {
                            log::warn!(
                                "[Normalizer {}] Permission channel closed for {}",
                                normalizer_id,
                                request_id_for_task
                            );
                            response_reason = Some("Permission channel closed".to_string());
                            PermissionCheckResult::Denied("Permission channel closed".to_string())
                        }
                        Err(_) => {
                            log::warn!(
                                "[Normalizer {}] Permission timeout for {} → Deny",
                                normalizer_id,
                                request_id_for_task
                            );
                            response_reason = Some("Permission timeout (120s)".to_string());
                            PermissionCheckResult::Denied("Permission timeout (120s)".to_string())
                        }
                    };

                    permission_handler.clear_pending_request(&request_id_for_task);

                    // If the frontend echoed back a vendor option id, the
                    // user picked from the vendor's own list (e.g. gemini's
                    // proceed_always / cancel). Prefer that over the 3-way
                    // mapping so the adapter can forward the id untouched.
                    let decision = if let Some(option_id) = response_vendor_option.clone() {
                        PermissionDecision::SelectedOption { option_id }
                    } else {
                        match &result {
                            PermissionCheckResult::Allowed => PermissionDecision::Allow,
                            PermissionCheckResult::Denied(_) => PermissionDecision::Deny,
                            PermissionCheckResult::Aborted => PermissionDecision::Abort,
                        }
                    };

                    if let Err(e) = executor
                        .respond_to_permission(
                            &session_ref,
                            request_id_for_task.clone(),
                            decision.clone(),
                        )
                        .await
                    {
                        log::warn!(
                            "[Normalizer {}] respond_to_permission failed for {}: {}",
                            normalizer_id,
                            request_id_for_task,
                            e
                        );
                    }

                    // Frontend schema: {approved, denied, canceled}.
                    let status = match &decision {
                        PermissionDecision::Allow | PermissionDecision::SelectedOption { .. } => {
                            "approved"
                        }
                        PermissionDecision::Deny => "denied",
                        PermissionDecision::Abort => "canceled",
                    };
                    permission_handler
                        .complete_permission_request(
                            &socket,
                            &message_codec,
                            &request_id_for_task,
                            status,
                            response_decision.as_deref(),
                            response_mode.as_deref(),
                            response_allow_tools.as_deref(),
                            response_reason.as_deref(),
                        )
                        .await;
                });

                // Keep draining executor stream. The resolver task above is
                // the only code that touches `respond_to_permission` for this
                // request.
                Ok(false)
            }

            ExecutorEvent::InjectedToolInvocation {
                request_id,
                tool_name,
                tool_input,
            } => {
                let matched_call_id = self.take_matching_tool_call_id(&tool_name, &tool_input);
                let call_id = matched_call_id
                    .clone()
                    .unwrap_or_else(|| request_id.clone());
                let annotated_input = with_host_owned_tool_metadata(tool_input, &request_id);

                if matched_call_id.is_none() {
                    log::warn!(
                        "[Normalizer {}] InjectedToolInvocation without prior ToolCallStart match; falling back to request_id={} for tool={}",
                        self.session_id,
                        request_id,
                        tool_name
                    );
                }

                self.send_persisted(acp_tool_call_payload(call_id, tool_name, annotated_input))
                    .await?;
                Ok(false)
            }

            ExecutorEvent::UsageUpdate(usage) => {
                log::debug!(
                    "[Normalizer {}] UsageUpdate in={} out={} cache_read={}",
                    self.session_id,
                    usage.input_tokens,
                    usage.output_tokens,
                    usage.cache_read_tokens
                );
                self.update_context_tokens(&usage);
                self.send_persisted(acp_token_count_payload(&usage)).await?;
                Ok(false)
            }

            ExecutorEvent::TurnComplete {
                final_text,
                iteration_count,
                usage,
            } => {
                log::info!(
                    "[Normalizer {}] TurnComplete iterations={} in={} out={}",
                    self.session_id,
                    iteration_count,
                    usage.input_tokens,
                    usage.output_tokens
                );
                self.update_context_tokens(&usage);
                self.flush_accumulated_thinking().await?;
                if let Some(text) = final_text.as_ref().filter(|s| !s.is_empty()) {
                    // Flush final assistant text as a persisted ACP message
                    // so reconnecting clients see it.
                    let acp_data = json!({
                        "type": "message",
                        "message": text,
                    });
                    self.send_persisted(acp_data).await?;
                }
                self.send_persisted(acp_task_complete_payload(&self.task_id))
                    .await?;
                self.emit_stream_callback(json!({ "type": "stream-end" }))
                    .await;
                self.emit_stream_callback(json!({ "type": "finished" }))
                    .await;
                Ok(true)
            }

            ExecutorEvent::Error {
                message,
                recoverable,
            } => {
                log::warn!(
                    "[Normalizer {}] executor error (recoverable={}): {}",
                    self.session_id,
                    recoverable,
                    message
                );
                let callback_message = message.clone();
                let acp_data = acp_error_payload(message, recoverable);
                // Always persist so the user sees the error as a chat bubble.
                // Recoverable errors (e.g. "not logged in", transient network
                // blip) used to go out as transient-only events that the user
                // never noticed. Persisting them matches the principle that
                // anything that ends a turn — or even meaningfully affects it
                // — should be visible in chat history.
                if !recoverable {
                    self.flush_accumulated_thinking().await?;
                }
                self.send_persisted(acp_data).await?;
                self.emit_stream_callback(json!({
                    "type": "error",
                    "message": callback_message,
                }))
                .await;
                if !recoverable {
                    // Fatal: close the turn explicitly.
                    self.send_persisted(acp_task_complete_payload(&self.task_id))
                        .await?;
                    self.emit_stream_callback(json!({ "type": "stream-end" }))
                        .await;
                    self.emit_stream_callback(json!({ "type": "finished" }))
                        .await;
                }
                // Recoverable errors leave the stream open so more events can
                // follow; the caller's TurnComplete (if any) will close it.
                Ok(!recoverable)
            }

            ExecutorEvent::NativeEvent { provider, payload } => {
                log::info!(
                    "[Normalizer {}] NativeEvent provider={} payload={}",
                    self.session_id,
                    provider,
                    serde_json::to_string(&payload)
                        .unwrap_or_else(|_| format!("{payload}"))
                        .chars()
                        .take(400)
                        .collect::<String>()
                );
                if provider.as_ref() == "codex" {
                    if let Some(completion) = codex_guardian_completion(&payload) {
                        self.permission_handler
                            .complete_permission_request(
                                &self.socket,
                                &self.message_codec,
                                &completion.request_id,
                                completion.status,
                                None,
                                None,
                                None,
                                None,
                            )
                            .await;
                        self.send_persisted(codex_guardian_tool_result_payload(&completion))
                            .await?;
                        return Ok(false);
                    }
                }
                if let Some(acp_data) = native_event_error_payload(provider.as_ref(), &payload) {
                    self.send_transient(acp_data).await?;
                }
                if provider.as_ref() == "claude" {
                    if let Some(acp_data) = claude_task_started_payload(&payload) {
                        self.send_persisted(acp_data).await?;
                    }
                    if let Some(acp_data) = claude_task_tool_call_payload(&payload) {
                        self.send_persisted(acp_data).await?;
                    }
                    if let Some(acp_data) = claude_task_progress_delta_payload(&payload) {
                        self.send_transient(acp_data).await?;
                    }
                    if let Some(acp_data) = claude_task_notification_tool_result_payload(&payload) {
                        self.send_persisted(acp_data).await?;
                    }
                    if let Some(acp_data) = claude_task_complete_payload(&payload) {
                        self.send_persisted(acp_data).await?;
                    }
                    self.sync_claude_background_task(&payload).await;
                }
                if let Some(acp_data) = native_event_prompt_suggestion_payload(&payload) {
                    self.send_persisted(acp_data).await?;
                }
                if let Some(acp_data) = native_event_image_payload(&payload) {
                    self.send_persisted(acp_data).await?;
                }
                if let Some(acp_data) = native_event_context_usage_payload(&payload) {
                    if let Some(total_tokens) = payload
                        .get("total_tokens")
                        .and_then(serde_json::Value::as_u64)
                    {
                        self.update_context_tokens_total(total_tokens);
                    }
                    self.send_persisted(acp_data).await?;
                }
                if let Some(acp_data) = native_event_session_state_payload(&payload) {
                    // Persist session-state so the normal sync path can drive
                    // the existing session status indicator off ACP updates.
                    self.send_persisted(acp_data).await?;
                }
                if let Some(acp_data) = native_event_compact_boundary_payload(&payload) {
                    self.send_persisted(acp_data).await?;
                }
                Ok(false)
            }
        }
    }

    async fn send_persisted(&self, acp_data: serde_json::Value) -> Result<(), String> {
        let message_json = self.build_acp_record_json(acp_data)?;
        let outbound_message = encode_session_payload(message_json.as_bytes(), &self.message_codec)
            .map_err(|e| format!("Failed to encode ACP message: {}", e))?;
        // `HappySocket::send_message` fans out to the installed LocalEventSink
        // (see `happy_client::local_sink`) when the socket is local, which
        // handles the SQLite append + Tauri notify. We don't need to persist
        // again here.
        self.socket
            .send_message(&self.session_id, &outbound_message, None)
            .await?;
        Ok(())
    }

    async fn send_transient(&self, acp_data: serde_json::Value) -> Result<(), String> {
        let message_json = self.build_acp_record_json(acp_data)?;
        let outbound_message = encode_session_payload(message_json.as_bytes(), &self.message_codec)
            .map_err(|e| format!("Failed to encode ACP message: {}", e))?;
        self.socket
            .send_transient_message(&self.session_id, &outbound_message)
            .await
    }

    async fn sync_claude_background_task(&self, payload: &serde_json::Value) {
        let Some(record) =
            update_claude_background_task_registry(&self.session_id, payload, now_millis())
        else {
            return;
        };

        let Some(socket) = runtime_hooks::machine_socket() else {
            return;
        };

        let session_id = self.session_id.clone();
        tokio::spawn(async move {
            let payload = match serde_json::to_value(&record) {
                Ok(payload) => payload,
                Err(err) => {
                    log::debug!(
                        "[Normalizer {}] Failed to serialize background task update: {}",
                        session_id,
                        err
                    );
                    return;
                }
            };

            if let Err(err) = socket
                .push_to_frontend("background-task-update", payload)
                .await
            {
                log::debug!(
                    "[Normalizer {}] Failed to emit background-task-update: {}",
                    session_id,
                    err
                );
            }
        });
    }

    async fn emit_stream_callback(&self, delta_json: serde_json::Value) {
        let Some(cb) = self.stream_callback.as_ref() else {
            log::warn!(
                "[Normalizer {}] stream_callback is None, dropping: {}",
                self.session_id,
                delta_json
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
            );
            return;
        };
        cb(delta_json).await;
    }

    /// Wrap `acp_data` in the standard `{role: agent, content: {type: acp, provider: cteno, data}}`
    /// envelope and JSON-encode it.
    fn build_acp_record_json(&self, acp_data: serde_json::Value) -> Result<String, String> {
        let message = json!({
            "role": "agent",
            "content": {
                "type": "acp",
                "provider": "cteno",
                "data": acp_data,
            },
            "meta": {
                "sentFrom": "cli",
            },
        });
        serde_json::to_string(&message)
            .map_err(|e| format!("Failed to serialize ACP message: {}", e))
    }

    // Assistant-side persistence is handled by `HappySocket::send_message` →
    // `LocalEventSink::on_message`. No helper method here anymore.
    // User-side persistence still uses `persist_local_user_message` (free fn
    // below) because user input does not go through the broadcast socket.

    /// Persist a user-authored message to the local `agent_sessions.messages`
    /// column. Call this *before* `executor.send_message` so the input shows
    /// up in history even if the vendor never echoes it back.
    ///
    /// No-op when the socket is remote (登录后模式): the server is the source
    /// of truth and will push the user message back through the normal
    /// session feed.
    pub fn persist_user_message(&self, text: &str, local_id: Option<&str>) -> Result<(), String> {
        if !self.socket.is_local() {
            return Ok(());
        }
        persist_local_user_message(
            &self.db_path,
            &self.session_id,
            &self.session_ref.vendor,
            text,
            local_id,
        )
    }
}

/// Append a user-authored message to `agent_sessions.messages` unconditionally.
///
/// Exposed for call sites that drive `executor.send_message` without
/// constructing an [`ExecutorNormalizer`] (e.g. `agent_rpc_handler` and
/// `multi_agent` workspace roles). Callers that do hold a normalizer should
/// prefer [`ExecutorNormalizer::persist_user_message`], which additionally
/// respects the `socket.is_local()` gate.
pub(crate) fn persist_local_user_message(
    db_path: &std::path::Path,
    session_id: &str,
    vendor: &str,
    text: &str,
    local_id: Option<&str>,
) -> Result<(), String> {
    append_local_session_message(
        db_path,
        session_id,
        vendor,
        "user",
        text.to_string(),
        local_id.map(|s| s.to_string()),
    )
}

/// Shared upsert + append helper for both user and assistant writes.
///
/// Guarantees:
/// - creates the session row with the given `vendor` if missing
/// - fixes up a stale `vendor` column if the row already exists but was
///   tagged differently (e.g. re-opened a session with a new executor)
/// - appends one `SessionMessage` and flushes via `update_messages`
///
/// The `update_messages` SQL is a full-column overwrite — callers that want
/// to batch multiple writes should read `session.messages`, mutate in-memory,
/// then call `update_messages` once. This helper serialises each append into
/// its own read-modify-write cycle and is **not** safe against concurrent
/// writers on the same `session_id` (tracked as P1 in the persistence audit).
fn append_local_session_message(
    db_path: &std::path::Path,
    session_id: &str,
    vendor: &str,
    role: &str,
    content: String,
    local_id: Option<String>,
) -> Result<(), String> {
    let manager = AgentSessionManager::new(db_path.to_path_buf());
    let mut session = match manager.get_session(session_id)? {
        Some(session) => session,
        None => {
            manager.create_session_with_id_and_vendor(session_id, "worker", None, None, vendor)?
        }
    };

    if session.vendor != vendor {
        manager.set_vendor(session_id, vendor)?;
        session.vendor = vendor.to_string();
    }

    session.messages.push(SessionMessage {
        role: role.to_string(),
        content,
        timestamp: chrono::Utc::now().to_rfc3339(),
        local_id,
    });
    manager.update_messages(session_id, &session.messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::happy_client::socket::{HappySocket, LocalEventSink};
    use async_trait::async_trait;
    use cteno_host_session_registry::{BackgroundTaskRegistry, BackgroundTaskStatus};
    use cteno_host_session_wire::ConnectionType;
    use futures_util::stream;
    use multi_agent_runtime_core::{
        AgentCapabilities, AgentExecutor, AgentExecutorError, EventStream, ModelChangeOutcome,
        ModelSpec, NativeMessage, NativeSessionId, Pagination, PermissionDecision, PermissionMode,
        ResumeHints, SessionFilter, SessionInfo, SessionMeta, SessionRef, SpawnSessionSpec,
        UserMessage,
    };
    use serde_json::Value;
    use std::borrow::Cow;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingLocalSink {
        persisted: Mutex<Vec<String>>,
        transient: Mutex<Vec<String>>,
    }

    impl RecordingLocalSink {
        fn persisted_messages(&self) -> Vec<Value> {
            self.persisted
                .lock()
                .unwrap()
                .iter()
                .map(|raw| serde_json::from_str(raw).unwrap())
                .collect()
        }
    }

    impl LocalEventSink for RecordingLocalSink {
        fn on_message(&self, _session_id: &str, encrypted_message: &str, _local_id: Option<&str>) {
            self.persisted
                .lock()
                .unwrap()
                .push(encrypted_message.to_string());
        }

        fn on_transient_message(&self, _session_id: &str, encrypted_message: &str) {
            self.transient
                .lock()
                .unwrap()
                .push(encrypted_message.to_string());
        }

        fn on_state_update(
            &self,
            _session_id: &str,
            _encrypted_state: Option<&str>,
            _version: u32,
        ) {
        }

        fn on_metadata_update(&self, _session_id: &str, _encrypted_metadata: &str, _version: u32) {}
    }

    struct NoopExecutor;

    #[async_trait]
    impl AgentExecutor for NoopExecutor {
        fn capabilities(&self) -> AgentCapabilities {
            AgentCapabilities {
                name: Cow::Borrowed("test"),
                protocol_version: Cow::Borrowed("test"),
                supports_list_sessions: false,
                supports_get_messages: false,
                supports_runtime_set_model: false,
                permission_mode_kind: multi_agent_runtime_core::PermissionModeKind::Dynamic,
                supports_resume: false,
                supports_multi_session_per_process: false,
                supports_injected_tools: false,
                supports_permission_closure: false,
                supports_interrupt: false,
            }
        }

        async fn spawn_session(
            &self,
            _spec: SpawnSessionSpec,
        ) -> Result<SessionRef, AgentExecutorError> {
            Err(AgentExecutorError::Unsupported {
                capability: "spawn_session",
            })
        }

        async fn resume_session(
            &self,
            _session_id: NativeSessionId,
            _hints: ResumeHints,
        ) -> Result<SessionRef, AgentExecutorError> {
            Err(AgentExecutorError::Unsupported {
                capability: "resume_session",
            })
        }

        async fn send_message(
            &self,
            _session: &SessionRef,
            _message: UserMessage,
        ) -> Result<EventStream, AgentExecutorError> {
            Ok(Box::pin(stream::empty()))
        }

        async fn respond_to_permission(
            &self,
            _session: &SessionRef,
            _request_id: String,
            _decision: PermissionDecision,
        ) -> Result<(), AgentExecutorError> {
            Err(AgentExecutorError::Unsupported {
                capability: "respond_to_permission",
            })
        }

        async fn interrupt(&self, _session: &SessionRef) -> Result<(), AgentExecutorError> {
            Err(AgentExecutorError::Unsupported {
                capability: "interrupt",
            })
        }

        async fn close_session(&self, _session: &SessionRef) -> Result<(), AgentExecutorError> {
            Ok(())
        }

        async fn set_permission_mode(
            &self,
            _session: &SessionRef,
            _mode: PermissionMode,
        ) -> Result<(), AgentExecutorError> {
            Err(AgentExecutorError::Unsupported {
                capability: "set_permission_mode",
            })
        }

        async fn set_model(
            &self,
            _session: &SessionRef,
            _model: ModelSpec,
        ) -> Result<ModelChangeOutcome, AgentExecutorError> {
            Err(AgentExecutorError::Unsupported {
                capability: "set_model",
            })
        }

        async fn list_sessions(
            &self,
            _filter: SessionFilter,
        ) -> Result<Vec<SessionMeta>, AgentExecutorError> {
            Ok(Vec::new())
        }

        async fn get_session_info(
            &self,
            _session_id: &NativeSessionId,
        ) -> Result<SessionInfo, AgentExecutorError> {
            Err(AgentExecutorError::Unsupported {
                capability: "get_session_info",
            })
        }

        async fn get_session_messages(
            &self,
            _session_id: &NativeSessionId,
            _pagination: Pagination,
        ) -> Result<Vec<NativeMessage>, AgentExecutorError> {
            Ok(Vec::new())
        }
    }

    /// Bootstrap the legacy `agent_sessions` schema expected by
    /// `AgentSessionManager`. The manager's `ensure_vendor_column` ALTER then
    /// promotes it to the current shape lazily on first open.
    fn init_agent_sessions_table(db_path: &std::path::Path) {
        let conn = rusqlite::Connection::open(db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE agent_sessions (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                user_id TEXT,
                messages TEXT NOT NULL DEFAULT '[]',
                context_data TEXT,
                status TEXT DEFAULT 'active',
                created_at TEXT,
                updated_at TEXT,
                expires_at TEXT,
                owner_session_id TEXT
            );
            "#,
        )
        .unwrap();
    }

    fn ensure_background_task_registry_for_tests() -> Arc<BackgroundTaskRegistry> {
        if let Ok(registry) = crate::local_services::background_task_registry() {
            registry
        } else {
            let registry = Arc::new(BackgroundTaskRegistry::new());
            crate::local_services::install_background_task_registry(registry.clone());
            registry
        }
    }

    #[test]
    fn task_complete_payload_uses_current_task_id() {
        let payload = acp_task_complete_payload("task-123");
        assert_eq!(payload["type"], "task_complete");
        assert_eq!(payload["id"], "task-123");
    }

    #[test]
    fn user_visible_executor_error_strips_protocol_wrapper() {
        let message = user_visible_executor_error(&AgentExecutorError::Protocol(
            "cteno-agent startup failed: panic: bootstrap failed".to_string(),
        ));
        assert_eq!(
            message,
            "cteno-agent startup failed: panic: bootstrap failed"
        );
    }

    #[test]
    fn user_visible_executor_error_formats_subprocess_exit() {
        let message = user_visible_executor_error(&AgentExecutorError::SubprocessExited {
            code: Some(101),
            stderr: "panic: broken state machine".to_string(),
        });
        assert_eq!(
            message,
            "subprocess exited unexpectedly (code 101). Last stderr: panic: broken state machine"
        );
    }

    #[test]
    fn user_visible_executor_error_preserves_readable_timeout_operation() {
        let message = user_visible_executor_error(&AgentExecutorError::Timeout {
            operation: "waiting for cteno-agent startup (last stderr: panic: bootstrap failed)"
                .to_string(),
            seconds: 30,
        });

        assert_eq!(
            message,
            "timeout after 30s: waiting for cteno-agent startup (last stderr: panic: bootstrap failed)"
        );
        assert!(!message.contains("spawn_session"));
    }

    #[test]
    fn user_visible_executor_error_adds_login_hint_for_missing_cteno_auth() {
        let message = user_visible_executor_error(&AgentExecutorError::Vendor {
            vendor: "cteno",
            message: "no Cteno API key configured: please log in or set CTENO_AGENT_API_KEY"
                .to_string(),
        });

        assert!(message.contains("请先登录"));
        assert!(message.contains("配置 API key"));
    }

    #[test]
    fn user_visible_executor_error_adds_login_hint_for_proxy_auth_gate() {
        let message = user_visible_executor_error(&AgentExecutorError::Vendor {
            vendor: "cteno",
            message: "profile 'proxy-default' requires Happy proxy auth, but you are not logged in and no direct profile is available".to_string(),
        });

        assert!(message.contains("请先登录"));
        assert!(message.contains("配置 API key"));
    }

    #[tokio::test]
    async fn surface_executor_failure_persists_task_complete_for_subprocess_exit() {
        let temp = tempfile::tempdir().expect("tempdir");
        crate::db::init_at_data_dir(temp.path()).expect("db init");
        let db_path = temp.path().join("db").join("cteno.db");

        let session_id = "surface-executor-failure-session".to_string();
        let task_id = "task-surface-executor-failure".to_string();
        let sink = Arc::new(RecordingLocalSink::default());
        let socket = Arc::new(HappySocket::local(ConnectionType::SessionScoped {
            session_id: session_id.clone(),
        }));
        socket.install_local_sink(sink.clone());

        let normalizer = ExecutorNormalizer::new(
            session_id,
            socket,
            SessionMessageCodec::plaintext(),
            None,
            Arc::new(PermissionHandler::new(
                "surface-executor-failure".to_string(),
                0,
            )),
            task_id.clone(),
            Arc::new(NoopExecutor),
            SessionRef {
                id: NativeSessionId::new("native-session"),
                vendor: "cteno",
                process_handle: multi_agent_runtime_core::ProcessHandleToken::new(),
                spawned_at: chrono::Utc::now(),
                workdir: temp.path().to_path_buf(),
            },
            "http://127.0.0.1:1".to_string(),
            "local-test".to_string(),
            db_path,
            None,
            None,
        );

        surface_executor_failure(
            &normalizer,
            &AgentExecutorError::SubprocessExited {
                code: Some(101),
                stderr: "panic: broken state machine".to_string(),
            },
        )
        .await
        .expect("subprocess exit should surface");

        let persisted = sink.persisted_messages();
        assert_eq!(persisted.len(), 2);
        assert_eq!(persisted[0]["content"]["data"]["type"], "error");
        assert_eq!(
            persisted[0]["content"]["data"]["message"],
            "subprocess exited unexpectedly (code 101). Last stderr: panic: broken state machine"
        );
        assert_eq!(persisted[1]["content"]["data"]["type"], "task_complete");
        assert_eq!(persisted[1]["content"]["data"]["id"], task_id);
    }

    #[tokio::test]
    async fn thinking_deltas_are_persisted_before_final_message_on_turn_complete() {
        let temp = tempfile::tempdir().expect("tempdir");
        crate::db::init_at_data_dir(temp.path()).expect("db init");
        let db_path = temp.path().join("db").join("cteno.db");

        let session_id = "thinking-persist-session".to_string();
        let sink = Arc::new(RecordingLocalSink::default());
        let socket = Arc::new(HappySocket::local(ConnectionType::SessionScoped {
            session_id: session_id.clone(),
        }));
        socket.install_local_sink(sink.clone());

        let normalizer = ExecutorNormalizer::new(
            session_id.clone(),
            socket,
            SessionMessageCodec::plaintext(),
            None,
            Arc::new(PermissionHandler::new(session_id.clone(), 0)),
            "task-thinking".to_string(),
            Arc::new(NoopExecutor),
            SessionRef {
                id: NativeSessionId::new("native-session"),
                vendor: "cteno",
                process_handle: multi_agent_runtime_core::ProcessHandleToken::new(),
                spawned_at: chrono::Utc::now(),
                workdir: temp.path().to_path_buf(),
            },
            "http://127.0.0.1:1".to_string(),
            "local-test".to_string(),
            db_path,
            None,
            None,
        );

        normalizer
            .process_event(ExecutorEvent::StreamDelta {
                kind: DeltaKind::Thinking,
                content: "first ".to_string(),
            })
            .await
            .expect("thinking delta should normalize");
        normalizer
            .process_event(ExecutorEvent::StreamDelta {
                kind: DeltaKind::Reasoning,
                content: "second".to_string(),
            })
            .await
            .expect("reasoning delta should normalize");
        let done = normalizer
            .process_event(ExecutorEvent::TurnComplete {
                final_text: Some("answer".to_string()),
                iteration_count: 1,
                usage: multi_agent_runtime_core::TokenUsage::default(),
            })
            .await
            .expect("turn complete should normalize");

        assert!(done);
        let persisted = sink.persisted_messages();
        assert_eq!(persisted.len(), 3);
        assert_eq!(persisted[0]["content"]["data"]["type"], "thinking");
        assert_eq!(persisted[0]["content"]["data"]["text"], "first second");
        assert_eq!(persisted[1]["content"]["data"]["type"], "message");
        assert_eq!(persisted[1]["content"]["data"]["message"], "answer");
        assert_eq!(persisted[2]["content"]["data"]["type"], "task_complete");
    }

    #[test]
    fn injected_tool_metadata_is_embedded_in_object_input() {
        let annotated = with_host_owned_tool_metadata(
            json!({
                "task": "Investigate failing eval",
                "agent_type": "reviewer",
            }),
            "tool-exec-1",
        );

        assert_eq!(annotated["task"], "Investigate failing eval");
        assert_eq!(annotated["agent_type"], "reviewer");
        assert_eq!(annotated[HOST_OWNED_TOOL_METADATA_KEY]["owned"], true);
        assert_eq!(
            annotated[HOST_OWNED_TOOL_METADATA_KEY]["requestId"],
            "tool-exec-1"
        );
        assert_eq!(
            annotated[HOST_OWNED_TOOL_METADATA_KEY]["source"],
            "injected_tool"
        );
    }

    #[test]
    fn injected_tool_match_prefers_latest_same_name_and_input() {
        let shared_input = json!({
            "task": "Summarize repo state",
        });
        let mut recent = Vec::new();

        remember_recent_tool_call(&mut recent, "call-1", "dispatch_task", &shared_input);
        remember_recent_tool_call(
            &mut recent,
            "call-2",
            "dispatch_task",
            &json!({"task": "Other"}),
        );
        remember_recent_tool_call(&mut recent, "call-3", "dispatch_task", &shared_input);

        assert_eq!(
            take_matching_recent_tool_call_id(&mut recent, "dispatch_task", &shared_input)
                .as_deref(),
            Some("call-3")
        );
        assert_eq!(recent.len(), 2);
        assert!(recent.iter().any(|entry| entry.call_id == "call-1"));
        assert!(recent.iter().any(|entry| entry.call_id == "call-2"));
    }

    #[test]
    fn tool_result_payload_uses_desktop_content_shape() {
        let payload = acp_tool_result_payload("call-9", Ok("done".to_string()));

        assert_eq!(payload["type"], "tool-result");
        assert_eq!(payload["callId"], "call-9");
        assert_eq!(payload["content"][0]["type"], "text");
        assert_eq!(payload["content"][0]["text"], "done");
        assert_eq!(payload["isError"], false);
        assert!(payload["id"].as_str().is_some());
    }

    #[test]
    fn persist_user_message_creates_session_row_with_user_role() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        init_agent_sessions_table(&db_path);

        persist_local_user_message(
            &db_path,
            "session-new",
            "claude",
            "hello world",
            Some("local-1"),
        )
        .unwrap();

        let manager = AgentSessionManager::new(db_path.clone());
        let session = manager
            .get_session("session-new")
            .unwrap()
            .expect("row should exist after first persist");
        assert_eq!(session.vendor, "claude");
        assert_eq!(session.messages.len(), 1);
        let msg = &session.messages[0];
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, "hello world");
        assert_eq!(msg.local_id.as_deref(), Some("local-1"));
    }

    #[test]
    fn persist_user_message_preserves_order_with_assistant_writes() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        init_agent_sessions_table(&db_path);

        persist_local_user_message(&db_path, "session-mix", "claude", "first user", None).unwrap();
        append_local_session_message(
            &db_path,
            "session-mix",
            "claude",
            "assistant",
            "first reply".to_string(),
            None,
        )
        .unwrap();
        persist_local_user_message(&db_path, "session-mix", "claude", "second user", None).unwrap();

        let manager = AgentSessionManager::new(db_path.clone());
        let session = manager.get_session("session-mix").unwrap().unwrap();
        let roles: Vec<&str> = session.messages.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant", "user"]);
        assert_eq!(session.messages[0].content, "first user");
        assert_eq!(session.messages[1].content, "first reply");
        assert_eq!(session.messages[2].content, "second user");
    }

    #[test]
    fn persist_user_message_fixes_mismatched_vendor_without_dropping_history() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        init_agent_sessions_table(&db_path);

        // Pretend a codex turn already wrote history to this session.
        append_local_session_message(
            &db_path,
            "session-mix-vendor",
            "codex",
            "assistant",
            "codex reply".to_string(),
            None,
        )
        .unwrap();

        // Now a claude-vendored call lands on the same session row. The
        // vendor column should flip, but prior messages must survive.
        persist_local_user_message(&db_path, "session-mix-vendor", "claude", "hi claude", None)
            .unwrap();

        let manager = AgentSessionManager::new(db_path.clone());
        let session = manager.get_session("session-mix-vendor").unwrap().unwrap();
        assert_eq!(session.vendor, "claude");
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].role, "assistant");
        assert_eq!(session.messages[0].content, "codex reply");
        assert_eq!(session.messages[1].role, "user");
        assert_eq!(session.messages[1].content, "hi claude");
    }

    #[test]
    fn rate_limit_native_events_map_to_recoverable_error_payloads() {
        let payload = native_event_error_payload("claude", &json!({ "kind": "rate_limit_event" }))
            .expect("rate limit event should map to an ACP error");

        assert_eq!(payload["type"], "error");
        assert_eq!(
            payload["message"],
            "Claude API rate limit reached. Retrying automatically."
        );
        assert_eq!(payload["recoverable"], true);
    }

    #[test]
    fn unrelated_native_events_do_not_emit_notifications() {
        assert!(native_event_error_payload("claude", &json!({ "kind": "user_frame" })).is_none());
    }

    #[test]
    fn claude_prompt_suggestion_maps_to_acp() {
        let payload = native_event_prompt_suggestion_payload(&json!({
            "kind": "prompt_suggestion",
            "suggestion": "Summarize the diff",
        }))
        .expect("prompt suggestion should map to ACP");

        assert_eq!(payload["type"], "prompt-suggestion");
        assert_eq!(payload["suggestions"][0], "Summarize the diff");
    }

    #[test]
    fn claude_base64_image_maps_to_acp() {
        let payload = native_event_image_payload(&json!({
            "kind": "assistant_image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": "aGVsbG8="
            }
        }))
        .expect("base64 image should map to ACP");

        assert_eq!(payload["type"], "image");
        assert_eq!(payload["source"]["type"], "base64");
        assert_eq!(payload["source"]["media_type"], "image/png");
        assert_eq!(payload["source"]["data"], "aGVsbG8=");
    }

    #[test]
    fn claude_url_image_maps_to_acp() {
        let payload = native_event_image_payload(&json!({
            "kind": "assistant_image",
            "image_url": "https://example.com/claude-image.png"
        }))
        .expect("url image should map to ACP");

        assert_eq!(payload["type"], "image");
        assert_eq!(payload["image_url"], "https://example.com/claude-image.png");
    }

    #[test]
    fn claude_native_session_state_maps_to_acp() {
        let payload = native_event_session_state_payload(&json!({
            "kind": "session_state_changed",
            "state": "running",
        }))
        .expect("session state should map to ACP");

        assert_eq!(payload["type"], "session-state");
        assert_eq!(payload["state"], "running");
    }

    #[test]
    fn claude_compact_boundary_maps_to_compaction_message() {
        let payload = native_event_compact_boundary_payload(&json!({
            "kind": "compact_boundary",
            "trigger": "auto",
            "pre_tokens": 4096,
        }))
        .expect("compact boundary should map to ACP");

        assert_eq!(payload["type"], "message");
        assert_eq!(payload["message"], "Compaction completed");
    }

    #[test]
    fn claude_task_started_maps_to_lifecycle_and_task_tool() {
        let native = json!({
            "kind": "task_started",
            "task_id": "task-1",
            "description": "Index repository",
            "tool_use_id": "toolu_task_1",
            "task_type": "background",
            "uuid": "uuid-1",
            "session_id": "session-1",
        });

        let lifecycle = claude_task_started_payload(&native).expect("task started should map");
        assert_eq!(lifecycle["type"], "task_started");
        assert_eq!(lifecycle["id"], "task-1");

        let tool_call = claude_task_tool_call_payload(&native).expect("task tool call should map");
        assert_eq!(tool_call["type"], "tool-call");
        assert_eq!(tool_call["callId"], "toolu_task_1");
        assert_eq!(tool_call["name"], "Task");
        assert_eq!(tool_call["input"]["description"], "Index repository");
        assert_eq!(tool_call["input"]["prompt"], "Index repository");
        assert_eq!(tool_call["input"]["taskId"], "task-1");
        assert_eq!(tool_call["input"]["taskType"], "background");
        assert_eq!(tool_call["input"]["uuid"], "uuid-1");
        assert_eq!(tool_call["input"]["sessionId"], "session-1");
    }

    #[test]
    fn claude_task_started_updates_background_registry_and_preserves_acp() {
        let registry = ensure_background_task_registry_for_tests();
        let task_id = format!("task-{}", Uuid::new_v4());
        registry.remove(&task_id);

        let native = json!({
            "kind": "task_started",
            "task_id": task_id,
            "description": "Index repository",
            "tool_use_id": "toolu_task_registry",
            "task_type": "bash",
            "session_id": "vendor-session-1",
        });

        let lifecycle = claude_task_started_payload(&native).expect("task started should map");
        assert_eq!(lifecycle["type"], "task_started");
        assert_eq!(lifecycle["id"], native["task_id"]);
        assert_eq!(lifecycle["description"], "Index repository");

        let updated = update_claude_background_task_registry("session-under-test", &native, 42)
            .expect("registry should return the upserted record");

        assert_eq!(updated.task_id, native["task_id"].as_str().unwrap());
        assert_eq!(updated.session_id, "session-under-test");
        assert_eq!(updated.vendor, "claude");
        assert_eq!(updated.task_type, "bash");
        assert_eq!(updated.status, BackgroundTaskStatus::Running);
        assert_eq!(updated.started_at, 42);
        assert_eq!(updated.description.as_deref(), Some("Index repository"));

        let stored = registry
            .get(native["task_id"].as_str().unwrap())
            .expect("task should be stored in registry");
        assert_eq!(stored.status, BackgroundTaskStatus::Running);
        assert_eq!(stored.session_id, "session-under-test");
        assert_eq!(stored.vendor, "claude");

        registry.remove(native["task_id"].as_str().unwrap());
    }

    #[test]
    fn claude_task_progress_maps_to_task_tool_update() {
        let tool_call_delta = claude_task_progress_delta_payload(&json!({
            "kind": "task_progress",
            "task_id": "task-1",
            "description": "Index repository",
            "summary": "Scanning Cargo manifests",
            "last_tool_name": "Read",
            "tool_use_id": "toolu_task_1",
            "task_type": "shell",
            "usage": {
                "total_tokens": 1234,
                "tool_uses": 5,
                "duration_ms": 9876
            }
        }))
        .expect("task progress should map");

        assert_eq!(tool_call_delta["type"], "tool-call-delta");
        assert_eq!(tool_call_delta["callId"], "toolu_task_1");
        assert_eq!(
            tool_call_delta["patch"]["description"],
            "Scanning Cargo manifests"
        );
        assert_eq!(
            tool_call_delta["patch"]["summary"],
            "Scanning Cargo manifests"
        );
        assert_eq!(tool_call_delta["patch"]["lastToolName"], "Read");
        assert_eq!(tool_call_delta["patch"]["taskId"], "task-1");
        assert_eq!(tool_call_delta["patch"]["taskType"], "shell");
        assert_eq!(tool_call_delta["patch"]["usage"]["total_tokens"], 1234);
    }

    #[test]
    fn claude_task_notification_maps_to_tool_result_and_task_complete() {
        let native = json!({
            "kind": "task_notification",
            "task_id": "task-1",
            "status": "completed",
            "summary": "Indexed 12 files",
            "tool_use_id": "toolu_task_1",
            "output_file": "/tmp/out.md",
        });

        let tool_result = claude_task_notification_tool_result_payload(&native)
            .expect("task notification should close the task tool");
        assert_eq!(tool_result["type"], "tool-result");
        assert_eq!(tool_result["callId"], "toolu_task_1");
        assert_eq!(tool_result["content"][0]["text"], "Indexed 12 files");
        assert_eq!(
            tool_result["content"][1]["text"],
            "output_file: /tmp/out.md"
        );
        assert_eq!(tool_result["isError"], false);

        let lifecycle = claude_task_complete_payload(&native)
            .expect("task notification should map to task complete");
        assert_eq!(lifecycle["type"], "task_complete");
        assert_eq!(lifecycle["id"], "task-1");
        assert_eq!(lifecycle["summary"], "Indexed 12 files");
    }

    #[test]
    fn codex_raw_thread_status_maps_to_acp() {
        let payload = native_event_session_state_payload(&serde_json::Value::String(
            r#"{"type":"thread/status/changed","status":"idle"}"#.to_string(),
        ))
        .expect("raw codex thread status should map to ACP");

        assert_eq!(payload["type"], "session-state");
        assert_eq!(payload["state"], "idle");
    }

    #[test]
    fn codex_guardian_completion_maps_to_approved_tool_result() {
        let completion = codex_guardian_completion(&json!({
            "kind": "codex_guardian_review_completed",
            "request_id": "guardian-1",
            "review": {
                "riskLevel": "medium",
                "autoApproved": true
            }
        }))
        .expect("guardian completion should parse");

        assert_eq!(
            completion,
            CodexGuardianReviewCompletion {
                request_id: "guardian-1".to_string(),
                status: "approved",
                risk_level: Some("medium".to_string()),
            }
        );

        let payload = codex_guardian_tool_result_payload(&completion);
        assert_eq!(payload["type"], "tool-result");
        assert_eq!(payload["callId"], "guardian-1");
        assert_eq!(payload["isError"], false);
        assert_eq!(payload["permissions"]["result"], "approved");
        assert_eq!(
            payload["content"][0]["text"],
            "Codex Guardian auto-approved (medium risk)"
        );
    }

    #[test]
    fn codex_guardian_permission_request_uses_marker_and_risk_description() {
        let input = json!({
            "__codex_guardian_review": true,
            "riskLevel": "high"
        });

        assert!(is_codex_guardian_permission_request(&input));
        assert_eq!(
            codex_guardian_permission_description(&input).as_deref(),
            Some("Risk level: high")
        );
    }

    #[tokio::test]
    async fn fatal_executor_errors_are_persisted_and_close_the_turn() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("cteno.db");
        init_agent_sessions_table(&db_path);

        let session_id = "session-error".to_string();
        let task_id = "task-error".to_string();
        let sink = Arc::new(RecordingLocalSink::default());
        let socket = Arc::new(HappySocket::local(ConnectionType::SessionScoped {
            session_id: session_id.clone(),
        }));
        socket.install_local_sink(sink.clone());

        let normalizer = ExecutorNormalizer::new(
            session_id.clone(),
            socket,
            SessionMessageCodec::plaintext(),
            None,
            Arc::new(PermissionHandler::new(session_id.clone(), 0)),
            task_id.clone(),
            Arc::new(NoopExecutor),
            SessionRef {
                id: NativeSessionId::new("native-session"),
                vendor: "cteno",
                process_handle: multi_agent_runtime_core::ProcessHandleToken::new(),
                spawned_at: chrono::Utc::now(),
                workdir: temp.path().to_path_buf(),
            },
            "http://127.0.0.1:1".to_string(),
            "local-test".to_string(),
            db_path,
            None,
            None,
        );

        let done = normalizer
            .process_event(ExecutorEvent::Error {
                message: "cteno-agent exited unexpectedly (code 101).".to_string(),
                recoverable: false,
            })
            .await
            .expect("fatal error should normalize");
        assert!(done, "fatal error should terminate the turn");

        let persisted = sink.persisted_messages();
        assert_eq!(persisted.len(), 2);
        assert_eq!(persisted[0]["content"]["data"]["type"], "error");
        assert_eq!(
            persisted[0]["content"]["data"]["message"],
            "cteno-agent exited unexpectedly (code 101)."
        );
        assert_eq!(persisted[0]["content"]["data"]["recoverable"], false);
        assert_eq!(persisted[1]["content"]["data"]["type"], "task_complete");
        assert_eq!(persisted[1]["content"]["data"]["id"], task_id);
    }
}
