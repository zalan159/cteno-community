//! Orchestration Flow Visualization
//!
//! In-memory store for orchestration flow graphs.
//!
//! Flows can be created directly via RPC/CLI (`create-orchestration-flow`).
//! As `ctenoctl dispatch --label` creates worker sessions, they are
//! automatically linked to flow nodes and status is updated in real time.

pub mod models;
pub mod store;

pub use models::*;
pub use store::OrchestrationStore;
