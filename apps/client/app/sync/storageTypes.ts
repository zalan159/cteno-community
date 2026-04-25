import { z } from "zod";

//
// Agent states
//

export const MetadataSchema = z.object({
    path: z.string(),
    host: z.string(),
    vendor: z.string().optional(), // Authoritative executor vendor for this session
    version: z.string().optional(),
    name: z.string().optional(),
    os: z.string().optional(),
    summary: z.object({
        text: z.string(),
        updatedAt: z.number()
    }).optional(),
    machineId: z.string().optional(),
    claudeSessionId: z.string().optional(), // Claude Code session ID
    tools: z.array(z.string()).optional(),
    slashCommands: z.array(z.string()).optional(),
    homeDir: z.string().optional(), // User's home directory on the machine
    happyHomeDir: z.string().optional(), // Happy configuration directory 
    hostPid: z.number().optional(), // Process ID of the session
    flavor: z.string().nullish(), // Session flavor/variant identifier
    modelId: z.string().optional(), // Selected model ID for this session
    proxyModelId: z.string().optional(), // Proxy model ID when using built-in proxy models
    permissionMode: z.string().optional() // Permission mode set at session creation (for worker sessions)
});

export type Metadata = z.infer<typeof MetadataSchema>;

export const AgentStateSchema = z.object({
    controlledByUser: z.boolean().nullish(),
    requests: z.record(z.string(), z.object({
        tool: z.string(),
        arguments: z.any(),
        createdAt: z.number().nullish()
    })).nullish(),
    completedRequests: z.record(z.string(), z.object({
        tool: z.string().nullish(),
        arguments: z.any(),
        createdAt: z.number().nullish(),
        completedAt: z.number().nullish(),
        status: z.enum(['canceled', 'denied', 'approved']),
        reason: z.string().nullish(),
        mode: z.string().nullish(),
        allowedTools: z.array(z.string()).nullish(),
        decision: z.enum(['approved', 'approved_for_session', 'denied', 'abort']).nullish()
    })).nullish()
});

export type AgentState = z.infer<typeof AgentStateSchema>;

export type RuntimeEffort = 'default' | 'low' | 'medium' | 'high' | 'xhigh' | 'max';

export interface Session {
    id: string,
    seq: number,
    createdAt: number,
    updatedAt: number,
    active: boolean,
    activeAt: number,
    metadata: Metadata | null,
    metadataVersion: number,
    agentState: AgentState | null,
    agentStateVersion: number,
    thinking: boolean,
    thinkingAt: number,
    thinkingStatus?: string,
    presence: "online" | number, // "online" when active, timestamp when last seen
    todos?: Array<{
        content: string;
        status: 'pending' | 'in_progress' | 'completed';
        priority: 'high' | 'medium' | 'low';
        id: string;
    }>;
    draft?: string | null; // Local draft message, not synced to server
    permissionMode?: 'default' | 'auto' | 'acceptEdits' | 'plan' | 'dontAsk' | 'bypassPermissions' | 'read-only' | 'safe-yolo' | 'yolo' | null; // Local permission mode, not synced to server
    runtimeEffort?: RuntimeEffort | null; // Local reasoning effort, not synced to server
    sandboxPolicy?: 'workspace_write' | 'unrestricted' | null; // Sandbox policy: workspace boundary enforcement
    modelMode?: 'default' | 'gemini-2.5-pro' | 'gemini-2.5-flash' | 'gemini-2.5-flash-lite' | null; // Local model mode, not synced to server
    // IMPORTANT: latestUsage is extracted from reducerState.latestUsage after message processing.
    // We store it directly on Session to ensure it's available immediately on load.
    // Do NOT store reducerState itself on Session - it's mutable and should only exist in SessionMessages.
    latestUsage?: {
        inputTokens: number;
        outputTokens: number;
        cacheCreation: number;
        cacheRead: number;
        contextSize: number;
        timestamp: number;
    } | null;
    contextTokens?: number;
    contextWindowTokens?: number;
    autoCompactTokenLimit?: number;
    compressionThreshold?: number;
    ownerSessionId?: string;
    // SSE streaming state: accumulates deltas for real-time display
    streamingText?: string;
    streamingThinking?: string;
    streamingNotice?: string;
    promptSuggestions?: string[];
}

// ---------------------------------------------------------------------------
// Vendor quota — machine-level plan/rate-limit snapshot.
//
// Populated by the daemon's `cteno-host-quota-monitor`, pulled via the
// `{machineId}:quota-read` RPC on a 60s frontend poll. Not session-scoped:
// every session with the same vendor shows the same data.
// ---------------------------------------------------------------------------

export type VendorQuotaId = 'claude' | 'codex' | 'gemini';
export type VendorQuotaShape = 'windows' | 'buckets';

export interface QuotaWindow {
    usedPercent: number;     // 0-100, "已用"。UI 显示剩余时 = 100 - usedPercent
    resetsAt?: number;       // unix seconds
    windowDurationMins?: number;
    status?: string;         // 'allowed' | 'allowed_warning' | 'rejected' | ...
    limitType?: string;      // '5h' | '7d' | ...
}

export interface QuotaBucket {
    modelId: string;
    tokenType: string;       // 'REQUESTS' | 'TOKENS'
    usedPercent: number;     // 0-100
    resetsAt?: number;
    remainingAmount?: string;
}

export interface QuotaCredits {
    hasCredits: boolean;
    unlimited: boolean;
    balance?: string;
}

export interface VendorQuota {
    provider: VendorQuotaId;
    shape: VendorQuotaShape;
    /** Present when shape === 'windows'. Keys like 'fiveHour', 'weekly', etc. */
    windows?: Record<string, QuotaWindow>;
    /** Present when shape === 'buckets'. */
    buckets?: QuotaBucket[];
    credits?: QuotaCredits;
    planType?: string;
    primaryModel?: string;
    updatedAt: number;       // unix seconds
    error?: string;          // set when the last probe failed
}

export interface SessionTaskLifecycleEntry {
    taskId: string;
    state: 'running' | 'completed' | 'error';
    updatedAt: number;
    startedAt?: number | null;
    completedAt?: number | null;
    summary?: string | null;
    description?: string | null;
    taskType?: string | null;
}

export interface DecryptedMessage {
    id: string,
    seq: number | null,
    localId: string | null,
    content: any,
    createdAt: number,
}

//
// Machine states
//

export const MachineMetadataSchema = z.object({
    host: z.string(),
    platform: z.string(),
    happyCliVersion: z.string(),
    happyHomeDir: z.string(), // Directory for Happy auth, settings, logs (usually .happy/ or .happy-dev/)
    homeDir: z.string(), // User's home directory (matches CLI field name)
    // Optional fields that may be added in future versions
    username: z.string().optional(),
    arch: z.string().optional(),
    displayName: z.string().optional(), // Custom display name for the machine
    // Daemon status fields
    daemonLastKnownStatus: z.enum(['running', 'shutting-down']).optional(),
    daemonLastKnownPid: z.number().optional(),
    shutdownRequestedAt: z.number().optional(),
    shutdownSource: z.enum(['happy-app', 'happy-cli', 'os-signal', 'unknown']).optional()
});

export type MachineMetadata = z.infer<typeof MachineMetadataSchema>;

export interface Machine {
    id: string;
    seq: number;
    createdAt: number;
    updatedAt: number;
    active: boolean;
    activeAt: number;  // Changed from lastActiveAt to activeAt for consistency
    metadata: MachineMetadata | null;
    metadataVersion: number;
    daemonState: any | null;  // Dynamic daemon state (runtime info)
    daemonStateVersion: number;
    decryptionFailed?: boolean;  // Flag to indicate decryption failure
}

//
// Git Status
//

//
// Persona
//

export interface Persona {
    id: string;
    name: string;
    avatarId: string;
    description: string;
    personalityNotes: string;
    model: string;
    modelId: string | null;
    agent?: 'cteno' | 'claude' | 'codex' | 'gemini';
    workdir: string;
    chatSessionId: string;
    isDefault: boolean;
    continuousBrowsing: boolean;
    createdAt: string;
    updatedAt: string;
}

export interface PersonaProject {
    machineId: string;
    workdir: string;
    createdAt: number;
    updatedAt: number;
}

export interface PersonaTaskSummary {
    sessionId: string;
    taskDescription: string;
    createdAt: string;
}

export interface WorkspaceBinding {
    personaId: string;
    workspaceId: string;
    templateId: string;
    provider: string;
    defaultRoleId: string | null;
    model: string;
    workdir: string;
    createdAt: string;
    updatedAt: string;
}

export interface WorkspaceMemberSummary {
    roleId: string | null;
    sessionId: string;
    agentId: string | null;
    taskDescription: string | null;
    createdAt: string;
}

export type WorkspaceVisibility = 'public' | 'private' | 'coordinator';
export type WorkspaceStatus = 'idle' | 'running' | 'requires_action' | 'closed';
export type MemberStatus = 'idle' | 'active' | 'blocked' | 'waiting' | 'offline';
export type WorkspaceMode = 'group_chat' | 'workflow_vote' | 'workflow_running';
export type WorkspaceActivityKind =
    | 'user_message'
    | 'coordinator_message'
    | 'claim_window_opened'
    | 'claim_window_closed'
    | 'workflow_vote_opened'
    | 'workflow_vote_approved'
    | 'workflow_vote_rejected'
    | 'workflow_started'
    | 'workflow_stage_started'
    | 'workflow_stage_completed'
    | 'workflow_completed'
    | 'member_claimed'
    | 'member_supporting'
    | 'member_declined'
    | 'member_progress'
    | 'member_blocked'
    | 'member_delivered'
    | 'member_summary'
    | 'dispatch_started'
    | 'dispatch_progress'
    | 'dispatch_completed'
    | 'system_notice';
export type DispatchStatus =
    | 'queued'
    | 'started'
    | 'running'
    | 'completed'
    | 'failed'
    | 'stopped';
export type ClaimStatus =
    | 'pending'
    | 'claimed'
    | 'supporting'
    | 'released'
    | 'declined';

export interface WorkspaceDispatch {
    dispatchId: string;
    workspaceId: string;
    roleId: string;
    instruction: string;
    summary?: string | null;
    visibility?: WorkspaceVisibility | null;
    sourceRoleId?: string | null;
    status: DispatchStatus;
    providerTaskId?: string | null;
    toolUseId?: string | null;
    createdAt: string;
    startedAt?: string | null;
    completedAt?: string | null;
    outputFile?: string | null;
    lastSummary?: string | null;
    resultText?: string | null;
    claimedByMemberIds?: string[] | null;
    claimStatus?: ClaimStatus | null;
}

export interface WorkspaceTurnAssignment {
    roleId: string;
    instruction: string;
    summary?: string | null;
    visibility?: WorkspaceVisibility | null;
}

export interface WorkspaceTurnPlan {
    coordinatorRoleId: string;
    responseText: string;
    assignments: WorkspaceTurnAssignment[];
    rationale?: string | null;
}

export type WorkflowVoteDecision = 'approve' | 'reject' | 'abstain';

export interface WorkspaceWorkflowVoteWindow {
    voteId: string;
    request: {
        message: string;
        visibility?: WorkspaceVisibility | null;
        maxAssignments?: number | null;
        preferRoleId?: string | null;
    };
    reason: string;
    candidateRoleIds: string[];
    timeoutMs?: number | null;
}

export interface WorkspaceWorkflowVoteResponse {
    roleId: string;
    decision: WorkflowVoteDecision;
    rationale: string;
    publicResponse?: string | null;
}

export interface WorkspaceRuntimeMember {
    memberId: string;
    workspaceId: string;
    roleId: string;
    roleName: string;
    direct?: boolean | null;
    sessionId?: string | null;
    status: MemberStatus;
    publicStateSummary?: string | null;
    lastActivityAt?: string | null;
}

export interface WorkspaceActivity {
    activityId: string;
    workspaceId: string;
    kind: WorkspaceActivityKind;
    visibility: WorkspaceVisibility;
    text: string;
    createdAt: string;
    roleId?: string | null;
    memberId?: string | null;
    dispatchId?: string | null;
    taskId?: string | null;
}

export interface WorkspaceRuntimeState {
    workspaceId: string;
    status: WorkspaceStatus;
    provider: string;
    sessionId?: string | null;
    startedAt?: string | null;
    roles: Record<string, any>;
    members: Record<string, WorkspaceRuntimeMember>;
    dispatches: Record<string, WorkspaceDispatch>;
    activities: WorkspaceActivity[];
    workflowRuntime?: {
        mode: WorkspaceMode;
        activeVoteWindow?: WorkspaceWorkflowVoteWindow | null;
        activeRequestMessage?: string | null;
        activeNodeId?: string | null;
        activeStageId?: string | null;
    } | null;
}

export interface WorkspaceEvent {
    type: string;
    timestamp: string;
    workspaceId: string;
    [key: string]: any;
}

export interface WorkspaceRuntimeSummary {
    state: WorkspaceRuntimeState;
    recentActivities: WorkspaceActivity[];
    recentEvents: WorkspaceEvent[];
}

export interface WorkspaceSummary {
    binding: WorkspaceBinding;
    persona: Persona;
    members: WorkspaceMemberSummary[];
    runtime?: WorkspaceRuntimeSummary | null;
}

// ============================================================================
// Orchestration Flow Types
// ============================================================================

export type FlowNodeStatus = 'pending' | 'running' | 'completed' | 'failed' | 'skipped';
export type FlowEdgeType = 'normal' | 'retry' | 'conditional';

export interface FlowNode {
    id: string;
    label: string;
    agentType?: string;
    status: FlowNodeStatus;
    sessionId?: string;
    iteration?: number;
    maxIterations?: number;
}

export interface FlowEdge {
    from: string;
    to: string;
    condition?: string;
    edgeType: FlowEdgeType;
}

export interface OrchestrationFlow {
    id: string;
    personaId: string;
    sessionId: string;
    title: string;
    nodes: FlowNode[];
    edges: FlowEdge[];
    createdAt: string;
}

//
// Git Status
//

// ============================================================================
// Agent Config (for agent management UI)
// ============================================================================

export interface AgentConfig {
    id: string;
    name: string;
    description: string;
    version: string;
    agent_type: string;
    instructions: string;
    model: string;
    temperature: number | null;
    max_tokens: number | null;
    tools: string[];
    skills: string[];
    source: 'builtin' | 'global' | 'workspace';
    allowed_tools: string[];
    excluded_tools: string[];
    expose_as_tool: boolean;
}

// ============================================================================
// Agent Types
// ============================================================================

export interface TargetPage {
    id: string;
    agentId: string;
    html: string;
    dataJson: string;
    mode: 'display' | 'awaiting_input';
    timeoutAt?: string;
    defaultResult?: string;
    createdAt: string;
    updatedAt: string;
}

export interface GoalTreePage {
    id: string;
    agentId: string;
    html: string;
    dataJson: string;
    createdAt: string;
    updatedAt: string;
}

export interface AgentNotification {
    id: string;
    agentId: string;
    title: string;
    body: string;
    category: string;
    priority: 'low' | 'normal' | 'high' | 'urgent';
    targetId?: string;
    read: boolean;
    createdAt: string;
}

export interface GitStatus {
    branch: string | null;
    isDirty: boolean;
    modifiedCount: number;
    untrackedCount: number;
    stagedCount: number;
    lastUpdatedAt: number;
    // Line change statistics - separated by staged vs unstaged
    stagedLinesAdded: number;
    stagedLinesRemoved: number;
    unstagedLinesAdded: number;
    unstagedLinesRemoved: number;
    // Computed totals
    linesAdded: number;      // stagedLinesAdded + unstagedLinesAdded
    linesRemoved: number;    // stagedLinesRemoved + unstagedLinesRemoved
    linesChanged: number;    // Total lines that were modified (added + removed)
    // Branch tracking information (from porcelain v2)
    upstreamBranch?: string | null; // Name of upstream branch
    aheadCount?: number; // Commits ahead of upstream
    behindCount?: number; // Commits behind upstream
    stashCount?: number; // Number of stash entries
}
