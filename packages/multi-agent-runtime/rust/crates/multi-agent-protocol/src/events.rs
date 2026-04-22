use serde::{Deserialize, Serialize};

use crate::{
    ClaimStatus, CoordinatorWorkflowDecision, TaskDispatch, WorkspaceActivity,
    WorkspaceClaimResponse, WorkspaceClaimWindow, WorkspaceMember, WorkspaceSpec, WorkspaceStatus,
    WorkspaceVisibility, WorkspaceWorkflowVoteResponse, WorkspaceWorkflowVoteWindow,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BaseWorkspaceEvent {
    pub timestamp: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceEvent {
    WorkspaceStarted {
        timestamp: String,
        workspace_id: String,
        spec: WorkspaceSpec,
    },
    WorkspaceInitialized {
        timestamp: String,
        workspace_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        available_agents: Vec<String>,
        available_tools: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        available_commands: Option<Vec<String>>,
    },
    WorkspaceStateChanged {
        timestamp: String,
        workspace_id: String,
        state: WorkspaceStatus,
    },
    MemberRegistered {
        timestamp: String,
        workspace_id: String,
        member: WorkspaceMember,
    },
    MemberStateChanged {
        timestamp: String,
        workspace_id: String,
        member: WorkspaceMember,
    },
    Message {
        timestamp: String,
        workspace_id: String,
        role: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        visibility: Option<WorkspaceVisibility>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        member_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },
    DispatchQueued {
        timestamp: String,
        workspace_id: String,
        dispatch: TaskDispatch,
    },
    DispatchClaimed {
        timestamp: String,
        workspace_id: String,
        dispatch: TaskDispatch,
        member: WorkspaceMember,
        claim_status: ClaimStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    ClaimWindowOpened {
        timestamp: String,
        workspace_id: String,
        claim_window: WorkspaceClaimWindow,
    },
    ClaimResponse {
        timestamp: String,
        workspace_id: String,
        claim_window_id: String,
        response: WorkspaceClaimResponse,
    },
    ClaimWindowClosed {
        timestamp: String,
        workspace_id: String,
        claim_window: WorkspaceClaimWindow,
        responses: Vec<WorkspaceClaimResponse>,
        selected_role_ids: Vec<String>,
    },
    WorkflowVoteOpened {
        timestamp: String,
        workspace_id: String,
        coordinator_decision: CoordinatorWorkflowDecision,
        vote_window: WorkspaceWorkflowVoteWindow,
    },
    WorkflowVoteResponse {
        timestamp: String,
        workspace_id: String,
        vote_id: String,
        response: WorkspaceWorkflowVoteResponse,
    },
    WorkflowVoteClosed {
        timestamp: String,
        workspace_id: String,
        coordinator_decision: CoordinatorWorkflowDecision,
        vote_window: WorkspaceWorkflowVoteWindow,
        responses: Vec<WorkspaceWorkflowVoteResponse>,
        approved: bool,
    },
    WorkflowStarted {
        timestamp: String,
        workspace_id: String,
        coordinator_decision: CoordinatorWorkflowDecision,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vote_window: Option<WorkspaceWorkflowVoteWindow>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stage_id: Option<String>,
    },
    WorkflowStageStarted {
        timestamp: String,
        workspace_id: String,
        node_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stage_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role_id: Option<String>,
    },
    WorkflowStageCompleted {
        timestamp: String,
        workspace_id: String,
        node_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stage_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role_id: Option<String>,
    },
    DispatchStarted {
        timestamp: String,
        workspace_id: String,
        dispatch: TaskDispatch,
        task_id: String,
        description: String,
    },
    DispatchProgress {
        timestamp: String,
        workspace_id: String,
        dispatch: TaskDispatch,
        task_id: String,
        description: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_tool_name: Option<String>,
    },
    DispatchCompleted {
        timestamp: String,
        workspace_id: String,
        dispatch: TaskDispatch,
        task_id: String,
        output_file: String,
        summary: String,
    },
    DispatchFailed {
        timestamp: String,
        workspace_id: String,
        dispatch: TaskDispatch,
        task_id: String,
        output_file: String,
        summary: String,
    },
    DispatchStopped {
        timestamp: String,
        workspace_id: String,
        dispatch: TaskDispatch,
        task_id: String,
        output_file: String,
        summary: String,
    },
    DispatchResult {
        timestamp: String,
        workspace_id: String,
        dispatch: TaskDispatch,
        task_id: String,
        result_text: String,
    },
    ActivityPublished {
        timestamp: String,
        workspace_id: String,
        activity: WorkspaceActivity,
    },
    ToolProgress {
        timestamp: String,
        workspace_id: String,
        tool_name: String,
        elapsed_time_seconds: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
    },
    Result {
        timestamp: String,
        workspace_id: String,
        subtype: String,
        is_error: bool,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
    },
    Error {
        timestamp: String,
        workspace_id: String,
        error: String,
    },
}
