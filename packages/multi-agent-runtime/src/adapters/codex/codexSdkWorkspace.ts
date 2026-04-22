import { randomUUID } from 'node:crypto';

import {
  Codex,
  type Thread,
  type ThreadEvent,
  type ThreadItem,
  type ApprovalMode as CodexApprovalMode,
  type ModelReasoningEffort,
  type SandboxMode as CodexSandboxMode,
  type WebSearchMode,
} from '@openai/codex-sdk';

import type {
  ActivityPublishedEvent,
  ClaimResponseEvent,
  ClaimWindowClosedEvent,
  ClaimWindowOpenedEvent,
  DispatchClaimedEvent,
  DispatchCompletedEvent,
  DispatchProgressEvent,
  DispatchResultEvent,
  ToolProgressEvent,
  WorkflowStartedEvent,
  WorkflowStageEvent,
  WorkflowVoteResponseEvent,
  WorkflowVoteWindowClosedEvent,
  WorkflowVoteWindowOpenedEvent,
  WorkspaceInitializedEvent,
  WorkspaceMessageEvent,
  WorkspaceStateChangedEvent,
  MemberRegisteredEvent,
  MemberStateChangedEvent,
  WorkspaceEvent,
} from '../../core/events.js';
import {
  type PersistedProviderState,
  LocalWorkspacePersistence,
} from '../../core/localPersistence.js';
import {
  resolveDispatchTarget,
  resolveRoleModel,
  resolveWorkspaceDefaultModel,
} from '../../core/providerResolution.js';
import { isTerminalDispatchStatus, WorkspaceRuntime } from '../../core/runtime.js';
import {
  applyTaskControlProtocolFromText,
  buildTaskControlPromptInjection,
} from '../../core/taskControlProtocol.js';
import type {
  ClaimStatus,
  CoordinatorWorkflowDecision,
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
  WorkflowWorklistRuntimeState,
  WorkspaceWorkflowVoteResponse,
  WorkspaceWorkflowVoteWindow,
} from '../../core/types.js';
import {
  buildWorkflowEntryPlan,
  buildPlanFromClaimResponses,
  buildCoordinatorDecisionPrompt,
  buildWorkflowVotePrompt,
  buildWorkspaceClaimPrompt,
  buildWorkspaceTurnPrompt,
  planWorkspaceTurnHeuristically,
  parseCoordinatorDecision,
  parseWorkspaceClaimResponse,
  parseWorkflowVoteResponse,
  parseWorkspaceTurnPlan,
  resolveClaimCandidateRoleIds,
  resolveWorkflowVoteCandidateRoleIds,
  shouldApproveWorkflowVote,
} from '../../core/workspaceTurn.js';
import { executeWorkflow } from '../../core/workflowExecution.js';

export interface CodexSdkWorkspaceOptions {
  spec: WorkspaceSpec;
  codexPathOverride?: string;
  baseUrl?: string;
  apiKey?: string;
  env?: Record<string, string>;
  config?: NonNullable<ConstructorParameters<typeof Codex>[0]>['config'];
  sandboxMode?: CodexSandboxMode;
  approvalPolicy?: CodexApprovalMode;
  workingDirectory?: string;
  skipGitRepoCheck?: boolean;
  modelReasoningEffort?: ModelReasoningEffort;
  networkAccessEnabled?: boolean;
  webSearchMode?: WebSearchMode;
  additionalDirectories?: string[];
  debug?: boolean;
}

export class CodexSdkWorkspace extends WorkspaceRuntime {
  private readonly spec: WorkspaceSpec;
  private readonly sandboxMode: CodexSandboxMode | undefined;
  private readonly approvalPolicy: CodexApprovalMode | undefined;
  private readonly workingDirectory: string | undefined;
  private readonly skipGitRepoCheck: boolean;
  private readonly modelReasoningEffort: ModelReasoningEffort | undefined;
  private readonly networkAccessEnabled: boolean | undefined;
  private readonly webSearchMode: WebSearchMode | undefined;
  private readonly additionalDirectories: string[] | undefined;
  private readonly debug: boolean;
  private readonly client: Codex;

  private readonly state: WorkspaceState;
  private readonly roleThreads = new Map<string, Thread>();
  private readonly roleThreadIds = new Map<string, string>();
  private readonly roleChains = new Map<string, Promise<void>>();
  private readonly activeRuns = new Map<
    string,
    {
      controller: AbortController;
      threadId?: string;
      roleId: string;
    }
  >();

  private active = false;
  private initialized = false;
  private initializedWithThread = false;
  private readonly persistence: LocalWorkspacePersistence | undefined;
  private persistenceFlushed = Promise.resolve();
  private restoredFromPersistence = false;

  constructor(options: CodexSdkWorkspaceOptions) {
    super();
    this.spec = options.spec;
    this.sandboxMode = options.sandboxMode;
    this.approvalPolicy = options.approvalPolicy;
    this.workingDirectory = options.workingDirectory ?? this.spec.cwd;
    this.skipGitRepoCheck = options.skipGitRepoCheck ?? true;
    this.modelReasoningEffort = options.modelReasoningEffort;
    this.networkAccessEnabled = options.networkAccessEnabled;
    this.webSearchMode = options.webSearchMode;
    this.additionalDirectories = options.additionalDirectories;
    this.debug = options.debug ?? false;
    this.persistence = LocalWorkspacePersistence.fromSpec(this.spec);
    this.client = new Codex({
      ...(options.codexPathOverride
        ? { codexPathOverride: options.codexPathOverride }
        : {}),
      ...(options.baseUrl ? { baseUrl: options.baseUrl } : {}),
      ...(options.apiKey ? { apiKey: options.apiKey } : {}),
      ...(options.env ? { env: options.env } : {}),
      ...(options.config ? { config: options.config } : {}),
    });

    this.state = {
      workspaceId: this.spec.id,
      status: 'idle',
      provider: 'codex-sdk',
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
  }

  static async restoreFromLocal(
    options: Omit<CodexSdkWorkspaceOptions, 'spec'> & {
      cwd: string;
      workspaceId: string;
    },
  ): Promise<CodexSdkWorkspace> {
    const persistence = LocalWorkspacePersistence.fromWorkspace(
      options.cwd,
      options.workspaceId,
    );
    const [spec, state, providerState] = await Promise.all([
      persistence.loadWorkspaceSpec(),
      persistence.loadWorkspaceState(),
      persistence.loadProviderState(),
    ]);
    const workspace = new CodexSdkWorkspace({
      ...options,
      spec,
    });
    workspace.applyPersistedState(state, providerState);
    workspace.restoredFromPersistence = true;
    return workspace;
  }

  getSnapshot(): WorkspaceState {
    return {
      ...this.state,
      roles: { ...this.state.roles },
      dispatches: { ...this.state.dispatches },
      members: { ...this.state.members },
      activities: [...this.state.activities],
    };
  }

  getPersistenceRoot(): string | undefined {
    return this.persistence?.root;
  }

  resetPersistentThreads(): void {
    this.roleThreads.clear();
    this.roleThreadIds.clear();
    delete this.state.sessionId;
  }

  async start(): Promise<void> {
    if (this.active) {
      return;
    }

    this.resetTransientRuntimeForStart();
    await this.ensurePersistenceInitialized();

    this.active = true;
    this.state.status = 'running';
    this.state.startedAt = new Date().toISOString();

    this.emitEvent({
      type: 'workspace.started',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      spec: this.spec,
    });
    for (const member of Object.values(this.state.members)) {
      const event: MemberRegisteredEvent = {
        type: 'member.registered',
        timestamp: new Date().toISOString(),
        workspaceId: this.spec.id,
        member: { ...member },
      };
      this.emitEvent(event);
    }

    this.emitInitialized({});
    this.emitStateChanged('running');
    this.activateProcessExitCleanup();
  }

  async assignRoleTask(request: RoleTaskRequest): Promise<TaskDispatch> {
    this.ensureStarted();

    const role = this.state.roles[request.roleId];
    if (!role) {
      throw new Error(`Unknown role: ${request.roleId}`);
    }

    const target = resolveDispatchTarget(this.spec, role, request);
    if (target.provider !== 'codex-sdk') {
      throw new Error(
        `CodexSdkWorkspace cannot execute provider "${target.provider}" for role "${role.id}".`,
      );
    }

    const dispatch: TaskDispatch = {
      dispatchId: randomUUID(),
      workspaceId: this.spec.id,
      roleId: role.id,
      provider: target.provider,
      model: target.model,
      instruction: request.instruction,
      status: 'queued',
      createdAt: new Date().toISOString(),
      ...(request.summary ? { summary: request.summary } : {}),
      ...(request.visibility ? { visibility: request.visibility } : {}),
      ...(request.sourceRoleId ? { sourceRoleId: request.sourceRoleId } : {}),
      ...(request.workflowNodeId ? { workflowNodeId: request.workflowNodeId } : {}),
      ...(request.stageId ? { stageId: request.stageId } : {}),
      ...(request.workItemId ? { workItemId: request.workItemId } : {}),
      ...(this.spec.claimPolicy?.mode !== 'claim'
        ? {
            claimStatus: 'claimed' satisfies ClaimStatus,
            claimedByMemberIds: [role.id],
          }
        : {
            claimStatus: 'pending' satisfies ClaimStatus,
          }),
    };

    this.state.dispatches[dispatch.dispatchId] = dispatch;

    this.emitEvent({
      type: 'dispatch.queued',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: { ...dispatch },
    });
    if (dispatch.claimStatus === 'claimed') {
      const event: DispatchClaimedEvent = {
        type: 'dispatch.claimed',
        timestamp: new Date().toISOString(),
        workspaceId: this.spec.id,
        dispatch: { ...dispatch },
        member: this.state.members[role.id]!,
        claimStatus: 'claimed',
        note: 'Assigned by policy',
      };
      this.emitEvent(event);
    }

    const previous = this.roleChains.get(role.id) ?? Promise.resolve();
    const current = previous
      .catch(() => undefined)
      .then(async () => this.executeDispatch(role, dispatch));
    this.roleChains.set(role.id, current);
    void current.finally(() => {
      if (this.roleChains.get(role.id) === current) {
        this.roleChains.delete(role.id);
      }
    });

    return { ...dispatch };
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
      ...(this.state.sessionId ? { sessionId: this.state.sessionId } : {}),
    };
    this.emitEvent(event);
    this.publishActivity('user_message', message, { visibility });
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
    const coordinatorRole = this.resolveCoordinatorRole();
    await this.send(request.message, request.visibility ?? 'public');
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
        ? this.openClaimWindow(effectiveRequest, claimCandidateRoleIds ?? this.spec.roles.map(role => role.id))
        : undefined;

    const claimResponses = claimWindow
      ? await this.collectClaimResponses(claimWindow, effectiveRequest, options.timeoutMs)
      : undefined;

    const plan = claimResponses
      ? buildPlanFromClaimResponses(this.spec, effectiveRequest, claimResponses)
      : shouldRunWorkflow
        ? buildWorkflowEntryPlan(this.spec, effectiveRequest)
        : coordinatorDecision.kind === 'delegate' && coordinatorDecision.targetRoleId
          ? {
              coordinatorRoleId: coordinatorRole.id,
              responseText: coordinatorDecision.responseText,
              assignments: planWorkspaceTurnHeuristically(this.spec, effectiveRequest).assignments,
              ...(coordinatorDecision.rationale ? { rationale: coordinatorDecision.rationale } : {}),
            }
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
    for (const run of this.activeRuns.values()) {
      run.controller.abort();
    }
    this.activeRuns.clear();
    await this.persistenceFlushed;
    if (this.persistence) {
      await this.persistence.deleteWorkspace();
    }
  }

  protected override emitEvent(event: WorkspaceEvent): void {
    super.emitEvent(event);
    this.schedulePersistence([event]);
  }

  async stopTask(taskId: string): Promise<void> {
    const activeRun = [...this.activeRuns.entries()].find(([dispatchId, run]) => {
      return run.threadId === taskId || dispatchId === taskId;
    });
    activeRun?.[1].controller.abort();
  }

  async close(): Promise<void> {
    await this.closeWithReason('Workspace closed before the active Codex task finished.');
  }

  private async executeDispatch(role: RoleSpec, dispatch: TaskDispatch): Promise<void> {
    const defaultModel = resolveRoleModel(this.spec, role);
    const model = dispatch.model ?? defaultModel;
    const usePersistentThread =
      !dispatch.workflowNodeId && !dispatch.stageId && !dispatch.workItemId && model === defaultModel;
    const thread = usePersistentThread
      ? this.ensureRoleThread(role)
      : this.createThread(role, model);
    const controller = new AbortController();
    this.activeRuns.set(dispatch.dispatchId, {
      controller,
      ...(thread.id ? { threadId: thread.id } : {}),
      roleId: role.id,
    });

    let completed = false;
    let resultText: string | undefined;
    const commandStartedAt = new Map<string, number>();

    try {
      const { events } = await thread.runStreamed(
        this.buildDispatchPrompt(role, dispatch),
        {
          signal: controller.signal,
        },
      );

      for await (const event of events) {
        if (event.type === 'thread.started') {
          if (usePersistentThread) {
            this.roleThreadIds.set(role.id, event.thread_id);
            this.roleThreads.set(role.id, thread);
            this.state.sessionId = event.thread_id;
          }
          this.activeRuns.get(dispatch.dispatchId)!.threadId = event.thread_id;

          const nextDispatch = this.mustGetDispatch(dispatch.dispatchId);
          nextDispatch.status = 'started';
          nextDispatch.providerTaskId = event.thread_id;
          nextDispatch.toolUseId = `codex-thread:${event.thread_id}`;
          nextDispatch.startedAt = new Date().toISOString();
          nextDispatch.lastSummary = dispatch.summary ?? dispatch.instruction;

          if (!this.initializedWithThread) {
            this.initializedWithThread = true;
            this.emitInitialized({ sessionId: event.thread_id });
          }

          this.emitEvent({
            type: 'dispatch.started',
            timestamp: new Date().toISOString(),
            workspaceId: this.spec.id,
            dispatch: this.cloneDispatch(nextDispatch),
            taskId: event.thread_id,
            description: dispatch.summary ?? dispatch.instruction,
          });
          this.updateMemberState(
            role.id,
            'active',
            dispatch.summary ?? dispatch.instruction,
            event.thread_id,
          );
          this.publishActivity('dispatch_started', dispatch.summary ?? dispatch.instruction, {
            roleId: role.id,
            dispatchId: dispatch.dispatchId,
            taskId: event.thread_id,
            visibility: nextDispatch.visibility ?? this.spec.activityPolicy?.defaultVisibility ?? 'public',
          });
          continue;
        }

        if (event.type === 'item.started') {
          this.handleItemStarted(dispatch.dispatchId, event.item, commandStartedAt);
          continue;
        }

        if (event.type === 'item.updated') {
          this.handleItemUpdated(dispatch.dispatchId, event.item);
          continue;
        }

        if (event.type === 'item.completed') {
          const itemResult = this.handleItemCompleted(
            dispatch.dispatchId,
            event.item,
            commandStartedAt,
          );
          if (itemResult?.resultText) {
            resultText = itemResult.resultText;
          }
          continue;
        }

        if (event.type === 'turn.completed') {
          completed = true;
          const threadId = thread.id ?? dispatch.dispatchId;
          this.finishDispatch(
            dispatch.dispatchId,
            threadId,
            buildFinishResult(
              'completed',
              resultText ? 'Codex completed the task.' : 'Codex finished the turn.',
              resultText,
            ),
          );
          continue;
        }

        if (event.type === 'turn.failed') {
          completed = true;
          const threadId = thread.id ?? dispatch.dispatchId;
          this.finishDispatch(
            dispatch.dispatchId,
            threadId,
            buildFinishResult('failed', event.error.message, resultText),
          );
          continue;
        }

        if (event.type === 'error') {
          this.emitEvent({
            type: 'error',
            timestamp: new Date().toISOString(),
            workspaceId: this.spec.id,
            error: new Error(event.message),
          });
        }
      }

      if (!completed) {
        const threadId = thread.id ?? dispatch.dispatchId;
        this.finishDispatch(
          dispatch.dispatchId,
          threadId,
          buildFinishResult(
            'stopped',
            'Codex stream ended before emitting a terminal event.',
            resultText,
          ),
        );
      }
    } catch (error) {
      const current = this.state.dispatches[dispatch.dispatchId];
      if (!completed && !(current && isTerminalDispatchStatus(current.status))) {
        const threadId = thread.id ?? dispatch.dispatchId;
        if (thread.id && usePersistentThread) {
          this.roleThreadIds.set(role.id, thread.id);
        }
        this.finishDispatch(
          dispatch.dispatchId,
          threadId,
          buildFinishResult(
            controller.signal.aborted ? 'stopped' : 'failed',
            error instanceof Error ? error.message : String(error),
            resultText,
          ),
        );
        this.emitEvent({
          type: 'error',
          timestamp: new Date().toISOString(),
          workspaceId: this.spec.id,
          error: error instanceof Error ? error : new Error(String(error)),
        });
      }
    } finally {
      this.activeRuns.delete(dispatch.dispatchId);
    }
  }

  private ensureRoleThread(role: RoleSpec): Thread {
    const existing = this.roleThreads.get(role.id);
    if (existing) {
      return existing;
    }

    const persistedThreadId = this.roleThreadIds.get(role.id);
    const thread = persistedThreadId
      ? this.client.resumeThread(persistedThreadId)
      : this.createThread(role);
    this.roleThreads.set(role.id, thread);
    if (thread.id) {
      this.roleThreadIds.set(role.id, thread.id);
    }
    return thread;
  }

  private createThread(role: RoleSpec, model = resolveRoleModel(this.spec, role)): Thread {
    return this.client.startThread({
      model,
      ...(this.sandboxMode ? { sandboxMode: this.sandboxMode } : {}),
      ...(this.workingDirectory
        ? { workingDirectory: this.workingDirectory }
        : {}),
      skipGitRepoCheck: this.skipGitRepoCheck,
      ...(this.modelReasoningEffort
        ? { modelReasoningEffort: this.modelReasoningEffort }
        : {}),
      ...(this.networkAccessEnabled !== undefined
        ? { networkAccessEnabled: this.networkAccessEnabled }
        : {}),
      ...(this.webSearchMode ? { webSearchMode: this.webSearchMode } : {}),
      ...(this.approvalPolicy ? { approvalPolicy: this.approvalPolicy } : {}),
      ...(this.additionalDirectories
        ? { additionalDirectories: this.additionalDirectories }
        : {}),
    });
  }

  private createClaimProbeThread(role: RoleSpec): Thread {
    return this.client.startThread({
      model: resolveRoleModel(this.spec, role),
      sandboxMode: 'read-only',
      ...(this.workingDirectory
        ? { workingDirectory: this.workingDirectory }
        : {}),
      skipGitRepoCheck: true,
      modelReasoningEffort: 'low',
      networkAccessEnabled: false,
      webSearchMode: 'disabled',
      approvalPolicy: 'never',
    });
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

  private handleItemStarted(
    dispatchId: string,
    item: ThreadItem,
    commandStartedAt: Map<string, number>,
  ): void {
    if (item.type === 'command_execution') {
      commandStartedAt.set(item.id, Date.now());
      this.emitDispatchProgress(dispatchId, {
        description: item.command,
        summary: 'Codex is executing a shell command.',
        lastToolName: 'Bash',
      });
      return;
    }

    if (item.type === 'web_search') {
      this.emitDispatchProgress(dispatchId, {
        description: item.query,
        summary: 'Codex started a web search.',
        lastToolName: 'WebSearch',
      });
      return;
    }

    if (item.type === 'mcp_tool_call') {
      this.emitDispatchProgress(dispatchId, {
        description: `${item.server}.${item.tool}`,
        summary: 'Codex started an MCP tool call.',
        lastToolName: item.tool,
      });
    }
  }

  private handleItemUpdated(dispatchId: string, item: ThreadItem): void {
    if (item.type === 'todo_list') {
      const incomplete = item.items.filter(todo => !todo.completed).length;
      this.emitDispatchProgress(dispatchId, {
        description: 'todo_list',
        summary: `Codex is tracking ${incomplete} remaining todo item(s).`,
        lastToolName: 'TodoList',
      });
      return;
    }

    if (item.type === 'reasoning') {
      this.emitDispatchProgress(dispatchId, {
        description: 'reasoning',
        summary: item.text,
        lastToolName: 'Reasoning',
      });
    }
  }

  private handleItemCompleted(
    dispatchId: string,
    item: ThreadItem,
    commandStartedAt: Map<string, number>,
  ): { resultText?: string } | undefined {
    if (item.type === 'command_execution') {
      const startedAt = commandStartedAt.get(item.id) ?? Date.now();
      const event: ToolProgressEvent = {
        type: 'tool.progress',
        timestamp: new Date().toISOString(),
        workspaceId: this.spec.id,
        taskId: this.mustGetDispatch(dispatchId).providerTaskId ?? dispatchId,
        toolName: 'Bash',
        elapsedTimeSeconds: Math.max(
          0,
          Math.round((Date.now() - startedAt) / 1000),
        ),
      };
      this.emitEvent(event);
      this.emitDispatchProgress(dispatchId, {
        description: item.command,
        summary:
          item.exit_code === 0
            ? 'Codex completed a shell command.'
            : `Codex command exited with code ${item.exit_code}.`,
        lastToolName: 'Bash',
      });
      return undefined;
    }

    if (item.type === 'agent_message') {
      const dispatch = this.mustGetDispatch(dispatchId);
      if (!dispatch.toolUseId) {
        dispatch.toolUseId = item.id;
      }

      const messageEvent: WorkspaceMessageEvent = {
        type: 'message',
        timestamp: new Date().toISOString(),
        workspaceId: this.spec.id,
        role: 'assistant',
        text: item.text,
        visibility: dispatch.visibility ?? 'public',
        memberId: dispatch.roleId,
        raw: item,
        ...(dispatch.providerTaskId ? { sessionId: dispatch.providerTaskId } : {}),
        ...(dispatch.toolUseId ? { parentToolUseId: dispatch.toolUseId } : {}),
      };
      this.emitEvent(messageEvent);
      this.updateMemberState(
        dispatch.roleId,
        'active',
        item.text,
        dispatch.providerTaskId,
      );
      this.publishActivity('member_summary', item.text, {
        roleId: dispatch.roleId,
        dispatchId,
        taskId: dispatch.providerTaskId ?? dispatch.dispatchId,
        visibility: dispatch.visibility ?? this.spec.activityPolicy?.defaultVisibility ?? 'public',
      });
      return { resultText: item.text };
    }

    if (item.type === 'file_change') {
      const changedPaths = item.changes.map(change => change.path).join(', ');
      this.emitDispatchProgress(dispatchId, {
        description: changedPaths || 'file changes',
        summary: 'Codex applied file changes.',
        lastToolName: 'ApplyPatch',
      });
      return undefined;
    }

    if (item.type === 'mcp_tool_call') {
      this.emitDispatchProgress(dispatchId, {
        description: `${item.server}.${item.tool}`,
        summary:
          item.status === 'failed'
            ? item.error?.message ?? 'Codex MCP call failed.'
            : 'Codex completed an MCP tool call.',
        lastToolName: item.tool,
      });
      return undefined;
    }

    if (item.type === 'web_search') {
      this.emitDispatchProgress(dispatchId, {
        description: item.query,
        summary: 'Codex completed a web search.',
        lastToolName: 'WebSearch',
      });
      return undefined;
    }

    if (item.type === 'error') {
      this.emitEvent({
        type: 'error',
        timestamp: new Date().toISOString(),
        workspaceId: this.spec.id,
        error: new Error(item.message),
      });
    }

    return undefined;
  }

  private emitDispatchProgress(
    dispatchId: string,
    progress: {
      description: string;
      summary?: string;
      lastToolName?: string;
    },
  ): void {
    const dispatch = this.mustGetDispatch(dispatchId);
    dispatch.status = 'running';
    if (progress.summary) {
      dispatch.lastSummary = progress.summary;
    }

    const event: DispatchProgressEvent = {
      type: 'dispatch.progress',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: this.cloneDispatch(dispatch),
      taskId: dispatch.providerTaskId ?? dispatch.dispatchId,
      description: progress.description,
      ...(progress.summary ? { summary: progress.summary } : {}),
      ...(progress.lastToolName ? { lastToolName: progress.lastToolName } : {}),
    };
    this.emitEvent(event);
    this.updateMemberState(
      dispatch.roleId,
      'active',
      progress.summary ?? progress.description,
      dispatch.providerTaskId,
    );
    this.publishActivity('member_progress', progress.summary ?? progress.description, {
      roleId: dispatch.roleId,
      dispatchId,
      taskId: dispatch.providerTaskId ?? dispatch.dispatchId,
      visibility: dispatch.visibility ?? this.spec.activityPolicy?.defaultVisibility ?? 'public',
    });
  }

  private finishDispatch(
    dispatchId: string,
    taskId: string,
    result: {
      status: 'completed' | 'failed' | 'stopped';
      summary: string;
      resultText?: string;
      outputFile?: string;
    },
  ): void {
    let dispatch = this.mustGetDispatch(dispatchId);
    if (isTerminalDispatchStatus(dispatch.status)) {
      return;
    }
    let cleanedResultText = result.resultText;
    if (result.resultText) {
      const parsed = applyTaskControlProtocolFromText(dispatch, result.resultText);
      dispatch = parsed.dispatch;
      this.state.dispatches[dispatchId] = dispatch;
      cleanedResultText = parsed.cleanedText || undefined;
    }
    dispatch.status = result.status;
    dispatch.completedAt = new Date().toISOString();
    dispatch.lastSummary = result.summary;
    if (result.outputFile) {
      dispatch.outputFile = result.outputFile;
    }
    if (cleanedResultText) {
      dispatch.resultText = cleanedResultText;
    } else {
      delete dispatch.resultText;
    }

    const event: DispatchCompletedEvent = {
      type:
        result.status === 'completed'
          ? 'dispatch.completed'
          : result.status === 'stopped'
            ? 'dispatch.stopped'
            : 'dispatch.failed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: this.cloneDispatch(dispatch),
      taskId,
      outputFile: result.outputFile ?? '',
      summary: result.summary,
    };
    this.emitEvent(event);
    this.updateMemberState(
      dispatch.roleId,
      result.status === 'completed'
        ? 'idle'
        : result.status === 'failed'
          ? 'blocked'
          : 'waiting',
      result.summary,
      taskId,
    );
    this.publishActivity(
      result.status === 'completed' ? 'member_delivered' : 'member_summary',
      result.summary,
      {
        roleId: dispatch.roleId,
        dispatchId,
        taskId,
        visibility: dispatch.visibility ?? this.spec.activityPolicy?.defaultVisibility ?? 'public',
      },
    );

    if (cleanedResultText) {
      const resultEvent: DispatchResultEvent = {
        type: 'dispatch.result',
        timestamp: new Date().toISOString(),
        workspaceId: this.spec.id,
        dispatch: this.cloneDispatch(dispatch),
        taskId,
        resultText: cleanedResultText,
      };
      this.emitEvent(resultEvent);
    }
  }

  protected override async handleProcessExitCleanup(reason: string): Promise<void> {
    await this.closeWithReason(`Process exit cleanup (${reason}) stopped the active Codex task.`);
  }

  protected override async handleDispatchTimeout(
    dispatchId: string,
    error: Error,
  ): Promise<void> {
    const run = this.activeRuns.get(dispatchId);
    run?.controller.abort();
    this.stopDispatch(
      dispatchId,
      `Timed out waiting for Codex dispatch termination: ${error.message}`,
    );
    await this.persistenceFlushed;
  }

  private buildDispatchPrompt(role: RoleSpec, dispatch: TaskDispatch): string {
    const parts = [
      `You are the ${role.name} role in the workspace "${this.spec.name}".`,
      role.description ? `Role description: ${role.description}` : null,
      `Follow this role-specific instruction set strictly:\n${role.agent.prompt}`,
      this.spec.orchestratorPrompt
        ? `Workspace orchestration context:\n${this.spec.orchestratorPrompt}`
        : null,
      role.outputRoot
        ? `Preferred output root for this role: ${role.outputRoot}`
        : null,
      dispatch.summary ? `Task summary: ${dispatch.summary}` : null,
      `Task instruction:\n${dispatch.instruction}`,
      buildTaskControlPromptInjection(dispatch),
      'Return a concise final answer after completing the task. If you create or edit files, mention the key output paths in the final answer.',
    ];

    return parts.filter(Boolean).join('\n\n');
  }

  private emitInitialized({ sessionId }: { sessionId?: string }): void {
    const event: WorkspaceInitializedEvent = {
      type: 'workspace.initialized',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      availableAgents: this.spec.roles.map(role => role.id),
      availableTools: this.spec.allowedTools ?? [],
      availableCommands: ['run', 'runStreamed', 'resumeThread'],
      ...(sessionId ? { sessionId } : {}),
    };
    this.initialized = true;
    this.emitEvent(event);
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
    const event: ClaimWindowOpenedEvent = {
      type: 'claim.window.opened',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      claimWindow,
    };
    this.emitEvent(event);
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
    const event: ClaimWindowClosedEvent = {
      type: 'claim.window.closed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      claimWindow,
      responses,
      selectedRoleIds,
    };
    this.emitEvent(event);
    this.publishActivity(
      'claim_window_closed',
      selectedRoleIds.length > 0
        ? `Claim window resolved: ${selectedRoleIds.map(roleId => `@${roleId}`).join(', ')}`
        : 'Claim window closed with no claimants.',
      { visibility: 'public' },
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
    const event: WorkflowVoteWindowOpenedEvent = {
      type: 'workflow.vote.opened',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      coordinatorDecision,
      voteWindow,
    };
    this.emitEvent(event);
    this.publishActivity('workflow_vote_opened', voteWindow.reason, {
      roleId: this.resolveCoordinatorRole().id,
      visibility: 'public',
    });
    return voteWindow;
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
    const event: WorkflowVoteWindowClosedEvent = {
      type: 'workflow.vote.closed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      coordinatorDecision,
      voteWindow,
      responses,
      approved,
    };
    this.emitEvent(event);
    this.publishActivity(
      approved ? 'workflow_vote_approved' : 'workflow_vote_rejected',
      approved ? 'Workflow vote approved.' : 'Workflow vote rejected.',
      {
        roleId: this.resolveCoordinatorRole().id,
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
          const response = parseWorkflowVoteResponse(
            {
              decision: 'abstain',
              confidence: 0,
              rationale: `@${role.id} did not return a workflow vote in time.`,
              publicResponse: `@${role.id} abstained.`,
            },
            role,
            this.spec,
            request,
            coordinatorDecision,
          );
          this.emitWorkflowVoteResponse(voteWindow, response);
          return response;
        }
      }),
    );
  }

  private emitWorkflowVoteResponse(
    voteWindow: WorkspaceWorkflowVoteWindow,
    response: WorkspaceWorkflowVoteResponse,
  ): void {
    const event: WorkflowVoteResponseEvent = {
      type: 'workflow.vote.response',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      voteId: voteWindow.voteId,
      response,
    };
    this.emitEvent(event);
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

  private emitClaimResponse(
    claimWindow: WorkspaceClaimWindow,
    response: WorkspaceClaimResponse,
  ): void {
    const event: ClaimResponseEvent = {
      type: 'claim.response',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      claimWindowId: claimWindow.windowId,
      response,
    };
    this.emitEvent(event);
    this.updateMemberState(
      response.roleId,
      'waiting',
      response.publicResponse ?? response.rationale,
    );
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

  async probeRoleClaim(
    role: RoleSpec,
    request: WorkspaceTurnRequest,
    timeoutMs: number,
  ): Promise<WorkspaceClaimResponse> {
    const thread = this.createClaimProbeThread(role);
    const schema = {
      type: 'object',
      properties: {
        decision: {
          type: 'string',
          enum: ['claim', 'support', 'decline'],
        },
        confidence: { type: 'number' },
        rationale: { type: 'string' },
        publicResponse: { type: 'string' },
        proposedInstruction: { type: 'string' },
      },
      required: ['decision', 'confidence', 'rationale', 'publicResponse', 'proposedInstruction'],
      additionalProperties: false,
    } as const;

    const turn = await thread.run(buildWorkspaceClaimPrompt(this.spec, role, request), {
      outputSchema: schema,
      signal: AbortSignal.timeout(timeoutMs),
    });
    return parseWorkspaceClaimResponse(turn.finalResponse, role, request);
  }

  async probeWorkflowVote(
    role: RoleSpec,
    request: WorkspaceTurnRequest,
    coordinatorDecision: CoordinatorWorkflowDecision,
    timeoutMs: number,
  ): Promise<WorkspaceWorkflowVoteResponse> {
    const thread = this.createClaimProbeThread(role);
    const schema = {
      type: 'object',
      properties: {
        decision: {
          type: 'string',
          enum: ['approve', 'reject', 'abstain'],
        },
        confidence: { type: 'number' },
        rationale: { type: 'string' },
        publicResponse: { type: 'string' },
      },
      required: ['decision', 'confidence', 'rationale', 'publicResponse'],
      additionalProperties: false,
    } as const;

    const turn = await thread.run(
      buildWorkflowVotePrompt(this.spec, role, request, coordinatorDecision),
      {
        outputSchema: schema,
        signal: AbortSignal.timeout(timeoutMs),
      },
    );
    return parseWorkflowVoteResponse(
      turn.finalResponse,
      role,
      this.spec,
      request,
      coordinatorDecision,
    );
  }

  async requestCoordinatorDecision(
    request: WorkspaceTurnRequest,
    timeoutMs = 120_000,
  ): Promise<CoordinatorWorkflowDecision> {
    const coordinatorRole = this.resolveCoordinatorRole();
    const thread = this.createClaimProbeThread(coordinatorRole);
    const schema = {
      type: 'object',
      properties: {
        kind: { type: 'string', enum: ['respond', 'delegate', 'propose_workflow'] },
        responseText: { type: 'string' },
        targetRoleId: { type: 'string' },
        workflowVoteReason: { type: 'string' },
        rationale: { type: 'string' },
      },
      required: ['kind', 'responseText', 'targetRoleId', 'workflowVoteReason', 'rationale'],
      additionalProperties: false,
    } as const;

    const turn = await thread.run(buildCoordinatorDecisionPrompt(this.spec, request), {
      outputSchema: schema,
      signal: AbortSignal.timeout(timeoutMs),
    });
    return parseCoordinatorDecision(turn.finalResponse, this.spec, request);
  }

  private emitCoordinatorSummary(text: string, roleId: string): void {
    const event: WorkspaceMessageEvent = {
      type: 'message',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      role: 'assistant',
      text,
      visibility: 'public',
      memberId: roleId,
      raw: {
        type: 'workspace_turn_summary',
        roleId,
      },
      ...(this.state.sessionId ? { sessionId: this.state.sessionId } : {}),
    };
    this.emitEvent(event);
    this.publishActivity('coordinator_message', text, {
      roleId,
      visibility: 'public',
    });
  }

  private emitWorkflowStarted(
    coordinatorDecision: CoordinatorWorkflowDecision,
    voteWindow?: WorkspaceWorkflowVoteWindow,
  ): void {
    const event: WorkflowStartedEvent = {
      type: 'workflow.started',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      coordinatorDecision,
      ...(voteWindow ? { voteWindow } : {}),
    };
    this.emitEvent(event);
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

  private emitWorkflowStageStarted(stageId: string, node: { id: string; title?: string; roleId?: string; reviewerRoleId?: string }): void {
    const event: WorkflowStageEvent = {
      type: 'workflow.stage.started',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      nodeId: node.id,
      stageId,
      ...(node.roleId ? { roleId: node.roleId } : node.reviewerRoleId ? { roleId: node.reviewerRoleId } : {}),
    };
    this.emitEvent(event);
    this.publishActivity('workflow_stage_started', `Workflow stage started: ${stageId}`, {
      ...(node.roleId ? { roleId: node.roleId } : node.reviewerRoleId ? { roleId: node.reviewerRoleId } : {}),
      visibility: 'public',
    });
  }

  private emitWorkflowStageCompleted(stageId: string, node: { id: string; title?: string; roleId?: string; reviewerRoleId?: string }): void {
    const event: WorkflowStageEvent = {
      type: 'workflow.stage.completed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      nodeId: node.id,
      stageId,
      ...(node.roleId ? { roleId: node.roleId } : node.reviewerRoleId ? { roleId: node.reviewerRoleId } : {}),
    };
    this.emitEvent(event);
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

  private emitStateChanged(state: WorkspaceStateChangedEvent['state']): void {
    const event: WorkspaceStateChangedEvent = {
      type: 'workspace.state.changed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      state,
    };
    this.emitEvent(event);
  }

  private updateMemberState(
    roleId: string,
    status: WorkspaceMember['status'],
    summary?: string,
    sessionId?: string,
  ): void {
    const member = this.state.members[roleId];
    if (!member) {
      return;
    }
    const nextMember: WorkspaceMember = {
      ...member,
      status,
      ...(summary ? { publicStateSummary: summary } : {}),
      ...(sessionId ? { sessionId } : {}),
      lastActivityAt: new Date().toISOString(),
    };
    this.state.members[roleId] = nextMember;
    const event: MemberStateChangedEvent = {
      type: 'member.state.changed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      member: { ...nextMember },
    };
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
    this.state.activities = [...this.state.activities, activity];
    const event: ActivityPublishedEvent = {
      type: 'activity.published',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      activity,
    };
    this.emitEvent(event);
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

    const event: DispatchClaimedEvent = {
      type: 'dispatch.claimed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: { ...dispatch },
      member: { ...member },
      claimStatus,
      ...(note ? { note } : {}),
    };
    this.emitEvent(event);
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

  private ensureStarted(): void {
    if (!this.active || !this.initialized) {
      throw new Error('Workspace has not been started.');
    }
  }

  private mustGetDispatch(dispatchId: string): TaskDispatch {
    const dispatch = this.state.dispatches[dispatchId];
    if (!dispatch) {
      throw new Error(`Unknown dispatch: ${dispatchId}`);
    }
    return dispatch;
  }

  private cloneDispatch(dispatch: TaskDispatch): TaskDispatch {
    return {
      ...dispatch,
    };
  }

  private applyPersistedState(
    snapshot: WorkspaceState,
    providerState: PersistedProviderState,
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

    for (const binding of Object.values(providerState.memberBindings)) {
      this.roleThreadIds.set(binding.roleId, binding.providerConversationId);
    }
  }

  private buildProviderState(): PersistedProviderState {
    return {
      workspaceId: this.spec.id,
      provider: 'codex-sdk',
      ...(this.state.sessionId ? { rootConversationId: this.state.sessionId } : {}),
      memberBindings: Object.fromEntries(
        Array.from(this.roleThreadIds.entries()).map(([roleId, threadId]) => [
          roleId,
          {
            roleId,
            providerConversationId: threadId,
            kind: 'thread' as const,
            updatedAt: new Date().toISOString(),
          },
        ]),
      ),
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
      .catch(error => {
        if (this.debug) {
          console.warn('[multi-agent-runtime] codex persistence failed', error);
        }
      });
  }

  private async closeWithReason(reason: string): Promise<void> {
    if (!this.active && this.activeRuns.size === 0 && !this.hasOpenDispatches()) {
      this.deactivateProcessExitCleanup();
      return;
    }

    this.stopOpenDispatches(reason);
    for (const run of this.activeRuns.values()) {
      run.controller.abort();
    }
    this.activeRuns.clear();
    this.roleChains.clear();
    this.active = false;
    this.state.status = 'closed';
    this.emitStateChanged('closed');
    await this.persistenceFlushed;
    this.deactivateProcessExitCleanup();
  }

  private stopOpenDispatches(summary: string): void {
    for (const dispatch of Object.values(this.state.dispatches)) {
      this.stopDispatch(dispatch.dispatchId, summary);
    }
  }

  private hasOpenDispatches(): boolean {
    return Object.values(this.state.dispatches).some(
      dispatch => !isTerminalDispatchStatus(dispatch.status),
    );
  }

  private stopDispatch(dispatchId: string, summary: string): void {
    const dispatch = this.state.dispatches[dispatchId];
    if (!dispatch || isTerminalDispatchStatus(dispatch.status)) {
      return;
    }

    const taskId =
      dispatch.providerTaskId ??
      this.activeRuns.get(dispatchId)?.threadId ??
      dispatch.dispatchId;
    this.finishDispatch(dispatchId, taskId, {
      status: 'stopped',
      summary,
      ...(dispatch.resultText ? { resultText: dispatch.resultText } : {}),
      ...(dispatch.outputFile ? { outputFile: dispatch.outputFile } : {}),
    });
  }

  private resetTransientRuntimeForStart(): void {
    if (this.state.status !== 'closed') {
      return;
    }

    this.initializedWithThread = false;
    this.state.status = 'idle';
  }
}

function buildFinishResult(
  status: 'completed' | 'failed' | 'stopped',
  summary: string,
  resultText?: string,
): {
  status: 'completed' | 'failed' | 'stopped';
  summary: string;
  resultText?: string;
} {
  return resultText === undefined
    ? { status, summary }
    : { status, summary, resultText };
}
