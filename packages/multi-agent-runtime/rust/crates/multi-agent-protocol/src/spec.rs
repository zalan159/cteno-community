use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MultiAgentProvider {
    ClaudeAgentSdk,
    CodexSdk,
    Cteno,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Default,
    AcceptEdits,
    Plan,
    DontAsk,
    BypassPermissions,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingSource {
    User,
    Project,
    Local,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceVisibility {
    Public,
    Private,
    Coordinator,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimMode {
    Direct,
    Claim,
    CoordinatorOnly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Pending,
    Claimed,
    Supporting,
    Released,
    Declined,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimDecision {
    Claim,
    Support,
    Decline,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowVoteDecision {
    Approve,
    Reject,
    Abstain,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorDecisionKind {
    Respond,
    Delegate,
    ProposeWorkflow,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowMode {
    FreeformTeam,
    Pipeline,
    Loop,
    ReviewLoop,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeType {
    Announce,
    Assign,
    Claim,
    Shell,
    Evaluate,
    Review,
    Branch,
    Loop,
    Artifact,
    Commit,
    Revert,
    Merge,
    Complete,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowEdgeCondition {
    Always,
    Success,
    Failure,
    Timeout,
    Pass,
    Fail,
    Approved,
    Rejected,
    Improved,
    EqualOrWorse,
    Crash,
    Retry,
    Exhausted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompletionStatus {
    Done,
    Stuck,
    Discarded,
    Crash,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClaimPolicy {
    pub mode: ClaimMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_assignees: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_supporting_claims: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_role_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_user_messages: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_coordinator_messages: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_dispatch_lifecycle: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_member_messages: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_visibility: Option<WorkspaceVisibility>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRetryPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowArtifactKind {
    Doc,
    Code,
    Report,
    Metric,
    Evidence,
    Result,
    TaskOrder,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowArtifactSpec {
    pub id: String,
    pub kind: WorkflowArtifactKind,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStageSpec {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_node_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowNodeSpec {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: WorkflowNodeType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_role_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<WorkflowRetryPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_artifacts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produces_artifacts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<WorkspaceVisibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEdgeSpec {
    pub from: String,
    pub to: String,
    pub when: WorkflowEdgeCondition,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompletionPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_node_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_node_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_status: Option<CompletionStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSpec {
    pub mode: WorkflowMode,
    pub entry_node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stages: Option<Vec<WorkflowStageSpec>>,
    pub nodes: Vec<WorkflowNodeSpec>,
    pub edges: Vec<WorkflowEdgeSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RoleAgentSpec {
    pub description: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<MultiAgentProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RoleSpec {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_root: Option<String>,
    pub agent: RoleAgentSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSpec {
    pub id: String,
    pub name: String,
    pub provider: MultiAgentProvider,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<Vec<SettingSource>>,
    pub roles: Vec<RoleSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_policy: Option<ClaimPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_policy: Option<ActivityPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_vote_policy: Option<WorkflowVotePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<WorkflowArtifactSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_policy: Option<CompletionPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RoleTaskRequest {
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTurnRequest {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<WorkspaceVisibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_assignments: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefer_role_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceClaimResponse {
    pub role_id: String,
    pub decision: ClaimDecision,
    pub confidence: f32,
    pub rationale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_response: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_instruction: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceClaimWindow {
    pub window_id: String,
    pub request: WorkspaceTurnRequest,
    pub candidate_role_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowVotePolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_approvals: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_approval_ratio: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_role_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceWorkflowVoteWindow {
    pub vote_id: String,
    pub request: WorkspaceTurnRequest,
    pub reason: String,
    pub candidate_role_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceWorkflowVoteResponse {
    pub role_id: String,
    pub decision: WorkflowVoteDecision,
    pub confidence: f32,
    pub rationale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_response: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CoordinatorWorkflowDecision {
    pub kind: CoordinatorDecisionKind,
    pub response_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_vote_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTurnAssignment {
    pub role_id: String,
    pub instruction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<WorkspaceVisibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTurnPlan {
    pub coordinator_role_id: String,
    pub response_text: String,
    pub assignments: Vec<WorkspaceTurnAssignment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}
