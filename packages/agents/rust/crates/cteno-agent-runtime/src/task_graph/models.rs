use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskNodeStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Blocked,
}

impl TaskNodeStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Blocked)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskGraphState {
    pub group_id: String,
    pub parent_session_id: String,
    pub nodes: Vec<TaskNodeState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNodeState {
    pub task_id: String,
    pub task_description: String,
    pub depends_on: Vec<String>,
    pub status: TaskNodeStatus,
    pub subagent_id: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub profile_id: Option<String>,
    pub skill_ids: Vec<String>,
    pub workdir: Option<String>,
    pub agent_type: Option<String>,
}

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
    pub agent_type: Option<String>,
}
