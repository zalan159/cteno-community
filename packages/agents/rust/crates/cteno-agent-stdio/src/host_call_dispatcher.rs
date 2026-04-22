//! stdio-backed implementation of `cteno_agent_runtime::hooks::HostCallDispatcher`.
//!
//! The runtime crate defines a generic dispatcher trait that in-process hook
//! impls can use to round-trip arbitrary hook method calls through the host.
//! This module provides the stdio binary's implementation: every `call` emits
//! an outbound `host_call_request` and awaits the matching `host_call_response`
//! via the shared `PendingHostCalls` map.
//!
//! Wave-3 scaffolding: the dispatcher itself is installed at boot, but no
//! in-runtime hook impl uses it yet. Later waves will replace the bespoke
//! `ToolExecutionRequest` flow for every hook trait with a thin adapter that
//! encodes (hook_name, method, params) and calls into this dispatcher.

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::oneshot;

use cteno_agent_runtime::hooks::HostCallDispatcher;

use crate::io::OutboundWriter;
use crate::pending::{new_host_call_id, HostCallResult, PendingHostCalls};
use crate::protocol::Outbound;

/// Dispatcher that sends hook-method invocations across the stdio protocol.
pub struct StdioHostCallDispatcher {
    writer: OutboundWriter,
    pending: PendingHostCalls,
}

impl StdioHostCallDispatcher {
    pub fn new(writer: OutboundWriter, pending: PendingHostCalls) -> Self {
        Self { writer, pending }
    }
}

#[async_trait]
impl HostCallDispatcher for StdioHostCallDispatcher {
    async fn call(
        &self,
        session_id: &str,
        hook_name: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let request_id = new_host_call_id();
        let (tx, rx) = oneshot::channel::<HostCallResult>();

        {
            let mut guard = self.pending.lock().await;
            guard.insert(request_id.clone(), tx);
        }

        self.writer
            .send(Outbound::HostCallRequest {
                session_id: session_id.to_string(),
                request_id: request_id.clone(),
                hook_name: hook_name.to_string(),
                method: method.to_string(),
                params,
            })
            .await;

        match rx.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(err),
            Err(_) => {
                // Receiver dropped before a response arrived — either the host
                // vanished or the pending entry was stolen. Clean up the slot
                // on a best-effort basis and fail closed.
                let mut guard = self.pending.lock().await;
                guard.remove(&request_id);
                Err(format!(
                    "host never answered host_call_request {request_id} (hook={hook_name}, method={method})"
                ))
            }
        }
    }
}
