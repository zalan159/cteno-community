use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ClaimStatus, MultiAgentProvider, RoleSpec, WorkspaceVisibility, WorkspaceWorkflowVoteWindow,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DispatchStatus {
    Queued,
    Started,
    Running,
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    Idle,
    Running,
    RequiresAction,
    Closed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemberStatus {
    Idle,
    Active,
    Blocked,
    Waiting,
    Offline,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    GroupChat,
    WorkflowVote,
    WorkflowRunning,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceActivityKind {
    UserMessage,
    CoordinatorMessage,
    ClaimWindowOpened,
    ClaimWindowClosed,
    MemberClaimed,
    MemberSupporting,
    MemberDeclined,
    MemberProgress,
    MemberBlocked,
    MemberDelivered,
    MemberSummary,
    WorkflowVoteOpened,
    WorkflowVoteApproved,
    WorkflowVoteRejected,
    WorkflowStarted,
    WorkflowStageStarted,
    WorkflowStageCompleted,
    WorkflowCompleted,
    DispatchStarted,
    DispatchProgress,
    DispatchCompleted,
    SystemNotice,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskDispatch {
    pub dispatch_id: Uuid,
    pub workspace_id: String,
    pub role_id: String,
    pub instruction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<WorkspaceVisibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_id: Option<String>,
    pub status: DispatchStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by_member_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_status: Option<ClaimStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMember {
    pub member_id: String,
    pub workspace_id: String,
    pub role_id: String,
    pub role_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub status: MemberStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_state_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceActivity {
    pub activity_id: Uuid,
    pub workspace_id: String,
    pub kind: WorkspaceActivityKind,
    pub visibility: WorkspaceVisibility,
    pub text: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceWorkflowRuntimeState {
    pub mode: WorkspaceMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_vote_window: Option<WorkspaceWorkflowVoteWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_request_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_stage_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    pub workspace_id: String,
    pub status: WorkspaceStatus,
    pub provider: MultiAgentProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    pub roles: BTreeMap<String, RoleSpec>,
    pub members: BTreeMap<String, WorkspaceMember>,
    pub dispatches: BTreeMap<Uuid, TaskDispatch>,
    pub activities: Vec<WorkspaceActivity>,
    pub workflow_runtime: WorkspaceWorkflowRuntimeState,
}
