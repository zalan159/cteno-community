import { randomUUID } from 'node:crypto';
import { access, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';

import type { WorkspaceEvent, WorkspaceInitializedEvent, WorkspaceMessageEvent, WorkflowStageEvent } from '../../core/events.js';
import {
  type PersistedProviderState,
  LocalWorkspacePersistence,
} from '../../core/localPersistence.js';
import {
  resolveDispatchTarget,
  resolveRoleModel,
  resolveRoleProvider,
  resolveWorkspaceDefaultModel,
} from '../../core/providerResolution.js';
import { WorkspaceRuntime } from '../../core/runtime.js';
import type {
  ClaimStatus,
  CoordinatorWorkflowDecision,
  MultiAgentProvider,
  RoleSpec,
  RoleTaskRequest,
  TaskDispatch,
  WorkspaceActivity,
  WorkspaceActivityKind,
  WorkspaceClaimResponse,
  WorkspaceClaimWindow,
  WorkspaceMember,
  WorkspaceSpec,
  WorkspaceState,
  WorkspaceTurnRequest,
  WorkspaceTurnResult,
  WorkspaceVisibility,
  WorkflowNodeSpec,
  WorkflowTaskListArtifact,
  WorkflowWorkItem,
  WorkflowWorkItemDocumentStatus,
  WorkflowWorklistRuntimeState,
  WorkspaceWorkflowVoteResponse,
  WorkspaceWorkflowVoteWindow,
} from '../../core/types.js';
import {
  buildPlanFromClaimResponses,
  buildWorkflowEntryPlan,
  getWorkflowEntryNode,
  planWorkspaceTurnHeuristically,
  resolveClaimCandidateRoleIds,
  resolveWorkflowVoteCandidateRoleIds,
  shouldApproveWorkflowVote,
} from '../../core/workspaceTurn.js';
import { executeWorkflow } from '../../core/workflowExecution.js';
import { ClaudeAgentWorkspace, type ClaudeAgentWorkspaceOptions } from '../claude/claudeAgentWorkspace.js';
import { CodexSdkWorkspace, type CodexSdkWorkspaceOptions } from '../codex/codexSdkWorkspace.js';

type ChildWorkspace = ClaudeAgentWorkspace | CodexSdkWorkspace;

export interface HybridWorkspaceOptions {
  spec: WorkspaceSpec;
  defaultModels?: Partial<Record<MultiAgentProvider, string>>;
  claude?: Omit<ClaudeAgentWorkspaceOptions, 'spec' | 'sessionId'>;
  codex?: Omit<CodexSdkWorkspaceOptions, 'spec'>;
  restoredChildren?: Partial<Record<MultiAgentProvider, ChildWorkspace>>;
  restoredChildSpecs?: Partial<Record<MultiAgentProvider, WorkspaceSpec>>;
}

export interface ResumePendingWorkOptions {
  message?: string;
  timeoutMs?: number;
  resultTimeoutMs?: number;
  retryFailedItems?: boolean;
  retryExhaustedItems?: boolean;
  retryBlockedItems?: boolean;
  retrySpecificItemIds?: string[];
  resetRunningItems?: boolean;
  resetAttemptsForRetriedItems?: boolean;
}

export class HybridWorkspace extends WorkspaceRuntime {
  private readonly spec: WorkspaceSpec;
  private readonly state: WorkspaceState;
  private readonly defaultModels: Partial<Record<MultiAgentProvider, string>>;
  private readonly childWorkspaces = new Map<MultiAgentProvider, ChildWorkspace>();
  private readonly childWorkspaceIds = new Map<MultiAgentProvider, string>();
  private readonly childUnsubscribers: Array<() => void> = [];
  private readonly childSessionIds = new Map<MultiAgentProvider, string>();
  private readonly persistence: LocalWorkspacePersistence | undefined;
  private persistenceFlushed = Promise.resolve();
  private restoredFromPersistence = false;
  private active = false;
  private initialized = false;

  constructor(options: HybridWorkspaceOptions) {
    super();
    this.spec = options.spec;
    this.defaultModels = options.defaultModels ?? {};
    this.persistence = LocalWorkspacePersistence.fromSpec(this.spec);
    this.assertHybridSpec();

    this.state = {
      workspaceId: this.spec.id,
      status: 'idle',
      provider: 'hybrid',
      roles: Object.fromEntries(this.spec.roles.map(role => [role.id, role])),
      dispatches: {},
      members: Object.fromEntries(
        this.spec.roles.map(role => [
          role.id,
          {
            memberId: role.id,
            workspaceId: this.spec.id,
            roleId: role.id,
            roleName: role.name,
            provider: resolveRoleProvider(this.spec, role),
            ...(role.direct !== undefined ? { direct: role.direct } : {}),
            status: 'idle',
          } satisfies WorkspaceMember,
        ]),
      ),
      activities: [],
      workflowRuntime: {
        mode: 'group_chat',
      },
    };

    const restoredChildren = options.restoredChildren;
    const restoredChildSpecs = options.restoredChildSpecs;

    if (restoredChildren && restoredChildSpecs) {
      for (const provider of ['claude-agent-sdk', 'codex-sdk'] as const) {
        const workspace = restoredChildren[provider];
        const childSpec = restoredChildSpecs[provider];
        if (!workspace || !childSpec) {
          continue;
        }
        this.childWorkspaceIds.set(provider, childSpec.id);
        this.childWorkspaces.set(provider, workspace);
        this.childUnsubscribers.push(
          workspace.onEvent(event => {
            this.handleChildEvent(provider, event);
          }),
        );
      }
    } else {
      const rolesByProvider = this.groupRolesByProvider();
      for (const [provider, roles] of rolesByProvider.entries()) {
        const childSpec = this.buildChildSpec(provider, roles);
        this.childWorkspaceIds.set(provider, childSpec.id);
        const workspace =
          provider === 'claude-agent-sdk'
            ? new ClaudeAgentWorkspace({
                ...(options.claude ?? {}),
                spec: childSpec,
              })
            : new CodexSdkWorkspace({
                ...(options.codex ?? {}),
                spec: childSpec,
              });
        this.childWorkspaces.set(provider, workspace);
        this.childUnsubscribers.push(
          workspace.onEvent(event => {
            this.handleChildEvent(provider, event);
          }),
        );
      }
    }
  }

  static async restoreFromLocal(
    options: Omit<HybridWorkspaceOptions, 'spec' | 'restoredChildren' | 'restoredChildSpecs'> & {
      cwd: string;
      workspaceId: string;
    },
  ): Promise<HybridWorkspace> {
    const persistence = LocalWorkspacePersistence.fromWorkspace(
      options.cwd,
      options.workspaceId,
    );
    const restoredEntries = (
      await Promise.all(
        (['claude-agent-sdk', 'codex-sdk'] as const).map(async provider => {
          const childWorkspaceId = `${options.workspaceId}--${provider === 'claude-agent-sdk' ? 'claude' : 'codex'}`;
          const persistence = LocalWorkspacePersistence.fromWorkspace(
            options.cwd,
            childWorkspaceId,
          );
          try {
            await access(persistence.workspaceSpecPath());
          } catch {
            return undefined;
          }

          const childSpec = await persistence.loadWorkspaceSpec();
          const childWorkspace =
            provider === 'claude-agent-sdk'
              ? await ClaudeAgentWorkspace.restoreFromLocal({
                  ...(options.claude ?? {}),
                  cwd: options.cwd,
                  workspaceId: childWorkspaceId,
                })
              : await CodexSdkWorkspace.restoreFromLocal({
                  ...(options.codex ?? {}),
                  cwd: options.cwd,
                  workspaceId: childWorkspaceId,
                });

          return {
            provider,
            childSpec,
            childWorkspace,
          };
        }),
      )
    ).filter(
      (
        entry,
      ): entry is {
        provider: MultiAgentProvider;
        childSpec: WorkspaceSpec;
        childWorkspace: ChildWorkspace;
      } => Boolean(entry),
    );

    if (restoredEntries.length === 0) {
      throw new Error(
        `No persisted hybrid child workspaces were found for workspace "${options.workspaceId}".`,
      );
    }

    const restoredChildren = Object.fromEntries(
      restoredEntries.map(entry => [entry.provider, entry.childWorkspace]),
    ) as Partial<Record<MultiAgentProvider, ChildWorkspace>>;
    const restoredChildSpecs = Object.fromEntries(
      restoredEntries.map(entry => [entry.provider, entry.childSpec]),
    ) as Partial<Record<MultiAgentProvider, WorkspaceSpec>>;

    const spec = HybridWorkspace.reconstructHybridSpec(
      options.workspaceId,
      restoredEntries.map(entry => ({
        provider: entry.provider,
        spec: entry.childSpec,
      })),
    );

    const workspace = new HybridWorkspace({
      ...options,
      spec,
      restoredChildren,
      restoredChildSpecs,
    });
    const childSnapshots = restoredEntries.map(entry => ({
      provider: entry.provider,
      snapshot: entry.childWorkspace.getSnapshot(),
    }));
    try {
      const [snapshot, providerState] = await Promise.all([
        persistence.loadWorkspaceState(),
        persistence.loadProviderState(),
      ]);
      workspace.applyPersistedState(snapshot, providerState);
      workspace.restoredFromPersistence = true;
    } catch {
      workspace.applyRestoredState(childSnapshots);
    }
    return workspace;
  }

  getSnapshot(): WorkspaceState {
    return {
      ...this.state,
      roles: { ...this.state.roles },
      dispatches: { ...this.state.dispatches },
      members: { ...this.state.members },
      activities: [...this.state.activities],
      workflowRuntime: { ...this.state.workflowRuntime },
    };
  }

  getPersistenceRoot(): string | undefined {
    return this.persistence?.root;
  }

  async start(): Promise<void> {
    if (this.active) {
      return;
    }

    await this.ensurePersistenceInitialized();

    this.active = true;
    this.state.startedAt = new Date().toISOString();
    this.emitEvent({
      type: 'workspace.started',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      spec: this.spec,
    });

    this.initialized = true;
    this.state.status = 'running';
    this.emitEvent(this.buildInitializedEvent());
    this.emitStateChanged('running');
    this.activateProcessExitCleanup();
  }

  async send(message: string, visibility: WorkspaceVisibility = 'public'): Promise<void> {
    this.ensureStarted();

    const event: WorkspaceMessageEvent = {
      type: 'message',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      role: 'user',
      text: message,
      visibility,
      raw: {
        type: 'workspace_user_message',
      },
    };
    this.emitLocalEvent(event);
    this.publishActivity('user_message', message, {
      visibility,
    });
  }

  async assignRoleTask(request: RoleTaskRequest): Promise<TaskDispatch> {
    this.ensureStarted();
    const role = this.state.roles[request.roleId];
    if (!role) {
      throw new Error(`Unknown role: ${request.roleId}`);
    }

    const target = resolveDispatchTarget(this.spec, role, request);
    const provider = target.provider;
    const child = await this.prepareChildWorkspaceForDispatch(provider, request);
    const dispatch = await child.assignRoleTask({
      ...request,
      provider: target.provider,
      model: target.model,
    });
    return this.toTopLevelDispatch(dispatch, provider);
  }

  async runRoleTask(
    request: RoleTaskRequest,
    options: { timeoutMs?: number; resultTimeoutMs?: number } = {},
  ): Promise<TaskDispatch> {
    return this.runDispatch(this.assignRoleTask(request), options);
  }

  async runWorkspaceTurn(
    request: WorkspaceTurnRequest,
    options: { timeoutMs?: number; resultTimeoutMs?: number } = {},
  ): Promise<WorkspaceTurnResult> {
    this.ensureStarted();
    await this.send(request.message, request.visibility ?? 'public');

    const coordinatorRole = this.resolveCoordinatorRole();
    const coordinatorDecision =
      request.workflowEntry === 'direct' && this.spec.workflow
        ? this.buildDirectWorkflowDecision(request)
        : await this.requestCoordinatorDecision(request, options.timeoutMs);
    this.emitCoordinatorSummary(coordinatorDecision.responseText, coordinatorRole.id);

    if (coordinatorDecision.kind === 'respond') {
      return {
        request,
        plan: {
          coordinatorRoleId: coordinatorRole.id,
          responseText: coordinatorDecision.responseText,
          assignments: [],
          ...(coordinatorDecision.rationale ? { rationale: coordinatorDecision.rationale } : {}),
        },
        dispatches: [],
      };
    }

    let workflowVoteWindow: WorkspaceWorkflowVoteWindow | undefined;
    let workflowVoteResponses: WorkspaceWorkflowVoteResponse[] | undefined;
    let shouldRunWorkflow = request.workflowEntry === 'direct' && Boolean(this.spec.workflow);
    if (shouldRunWorkflow) {
      this.emitWorkflowStarted(coordinatorDecision);
    } else if (coordinatorDecision.kind === 'propose_workflow') {
      workflowVoteWindow = this.openWorkflowVoteWindow(
        request,
        coordinatorDecision,
        resolveWorkflowVoteCandidateRoleIds(this.spec, request, coordinatorDecision),
      );
      workflowVoteResponses = await this.collectWorkflowVoteResponses(
        workflowVoteWindow,
        request,
        coordinatorDecision,
        options.timeoutMs,
      );
      shouldRunWorkflow = shouldApproveWorkflowVote(this.spec, workflowVoteResponses);
      this.closeWorkflowVoteWindow(
        workflowVoteWindow,
        coordinatorDecision,
        workflowVoteResponses,
        shouldRunWorkflow,
      );
      if (!shouldRunWorkflow) {
        return {
          request,
          workflowVoteWindow,
          workflowVoteResponses,
          plan: {
            coordinatorRoleId: coordinatorRole.id,
            responseText: coordinatorDecision.responseText,
            assignments: [],
            rationale: 'Workflow vote rejected; staying in group chat mode.',
          },
          dispatches: [],
        };
      }
      this.emitWorkflowStarted(coordinatorDecision, workflowVoteWindow);
    }

    const effectiveRequest =
      coordinatorDecision.kind === 'delegate' && coordinatorDecision.targetRoleId
        ? { ...request, preferRoleId: coordinatorDecision.targetRoleId }
        : request;

    const claimCandidateRoleIds =
      !shouldRunWorkflow && this.spec.claimPolicy?.mode === 'claim'
        ? resolveClaimCandidateRoleIds(this.spec, effectiveRequest)
        : undefined;
    const claimWindow =
      !shouldRunWorkflow && this.spec.claimPolicy?.mode === 'claim'
        ? this.openClaimWindow(
            effectiveRequest,
            claimCandidateRoleIds ?? this.spec.roles.map(role => role.id),
          )
        : undefined;

    const claimResponses = claimWindow
      ? await this.collectClaimResponses(claimWindow, effectiveRequest, options.timeoutMs)
      : undefined;

    const plan = claimResponses
      ? buildPlanFromClaimResponses(this.spec, effectiveRequest, claimResponses)
      : shouldRunWorkflow
        ? buildWorkflowEntryPlan(this.spec, effectiveRequest)
        : {
            coordinatorRoleId: coordinatorRole.id,
            responseText: coordinatorDecision.responseText,
            assignments: planWorkspaceTurnHeuristically(this.spec, effectiveRequest).assignments,
            ...(coordinatorDecision.rationale ? { rationale: coordinatorDecision.rationale } : {}),
          };

    if (claimWindow) {
      this.closeClaimWindow(
        claimWindow,
        claimResponses ?? [],
        plan.assignments.map(assignment => assignment.roleId),
      );
    }

    const dispatches = shouldRunWorkflow
      ? await this.executeWorkflowTurn(effectiveRequest, coordinatorRole.id, options)
      : await this.executePlannedAssignments(
          plan.assignments,
          request,
          coordinatorRole.id,
          claimResponses,
          options,
        );

    return {
      request,
      ...(claimWindow ? { claimWindow } : {}),
      ...(claimResponses ? { claimResponses } : {}),
      ...(workflowVoteWindow ? { workflowVoteWindow } : {}),
      ...(workflowVoteResponses ? { workflowVoteResponses } : {}),
      plan,
      dispatches,
    };
  }

  private async executePlannedAssignments(
    assignments: WorkspaceTurnResult['plan']['assignments'],
    request: WorkspaceTurnRequest,
    coordinatorRoleId: string,
    claimResponses: WorkspaceClaimResponse[] | undefined,
    options: { timeoutMs?: number; resultTimeoutMs?: number },
  ): Promise<TaskDispatch[]> {
    const dispatches: TaskDispatch[] = [];
    for (const assignment of assignments) {
      const dispatch = await this.assignRoleTask({
        roleId: assignment.roleId,
        instruction: assignment.instruction,
        ...(assignment.summary ? { summary: assignment.summary } : {}),
        ...(assignment.provider ? { provider: assignment.provider } : {}),
        ...(assignment.model ? { model: assignment.model } : {}),
        visibility: assignment.visibility ?? request.visibility ?? 'public',
        sourceRoleId: coordinatorRoleId,
        ...(assignment.workflowNodeId ? { workflowNodeId: assignment.workflowNodeId } : {}),
        ...(assignment.stageId ? { stageId: assignment.stageId } : {}),
      });
      const claimResponse = claimResponses?.find(response => response.roleId === assignment.roleId);
      this.claimDispatch(
        dispatch.dispatchId,
        assignment.roleId,
        claimResponse?.publicResponse ?? claimResponse?.rationale ?? 'Claimed by runtime routing',
        claimResponse?.decision === 'support' ? 'supporting' : 'claimed',
      );
      dispatches.push(await this.runDispatch(Promise.resolve(dispatch), options));
    }
    return dispatches;
  }

  private async executeWorkflowTurn(
    request: WorkspaceTurnRequest,
    coordinatorRoleId: string,
    options: { timeoutMs?: number; resultTimeoutMs?: number },
  ): Promise<TaskDispatch[]> {
    const result = await executeWorkflow(
      this.spec,
      request,
      async (assignment, node) => {
        const dispatch = await this.assignRoleTask({
          roleId: assignment.roleId,
          instruction: assignment.instruction,
          ...(assignment.summary ? { summary: assignment.summary } : {}),
          ...(assignment.provider ? { provider: assignment.provider } : {}),
          ...(assignment.model ? { model: assignment.model } : {}),
          visibility: assignment.visibility ?? request.visibility ?? 'public',
          sourceRoleId: coordinatorRoleId,
          workflowNodeId: node.id,
          ...(assignment.stageId ? { stageId: assignment.stageId } : {}),
          ...(assignment.workItemId ? { workItemId: assignment.workItemId } : {}),
        });
        this.claimDispatch(
          dispatch.dispatchId,
          assignment.roleId,
          `Claimed workflow node "${node.title ?? node.id}".`,
          'claimed',
        );
        return this.runDispatch(Promise.resolve(dispatch), options);
      },
      {
        onNodeStarted: node => this.enterWorkflowNode(node),
        onStageStarted: (stageId, node) => this.emitWorkflowStageStarted(stageId, node),
        onStageCompleted: (stageId, node) => this.emitWorkflowStageCompleted(stageId, node),
        onWorklistUpdated: (node, worklist) => this.updateWorklistState(node.id, worklist),
        onCompleted: (workflowResult, lastNode) => this.finishWorkflowExecution(workflowResult.completionStatus, lastNode),
      },
    );

    return result.dispatches;
  }

  async deleteWorkspace(): Promise<void> {
    this.deactivateProcessExitCleanup();
    await this.persistenceFlushed;
    await Promise.all(
      [...this.childWorkspaces.values()].map(workspace => workspace.deleteWorkspace()),
    );
    if (this.persistence) {
      await this.persistence.deleteWorkspace();
    }
  }

  async close(): Promise<void> {
    if (!this.active) {
      this.deactivateProcessExitCleanup();
      return;
    }

    await Promise.all(
      [...this.childWorkspaces.values()].map(workspace => workspace.close()),
    );
    this.active = false;
    this.initialized = false;
    this.state.status = 'closed';
    this.emitStateChanged('closed');
    await this.persistenceFlushed;
    this.deactivateProcessExitCleanup();
  }

  protected override emitEvent(event: WorkspaceEvent): void {
    super.emitEvent(event);
    this.schedulePersistence([event]);
  }

  protected override async handleProcessExitCleanup(_reason: string): Promise<void> {
    await this.close();
  }

  async resumePendingWork(options: ResumePendingWorkOptions = {}): Promise<TaskDispatch[]> {
    if (!this.active) {
      await this.start();
    }

    const requestMessage = options.message ?? this.findLatestUserMessage();
    if (!requestMessage) {
      throw new Error('Cannot resume workflow without a request message. Provide options.message.');
    }

    this.resetProviderResumeState();

    const resumeNode = await this.resolveResumeNode({
      retryFailedItems: options.retryFailedItems ?? true,
      retryExhaustedItems: options.retryExhaustedItems ?? false,
      retryBlockedItems: options.retryBlockedItems ?? false,
      retrySpecificItemIds: options.retrySpecificItemIds ?? [],
      resetRunningItems: options.resetRunningItems ?? true,
      resetAttemptsForRetriedItems: options.resetAttemptsForRetriedItems ?? false,
    });
    if (!resumeNode) {
      return [];
    }

    const request: WorkspaceTurnRequest = {
      message: requestMessage,
      visibility: 'public',
    };
    const coordinatorRoleId = this.resolveCoordinatorRole().id;
    const result = await executeWorkflow(
      this.spec,
      request,
      async (assignment, node) => {
        const dispatch = await this.assignRoleTask({
          roleId: assignment.roleId,
          instruction: assignment.instruction,
          ...(assignment.summary ? { summary: assignment.summary } : {}),
          ...(assignment.provider ? { provider: assignment.provider } : {}),
          ...(assignment.model ? { model: assignment.model } : {}),
          visibility: assignment.visibility ?? request.visibility ?? 'public',
          sourceRoleId: coordinatorRoleId,
          workflowNodeId: node.id,
          ...(assignment.stageId ? { stageId: assignment.stageId } : {}),
          ...(assignment.workItemId ? { workItemId: assignment.workItemId } : {}),
        });
        this.claimDispatch(
          dispatch.dispatchId,
          assignment.roleId,
          `Resumed workflow node "${node.title ?? node.id}".`,
          'claimed',
        );
        return this.runDispatch(Promise.resolve(dispatch), {
          ...(options.timeoutMs !== undefined ? { timeoutMs: options.timeoutMs } : {}),
          ...(options.resultTimeoutMs !== undefined
            ? { resultTimeoutMs: options.resultTimeoutMs }
            : {}),
        });
      },
      {
        onNodeStarted: node => this.enterWorkflowNode(node),
        onStageStarted: (stageId, node) => this.emitWorkflowStageStarted(stageId, node),
        onStageCompleted: (stageId, node) => this.emitWorkflowStageCompleted(stageId, node),
        onWorklistUpdated: (node, worklist) => this.updateWorklistState(node.id, worklist),
        onCompleted: (workflowResult, lastNode) =>
          this.finishWorkflowExecution(workflowResult.completionStatus, lastNode),
      },
      {
        startNodeId: resumeNode.id,
      },
    );

    return result.dispatches;
  }

  private assertHybridSpec(): void {
    if (this.spec.provider !== 'hybrid') {
      throw new Error('HybridWorkspace requires spec.provider = "hybrid".');
    }

    if (this.spec.roles.length === 0) {
      throw new Error('HybridWorkspace requires at least one role.');
    }

    for (const role of this.spec.roles) {
      resolveRoleProvider(this.spec, role);
    }
  }

  private groupRolesByProvider(): Map<MultiAgentProvider, RoleSpec[]> {
    const map = new Map<MultiAgentProvider, RoleSpec[]>();
    for (const role of this.spec.roles) {
      const provider = resolveRoleProvider(this.spec, role);
      const bucket = map.get(provider) ?? [];
      bucket.push(role);
      map.set(provider, bucket);
    }
    return map;
  }

  private buildChildSpec(provider: MultiAgentProvider, roles: RoleSpec[]): WorkspaceSpec {
    const defaultModel = this.resolveDefaultModel(provider, roles);
    const roleIds = new Set(roles.map(role => role.id));
    const defaultRoleId =
      this.spec.defaultRoleId && roleIds.has(this.spec.defaultRoleId)
        ? this.spec.defaultRoleId
        : roles[0]?.id;
    const coordinatorRoleId =
      this.spec.coordinatorRoleId && roleIds.has(this.spec.coordinatorRoleId)
        ? this.spec.coordinatorRoleId
        : defaultRoleId;

    return {
      ...this.spec,
      id: `${this.spec.id}--${provider === 'claude-agent-sdk' ? 'claude' : 'codex'}`,
      provider,
      defaultProvider: provider,
      defaultModel,
      model: defaultModel,
      roles,
      ...(defaultRoleId ? { defaultRoleId } : {}),
      ...(coordinatorRoleId ? { coordinatorRoleId } : {}),
      ...(this.spec.claimPolicy
        ? {
            claimPolicy: {
              ...this.spec.claimPolicy,
              ...(this.spec.claimPolicy.fallbackRoleId &&
              roleIds.has(this.spec.claimPolicy.fallbackRoleId)
                ? { fallbackRoleId: this.spec.claimPolicy.fallbackRoleId }
                : defaultRoleId
                  ? { fallbackRoleId: defaultRoleId }
                  : {}),
            },
          }
        : {}),
    };
  }

  private resolveDefaultModel(provider: MultiAgentProvider, roles: RoleSpec[]): string {
    const explicitRoleModel = roles.find(role => role.agent.model)?.agent.model;
    const defaultModel =
      this.defaultModels[provider] ??
      (this.spec.defaultProvider === provider ? this.spec.defaultModel ?? this.spec.model : undefined) ??
      explicitRoleModel ??
      resolveWorkspaceDefaultModel(this.spec, provider);
    if (!defaultModel) {
      throw new Error(
        `No model configured for ${provider}. Set role.agent.model or HybridWorkspace defaultModels.`,
      );
    }
    return defaultModel;
  }
  private resolveCoordinatorRole(): RoleSpec {
    const coordinatorRoleId =
      this.spec.coordinatorRoleId ?? this.spec.defaultRoleId ?? this.spec.roles[0]?.id;
    if (!coordinatorRoleId) {
      throw new Error('Workspace has no coordinator role.');
    }
    const coordinatorRole = this.state.roles[coordinatorRoleId];
    if (!coordinatorRole) {
      throw new Error(`Unknown coordinator role: ${coordinatorRoleId}`);
    }
    return coordinatorRole;
  }

  private async requestCoordinatorDecision(
    request: WorkspaceTurnRequest,
    timeoutMs = 120_000,
  ): Promise<CoordinatorWorkflowDecision> {
    const coordinatorRole = this.resolveCoordinatorRole();
    const provider = resolveRoleProvider(this.spec, coordinatorRole);
    const child = await this.ensureChildWorkspaceStarted(provider);
    return child.requestCoordinatorDecision(request, timeoutMs);
  }

  private async probeRoleClaim(
    role: RoleSpec,
    request: WorkspaceTurnRequest,
    timeoutMs: number,
  ): Promise<WorkspaceClaimResponse> {
    const provider = resolveRoleProvider(this.spec, role);
    const child = await this.ensureChildWorkspaceStarted(provider);
    return child.probeRoleClaim(role, request, timeoutMs);
  }

  private async probeWorkflowVote(
    role: RoleSpec,
    request: WorkspaceTurnRequest,
    coordinatorDecision: CoordinatorWorkflowDecision,
    timeoutMs: number,
  ): Promise<WorkspaceWorkflowVoteResponse> {
    const provider = resolveRoleProvider(this.spec, role);
    const child = await this.ensureChildWorkspaceStarted(provider);
    return child.probeWorkflowVote(role, request, coordinatorDecision, timeoutMs);
  }

  private async ensureChildWorkspaceStarted(
    provider: MultiAgentProvider,
  ): Promise<ChildWorkspace> {
    const child = this.getChildWorkspace(provider);
    await child.start();
    return child;
  }

  private async prepareChildWorkspaceForDispatch(
    provider: MultiAgentProvider,
    request: RoleTaskRequest,
  ): Promise<ChildWorkspace> {
    if (!this.isStatelessWorkflowDispatch(request)) {
      return this.ensureChildWorkspaceStarted(provider);
    }

    const child = this.getChildWorkspace(provider);
    this.resetChildPersistentState(child);
    await child.close();
    await child.start();
    return child;
  }

  private isStatelessWorkflowDispatch(request: RoleTaskRequest): boolean {
    return Boolean(request.workflowNodeId || request.stageId || request.workItemId);
  }

  private resetChildPersistentState(child: ChildWorkspace): void {
    if (child instanceof ClaudeAgentWorkspace) {
      child.resetPersistentSession();
      return;
    }
    if (child instanceof CodexSdkWorkspace) {
      child.resetPersistentThreads();
    }
  }

  private getChildWorkspace(provider: MultiAgentProvider): ChildWorkspace {
    const workspace = this.childWorkspaces.get(provider);
    if (!workspace) {
      throw new Error(`No child workspace registered for provider ${provider}.`);
    }
    return workspace;
  }

  private handleChildEvent(provider: MultiAgentProvider, event: WorkspaceEvent): void {
    if (event.type === 'workspace.started') {
      return;
    }

    if (event.type === 'workspace.initialized') {
      if (event.sessionId) {
        this.childSessionIds.set(provider, event.sessionId);
      }
      return;
    }

    if (event.type === 'workspace.state.changed') {
      return;
    }

    const rewritten = this.rewriteEvent(provider, event);
    this.applyEventToState(rewritten);
    this.emitEvent(rewritten);
  }

  private rewriteEvent(provider: MultiAgentProvider, event: WorkspaceEvent): WorkspaceEvent {
    switch (event.type) {
      case 'member.registered':
      case 'member.state.changed':
        return {
          ...event,
          workspaceId: this.spec.id,
          member: {
            ...event.member,
            workspaceId: this.spec.id,
            provider,
          },
        };
      case 'dispatch.queued':
      case 'dispatch.started':
      case 'dispatch.progress':
      case 'dispatch.completed':
      case 'dispatch.failed':
      case 'dispatch.stopped':
      case 'dispatch.result':
      case 'dispatch.claimed':
        return {
          ...event,
          workspaceId: this.spec.id,
          dispatch: {
            ...event.dispatch,
            workspaceId: this.spec.id,
            provider,
          },
          ...('member' in event && event.member
            ? {
                member: {
                  ...event.member,
                  workspaceId: this.spec.id,
                  provider,
                },
              }
            : {}),
        } as WorkspaceEvent;
      case 'message':
        return {
          ...event,
          workspaceId: this.spec.id,
        };
      case 'activity.published':
        return {
          ...event,
          workspaceId: this.spec.id,
          activity: {
            ...event.activity,
            workspaceId: this.spec.id,
          },
        };
      case 'claim.window.opened':
      case 'claim.window.closed':
      case 'claim.response':
      case 'workflow.vote.opened':
      case 'workflow.vote.closed':
      case 'workflow.vote.response':
      case 'workflow.started':
      case 'workflow.stage.started':
      case 'workflow.stage.completed':
      case 'tool.progress':
      case 'result':
      case 'error':
        return {
          ...event,
          workspaceId: this.spec.id,
        };
      default:
        return event;
    }
  }

  private applyEventToState(event: WorkspaceEvent): void {
    switch (event.type) {
      case 'member.registered':
      case 'member.state.changed':
        this.state.members[event.member.roleId] = { ...event.member };
        return;
      case 'dispatch.queued':
      case 'dispatch.started':
      case 'dispatch.progress':
      case 'dispatch.completed':
      case 'dispatch.failed':
      case 'dispatch.stopped':
      case 'dispatch.result':
      case 'dispatch.claimed':
        this.state.dispatches[event.dispatch.dispatchId] = { ...event.dispatch };
        return;
      case 'activity.published':
        this.state.activities = [...this.state.activities, event.activity];
        return;
      default:
        return;
    }
  }

  private buildInitializedEvent(): WorkspaceInitializedEvent {
    return {
      type: 'workspace.initialized',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      availableAgents: this.spec.roles.map(role => role.id),
      availableTools: this.spec.allowedTools ?? [],
      availableCommands: ['runWorkspaceTurn', 'runRoleTask', 'assignRoleTask'],
    };
  }

  private emitStateChanged(state: WorkspaceState['status']): void {
    this.emitEvent({
      type: 'workspace.state.changed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      state,
    });
  }

  private emitLocalEvent(event: WorkspaceEvent): void {
    this.applyEventToState(event);
    this.emitEvent(event);
  }

  private publishActivity(
    kind: WorkspaceActivityKind,
    text: string,
    details: {
      roleId?: string;
      dispatchId?: string;
      taskId?: string;
      visibility?: WorkspaceVisibility;
    } = {},
  ): void {
    const activity: WorkspaceActivity = {
      activityId: randomUUID(),
      workspaceId: this.spec.id,
      kind,
      visibility: details.visibility ?? this.spec.activityPolicy?.defaultVisibility ?? 'public',
      text,
      createdAt: new Date().toISOString(),
      ...(details.roleId ? { roleId: details.roleId, memberId: details.roleId } : {}),
      ...(details.dispatchId ? { dispatchId: details.dispatchId } : {}),
      ...(details.taskId ? { taskId: details.taskId } : {}),
    };
    this.emitLocalEvent({
      type: 'activity.published',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      activity,
    });
  }

  private emitCoordinatorSummary(text: string, roleId: string): void {
    this.publishActivity('coordinator_message', text, {
      roleId,
      visibility: 'public',
    });
  }

  private openClaimWindow(
    request: WorkspaceTurnRequest,
    candidateRoleIds: string[],
  ): WorkspaceClaimWindow {
    const claimWindow: WorkspaceClaimWindow = {
      windowId: randomUUID(),
      request,
      candidateRoleIds,
      ...(this.spec.claimPolicy?.claimTimeoutMs
        ? { timeoutMs: this.spec.claimPolicy.claimTimeoutMs }
        : {}),
    };
    this.emitLocalEvent({
      type: 'claim.window.opened',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      claimWindow,
    });
    this.publishActivity('claim_window_opened', `Claim window opened for: ${request.message}`, {
      visibility: 'public',
    });
    return claimWindow;
  }

  private closeClaimWindow(
    claimWindow: WorkspaceClaimWindow,
    responses: WorkspaceClaimResponse[],
    selectedRoleIds: string[],
  ): void {
    this.emitLocalEvent({
      type: 'claim.window.closed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      claimWindow,
      responses,
      selectedRoleIds,
    });
    this.publishActivity(
      'claim_window_closed',
      selectedRoleIds.length > 0
        ? `Claim window resolved: ${selectedRoleIds.map(roleId => `@${roleId}`).join(', ')}`
        : 'Claim window closed with no claimants.',
      { visibility: 'public' },
    );
  }

  private emitClaimResponse(
    claimWindow: WorkspaceClaimWindow,
    response: WorkspaceClaimResponse,
  ): void {
    this.emitLocalEvent({
      type: 'claim.response',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      claimWindowId: claimWindow.windowId,
      response,
    });
    this.publishActivity(
      response.decision === 'claim'
        ? 'member_claimed'
        : response.decision === 'support'
          ? 'member_supporting'
          : 'member_declined',
      response.publicResponse ?? response.rationale,
      {
        roleId: response.roleId,
        visibility: 'public',
      },
    );
  }

  private async collectClaimResponses(
    claimWindow: WorkspaceClaimWindow,
    request: WorkspaceTurnRequest,
    timeoutMs = 120_000,
  ): Promise<WorkspaceClaimResponse[]> {
    const claimProbeTimeout = Math.max(
      5_000,
      Math.min(timeoutMs, this.spec.claimPolicy?.claimTimeoutMs ?? 30_000),
    );

    return Promise.all(
      claimWindow.candidateRoleIds.map(async roleId => {
        const role = this.spec.roles.find(value => value.id === roleId);
        if (!role) {
          const response: WorkspaceClaimResponse = {
            roleId,
            decision: 'decline',
            confidence: 0,
            rationale: `@${roleId} is not available for this claim window.`,
            publicResponse: `@${roleId} passed on this request.`,
          };
          this.emitClaimResponse(claimWindow, response);
          return response;
        }
        try {
          const response = await this.probeRoleClaim(role, request, claimProbeTimeout);
          this.emitClaimResponse(claimWindow, response);
          return response;
        } catch {
          const response: WorkspaceClaimResponse = {
            roleId: role.id,
            decision: 'decline',
            confidence: 0,
            rationale: `@${role.id} did not return a valid claim response in time.`,
            publicResponse: `@${role.id} passed on this request.`,
          };
          this.emitClaimResponse(claimWindow, response);
          return response;
        }
      }),
    );
  }

  private openWorkflowVoteWindow(
    request: WorkspaceTurnRequest,
    coordinatorDecision: CoordinatorWorkflowDecision,
    candidateRoleIds: string[],
  ): WorkspaceWorkflowVoteWindow {
    this.state.workflowRuntime = {
      ...this.state.workflowRuntime,
      mode: 'workflow_vote',
    };
    const voteWindow: WorkspaceWorkflowVoteWindow = {
      voteId: randomUUID(),
      request,
      reason: coordinatorDecision.workflowVoteReason ?? coordinatorDecision.responseText,
      candidateRoleIds,
      ...(this.spec.workflowVotePolicy?.timeoutMs
        ? { timeoutMs: this.spec.workflowVotePolicy.timeoutMs }
        : {}),
    };
    this.state.workflowRuntime.activeVoteWindow = voteWindow;
    this.emitLocalEvent({
      type: 'workflow.vote.opened',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      coordinatorDecision,
      voteWindow,
    });
    this.publishActivity('workflow_vote_opened', voteWindow.reason, {
      roleId: this.resolveCoordinatorRole().id,
      visibility: 'public',
    });
    return voteWindow;
  }

  private emitWorkflowVoteResponse(
    voteWindow: WorkspaceWorkflowVoteWindow,
    response: WorkspaceWorkflowVoteResponse,
  ): void {
    this.emitLocalEvent({
      type: 'workflow.vote.response',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      voteId: voteWindow.voteId,
      response,
    });
    this.publishActivity(
      response.decision === 'approve'
        ? 'workflow_vote_approved'
        : response.decision === 'reject'
          ? 'workflow_vote_rejected'
          : 'member_summary',
      response.publicResponse ?? response.rationale,
      {
        roleId: response.roleId,
        visibility: 'public',
      },
    );
  }

  private async collectWorkflowVoteResponses(
    voteWindow: WorkspaceWorkflowVoteWindow,
    request: WorkspaceTurnRequest,
    coordinatorDecision: CoordinatorWorkflowDecision,
    timeoutMs = 120_000,
  ): Promise<WorkspaceWorkflowVoteResponse[]> {
    const voteTimeout = Math.max(
      5_000,
      Math.min(timeoutMs, this.spec.workflowVotePolicy?.timeoutMs ?? 30_000),
    );

    return Promise.all(
      voteWindow.candidateRoleIds.map(async roleId => {
        const role = this.spec.roles.find(value => value.id === roleId);
        if (!role) {
          const response: WorkspaceWorkflowVoteResponse = {
            roleId,
            decision: 'abstain',
            confidence: 0,
            rationale: `@${roleId} is not available for workflow voting.`,
            publicResponse: `@${roleId} abstained.`,
          };
          this.emitWorkflowVoteResponse(voteWindow, response);
          return response;
        }
        try {
          const response = await this.probeWorkflowVote(
            role,
            request,
            coordinatorDecision,
            voteTimeout,
          );
          this.emitWorkflowVoteResponse(voteWindow, response);
          return response;
        } catch {
          const response: WorkspaceWorkflowVoteResponse = {
            roleId: role.id,
            decision: 'abstain',
            confidence: 0,
            rationale: `@${role.id} did not return a workflow vote in time.`,
            publicResponse: `@${role.id} abstained.`,
          };
          this.emitWorkflowVoteResponse(voteWindow, response);
          return response;
        }
      }),
    );
  }

  private closeWorkflowVoteWindow(
    voteWindow: WorkspaceWorkflowVoteWindow,
    coordinatorDecision: CoordinatorWorkflowDecision,
    responses: WorkspaceWorkflowVoteResponse[],
    approved: boolean,
  ): void {
    this.state.workflowRuntime = {
      mode: approved ? 'workflow_running' : 'group_chat',
      ...(this.state.workflowRuntime.activeNodeId
        ? { activeNodeId: this.state.workflowRuntime.activeNodeId }
        : {}),
      ...(this.state.workflowRuntime.activeStageId
        ? { activeStageId: this.state.workflowRuntime.activeStageId }
        : {}),
    };
    this.emitLocalEvent({
      type: 'workflow.vote.closed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      coordinatorDecision,
      voteWindow,
      responses,
      approved,
    });
    this.publishActivity(
      approved ? 'workflow_vote_approved' : 'workflow_vote_rejected',
      approved ? 'Workflow vote approved.' : 'Workflow vote rejected.',
      {
        roleId: this.resolveCoordinatorRole().id,
        visibility: 'public',
      },
    );
  }

  private emitWorkflowStarted(
    coordinatorDecision: CoordinatorWorkflowDecision,
    voteWindow?: WorkspaceWorkflowVoteWindow,
  ): void {
    this.emitLocalEvent({
      type: 'workflow.started',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      coordinatorDecision,
      ...(voteWindow ? { voteWindow } : {}),
    });
    this.publishActivity('workflow_started', coordinatorDecision.responseText, {
      roleId: this.resolveCoordinatorRole().id,
      visibility: 'public',
    });
  }

  private buildDirectWorkflowDecision(
    request: WorkspaceTurnRequest,
  ): CoordinatorWorkflowDecision {
    const entryNodeId = this.spec.workflow?.entryNodeId ?? 'workflow-entry';
    return {
      kind: 'propose_workflow',
      responseText: `Direct workflow entry requested. Starting workflow at "${entryNodeId}".`,
      workflowVoteReason: 'Caller requested direct workflow execution.',
      rationale:
        `Bypassed coordinator workflow vote because runWorkspaceTurn() received workflowEntry="direct" for: ${request.message}`,
    };
  }

  private enterWorkflowNode(node: { id: string; stageId?: string; title?: string }): void {
    this.state.workflowRuntime = {
      ...this.state.workflowRuntime,
      mode: 'workflow_running',
      activeNodeId: node.id,
      ...(node.stageId ? { activeStageId: node.stageId } : {}),
    };
  }

  private emitWorkflowStageStarted(stageId: string, node: { id: string; roleId?: string; reviewerRoleId?: string }): void {
    this.emitLocalEvent({
      type: 'workflow.stage.started',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      nodeId: node.id,
      stageId,
      ...(node.roleId ? { roleId: node.roleId } : node.reviewerRoleId ? { roleId: node.reviewerRoleId } : {}),
    } satisfies WorkflowStageEvent);
    this.publishActivity('workflow_stage_started', `Workflow stage started: ${stageId}`, {
      ...(node.roleId ? { roleId: node.roleId } : node.reviewerRoleId ? { roleId: node.reviewerRoleId } : {}),
      visibility: 'public',
    });
  }

  private emitWorkflowStageCompleted(stageId: string, node: { id: string; roleId?: string; reviewerRoleId?: string }): void {
    this.emitLocalEvent({
      type: 'workflow.stage.completed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      nodeId: node.id,
      stageId,
      ...(node.roleId ? { roleId: node.roleId } : node.reviewerRoleId ? { roleId: node.reviewerRoleId } : {}),
    } satisfies WorkflowStageEvent);
    this.publishActivity('workflow_stage_completed', `Workflow stage completed: ${stageId}`, {
      ...(node.roleId ? { roleId: node.roleId } : node.reviewerRoleId ? { roleId: node.reviewerRoleId } : {}),
      visibility: 'public',
    });
  }

  private finishWorkflowExecution(
    status: 'done' | 'stuck' | 'discarded' | 'crash',
    lastNode?: { id: string; title?: string; stageId?: string },
  ): void {
    this.state.workflowRuntime = {
      mode: 'group_chat',
      ...(this.state.workflowRuntime.worklists
        ? { worklists: { ...this.state.workflowRuntime.worklists } }
        : {}),
    };
    this.publishActivity(
      'workflow_completed',
      `Workflow ${status} at ${lastNode?.title ?? lastNode?.id ?? 'unknown node'}.`,
      {
        visibility: 'public',
      },
    );
  }

  private updateWorklistState(nodeId: string, worklist: WorkflowWorklistRuntimeState): void {
    this.state.workflowRuntime = {
      ...this.state.workflowRuntime,
      worklists: {
        ...(this.state.workflowRuntime.worklists ?? {}),
        [nodeId]: worklist,
      },
    };
  }

  private claimDispatch(
    dispatchId: string,
    roleId: string,
    note?: string,
    claimStatus: ClaimStatus = 'claimed',
  ): void {
    const dispatch = this.state.dispatches[dispatchId];
    const member = this.state.members[roleId];
    if (!dispatch || !member) {
      return;
    }

    dispatch.claimStatus = claimStatus;
    dispatch.claimedByMemberIds = Array.from(
      new Set([...(dispatch.claimedByMemberIds ?? []), roleId]),
    );

    this.emitLocalEvent({
      type: 'dispatch.claimed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: { ...dispatch },
      member: { ...member },
      claimStatus,
      ...(note ? { note } : {}),
    });
    this.publishActivity(
      claimStatus === 'supporting'
        ? 'member_supporting'
        : claimStatus === 'declined'
          ? 'member_declined'
          : 'member_claimed',
      note ?? `${member.roleName} claimed the task.`,
      {
        roleId,
        dispatchId,
        visibility: dispatch.visibility ?? this.spec.activityPolicy?.defaultVisibility ?? 'public',
      },
    );
  }

  private toTopLevelDispatch(dispatch: TaskDispatch, provider: MultiAgentProvider): TaskDispatch {
    return {
      ...dispatch,
      workspaceId: this.spec.id,
      provider,
    };
  }

  private ensureStarted(): void {
    if (!this.active || !this.initialized) {
      throw new Error('Workspace has not been started.');
    }
  }

  private findLatestUserMessage(): string | undefined {
    const userActivities = [...this.state.activities]
      .filter(activity => activity.kind === 'user_message')
      .sort((left, right) => right.createdAt.localeCompare(left.createdAt));
    return userActivities[0]?.text ?? this.inferUserMessageFromDispatches();
  }

  private inferUserMessageFromDispatches(): string | undefined {
    const dispatches = Object.values(this.state.dispatches).sort((left, right) =>
      right.createdAt.localeCompare(left.createdAt),
    );
    for (const dispatch of dispatches) {
      const match = dispatch.instruction.match(
        /Original user request:\s*([\s\S]+?)(?:\n(?:Implement the task|Task description:|Acceptance criteria:|Latest coder handoff:|Context from the approved implementation\/evaluation:)|$)/,
      );
      const candidate = match?.[1]?.trim();
      if (candidate) {
        return candidate;
      }
    }
    return undefined;
  }

  private async resolveResumeNode(options: {
    retryFailedItems: boolean;
    retryExhaustedItems: boolean;
    retryBlockedItems: boolean;
    retrySpecificItemIds: string[];
    resetRunningItems: boolean;
    resetAttemptsForRetriedItems: boolean;
  }): Promise<WorkflowNodeSpec | undefined> {
    const workflow = this.spec.workflow;
    if (!workflow) {
      return undefined;
    }

    const nodeById = new Map(workflow.nodes.map(node => [node.id, node]));
    const activeNodeId = this.state.workflowRuntime.activeNodeId;
    if (activeNodeId) {
      const activeNode = nodeById.get(activeNodeId);
      if (activeNode) {
        if (activeNode.type === 'worklist') {
          await this.prepareWorklistForResume(activeNode, options);
        }
        return activeNode;
      }
    }

    for (const node of workflow.nodes) {
      if (node.type !== 'worklist') {
        continue;
      }
      const resumable = await this.prepareWorklistForResume(node, options);
      if (resumable) {
        return node;
      }
    }

    return getWorkflowEntryNode(this.spec) ?? undefined;
  }

  private async prepareWorklistForResume(
    node: WorkflowNodeSpec,
    options: {
      retryFailedItems: boolean;
      retryExhaustedItems: boolean;
      retryBlockedItems: boolean;
      retrySpecificItemIds: string[];
      resetRunningItems: boolean;
      resetAttemptsForRetriedItems: boolean;
    },
  ): Promise<boolean> {
    const artifactId = node.worklistArtifactId ?? node.producesArtifacts?.[0];
    if (!artifactId || !this.spec.cwd) {
      return false;
    }

    const artifact = this.spec.artifacts?.find(value => value.id === artifactId);
    if (!artifact) {
      return false;
    }

    const artifactPath = path.resolve(this.spec.cwd, artifact.path);
    let raw = '';
    try {
      raw = await readFile(artifactPath, 'utf8');
    } catch {
      return false;
    }

    let document: WorkflowTaskListArtifact;
    try {
      document = parseTaskListArtifact(raw);
    } catch {
      return false;
    }

    let changed = false;
    let hasResumableItems = false;
    const forcedRetryIds = new Set(options.retrySpecificItemIds);
    for (const item of document.items) {
      const maxAttempts = item.maxAttempts;
      const attempts = item.attempts ?? 0;
      const resetAttempts = () => {
        if (options.resetAttemptsForRetriedItems) {
          item.attempts = 0;
        }
      };
      if (item.status === 'completed') {
        continue;
      }
      if (item.status === 'pending') {
        hasResumableItems = true;
        continue;
      }
      if (item.status === 'running' && options.resetRunningItems) {
        item.status = 'pending';
        resetAttempts();
        changed = true;
        hasResumableItems = true;
        continue;
      }
      if (item.status === 'blocked' && (options.retryBlockedItems || forcedRetryIds.has(item.id))) {
        item.status = 'pending';
        resetAttempts();
        changed = true;
        hasResumableItems = true;
        continue;
      }
      if (
        item.status === 'failed' &&
        (forcedRetryIds.has(item.id) ||
          (options.retryFailedItems &&
            (options.retryExhaustedItems || maxAttempts === undefined || attempts < maxAttempts)))
      ) {
        item.status = 'pending';
        resetAttempts();
        changed = true;
        hasResumableItems = true;
        continue;
      }
    }

    if (changed) {
      await writeFile(artifactPath, `${JSON.stringify(document, null, 2)}\n`, 'utf8');
    }

    return hasResumableItems;
  }

  private applyRestoredState(
    childSnapshots: Array<{ provider: MultiAgentProvider; snapshot: WorkspaceState }>,
  ): void {
    const mergedDispatches = Object.fromEntries(
      childSnapshots.flatMap(({ provider, snapshot }) =>
        Object.values(snapshot.dispatches).map(dispatch => [
          dispatch.dispatchId,
          {
            ...dispatch,
            workspaceId: this.spec.id,
            provider,
          },
        ]),
      ),
    );

    const mergedMembers = Object.fromEntries(
      childSnapshots.flatMap(({ provider, snapshot }) =>
        Object.values(snapshot.members).map(member => [
          member.roleId,
          {
            ...member,
            workspaceId: this.spec.id,
            provider,
          },
        ]),
      ),
    );

    const mergedActivities = childSnapshots
      .flatMap(({ snapshot }) =>
        snapshot.activities.map(activity => ({
          ...activity,
          workspaceId: this.spec.id,
        })),
      )
      .sort((left, right) => left.createdAt.localeCompare(right.createdAt));

    for (const { provider, snapshot } of childSnapshots) {
      if (snapshot.sessionId) {
        this.childSessionIds.set(provider, snapshot.sessionId);
      }
    }

    this.state.status = childSnapshots.some(({ snapshot }) => snapshot.status === 'running')
      ? 'running'
      : childSnapshots.some(({ snapshot }) => snapshot.status === 'closed')
        ? 'closed'
        : childSnapshots.some(({ snapshot }) => snapshot.status === 'idle')
          ? 'idle'
          : this.state.status;
    this.state.roles = Object.fromEntries(this.spec.roles.map(role => [role.id, role]));
    this.state.dispatches = mergedDispatches;
    this.state.members = {
      ...this.state.members,
      ...mergedMembers,
    };
    this.state.activities = mergedActivities;

    const startedAt = childSnapshots
      .map(({ snapshot }) => snapshot.startedAt)
      .filter((value): value is string => Boolean(value))
      .sort()[0];
    if (startedAt) {
      this.state.startedAt = startedAt;
    }

    const workflowRuntime = childSnapshots.reduce<WorkspaceState['workflowRuntime']>(
      (accumulator, { snapshot }) => {
        const current = snapshot.workflowRuntime;
        return {
          ...accumulator,
          ...(current.activeNodeId ? { activeNodeId: current.activeNodeId } : {}),
          ...(current.activeStageId ? { activeStageId: current.activeStageId } : {}),
          ...(current.activeVoteWindow ? { activeVoteWindow: current.activeVoteWindow } : {}),
          ...(current.worklists ? { worklists: { ...(accumulator.worklists ?? {}), ...current.worklists } } : {}),
          mode:
            current.mode !== 'group_chat'
              ? current.mode
              : accumulator.mode,
        };
      },
      {
        mode: Object.values(mergedDispatches).some(
          dispatch =>
            dispatch.status === 'queued' || dispatch.status === 'started' || dispatch.status === 'running',
        )
          ? 'workflow_running'
          : 'group_chat',
      },
    );
    this.state.workflowRuntime = workflowRuntime;
  }

  private applyPersistedState(
    snapshot: WorkspaceState,
    _providerState: PersistedProviderState,
  ): void {
    this.state.status = snapshot.status;
    if (snapshot.sessionId) {
      this.state.sessionId = snapshot.sessionId;
    } else {
      delete this.state.sessionId;
    }
    if (snapshot.startedAt) {
      this.state.startedAt = snapshot.startedAt;
    } else {
      delete this.state.startedAt;
    }
    this.state.roles = { ...snapshot.roles };
    this.state.dispatches = { ...snapshot.dispatches };
    this.state.members = { ...snapshot.members };
    this.state.activities = [...snapshot.activities];
    this.state.workflowRuntime = { ...snapshot.workflowRuntime };
  }

  private resetProviderResumeState(): void {
    this.childSessionIds.clear();
    delete this.state.sessionId;
    const codexWorkspace = this.childWorkspaces.get('codex-sdk');
    if (codexWorkspace instanceof CodexSdkWorkspace) {
      codexWorkspace.resetPersistentThreads();
    }
  }

  private buildProviderState(): PersistedProviderState {
    return {
      workspaceId: this.spec.id,
      provider: 'hybrid',
      memberBindings: {},
      metadata: {
        childWorkspaceIds: Object.fromEntries(this.childWorkspaceIds.entries()),
      },
      updatedAt: new Date().toISOString(),
    };
  }

  private async ensurePersistenceInitialized(): Promise<void> {
    if (!this.persistence) {
      return;
    }

    if (this.restoredFromPersistence) {
      return;
    }

    await this.persistence.ensureWorkspaceInitialized(this.spec);
  }

  private schedulePersistence(events: WorkspaceEvent[]): void {
    if (!this.persistence) {
      return;
    }

    this.persistenceFlushed = this.persistenceFlushed
      .then(async () =>
        this.persistence?.persistRuntime({
          state: this.getSnapshot(),
          events,
          providerState: this.buildProviderState(),
        }),
      )
      .catch(() => undefined);
  }

  private static reconstructHybridSpec(
    workspaceId: string,
    childSpecs: Array<{ provider: MultiAgentProvider; spec: WorkspaceSpec }>,
  ): WorkspaceSpec {
    const orderedChildSpecs = ['claude-agent-sdk', 'codex-sdk']
      .map(provider => childSpecs.find(entry => entry.provider === provider))
      .filter((entry): entry is { provider: MultiAgentProvider; spec: WorkspaceSpec } => Boolean(entry));
    const baseSpec = orderedChildSpecs[0]?.spec;
    if (!baseSpec) {
      throw new Error(`Cannot reconstruct hybrid workspace "${workspaceId}" without child specs.`);
    }

    const roles = orderedChildSpecs.flatMap(({ provider, spec }) =>
      spec.roles.map(role => ({
        ...role,
        agent: {
          ...role.agent,
          provider,
          model: resolveRoleModel(spec, role, provider),
        },
      })),
    );

    const defaultRoleId =
      orderedChildSpecs
        .map(entry => entry.spec.defaultRoleId)
        .find(
          (roleId): roleId is string =>
            Boolean(roleId) && roles.some(role => role.id === roleId),
        ) ?? roles[0]?.id;
    const coordinatorRoleId =
      orderedChildSpecs
        .map(entry => entry.spec.coordinatorRoleId)
        .find(
          (roleId): roleId is string =>
            Boolean(roleId) && roles.some(role => role.id === roleId),
        ) ?? defaultRoleId;
    const defaultRole = defaultRoleId
      ? roles.find(role => role.id === defaultRoleId)
      : undefined;
    const defaultProvider = defaultRole?.agent.provider;
    const defaultModel = defaultRole?.agent.model;
    const claimPolicy = baseSpec.claimPolicy;

    return {
      ...baseSpec,
      id: workspaceId,
      provider: 'hybrid',
      roles,
      ...(defaultProvider ? { defaultProvider } : {}),
      ...(defaultModel ? { defaultModel, model: defaultModel } : {}),
      ...(defaultRoleId ? { defaultRoleId } : {}),
      ...(coordinatorRoleId ? { coordinatorRoleId } : {}),
      ...(claimPolicy
        ? {
            claimPolicy: {
              ...claimPolicy,
              ...(claimPolicy.fallbackRoleId &&
              roles.some(role => role.id === claimPolicy.fallbackRoleId)
                ? { fallbackRoleId: claimPolicy.fallbackRoleId }
                : defaultRoleId
                  ? { fallbackRoleId: defaultRoleId }
                  : {}),
            },
          }
        : {}),
    };
  }
}

function parseTaskListArtifact(content: string): WorkflowTaskListArtifact {
  const parsed = JSON.parse(content) as {
    version?: number;
    mode?: WorkflowTaskListArtifact['mode'];
    summary?: string;
    items?: WorkflowWorkItem[];
    tasks?: WorkflowWorkItem[];
  };
  const rawItems = Array.isArray(parsed.items)
    ? parsed.items
    : Array.isArray(parsed.tasks)
      ? parsed.tasks
      : [];

  return {
    version: 1,
    ...(parsed.mode ? { mode: parsed.mode } : {}),
    ...(parsed.summary ? { summary: parsed.summary } : {}),
    items: rawItems.map(item => {
      const normalizedStatus = normalizeResumeWorkItemStatus(item.status);
      return {
        ...item,
        status: normalizedStatus,
        attempts: Math.max(0, item.attempts ?? 0),
      };
    }),
  };
}

function normalizeResumeWorkItemStatus(
  status: WorkflowWorkItemDocumentStatus | undefined,
): WorkflowWorkItemDocumentStatus {
  switch (status) {
    case 'done':
    case 'complete':
      return 'completed';
    case 'running':
    case 'completed':
    case 'failed':
    case 'blocked':
    case 'discarded':
      return status;
    case 'pending':
    default:
      return 'pending';
  }
}
