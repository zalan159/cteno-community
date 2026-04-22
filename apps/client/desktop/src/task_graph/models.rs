//! Task Graph data models (in-memory, no DB persistence).

use serde::{Deserialize, Serialize};

/// Status of a task node within a task graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskNodeStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

/// In-memory state for an entire task graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskGraphState {
    pub group_id: String,
    pub owner_id: String,
    pub nodes: Vec<TaskNodeState>,
}

/// In-memory state for a single task node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNodeState {
    /// User-defined task ID within the group (e.g. "crawl", "analyze").
    pub task_id: String,
    pub task_description: String,
    /// IDs of upstream tasks that must complete before this one starts.
    pub depends_on: Vec<String>,
    pub status: TaskNodeStatus,
    /// Worker session ID (set when the task starts running).
    pub session_id: Option<String>,
    /// Worker's final output (set when the task completes).
    pub result: Option<String>,
    /// LLM profile override for this task.
    pub profile_id: Option<String>,
    /// Skill IDs to pre-activate in the worker session.
    pub skill_ids: Vec<String>,
    /// Working directory override for this task.
    pub workdir: Option<String>,
    /// Agent type: None = general worker, Some("browser") = browser agent.
    pub agent_type: Option<String>,
}

/// Input for a single task node when creating a task graph (from LLM tool call).
#[derive(Debug, Clone, Deserialize)]
pub struct TaskNodeInput {
    pub id: String,
    pub task: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub profile_id: Option<String>,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    pub workdir: Option<String>,
    /// Agent type: None = general worker, Some("browser") = browser agent.
    pub agent_type: Option<String>,
}
