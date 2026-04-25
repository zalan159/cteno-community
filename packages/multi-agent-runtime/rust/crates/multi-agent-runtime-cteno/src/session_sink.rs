//! Host-side sink for cteno-agent events that flow **outside** any active
//! per-turn `EventStream`.
//!
//! Two flow categories live here:
//!
//! 1. Background ACP messages (`on_session_message`) — legacy SubAgent
//!    notifications that the runtime emits as `Outbound::Acp` with
//!    `source: "subagent"`. Once the runtime is fully on the new
//!    lifecycle wire below, this hook becomes pure defensive sugar; it
//!    stays for future runtime-owned background ACP flows.
//!
//! 2. SubAgent lifecycle (`on_subagent_lifecycle`) — Spawned / Started /
//!    Completed / Failed / Stopped frames emitted by
//!    `SubAgentManager` (Phase A). The host updates a process-local
//!    SubAgent registry mirror (Phase C) so the BackgroundRunsModal can
//!    show live progress without polling.
//!
//! Replaces the older `BackgroundAcpSink` single-`Fn` callback. Trait
//! shape lets callers add new event categories without breaking
//! signature, and lets the dispatcher pattern-match by frame type
//! instead of by JSON payload shape.

use std::sync::Arc;

pub type SessionEventSinkArc = Arc<dyn SessionEventSink>;

pub trait SessionEventSink: Send + Sync + 'static {
    /// Persisted ACP frame arriving outside any active per-turn stream.
    /// Currently unused after the SubagentMessage→ACP path was retired
    /// in favour of the explicit `AutonomousTurnStart` boundary frame;
    /// kept on the trait so a future runtime-owned background ACP flow
    /// can opt back in without re-introducing a sink type.
    fn on_session_message(&self, session_id: &str, acp_data: serde_json::Value);

    /// SubAgent lifecycle transition observed in the agent process.
    /// Hosts mirror this into a registry the UI can subscribe to.
    fn on_subagent_lifecycle(
        &self,
        parent_session_id: &str,
        event: SubAgentLifecycleEvent,
    );
}

/// Adapter-side mirror of the wire `SubAgentLifecycleEventWire`. Lifted
/// here so consumers don't have to depend on `crate::protocol`.
#[derive(Debug, Clone)]
pub enum SubAgentLifecycleEvent {
    Spawned {
        subagent_id: String,
        agent_id: String,
        task: String,
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

impl SubAgentLifecycleEvent {
    pub(crate) fn from_wire(wire: crate::protocol::SubAgentLifecycleEventWire) -> Self {
        use crate::protocol::SubAgentLifecycleEventWire as W;
        match wire {
            W::Spawned {
                subagent_id,
                agent_id,
                task,
                label,
                created_at_ms,
            } => Self::Spawned {
                subagent_id,
                agent_id,
                task,
                label,
                created_at_ms,
            },
            W::Started {
                subagent_id,
                started_at_ms,
            } => Self::Started {
                subagent_id,
                started_at_ms,
            },
            W::Updated {
                subagent_id,
                iteration_count,
            } => Self::Updated {
                subagent_id,
                iteration_count,
            },
            W::Completed {
                subagent_id,
                result,
                completed_at_ms,
            } => Self::Completed {
                subagent_id,
                result,
                completed_at_ms,
            },
            W::Failed {
                subagent_id,
                error,
                completed_at_ms,
            } => Self::Failed {
                subagent_id,
                error,
                completed_at_ms,
            },
            W::Stopped {
                subagent_id,
                completed_at_ms,
            } => Self::Stopped {
                subagent_id,
                completed_at_ms,
            },
        }
    }
}
