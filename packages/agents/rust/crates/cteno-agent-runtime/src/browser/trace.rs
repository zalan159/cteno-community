//! Browser dialog event types.

use serde::{Deserialize, Serialize};

/// A native browser dialog that was auto-handled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialogEvent {
    pub timestamp: f64,
    /// "alert" | "confirm" | "prompt" | "beforeunload" | "auth"
    pub dialog_type: String,
    pub message: String,
    pub accepted: bool,
}
