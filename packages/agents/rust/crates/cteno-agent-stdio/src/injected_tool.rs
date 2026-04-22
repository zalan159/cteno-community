//! Executor for host-owned tools injected over the stdio protocol.
//!
//! When the host sends `tool_inject`, we register a `ToolConfig` into the
//! global `ToolRegistry` whose executor is an `InjectedToolExecutor`. On
//! invocation it:
//!
//! 1. Extracts the session id from the input's `__session_id` field (the
//!    runtime auto-injects this — see `autonomous_agent::inject_session_context`).
//! 2. Strips runtime-only underscore-prefixed fields before forwarding the
//!    input back to the host (the host-side tool only knows its own schema).
//! 3. Emits a `tool_execution_request` on stdout.
//! 4. Awaits a matching `tool_execution_response` via the pending map.
//!
//! Registration is idempotent: calling `inject_tool` twice with the same name
//! replaces the previous executor. This means sessions share one injected
//! tool surface — appropriate for the Tauri host that registers the same
//! orchestration tool set (dispatch_task, ask_persona, ...) for every
//! session. Cross-session dispatch is disambiguated by `__session_id` in the
//! tool input, not by the registry key.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::sync::{oneshot, RwLock as AsyncRwLock};

use cteno_agent_runtime::tool::registry::ToolRegistry;
use cteno_agent_runtime::tool::{ToolCategory, ToolConfig, ToolExecutor};

use crate::io::OutboundWriter;
use crate::pending::{new_tool_exec_id, PendingToolExecs};
use crate::protocol::{InjectedTool, Outbound};

/// Default description used when the host omits one.
const DEFAULT_DESCRIPTION: &str = "Host-owned tool (stdio-injected).";

pub struct InjectedToolExecutor {
    tool_name: String,
    writer: OutboundWriter,
    pending: PendingToolExecs,
}

impl InjectedToolExecutor {
    pub fn new(
        tool_name: String,
        writer: OutboundWriter,
        pending: PendingToolExecs,
    ) -> Self {
        Self {
            tool_name,
            writer,
            pending,
        }
    }
}

#[async_trait]
impl ToolExecutor for InjectedToolExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        // Pull the session id from the auto-injected __session_id field. We
        // surface this into the protocol message so the host can route the
        // execution request to the right session context.
        let session_id = input
            .get("__session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Strip all runtime-internal `__`-prefixed fields before handing the
        // input to the host: the host's tool contract only knows the public
        // schema.
        let clean_input = strip_internal_fields(&input);

        let request_id = new_tool_exec_id();
        let (tx, rx) = oneshot::channel::<Result<String, String>>();

        {
            let mut guard = self.pending.lock().await;
            guard.insert(request_id.clone(), tx);
        }

        self.writer
            .send(Outbound::ToolExecutionRequest {
                session_id,
                request_id: request_id.clone(),
                tool_name: self.tool_name.clone(),
                tool_input: clean_input,
            })
            .await;

        match rx.await {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(err)) => Err(err),
            Err(_) => {
                // Clean up our map slot in case the sender was dropped without
                // delivery (agent shutdown, host crash, ...).
                let mut guard = self.pending.lock().await;
                guard.remove(&request_id);
                Err(format!(
                    "host never answered tool_execution_request {request_id} for tool '{}'",
                    self.tool_name
                ))
            }
        }
    }
}

fn strip_internal_fields(input: &Value) -> Value {
    match input {
        Value::Object(obj) => {
            let mut out = Map::new();
            for (k, v) in obj {
                if !k.starts_with("__") {
                    out.insert(k.clone(), v.clone());
                }
            }
            Value::Object(out)
        }
        _ => input.clone(),
    }
}

/// Register (or replace) a host-owned tool in the given registry. The tool
/// is marked as `always_load` so it shows up in the LLM's immediate tool set
/// without needing `tool_search`.
pub async fn inject_tool(
    registry: &Arc<AsyncRwLock<ToolRegistry>>,
    tool: InjectedTool,
    writer: OutboundWriter,
    pending: PendingToolExecs,
) {
    let description = if tool.description.is_empty() {
        DEFAULT_DESCRIPTION.to_string()
    } else {
        tool.description
    };

    let input_schema = if tool.input_schema.is_null() {
        serde_json::json!({ "type": "object", "properties": {} })
    } else {
        tool.input_schema
    };

    let config = ToolConfig {
        id: tool.name.clone(),
        name: tool.name.clone(),
        description,
        category: ToolCategory::System,
        input_schema,
        instructions: String::new(),
        supports_background: false,
        should_defer: false,
        always_load: true,
        search_hint: None,
        is_read_only: false,
        is_concurrency_safe: false,
    };

    let executor = Arc::new(InjectedToolExecutor::new(
        tool.name.clone(),
        writer,
        pending,
    ));

    let mut guard = registry.write().await;
    // Replace-on-conflict: unregister first so the cached llm_tools map is
    // refreshed with the new schema.
    guard.unregister(&tool.name);
    guard.register(config, executor);
    log::info!("stdio: injected host-owned tool '{}'", tool.name);
}
