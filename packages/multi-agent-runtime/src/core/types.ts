export type MultiAgentProvider = 'claude-agent-sdk' | 'codex-sdk';
export type WorkspaceProvider = MultiAgentProvider | 'hybrid';

export type WorkspaceVisibility = 'public' | 'private' | 'coordinator';
export type ClaimMode = 'direct' | 'claim' | 'coordinator_only';
export type ClaimStatus = 'pending' | 'claimed' | 'supporting' | 'released' | 'declined';
export type ClaimDecision = 'claim' | 'support' | 'decline';
export type WorkflowVoteDecision = 'approve' | 'reject' | 'abstain';
export type CoordinatorDecisionKind = 'respond' | 'delegate' | 'propose_workflow';
export type MemberStatus = 'idle' | 'active' | 'blocked' | 'waiting' | 'offline';
export type WorkspaceMode = 'group_chat' | 'workflow_vote' | 'workflow_running';
export type WorkflowMode = 'freeform_team' | 'pipeline' | 'loop' | 'review_loop';
export type WorkflowNodeType =
  | 'announce'
  | 'assign'
  | 'claim'
  | 'worklist'
  | 'shell'
  | 'evaluate'
  | 'review'
  | 'branch'
  | 'loop'
  | 'artifact'
  | 'commit'
  | 'revert'
  | 'merge'
  | 'complete';
export type WorkflowEdgeCondition =
  | 'always'
  | 'success'
  | 'failure'
  | 'timeout'
  | 'pass'
  | 'fail'
  | 'approved'
  | 'rejected'
  | 'improved'
  | 'equal_or_worse'
  | 'crash'
  | 'retry'
  | 'exhausted';
export type CompletionStatus = 'done' | 'stuck' | 'discarded' | 'crash';
export type WorkspaceActivityKind =
  | 'user_message'
  | 'coordinator_message'
  | 'claim_window_opened'
  | 'claim_window_closed'
  | 'member_claimed'
  | 'member_supporting'
  | 'member_declined'
  | 'member_progress'
  | 'member_blocked'
  | 'member_delivered'
  | 'member_summary'
  | 'workflow_vote_opened'
  | 'workflow_vote_approved'
  | 'workflow_vote_rejected'
  | 'workflow_started'
  | 'workflow_stage_started'
  | 'workflow_stage_completed'
  | 'workflow_completed'
  | 'dispatch_started'
  | 'dispatch_progress'
  | 'dispatch_completed'
  | 'system_notice';

export type PermissionMode =
  | 'default'
  | 'acceptEdits'
  | 'plan'
  | 'dontAsk'
  | 'bypassPermissions';

export interface RoleAgentSpec {
  description: string;
  prompt: string;
  provider?: MultiAgentProvider;
  tools?: string[];
  disallowedTools?: string[];
  model?: string;
  skills?: string[];
  mcpServers?: string[];
  initialPrompt?: string;
  maxTurns?: number;
  background?: boolean;
  effort?: 'low' | 'medium' | 'high' | 'max' | number;
  permissionMode?: PermissionMode;
}

export interface RoleSpec {
  id: string;
  name: string;
  description?: string;
  direct?: boolean;
  outputRoot?: string;
  agent: RoleAgentSpec;
}

export interface ClaimPolicy {
  mode: ClaimMode;
  claimTimeoutMs?: number;
  maxAssignees?: number;
  allowSupportingClaims?: boolean;
  fallbackRoleId?: string;
}

export interface ActivityPolicy {
  publishUserMessages?: boolean;
  publishCoordinatorMessages?: boolean;
  publishDispatchLifecycle?: boolean;
  publishMemberMessages?: boolean;
  defaultVisibility?: WorkspaceVisibility;
}

export interface WorkflowVotePolicy {
  timeoutMs?: number;
  minimumApprovals?: number;
  requiredApprovalRatio?: number;
  candidateRoleIds?: string[];
}

export interface WorkflowRetryPolicy {
  maxAttempts?: number;
}

export type WorkflowArtifactKind =
  | 'doc'
  | 'code'
  | 'report'
  | 'metric'
  | 'evidence'
  | 'result'
  | 'task_order'
  | 'task_list'
  | 'ledger';

export type WorkflowWorkItemStatus =
  | 'pending'
  | 'running'
  | 'completed'
  | 'failed'
  | 'blocked'
  | 'discarded'
  | 'abandoned'
  | 'superseded';

export type TaskControlStatus =
  | 'pending'
  | 'running'
  | 'completed'
  | 'failed'
  | 'blocked'
  | 'retry_requested';

export type TaskControlReviewVerdict = 'approved' | 'rejected';

export type TaskControlEvidenceKind =
  | 'note'
  | 'log'
  | 'command'
  | 'test'
  | 'diff'
  | 'file'
  | 'report';

export type TaskControlToolName =
  | 'task.set_status'
  | 'task.write_handoff'
  | 'task.submit_review'
  | 'task.record_evidence';

export type TaskControlMetadataValue = string | number | boolean | null;

export interface TaskControlStatusRecord {
  taskId: string;
  status: TaskControlStatus;
  updatedAt: string;
  summary?: string;
  reason?: string;
  actorRoleId?: string;
  metadata?: Record<string, TaskControlMetadataValue>;
}

export interface TaskControlHandoffRecord {
  taskId: string;
  summary: string;
  updatedAt: string;
  details?: string;
  fromRoleId?: string;
  toRoleId?: string;
  metadata?: Record<string, TaskControlMetadataValue>;
}

export interface TaskControlReviewRecord {
  taskId: string;
  verdict: TaskControlReviewVerdict;
  summary: string;
  updatedAt: string;
  issues?: string[];
  retryable?: boolean;
  actorRoleId?: string;
  metadata?: Record<string, TaskControlMetadataValue>;
}

export interface TaskControlEvidenceRecord {
  taskId: string;
  evidenceId: string;
  kind: TaskControlEvidenceKind;
  summary: string;
  recordedAt: string;
  path?: string;
  content?: string;
  actorRoleId?: string;
  metadata?: Record<string, TaskControlMetadataValue>;
}

export interface TaskControlState {
  latestStatus?: TaskControlStatusRecord;
  latestHandoff?: TaskControlHandoffRecord;
  latestReview?: TaskControlReviewRecord;
  evidence: TaskControlEvidenceRecord[];
}

export interface TaskSetStatusInput {
  taskId?: string;
  status: TaskControlStatus;
  summary?: string;
  reason?: string;
  metadata?: Record<string, TaskControlMetadataValue>;
}

export interface TaskWriteHandoffInput {
  taskId?: string;
  summary: string;
  details?: string;
  toRoleId?: string;
  metadata?: Record<string, TaskControlMetadataValue>;
}

export interface TaskSubmitReviewInput {
  taskId?: string;
  verdict: TaskControlReviewVerdict;
  summary: string;
  issues?: string[];
  retryable?: boolean;
  metadata?: Record<string, TaskControlMetadataValue>;
}

export interface TaskRecordEvidenceInput {
  taskId?: string;
  evidenceId?: string;
  kind: TaskControlEvidenceKind;
  summary: string;
  path?: string;
  content?: string;
  metadata?: Record<string, TaskControlMetadataValue>;
}

export type TaskControlToolCall =
  | { toolName: 'task.set_status'; input: TaskSetStatusInput }
  | { toolName: 'task.write_handoff'; input: TaskWriteHandoffInput }
  | { toolName: 'task.submit_review'; input: TaskSubmitReviewInput }
  | { toolName: 'task.record_evidence'; input: TaskRecordEvidenceInput };

export type WorkflowWorkItemDocumentStatus =
  | WorkflowWorkItemStatus
  | 'done'
  | 'complete';

export type WorkflowWorklistMode = 'finite' | 'replenishing';

export interface WorkflowWorkItem {
  id: string;
  title: string;
  description: string;
  status?: WorkflowWorkItemDocumentStatus;
  attempts?: number;
  maxAttempts?: number;
  dependsOn?: string[];
  goalsFile?: string;
  referenceFiles?: string[];
  files?: string[];
  acceptanceCriteria?: string[];
  metadata?: Record<string, string | number | boolean | null>;
}

export interface WorkflowTaskListArtifact {
  version: 1;
  mode?: WorkflowWorklistMode;
  summary?: string;
  items: WorkflowWorkItem[];
}

export interface WorkflowWorklistRuntimeItemState {
  itemId: string;
  title: string;
  status: WorkflowWorkItemStatus;
  attempts: number;
  maxAttempts?: number;
  dispatchId?: string;
  lastSummary?: string;
  lastFeedback?: string;
  evaluatorDispatchId?: string;
  commitDispatchId?: string;
  qaDispatchId?: string;
  taskControl?: TaskControlState;
  updatedAt: string;
}

export interface WorkflowWorklistRuntimeState {
  nodeId: string;
  artifactId: string;
  mode: WorkflowWorklistMode;
  plannerRoleId?: string;
  workerRoleId: string;
  batchCount: number;
  completedItemIds: string[];
  failedItemIds: string[];
  items: Record<string, WorkflowWorklistRuntimeItemState>;
  lastUpdatedAt: string;
}

export interface WorkflowArtifactSpec {
  id: string;
  kind: WorkflowArtifactKind;
  path: string;
  ownerRoleId?: string;
  required?: boolean;
  description?: string;
}

export interface WorkflowStageSpec {
  id: string;
  name: string;
  description?: string;
  entryNodeId?: string;
  exitNodeIds?: string[];
}

export interface WorkflowWorklistEvaluateSpec {
  roleId: string;
  promptTemplate?: string;
  onReject?: 'retry' | 'fail';
}

export interface WorkflowWorklistAfterApproveSpec {
  action: 'none' | 'commit';
  roleId?: string;
  promptTemplate?: string;
}

export interface WorkflowWorklistAfterCommitSpec {
  roleId: string;
  promptTemplate?: string;
  onFailure?: 'retry' | 'fail' | 'warn';
}

export interface WorkflowWorklistLifecycleSpec {
  evaluate?: WorkflowWorklistEvaluateSpec;
  afterApprove?: WorkflowWorklistAfterApproveSpec;
  afterCommit?: WorkflowWorklistAfterCommitSpec;
}

export interface WorkflowNodeSpec {
  id: string;
  type: WorkflowNodeType;
  title?: string;
  provider?: MultiAgentProvider;
  model?: string;
  roleId?: string;
  reviewerRoleId?: string;
  candidateRoleIds?: string[];
  command?: string;
  evaluator?: string;
  prompt?: string;
  worklistArtifactId?: string;
  sharedLessonsPath?: string;
  taskCliPath?: string;
  itemLifecycle?: WorkflowWorklistLifecycleSpec;
  workerRoleId?: string;
  plannerRoleId?: string;
  plannerPrompt?: string;
  worklistMode?: WorkflowWorklistMode;
  replenish?: 'never' | 'when_empty';
  maxBatches?: number;
  stopOnItemFailure?: boolean;
  reloadWorklistAfterItem?: boolean;
  /**
   * When a work item goes back to `pending` (review/QA rejection, retryable commit failure),
   * run `git reset --hard` in the workspace cwd to the SHA captured before the first attempt
   * so the coder restarts from a clean tree. Silently skipped when the cwd is not a git repo.
   */
  revertOnRetry?: boolean;
  /**
   * When a work item terminates as `failed` or `discarded` (retries exhausted or non-retryable
   * rejection), run `git reset --hard` to the pre-task SHA so partial commits do not linger.
   */
  revertOnExhaustedFailure?: boolean;
  /**
   * Wall-clock timeout in milliseconds for a single work-item lifecycle (worker + evaluator +
   * commit + afterCommit). When exceeded, the lifecycle is treated as failed and the item
   * enters the usual retry/exhausted path (so revertOnRetry / revertOnExhaustedFailure apply).
   */
  itemTimeoutMs?: number;
  itemPromptTemplate?: string;
  timeoutMs?: number;
  retry?: WorkflowRetryPolicy;
  requiresArtifacts?: string[];
  producesArtifacts?: string[];
  visibility?: WorkspaceVisibility;
  stageId?: string;
}

export interface WorkflowEdgeSpec {
  from: string;
  to: string;
  when: WorkflowEdgeCondition;
}

export interface CompletionPolicy {
  successNodeIds?: string[];
  failureNodeIds?: string[];
  maxIterations?: number;
  defaultStatus?: CompletionStatus;
}

export interface WorkflowSpec {
  mode: WorkflowMode;
  entryNodeId: string;
  stages?: WorkflowStageSpec[];
  nodes: WorkflowNodeSpec[];
  edges: WorkflowEdgeSpec[];
}

export interface WorkspaceSpec {
  id: string;
  name: string;
  provider: WorkspaceProvider;
  model?: string;
  defaultProvider?: MultiAgentProvider;
  defaultModel?: string;
  cwd?: string;
  orchestratorPrompt?: string;
  allowedTools?: string[];
  disallowedTools?: string[];
  permissionMode?: PermissionMode;
  settingSources?: Array<'user' | 'project' | 'local'>;
  roles: RoleSpec[];
  defaultRoleId?: string;
  coordinatorRoleId?: string;
  claimPolicy?: ClaimPolicy;
  activityPolicy?: ActivityPolicy;
  workflowVotePolicy?: WorkflowVotePolicy;
  workflow?: WorkflowSpec;
  artifacts?: WorkflowArtifactSpec[];
  completionPolicy?: CompletionPolicy;
}

export interface RoleTaskRequest {
  roleId: string;
  instruction: string;
  summary?: string;
  provider?: MultiAgentProvider;
  model?: string;
  visibility?: WorkspaceVisibility;
  sourceRoleId?: string;
  workflowNodeId?: string;
  stageId?: string;
  workItemId?: string;
}

export interface WorkspaceTurnRequest {
  message: string;
  visibility?: WorkspaceVisibility;
  maxAssignments?: number;
  preferRoleId?: string;
  workflowEntry?: 'auto' | 'direct';
}

export interface WorkspaceClaimResponse {
  roleId: string;
  decision: ClaimDecision;
  confidence: number;
  rationale: string;
  publicResponse?: string;
  proposedInstruction?: string;
}

export interface WorkspaceClaimWindow {
  windowId: string;
  request: WorkspaceTurnRequest;
  candidateRoleIds: string[];
  timeoutMs?: number;
}

export interface WorkspaceWorkflowVoteWindow {
  voteId: string;
  request: WorkspaceTurnRequest;
  reason: string;
  candidateRoleIds: string[];
  timeoutMs?: number;
}

export interface WorkspaceWorkflowVoteResponse {
  roleId: string;
  decision: WorkflowVoteDecision;
  confidence: number;
  rationale: string;
  publicResponse?: string;
}

export interface CoordinatorWorkflowDecision {
  kind: CoordinatorDecisionKind;
  responseText: string;
  targetRoleId?: string;
  workflowVoteReason?: string;
  rationale?: string;
}

export interface WorkspaceTurnAssignment {
  roleId: string;
  instruction: string;
  summary?: string;
  provider?: MultiAgentProvider;
  model?: string;
  visibility?: WorkspaceVisibility;
  workflowNodeId?: string;
  stageId?: string;
  workItemId?: string;
}

export interface WorkspaceTurnPlan {
  coordinatorRoleId: string;
  responseText: string;
  assignments: WorkspaceTurnAssignment[];
  rationale?: string;
}

export interface WorkspaceTurnResult {
  request: WorkspaceTurnRequest;
  coordinatorDispatch?: TaskDispatch;
  claimWindow?: WorkspaceClaimWindow;
  claimResponses?: WorkspaceClaimResponse[];
  workflowVoteWindow?: WorkspaceWorkflowVoteWindow;
  workflowVoteResponses?: WorkspaceWorkflowVoteResponse[];
  plan: WorkspaceTurnPlan;
  dispatches: TaskDispatch[];
}

export interface TaskDispatch {
  dispatchId: string;
  workspaceId: string;
  roleId: string;
  provider?: MultiAgentProvider;
  model?: string;
  instruction: string;
  summary?: string;
  visibility?: WorkspaceVisibility;
  sourceRoleId?: string;
  workflowNodeId?: string;
  stageId?: string;
  workItemId?: string;
  status: 'queued' | 'started' | 'running' | 'completed' | 'failed' | 'stopped';
  providerTaskId?: string;
  toolUseId?: string;
  createdAt: string;
  startedAt?: string;
  completedAt?: string;
  outputFile?: string;
  lastSummary?: string;
  resultText?: string;
  taskControl?: TaskControlState;
  claimedByMemberIds?: string[];
  claimStatus?: ClaimStatus;
}

export interface WorkspaceMember {
  memberId: string;
  workspaceId: string;
  roleId: string;
  roleName: string;
  provider?: MultiAgentProvider;
  direct?: boolean;
  sessionId?: string;
  status: MemberStatus;
  publicStateSummary?: string;
  lastActivityAt?: string;
}

export interface WorkspaceActivity {
  activityId: string;
  workspaceId: string;
  kind: WorkspaceActivityKind;
  visibility: WorkspaceVisibility;
  text: string;
  createdAt: string;
  roleId?: string;
  memberId?: string;
  dispatchId?: string;
  taskId?: string;
}

export interface WorkspaceWorkflowRuntimeState {
  mode: WorkspaceMode;
  activeVoteWindow?: WorkspaceWorkflowVoteWindow;
  activeNodeId?: string;
  activeStageId?: string;
  worklists?: Record<string, WorkflowWorklistRuntimeState>;
}

export interface WorkspaceState {
  workspaceId: string;
  status: 'idle' | 'running' | 'requires_action' | 'closed';
  provider: WorkspaceProvider;
  sessionId?: string;
  startedAt?: string;
  roles: Record<string, RoleSpec>;
  dispatches: Record<string, TaskDispatch>;
  members: Record<string, WorkspaceMember>;
  activities: WorkspaceActivity[];
  workflowRuntime: WorkspaceWorkflowRuntimeState;
}
