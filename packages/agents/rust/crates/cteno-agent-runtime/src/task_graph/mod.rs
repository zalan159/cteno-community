//! Session-local DAG task graph execution.
//!
//! This module owns Cteno runtime-native task graphs. It intentionally lives
//! in the agent runtime, not the desktop host: nodes are spawned as runtime
//! SubAgents, and subagent completion notifications advance the graph.

pub mod engine;
pub mod manager;
pub mod models;

pub use engine::{build_group_summary, validate_dag};
pub use manager::{global, TaskGraphManager};
pub use models::*;
