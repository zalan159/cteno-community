//! Orchestration flow data models.

use serde::{Deserialize, Serialize};

/// Status of a flow node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlowNodeStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl FlowNodeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// Type of edge between flow nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlowEdgeType {
    Normal,
    Retry,
    Conditional,
}

/// A node in the orchestration flow graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowNode {
    /// Unique ID that matches the `--label` argument in `ctenoctl dispatch`.
    pub id: String,
    /// Display label for the node.
    pub label: String,
    /// Optional agent type hint (e.g. "worker", "browser").
    pub agent_type: Option<String>,
    /// Current status of this node.
    pub status: FlowNodeStatus,
    /// Session ID of the worker running this node (set at runtime).
    pub session_id: Option<String>,
    /// Current iteration for loop nodes.
    pub iteration: Option<u32>,
    /// Maximum iterations for loop nodes.
    pub max_iterations: Option<u32>,
}

/// An edge between two flow nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowEdge {
    /// Source node ID.
    pub from: String,
    /// Target node ID.
    pub to: String,
    /// Optional condition label (e.g. "pass", "fail").
    pub condition: Option<String>,
    /// Edge type: normal forward, retry (back edge), or conditional.
    pub edge_type: FlowEdgeType,
}

/// A complete orchestration flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationFlow {
    pub id: String,
    /// The persona that owns this flow.
    pub persona_id: String,
    /// The persona's chat session ID (for correlation).
    pub session_id: String,
    /// Display title for the flow.
    pub title: String,
    /// Nodes in the flow.
    pub nodes: Vec<FlowNode>,
    /// Edges connecting nodes.
    pub edges: Vec<FlowEdge>,
    /// ISO 8601 creation timestamp.
    pub created_at: String,
}
