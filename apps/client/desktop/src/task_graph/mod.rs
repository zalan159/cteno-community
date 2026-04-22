//! Shared Task Graph (DAG) Engine
//!
//! Provides a reusable DAG execution engine for multi-step task workflows.
//! Used by both PersonaManager (task dispatch) and HypothesisManager (experiments).

pub mod engine;
pub mod models;

pub use engine::{build_group_summary, TaskGraphDelegate, TaskGraphEngine};
pub use models::*;
