import { execFile } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';

const execFileAsync = promisify(execFile);

import { resolveWorkflowNodeModel, resolveWorkflowNodeProvider } from './providerResolution.js';
import {
  loadTaskListDocument,
  mergeWorklistDocumentWithRuntime,
  normalizeWorkItem,
  normalizeWorkItemStatus,
  persistTaskListDocument,
  reloadMergedTaskListDocument,
  resolveWorkspacePath,
  taskListContextFromSpec,
  type LoadedWorklistDocument,
} from './taskFiles.js';
import {
  cloneTaskControlState,
  extractTaskControlFeedback,
  inferStructuredOutcome,
  mergeTaskControlStates,
  resolveStructuredWorkItemStatus,
} from './taskControl.js';
import type {
  CompletionStatus,
  RoleSpec,
  TaskDispatch,
  WorkflowEdgeCondition,
  WorkflowWorkItemDocumentStatus,
  WorkflowNodeSpec,
  WorkflowSpec,
  WorkflowTaskListArtifact,
  WorkflowWorkItem,
  WorkflowWorkItemStatus,
  WorkflowWorklistMode,
  WorkflowWorklistRuntimeState,
  WorkspaceSpec,
  WorkspaceTurnAssignment,
  WorkspaceTurnRequest,
} from './types.js';
import { buildWorkflowDispatchAssignment, getWorkflowEntryNode } from './workspaceTurn.js';

export interface WorkflowExecutionResult {
  dispatches: TaskDispatch[];
  visitedNodeIds: string[];
  completionStatus: CompletionStatus;
  finalNodeId?: string;
}

export interface WorkflowExecutionHooks {
  onNodeStarted?: (node: WorkflowNodeSpec) => void | Promise<void>;
  onNodeCompleted?: (
    node: WorkflowNodeSpec,
    dispatch: TaskDispatch | undefined,
    outcome: WorkflowEdgeCondition,
  ) => void | Promise<void>;
  onStageStarted?: (stageId: string, node: WorkflowNodeSpec) => void | Promise<void>;
  onStageCompleted?: (stageId: string, node: WorkflowNodeSpec) => void | Promise<void>;
  onWorklistUpdated?: (
    node: WorkflowNodeSpec,
    worklist: WorkflowWorklistRuntimeState,
  ) => void | Promise<void>;
  onCompleted?: (
    result: WorkflowExecutionResult,
    lastNode: WorkflowNodeSpec | undefined,
  ) => void | Promise<void>;
}

export interface WorkflowExecutionOptions {
  startNodeId?: string;
}

interface WorkflowNodeExecutionContext {
  spec: WorkspaceSpec;
  request: WorkspaceTurnRequest;
  runAssignment: (
    assignment: WorkspaceTurnAssignment,
    node: WorkflowNodeSpec,
  ) => Promise<TaskDispatch>;
  hooks: WorkflowExecutionHooks;
}

interface WorkflowNodeExecutionResult {
  dispatches: TaskDispatch[];
  outcome: WorkflowEdgeCondition;
  completionStatus?: CompletionStatus;
}

type WorkflowNodeExecutor = (
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
) => Promise<WorkflowNodeExecutionResult>;

const WORKFLOW_NODE_EXECUTORS: Partial<Record<string, WorkflowNodeExecutor>> = {
  complete: executeCompleteNode,
  worklist: executeWorklistNode,
};

export async function executeWorkflow(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  runAssignment: (assignment: WorkspaceTurnAssignment, node: WorkflowNodeSpec) => Promise<TaskDispatch>,
  hooks: WorkflowExecutionHooks = {},
  options: WorkflowExecutionOptions = {},
): Promise<WorkflowExecutionResult> {
  const workflow = spec.workflow;
  if (!workflow) {
    return {
      dispatches: [],
      visitedNodeIds: [],
      completionStatus: spec.completionPolicy?.defaultStatus ?? 'stuck',
    };
  }

  const nodeById = new Map(workflow.nodes.map(node => [node.id, node]));
  let currentNode =
    options.startNodeId
      ? workflow.nodes.find(node => node.id === options.startNodeId)
      : getWorkflowEntryNode(spec);
  if (!currentNode) {
    return {
      dispatches: [],
      visitedNodeIds: [],
      completionStatus: spec.completionPolicy?.defaultStatus ?? 'stuck',
    };
  }

  const maxIterations = Math.max(1, spec.completionPolicy?.maxIterations ?? 8);
  const maxSteps = Math.max(workflow.nodes.length, 1) * maxIterations;
  const dispatches: TaskDispatch[] = [];
  const visitedNodeIds: string[] = [];
  let activeStageId: string | undefined;
  let lastNode: WorkflowNodeSpec | undefined;

  for (let iteration = 0; iteration < maxSteps && currentNode; iteration += 1) {
    const node = currentNode;
    lastNode = node;
    visitedNodeIds.push(node.id);

    if (node.stageId && node.stageId !== activeStageId) {
      if (activeStageId && hooks.onStageCompleted) {
        await hooks.onStageCompleted(activeStageId, node);
      }
      activeStageId = node.stageId;
      if (hooks.onStageStarted) {
        await hooks.onStageStarted(activeStageId, node);
      }
    }

    if (hooks.onNodeStarted) {
      await hooks.onNodeStarted(node);
    }

    const execution = await executeWorkflowNode(
      {
        spec,
        request,
        runAssignment,
        hooks,
      },
      node,
    );
    dispatches.push(...execution.dispatches);

    const completionStatus =
      execution.completionStatus ?? resolveCompletionStatus(spec, node.id, execution.outcome);
    const terminalDispatch = execution.dispatches.at(-1);

    if (hooks.onNodeCompleted) {
      await hooks.onNodeCompleted(node, terminalDispatch, execution.outcome);
    }

    if (completionStatus) {
      const result: WorkflowExecutionResult = {
        dispatches,
        visitedNodeIds,
        completionStatus,
        finalNodeId: node.id,
      };
      if (activeStageId && hooks.onStageCompleted) {
        await hooks.onStageCompleted(activeStageId, node);
      }
      if (hooks.onCompleted) {
        await hooks.onCompleted(result, node);
      }
      return result;
    }

    const nextNodeId = resolveNextWorkflowNodeId(workflow, node.id, execution.outcome);
    if (!nextNodeId) {
      const result: WorkflowExecutionResult = {
        dispatches,
        visitedNodeIds,
        completionStatus:
          terminalDispatch?.status === 'failed'
            ? 'crash'
            : terminalDispatch?.status === 'stopped'
              ? 'stuck'
              : spec.completionPolicy?.defaultStatus ?? 'stuck',
        finalNodeId: node.id,
      };
      if (activeStageId && hooks.onStageCompleted) {
        await hooks.onStageCompleted(activeStageId, node);
      }
      if (hooks.onCompleted) {
        await hooks.onCompleted(result, node);
      }
      return result;
    }

    const nextNode = nodeById.get(nextNodeId);
    if (!nextNode) {
      const result: WorkflowExecutionResult = {
        dispatches,
        visitedNodeIds,
        completionStatus: 'crash',
        finalNodeId: node.id,
      };
      if (activeStageId && hooks.onStageCompleted) {
        await hooks.onStageCompleted(activeStageId, node);
      }
      if (hooks.onCompleted) {
        await hooks.onCompleted(result, node);
      }
      return result;
    }

    if (
      activeStageId &&
      node.stageId &&
      nextNode.stageId !== node.stageId &&
      hooks.onStageCompleted
    ) {
      await hooks.onStageCompleted(activeStageId, node);
      activeStageId = undefined;
    }

    currentNode = nextNode;
  }

  const result: WorkflowExecutionResult = {
    dispatches,
    visitedNodeIds,
    completionStatus: spec.completionPolicy?.defaultStatus ?? 'stuck',
    ...(lastNode ? { finalNodeId: lastNode.id } : {}),
  };
  if (lastNode && activeStageId && hooks.onStageCompleted) {
    await hooks.onStageCompleted(activeStageId, lastNode);
  }
  if (hooks.onCompleted) {
    await hooks.onCompleted(result, lastNode);
  }
  return result;
}

async function executeWorkflowNode(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
): Promise<WorkflowNodeExecutionResult> {
  const executor = WORKFLOW_NODE_EXECUTORS[node.type] ?? executeAssignmentNode;
  return executor(context, node);
}

async function executeCompleteNode(): Promise<WorkflowNodeExecutionResult> {
  return {
    dispatches: [],
    outcome: 'success',
    completionStatus: 'done',
  };
}

async function executeAssignmentNode(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
): Promise<WorkflowNodeExecutionResult> {
  const assignment = buildWorkflowDispatchAssignment(context.spec, context.request, node);
  if (!assignment) {
    return {
      dispatches: [],
      outcome: 'success',
    };
  }

  const dispatch = await context.runAssignment(assignment, node);
  const outgoingEdges = context.spec.workflow?.edges.filter(edge => edge.from === node.id) ?? [];
  return {
    dispatches: [dispatch],
    outcome: inferWorkflowOutcome(node, dispatch, outgoingEdges.map(edge => edge.when)),
  };
}

async function executeWorklistNode(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
): Promise<WorkflowNodeExecutionResult> {
  const artifactId = node.worklistArtifactId ?? node.producesArtifacts?.[0];
  const workerRoleId = resolveWorklistWorkerRoleId(node);
  if (!artifactId || !workerRoleId) {
    return {
      dispatches: [],
      outcome: 'failure',
    };
  }

  const mode = node.worklistMode ?? (node.replenish === 'when_empty' ? 'replenishing' : 'finite');
  const worklist = createEmptyRuntimeWorklist(node, artifactId, workerRoleId, mode);
  const dispatches: TaskDispatch[] = [];
  const maxBatches = Math.max(
    1,
    node.maxBatches ?? (mode === 'replenishing' ? specDefaultMaxBatches(context.spec) : 1),
  );

  for (let batchIndex = 0; batchIndex < maxBatches; batchIndex += 1) {
    let loaded = await ensureWorklistLoaded(context, node, worklist, artifactId, batchIndex > 0);
    if (!loaded) {
      return {
        dispatches,
        outcome: batchIndex === 0 ? 'failure' : 'success',
      };
    }

    synchronizeRuntimeWorklist(worklist, loaded.document);
    await publishWorklistUpdate(context, node, worklist);

    let processedItem = false;
    const preTaskShas = new Map<string, string>();
    while (true) {
      if (node.reloadWorklistAfterItem) {
        const refreshed = await loadWorklistDocument(context.spec, artifactId);
        if (refreshed) {
          loaded = refreshed;
          synchronizeRuntimeWorklist(worklist, loaded.document);
          await publishWorklistUpdate(context, node, worklist);
        }
      }
      const nextItem = selectNextReadyItem(loaded.document, worklist);
      if (!nextItem) {
        break;
      }

      processedItem = true;
      if (
        (node.revertOnRetry || node.revertOnExhaustedFailure) &&
        !preTaskShas.has(nextItem.id) &&
        context.spec.cwd
      ) {
        const sha = await captureGitHeadSha(context.spec.cwd);
        if (sha) {
          preTaskShas.set(nextItem.id, sha);
        }
      }
      const assignment = buildWorklistItemAssignment(
        context.spec,
        context.request,
        node,
        workerRoleId,
        nextItem,
        worklist,
      );
      markWorkItemRunning(worklist, nextItem);
      updateDocumentItemStatus(loaded.document, nextItem.id, 'running', worklist.items[nextItem.id]?.attempts ?? 1);
      loaded = await persistAndReloadMergedWorklistDocument(context.spec, artifactId, loaded, worklist);
      await publishWorklistUpdate(context, node, worklist);

      const finalStatus = await runWorklistItemLifecycleWithTimeout(
        context,
        node,
        nextItem,
        artifactId,
        workerRoleId,
        worklist,
        assignment,
        dispatches,
      );

      const preSha = preTaskShas.get(nextItem.id);
      if (preSha) {
        const shouldRevertOnRetry = node.revertOnRetry && finalStatus === 'pending';
        const shouldRevertOnExhausted =
          node.revertOnExhaustedFailure &&
          (finalStatus === 'failed' || finalStatus === 'discarded');
        if ((shouldRevertOnRetry || shouldRevertOnExhausted) && context.spec.cwd) {
          await gitResetHard(context.spec.cwd, preSha, nextItem.id, finalStatus);
        }
        if (finalStatus !== 'pending') {
          preTaskShas.delete(nextItem.id);
        }
      }

      updateDocumentItemStatus(
        loaded.document,
        nextItem.id,
        finalStatus,
        worklist.items[nextItem.id]?.attempts ?? 1,
      );
      loaded = await persistAndReloadMergedWorklistDocument(context.spec, artifactId, loaded, worklist);
      await publishWorklistUpdate(context, node, worklist);

      if (
        (finalStatus === 'failed' || finalStatus === 'blocked') &&
        (node.stopOnItemFailure ?? true)
      ) {
        return {
          dispatches,
          outcome: 'failure',
        };
      }
    }

    if (hasBlockingFailure(loaded.document, worklist)) {
      return {
        dispatches,
        outcome: 'failure',
      };
    }

    const hasPending = loaded.document.items.some(item => {
      const runtimeItem = worklist.items[item.id];
      return (runtimeItem?.status ?? normalizeWorkItemStatus(item.status)) === 'pending';
    });
    if (hasPending && !processedItem) {
      return {
        dispatches,
        outcome: 'failure',
      };
    }

    if (mode !== 'replenishing' || node.replenish !== 'when_empty') {
      return {
        dispatches,
        outcome: worklist.failedItemIds.length > 0 ? 'failure' : 'success',
      };
    }

    if (!node.plannerRoleId || !node.plannerPrompt || batchIndex + 1 >= maxBatches) {
      return {
        dispatches,
        outcome: worklist.failedItemIds.length > 0 ? 'failure' : 'success',
      };
    }

    const beforePending = countPendingItems(worklist);
    const plannerDispatch = await runWorklistPlanner(context, node, worklist, artifactId);
    if (!plannerDispatch) {
      return {
        dispatches,
        outcome: worklist.failedItemIds.length > 0 ? 'failure' : 'success',
      };
    }

    dispatches.push(plannerDispatch);
    loaded = await loadWorklistDocument(context.spec, artifactId);
    if (!loaded) {
      return {
        dispatches,
        outcome: 'failure',
      };
    }

    synchronizeRuntimeWorklist(worklist, loaded.document);
    await publishWorklistUpdate(context, node, worklist);

    if (countPendingItems(worklist) <= beforePending) {
      return {
        dispatches,
        outcome: worklist.failedItemIds.length > 0 ? 'failure' : 'success',
      };
    }
  }

  return {
    dispatches,
    outcome: worklist.failedItemIds.length > 0 ? 'failure' : 'success',
  };
}

function specDefaultMaxBatches(spec: WorkspaceSpec): number {
  return Math.max(1, spec.completionPolicy?.maxIterations ?? 3);
}

async function ensureWorklistLoaded(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
  worklist: WorkflowWorklistRuntimeState,
  artifactId: string,
  allowReplan: boolean,
): Promise<LoadedWorklistDocument | undefined> {
  let loaded = await loadWorklistDocument(context.spec, artifactId);
  if (loaded && loaded.document.items.length > 0) {
    return loaded;
  }

  if (!node.plannerRoleId || !node.plannerPrompt) {
    return loaded;
  }

  if (allowReplan || !loaded || loaded.document.items.length === 0) {
    const plannerDispatch = await runWorklistPlanner(context, node, worklist, artifactId);
    if (!plannerDispatch) {
      return loaded;
    }
  }

  return loadWorklistDocument(context.spec, artifactId);
}

async function runWorklistPlanner(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
  worklist: WorkflowWorklistRuntimeState,
  artifactId: string,
): Promise<TaskDispatch | undefined> {
  const role = context.spec.roles.find(value => value.id === node.plannerRoleId);
  if (!role) {
    return undefined;
  }

  const assignment = buildWorklistPlannerAssignment(
    context.spec,
    context.request,
    node,
    role,
    artifactId,
    worklist,
  );
  const dispatch = await context.runAssignment(assignment, node);
  worklist.batchCount += 1;
  worklist.lastUpdatedAt = new Date().toISOString();
  await publishWorklistUpdate(context, node, worklist);
  return dispatch;
}

function resolveWorklistWorkerRoleId(node: WorkflowNodeSpec): string | undefined {
  return node.workerRoleId ?? node.roleId;
}

function resolveWorklistEvaluationRoleId(node: WorkflowNodeSpec): string | undefined {
  return node.itemLifecycle?.evaluate?.roleId;
}

function resolveWorklistEvaluationPromptTemplate(node: WorkflowNodeSpec): string | undefined {
  return node.itemLifecycle?.evaluate?.promptTemplate;
}

function resolveWorklistRejectMode(node: WorkflowNodeSpec): 'retry' | 'fail' {
  return node.itemLifecycle?.evaluate?.onReject ?? 'retry';
}

function resolveWorklistAfterApproveAction(node: WorkflowNodeSpec): 'none' | 'commit' {
  return node.itemLifecycle?.afterApprove?.action ?? 'none';
}

function resolveWorklistCommitRoleId(
  node: WorkflowNodeSpec,
  fallbackRoleId: string,
): string {
  return node.itemLifecycle?.afterApprove?.roleId ?? fallbackRoleId;
}

function resolveWorklistCommitPromptTemplate(node: WorkflowNodeSpec): string | undefined {
  return node.itemLifecycle?.afterApprove?.promptTemplate;
}

function resolveWorklistAfterCommitRoleId(node: WorkflowNodeSpec): string | undefined {
  return node.itemLifecycle?.afterCommit?.roleId;
}

function resolveWorklistAfterCommitPromptTemplate(node: WorkflowNodeSpec): string | undefined {
  return node.itemLifecycle?.afterCommit?.promptTemplate;
}

function resolveWorklistAfterCommitFailureMode(
  node: WorkflowNodeSpec,
): 'retry' | 'fail' | 'warn' {
  return node.itemLifecycle?.afterCommit?.onFailure ?? 'warn';
}

async function runWorklistItemLifecycleWithTimeout(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
  item: WorkflowWorkItem,
  artifactId: string,
  workerRoleId: string,
  worklist: WorkflowWorklistRuntimeState,
  assignment: WorkspaceTurnAssignment,
  dispatches: TaskDispatch[],
): Promise<WorkflowWorkItemStatus> {
  const timeoutMs = node.itemTimeoutMs;
  const lifecycle = executeWorklistItemLifecycle(
    context,
    node,
    item,
    artifactId,
    workerRoleId,
    worklist,
    assignment,
    dispatches,
  );
  if (!timeoutMs || timeoutMs <= 0) {
    return lifecycle;
  }
  let timeoutHandle: NodeJS.Timeout | undefined;
  const timeoutPromise = new Promise<'__TIMEOUT__'>(resolve => {
    timeoutHandle = setTimeout(() => resolve('__TIMEOUT__'), timeoutMs);
  });
  try {
    const winner = await Promise.race([lifecycle, timeoutPromise]);
    if (winner === '__TIMEOUT__') {
      // eslint-disable-next-line no-console
      console.warn(
        `[worklist] item "${item.id}" exceeded itemTimeoutMs=${timeoutMs}; marking failed/retry`,
      );
      return markWorkItemTimeout(worklist, item, timeoutMs);
    }
    return winner;
  } finally {
    if (timeoutHandle) clearTimeout(timeoutHandle);
  }
}

function markWorkItemTimeout(
  worklist: WorkflowWorklistRuntimeState,
  item: WorkflowWorkItem,
  timeoutMs: number,
): WorkflowWorkItemStatus {
  const previous = worklist.items[item.id];
  const attempts = previous?.attempts ?? Math.max(1, item.attempts ?? 1);
  const maxAttempts = item.maxAttempts ?? previous?.maxAttempts;
  const exhausted = maxAttempts !== undefined && attempts >= maxAttempts;
  const finalStatus: WorkflowWorkItemStatus = exhausted ? 'failed' : 'pending';
  const feedback = `Timed out after ${timeoutMs}ms (attempt ${attempts}${maxAttempts ? '/' + maxAttempts : ''}). Address the root cause before retrying.`;
  worklist.items[item.id] = {
    itemId: item.id,
    title: item.title,
    status: finalStatus,
    attempts,
    ...(maxAttempts !== undefined ? { maxAttempts } : {}),
    ...(previous?.dispatchId ? { dispatchId: previous.dispatchId } : {}),
    ...(previous?.evaluatorDispatchId ? { evaluatorDispatchId: previous.evaluatorDispatchId } : {}),
    ...(previous?.commitDispatchId ? { commitDispatchId: previous.commitDispatchId } : {}),
    ...(previous?.qaDispatchId ? { qaDispatchId: previous.qaDispatchId } : {}),
    lastSummary: `timeout at ${new Date().toISOString()}`,
    lastFeedback: feedback,
    ...(previous?.taskControl ? { taskControl: cloneTaskControlState(previous.taskControl) } : {}),
    updatedAt: new Date().toISOString(),
  };
  if (finalStatus === 'pending') {
    worklist.failedItemIds = worklist.failedItemIds.filter(value => value !== item.id);
    worklist.completedItemIds = worklist.completedItemIds.filter(value => value !== item.id);
  } else {
    worklist.failedItemIds = uniqueIds([...worklist.failedItemIds, item.id]);
    worklist.completedItemIds = worklist.completedItemIds.filter(value => value !== item.id);
  }
  worklist.lastUpdatedAt = new Date().toISOString();
  return finalStatus;
}

async function executeWorklistItemLifecycle(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
  item: WorkflowWorkItem,
  artifactId: string,
  workerRoleId: string,
  worklist: WorkflowWorklistRuntimeState,
  workerAssignment: WorkspaceTurnAssignment,
  dispatches: TaskDispatch[],
): Promise<WorkflowWorkItemStatus> {
  const workerDispatch = await context.runAssignment(workerAssignment, node);
  dispatches.push(workerDispatch);

  if (workerDispatch.status !== 'completed') {
    return markWorkItemFromDispatch(worklist, item, workerDispatch);
  }

  const documentStatusOverride = await loadWorkItemDocumentStatusOverride(
    context.spec,
    artifactId,
    item.id,
  );
  if (documentStatusOverride === 'abandoned' || documentStatusOverride === 'superseded') {
    return markWorkItemAbandonedOrSuperseded(
      worklist,
      item,
      workerDispatch,
      documentStatusOverride,
    );
  }

  const reviewDispatch = resolveWorklistEvaluationRoleId(node)
    ? await runWorklistEvaluation(context, node, item, worklist, workerDispatch)
    : undefined;
  if (reviewDispatch) {
    dispatches.push(reviewDispatch);
    const reviewOutcome = inferDispatchOutcome(reviewDispatch, ['approved', 'rejected']);
    if (reviewOutcome !== 'approved') {
      const rejectMode = resolveWorklistRejectMode(node);
      return markWorkItemForRetryOrFailure(
        worklist,
        item,
        reviewDispatch,
        extractWorkItemFeedback(reviewDispatch),
        'evaluation',
        rejectMode,
      );
    }
  }

  if (resolveWorklistAfterApproveAction(node) === 'commit') {
    const commitDispatch = await runWorklistCommit(
      context,
      node,
      item,
      worklist,
      workerDispatch,
      reviewDispatch,
      workerRoleId,
    );
    dispatches.push(commitDispatch);
    if (commitDispatch.status !== 'completed') {
      return markWorkItemForRetryOrFailure(
        worklist,
        item,
        commitDispatch,
        extractWorkItemFeedback(commitDispatch),
        'commit',
      );
    }
    let qaDispatch: TaskDispatch | undefined;
    if (resolveWorklistAfterCommitRoleId(node)) {
      qaDispatch = await runWorklistQa(
        context,
        node,
        item,
        worklist,
        workerDispatch,
        reviewDispatch,
        commitDispatch,
      );
      dispatches.push(qaDispatch);
      const qaOutcome = inferDispatchOutcome(qaDispatch, ['approved', 'rejected']);
      const qaFailed = qaDispatch.status !== 'completed' || qaOutcome === 'rejected';
      if (qaFailed) {
        const qaMode = resolveWorklistAfterCommitFailureMode(node);
        if (qaMode === 'retry') {
          return markWorkItemForRetryOrFailure(
            worklist,
            item,
            qaDispatch,
            extractWorkItemFeedback(qaDispatch),
            'qa',
            'retry',
          );
        }
        if (qaMode === 'fail') {
          return markWorkItemForRetryOrFailure(
            worklist,
            item,
            qaDispatch,
            extractWorkItemFeedback(qaDispatch),
            'qa',
            'fail',
          );
        }
      }
    }
    const previous = worklist.items[item.id];
    const completedSummary =
      (qaDispatch?.lastSummary ?? qaDispatch?.resultText
        ? summarizeResult(qaDispatch?.lastSummary ?? qaDispatch?.resultText ?? '')
        : undefined) ??
      commitDispatch.lastSummary ??
      (commitDispatch.resultText
        ? summarizeResult(commitDispatch.resultText)
        : previous?.lastSummary);
    const mergedTaskControl = mergeTaskControlStates(
      mergeTaskControlStates(previous?.taskControl, workerDispatch.taskControl),
      mergeTaskControlStates(
        mergeTaskControlStates(reviewDispatch?.taskControl, commitDispatch.taskControl),
        qaDispatch?.taskControl,
      ),
    );
    worklist.items[item.id] = {
      itemId: item.id,
      title: item.title,
      status: 'completed',
      attempts: previous?.attempts ?? 1,
      ...(item.maxAttempts !== undefined ? { maxAttempts: item.maxAttempts } : {}),
      dispatchId: workerDispatch.dispatchId,
      ...(completedSummary ? { lastSummary: completedSummary } : {}),
      ...(previous?.lastFeedback ? { lastFeedback: previous.lastFeedback } : {}),
      ...(reviewDispatch?.dispatchId ? { evaluatorDispatchId: reviewDispatch.dispatchId } : {}),
      commitDispatchId: commitDispatch.dispatchId,
      ...(qaDispatch?.dispatchId ? { qaDispatchId: qaDispatch.dispatchId } : {}),
      ...(mergedTaskControl ? { taskControl: mergedTaskControl } : {}),
      updatedAt: new Date().toISOString(),
    };
    worklist.completedItemIds = uniqueIds([...worklist.completedItemIds, item.id]);
    worklist.failedItemIds = worklist.failedItemIds.filter(value => value !== item.id);
    worklist.lastUpdatedAt = new Date().toISOString();
    return 'completed';
  }

  const finalStatus = markWorkItemFromDispatch(worklist, item, workerDispatch);
  if (reviewDispatch) {
    const previous = worklist.items[item.id];
    if (previous) {
      const mergedTaskControl = mergeTaskControlStates(previous.taskControl, reviewDispatch.taskControl);
      worklist.items[item.id] = {
        itemId: previous.itemId,
        title: previous.title,
        status: previous.status,
        attempts: previous.attempts,
        ...(previous.maxAttempts !== undefined ? { maxAttempts: previous.maxAttempts } : {}),
        ...(previous.dispatchId ? { dispatchId: previous.dispatchId } : {}),
        ...(previous.lastSummary ? { lastSummary: previous.lastSummary } : {}),
        ...(previous.lastFeedback ? { lastFeedback: previous.lastFeedback } : {}),
        evaluatorDispatchId: reviewDispatch.dispatchId,
        ...(previous.commitDispatchId ? { commitDispatchId: previous.commitDispatchId } : {}),
        ...(mergedTaskControl ? { taskControl: mergedTaskControl } : {}),
        updatedAt: new Date().toISOString(),
      };
    }
  }
  return finalStatus;
}

async function runWorklistEvaluation(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
  item: WorkflowWorkItem,
  worklist: WorkflowWorklistRuntimeState,
  workerDispatch: TaskDispatch,
): Promise<TaskDispatch> {
  const evaluationRoleId = resolveWorklistEvaluationRoleId(node);
  if (!evaluationRoleId) {
    throw new Error(`Workflow node "${node.id}" does not declare an evaluation role.`);
  }
  const role = mustFindRole(context.spec, evaluationRoleId);
  const assignment = buildWorklistEvaluationAssignment(
    context.spec,
    context.request,
    node,
    role,
    item,
    worklist,
    workerDispatch,
  );
  return context.runAssignment(assignment, node);
}

async function runWorklistCommit(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
  item: WorkflowWorkItem,
  worklist: WorkflowWorklistRuntimeState,
  workerDispatch: TaskDispatch,
  reviewDispatch: TaskDispatch | undefined,
  fallbackRoleId: string,
): Promise<TaskDispatch> {
  const commitRoleId = resolveWorklistCommitRoleId(node, fallbackRoleId);
  const role = mustFindRole(context.spec, commitRoleId);
  const assignment = buildWorklistCommitAssignment(
    context.spec,
    context.request,
    node,
    role,
    item,
    worklist,
    workerDispatch,
    reviewDispatch,
  );
  return context.runAssignment(assignment, node);
}

async function runWorklistQa(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
  item: WorkflowWorkItem,
  worklist: WorkflowWorklistRuntimeState,
  workerDispatch: TaskDispatch,
  reviewDispatch: TaskDispatch | undefined,
  commitDispatch: TaskDispatch,
): Promise<TaskDispatch> {
  const qaRoleId = resolveWorklistAfterCommitRoleId(node);
  if (!qaRoleId) {
    throw new Error(`Workflow node "${node.id}" does not declare an afterCommit QA role.`);
  }
  const role = mustFindRole(context.spec, qaRoleId);
  const assignment = buildWorklistQaAssignment(
    context.spec,
    context.request,
    node,
    role,
    item,
    worklist,
    workerDispatch,
    reviewDispatch,
    commitDispatch,
  );
  return context.runAssignment(assignment, node);
}

function buildWorklistQaAssignment(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  node: WorkflowNodeSpec,
  role: RoleSpec,
  item: WorkflowWorkItem,
  worklist: WorkflowWorklistRuntimeState,
  workerDispatch: TaskDispatch,
  reviewDispatch: TaskDispatch | undefined,
  commitDispatch: TaskDispatch,
): WorkspaceTurnAssignment {
  const template = resolveWorklistAfterCommitPromptTemplate(node)?.trim();
  const workerResult =
    reviewDispatch?.resultText ??
    reviewDispatch?.lastSummary ??
    workerDispatch.resultText ??
    workerDispatch.lastSummary ??
    '';
  const commitResult = commitDispatch.resultText ?? commitDispatch.lastSummary ?? '';
  const instruction = template
    ? applyTemplate(spec, node, template, item, request.message, worklist, {
        workerResult,
        commitMessage: commitResult,
      })
    : [
        `You are the QA agent verifying committed work item "${item.title}" (${item.id}).`,
        `Task description:\n${item.description}`,
        item.files?.length ? `Files most relevant: ${item.files.join(', ')}` : null,
        item.acceptanceCriteria?.length
          ? `Acceptance criteria:\n${item.acceptanceCriteria.map(value => `- ${value}`).join('\n')}`
          : null,
        workerResult ? `Coder/reviewer handoff:\n${workerResult}` : null,
        commitResult ? `Commit dispatch output:\n${commitResult}` : null,
        'Execute the relevant end-to-end eval cases from tests/eval/ that cover this task. Update case status markers in-place (pass / fail / flaky / skip with reason).',
        'Start your response with either "APPROVED:" or "REJECTED:". If rejected, list the failing cases and the concrete problems the coding agent must fix next.',
      ]
        .filter(Boolean)
        .join('\n');

  return {
    roleId: role.id,
    summary: `QA eval for work item ${item.id}: ${item.title}`,
    provider: resolveWorkflowNodeProvider(spec, role, node),
    model: resolveWorkflowNodeModel(spec, role, node),
    instruction,
    visibility:
      node.visibility ??
      request.visibility ??
      spec.activityPolicy?.defaultVisibility ??
      'public',
    workflowNodeId: node.id,
    ...(node.stageId ? { stageId: node.stageId } : {}),
    workItemId: item.id,
  };
}

function buildWorklistPlannerAssignment(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  node: WorkflowNodeSpec,
  role: RoleSpec,
  artifactId: string,
  worklist: WorkflowWorklistRuntimeState,
): WorkspaceTurnAssignment {
  const artifact = mustFindArtifact(spec, artifactId);
  const artifactPath = resolveArtifactAbsolutePath(spec, artifact.path);
  return {
    roleId: role.id,
    summary: node.title ? `${node.title} planner` : `Plan worklist for ${node.id}`,
    provider: resolveWorkflowNodeProvider(spec, role, node),
    model: resolveWorkflowNodeModel(spec, role, node),
    instruction: [
      `You are preparing structured work items for workflow node "${node.title ?? node.id}".`,
      node.stageId ? `Current stage: ${node.stageId}.` : null,
      `Write or update the worklist JSON at: ${artifactPath}.`,
      'Use valid JSON only.',
      'Preferred shape: {"version":1,"mode":"finite","items":[{"id":"task-id","title":"Short title","description":"Detailed instruction","status":"pending","attempts":0,"maxAttempts":2}]}',
      'A top-level "tasks" array is also accepted for compatibility, but prefer "items".',
      worklist.completedItemIds.length > 0
        ? `Already completed item ids: ${worklist.completedItemIds.join(', ')}.`
        : null,
      worklist.failedItemIds.length > 0
        ? `Previously failed item ids: ${worklist.failedItemIds.join(', ')}. Avoid reusing them unless you are explicitly retrying with a better plan.`
        : null,
      node.plannerPrompt ? `Planning instructions: ${node.plannerPrompt}` : null,
      `Original user request: ${request.message}`,
      'Only include actionable work items that should be executed next.',
    ]
      .filter(Boolean)
      .join('\n'),
    visibility:
      node.visibility ??
      request.visibility ??
      spec.activityPolicy?.defaultVisibility ??
      'public',
    workflowNodeId: node.id,
    ...(node.stageId ? { stageId: node.stageId } : {}),
  };
}

function buildWorklistEvaluationAssignment(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  node: WorkflowNodeSpec,
  role: RoleSpec,
  item: WorkflowWorkItem,
  worklist: WorkflowWorklistRuntimeState,
  workerDispatch: TaskDispatch,
): WorkspaceTurnAssignment {
  const runtimeItem = worklist.items[item.id];
  const template = resolveWorklistEvaluationPromptTemplate(node)?.trim();
  const instruction = template
    ? applyTemplate(spec, node, template, item, request.message, worklist, {
        workerResult: workerDispatch.resultText ?? workerDispatch.lastSummary ?? '',
      })
    : [
        `You are evaluating completed work item "${item.title}" (${item.id}).`,
        `Original task description:\n${item.description}`,
        item.files?.length ? `Files expected for this task: ${item.files.join(', ')}` : null,
        item.acceptanceCriteria?.length
          ? `Acceptance criteria:\n${item.acceptanceCriteria.map(value => `- ${value}`).join('\n')}`
          : null,
        runtimeItem?.lastFeedback
          ? `Previous rejection feedback that should now be resolved:\n${runtimeItem.lastFeedback}`
          : null,
        workerDispatch.resultText
          ? `Latest coder handoff:\n${workerDispatch.resultText}`
          : workerDispatch.lastSummary
            ? `Latest coder handoff summary:\n${workerDispatch.lastSummary}`
            : null,
        'Inspect the relevant files and decide whether the task is complete.',
        'Start your response with either "APPROVED:" or "REJECTED:".',
        'If rejected, provide concrete, actionable feedback for the coding agent to fix next.',
      ]
        .filter(Boolean)
        .join('\n');

  return {
    roleId: role.id,
    summary: `Evaluate work item ${item.id}: ${item.title}`,
    provider: resolveWorkflowNodeProvider(spec, role, node),
    model: resolveWorkflowNodeModel(spec, role, node),
    instruction,
    visibility:
      node.visibility ??
      request.visibility ??
      spec.activityPolicy?.defaultVisibility ??
      'public',
    workflowNodeId: node.id,
    ...(node.stageId ? { stageId: node.stageId } : {}),
    workItemId: item.id,
  };
}

function buildWorklistCommitAssignment(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  node: WorkflowNodeSpec,
  role: RoleSpec,
  item: WorkflowWorkItem,
  worklist: WorkflowWorklistRuntimeState,
  workerDispatch: TaskDispatch,
  reviewDispatch: TaskDispatch | undefined,
): WorkspaceTurnAssignment {
  const commitMessage = buildDefaultCommitMessage(item);
  const template = resolveWorklistCommitPromptTemplate(node)?.trim();
  const instruction = template
    ? applyTemplate(spec, node, template, item, request.message, worklist, {
        workerResult:
          reviewDispatch?.resultText ??
          reviewDispatch?.lastSummary ??
          workerDispatch.resultText ??
          workerDispatch.lastSummary ??
          '',
        commitMessage,
      })
    : [
        `You are committing the approved work for item "${item.title}" (${item.id}).`,
        item.files?.length ? `Stage and commit the files most relevant to this task: ${item.files.join(', ')}.` : null,
        workerDispatch.resultText ? `Coder handoff:\n${workerDispatch.resultText}` : null,
        reviewDispatch?.resultText ? `Evaluator approval note:\n${reviewDispatch.resultText}` : null,
        `Create a git commit with this message: ${commitMessage}`,
        'Only commit changes required for this approved task. Do not revert unrelated user changes.',
        'If the task is already committed or there is nothing to commit, explain clearly why.',
      ]
        .filter(Boolean)
        .join('\n');

  return {
    roleId: role.id,
    summary: `Commit work item ${item.id}: ${item.title}`,
    provider: resolveWorkflowNodeProvider(spec, role, node),
    model: resolveWorkflowNodeModel(spec, role, node),
    instruction,
    visibility:
      node.visibility ??
      request.visibility ??
      spec.activityPolicy?.defaultVisibility ??
      'public',
    workflowNodeId: node.id,
    ...(node.stageId ? { stageId: node.stageId } : {}),
    workItemId: item.id,
  };
}

function buildWorklistItemAssignment(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  node: WorkflowNodeSpec,
  workerRoleId: string,
  item: WorkflowWorkItem,
  worklist: WorkflowWorklistRuntimeState,
): WorkspaceTurnAssignment {
  const role = spec.roles.find(value => value.id === workerRoleId);
  if (!role) {
    throw new Error(`Unknown worklist worker role: ${workerRoleId}`);
  }

  return {
    roleId: role.id,
    summary: `Work item ${item.id}: ${item.title}`,
    provider: resolveWorkflowNodeProvider(spec, role, node),
    model: resolveWorkflowNodeModel(spec, role, node),
    instruction: buildWorklistItemInstruction(spec, node, item, request.message, worklist),
    visibility:
      node.visibility ??
      request.visibility ??
      spec.activityPolicy?.defaultVisibility ??
      'public',
    workflowNodeId: node.id,
    ...(node.stageId ? { stageId: node.stageId } : {}),
    workItemId: item.id,
  };
}

function buildWorklistItemInstruction(
  spec: WorkspaceSpec,
  node: WorkflowNodeSpec,
  item: WorkflowWorkItem,
  requestMessage: string,
  worklist: WorkflowWorklistRuntimeState,
): string {
  const template = node.itemPromptTemplate?.trim();
  if (template) {
    return applyTemplate(spec, node, template, item, requestMessage, worklist);
  }

  const runtimeItem = worklist.items[item.id];
  const taskListPath = resolveWorklistTaskListAbsolutePath(spec, node);
  const sharedLessonsPath = resolveSharedLessonsAbsolutePath(spec, node);
  const taskCliCommand = resolveTaskCliCommand(spec, node);

  return [
    `You are executing work item "${item.title}" (${item.id}).`,
    `Item description:\n${item.description}`,
    item.goalsFile ? `Goals file: ${item.goalsFile}` : null,
    item.referenceFiles?.length ? `Reference files: ${item.referenceFiles.join(', ')}` : null,
    item.files?.length ? `Target files: ${item.files.join(', ')}` : null,
    item.acceptanceCriteria?.length
      ? `Acceptance criteria:\n${item.acceptanceCriteria.map(value => `- ${value}`).join('\n')}`
      : null,
    sharedLessonsPath
      ? `Before making changes, read the shared lessons file if it exists: ${sharedLessonsPath}`
      : null,
    taskListPath ? `Task list source of truth: ${taskListPath}` : null,
    taskCliCommand
      ? `Use the task CLI instead of hand-editing tasks.json when you need to inspect/update tasks: ${taskCliCommand}`
      : null,
    runtimeItem?.lastFeedback
      ? `Previous evaluator feedback to address:\n${runtimeItem.lastFeedback}`
      : null,
    `Original user request: ${requestMessage}`,
    taskCliCommand
      ? `If this task is too large, add follow-up tasks immediately after ${item.id} with the CLI, then mark ${item.id} as abandoned or superseded with a clear reason before handing off.`
      : null,
    taskCliCommand && sharedLessonsPath
      ? `Before finishing, append any reusable pitfalls or environment lessons to ${sharedLessonsPath} via the CLI lessons:add command.`
      : null,
    'Complete only this work item and report what changed.',
  ]
    .filter(Boolean)
    .join('\n');
}

function applyTemplate(
  spec: WorkspaceSpec,
  node: WorkflowNodeSpec,
  template: string,
  item: WorkflowWorkItem,
  requestMessage: string,
  worklist?: WorkflowWorklistRuntimeState,
  extras: Record<string, string> = {},
): string {
  const runtimeItem = worklist?.items[item.id];
  const taskListPath = resolveWorklistTaskListAbsolutePath(spec, node);
  const sharedLessonsPath = resolveSharedLessonsAbsolutePath(spec, node);
  const taskCliCommand = resolveTaskCliCommand(spec, node);
  return template
    .replaceAll('{{id}}', item.id)
    .replaceAll('{{title}}', item.title)
    .replaceAll('{{description}}', item.description)
    .replaceAll('{{request}}', requestMessage)
    .replaceAll('{{feedback}}', runtimeItem?.lastFeedback ?? '')
    .replaceAll('{{attempts}}', String(runtimeItem?.attempts ?? item.attempts ?? 0))
    .replaceAll('{{files}}', item.files?.join(', ') ?? '')
    .replaceAll('{{acceptance_criteria}}', item.acceptanceCriteria?.join('\n') ?? '')
    .replaceAll('{{worker_result}}', extras.workerResult ?? '')
    .replaceAll('{{commit_message}}', extras.commitMessage ?? '')
    .replaceAll('{{task_list_path}}', taskListPath ?? '')
    .replaceAll('{{shared_lessons_path}}', sharedLessonsPath ?? '')
    .replaceAll('{{task_cli_command}}', taskCliCommand ?? '');
}

function createEmptyRuntimeWorklist(
  node: WorkflowNodeSpec,
  artifactId: string,
  workerRoleId: string,
  mode: WorkflowWorklistMode,
): WorkflowWorklistRuntimeState {
  return {
    nodeId: node.id,
    artifactId,
    mode,
    ...(node.plannerRoleId ? { plannerRoleId: node.plannerRoleId } : {}),
    workerRoleId,
    batchCount: 0,
    completedItemIds: [],
    failedItemIds: [],
    items: {},
    lastUpdatedAt: new Date().toISOString(),
  };
}

async function publishWorklistUpdate(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
  worklist: WorkflowWorklistRuntimeState,
): Promise<void> {
  worklist.lastUpdatedAt = new Date().toISOString();
  if (context.hooks.onWorklistUpdated) {
    await context.hooks.onWorklistUpdated(node, cloneWorklistState(worklist));
  }
}

function cloneWorklistState(worklist: WorkflowWorklistRuntimeState): WorkflowWorklistRuntimeState {
  return {
    ...worklist,
    completedItemIds: [...worklist.completedItemIds],
    failedItemIds: [...worklist.failedItemIds],
    items: Object.fromEntries(
      Object.entries(worklist.items).map(([itemId, state]) => [
        itemId,
        {
          ...state,
          ...(state.taskControl ? { taskControl: cloneTaskControlState(state.taskControl) } : {}),
        },
      ]),
    ),
  };
}

async function loadWorklistDocument(
  spec: WorkspaceSpec,
  artifactId: string,
): Promise<LoadedWorklistDocument | undefined> {
  const artifact = spec.artifacts?.find(value => value.id === artifactId);
  if (!artifact) {
    return undefined;
  }
  return loadTaskListDocument(taskListContextFromSpec(spec, artifact.path));
}

async function persistWorklistDocument(
  spec: WorkspaceSpec,
  artifactId: string,
  loaded: LoadedWorklistDocument,
): Promise<void> {
  const artifact = mustFindArtifact(spec, artifactId);
  await persistTaskListDocument(taskListContextFromSpec(spec, artifact.path), loaded);
}

async function persistAndReloadMergedWorklistDocument(
  spec: WorkspaceSpec,
  artifactId: string,
  loaded: LoadedWorklistDocument,
  worklist: WorkflowWorklistRuntimeState,
): Promise<LoadedWorklistDocument> {
  const artifact = mustFindArtifact(spec, artifactId);
  const merged = await reloadMergedTaskListDocument(
    taskListContextFromSpec(spec, artifact.path),
    loaded,
    worklist,
  );
  await persistTaskListDocument(taskListContextFromSpec(spec, artifact.path), merged);
  return loadTaskListDocument(taskListContextFromSpec(spec, artifact.path));
}

async function captureGitHeadSha(cwd: string): Promise<string | undefined> {
  try {
    const { stdout } = await execFileAsync('git', ['rev-parse', 'HEAD'], {
      cwd,
      encoding: 'utf8',
    });
    const sha = stdout.trim();
    return sha.length > 0 ? sha : undefined;
  } catch {
    return undefined;
  }
}

async function gitResetHard(
  cwd: string,
  sha: string,
  itemId: string,
  finalStatus: WorkflowWorkItemStatus,
): Promise<void> {
  try {
    await execFileAsync('git', ['reset', '--hard', sha], { cwd, encoding: 'utf8' });
    // eslint-disable-next-line no-console
    console.log(
      `[worklist] reverted worktree to ${sha.slice(0, 8)} after ${finalStatus} of "${itemId}"`,
    );
  } catch (error) {
    // eslint-disable-next-line no-console
    console.warn(
      `[worklist] git reset --hard ${sha.slice(0, 8)} failed for "${itemId}":`,
      (error as Error).message,
    );
  }
}

function resolveArtifactAbsolutePath(spec: WorkspaceSpec, artifactPath: string): string {
  return resolveWorkspacePath(spec.cwd, artifactPath);
}

function mustFindArtifact(spec: WorkspaceSpec, artifactId: string) {
  const artifact = spec.artifacts?.find(value => value.id === artifactId);
  if (!artifact) {
    throw new Error(`Unknown workflow artifact: ${artifactId}`);
  }
  return artifact;
}

function resolveWorklistTaskListAbsolutePath(
  spec: WorkspaceSpec,
  node: WorkflowNodeSpec,
): string | undefined {
  const artifactId = node.worklistArtifactId ?? node.producesArtifacts?.[0];
  if (!artifactId) {
    return undefined;
  }
  const artifact = spec.artifacts?.find(value => value.id === artifactId);
  return artifact ? resolveArtifactAbsolutePath(spec, artifact.path) : undefined;
}

function resolveSharedLessonsAbsolutePath(
  spec: WorkspaceSpec,
  node: WorkflowNodeSpec,
): string | undefined {
  return node.sharedLessonsPath ? resolveWorkspacePath(spec.cwd, node.sharedLessonsPath) : undefined;
}

function resolveTaskCliCommand(
  spec: WorkspaceSpec,
  node: WorkflowNodeSpec,
): string | undefined {
  if (!node.taskCliPath) {
    return undefined;
  }
  return `node ${resolveWorkspacePath(spec.cwd, node.taskCliPath)}`;
}

async function loadWorkItemDocumentStatusOverride(
  spec: WorkspaceSpec,
  artifactId: string,
  itemId: string,
): Promise<WorkflowWorkItemStatus | undefined> {
  const loaded = await loadWorklistDocument(spec, artifactId);
  const item = loaded?.document.items.find(value => value.id === itemId);
  return item ? normalizeWorkItemStatus(item.status) : undefined;
}

function synchronizeRuntimeWorklist(
  worklist: WorkflowWorklistRuntimeState,
  document: WorkflowTaskListArtifact,
): void {
  if (document.mode) {
    worklist.mode = document.mode;
  }

  const completedItemIds: string[] = [];
  const failedItemIds: string[] = [];
  for (const item of document.items) {
    const previous = worklist.items[item.id];
    const status = previous?.status ?? normalizeWorkItemStatus(item.status);
    const attempts = Math.max(previous?.attempts ?? 0, item.attempts ?? 0);
    const nextState = {
      itemId: item.id,
      title: item.title,
      status,
      attempts,
      ...(item.maxAttempts !== undefined ? { maxAttempts: item.maxAttempts } : {}),
      ...(previous?.dispatchId ? { dispatchId: previous.dispatchId } : {}),
      ...(previous?.lastSummary ? { lastSummary: previous.lastSummary } : {}),
      ...(previous?.lastFeedback ? { lastFeedback: previous.lastFeedback } : {}),
      ...(previous?.evaluatorDispatchId ? { evaluatorDispatchId: previous.evaluatorDispatchId } : {}),
      ...(previous?.commitDispatchId ? { commitDispatchId: previous.commitDispatchId } : {}),
      ...(previous?.taskControl ? { taskControl: cloneTaskControlState(previous.taskControl) } : {}),
      updatedAt: new Date().toISOString(),
    };
    worklist.items[item.id] = nextState;
    if (status === 'completed') {
      completedItemIds.push(item.id);
    }
    if (status === 'failed' || status === 'discarded' || status === 'blocked') {
      failedItemIds.push(item.id);
    }
  }
  worklist.completedItemIds = completedItemIds;
  worklist.failedItemIds = failedItemIds;
  worklist.lastUpdatedAt = new Date().toISOString();
}

function selectNextReadyItem(
  document: WorkflowTaskListArtifact,
  worklist: WorkflowWorklistRuntimeState,
): WorkflowWorkItem | undefined {
  return document.items.find(item => {
    const runtimeItem = worklist.items[item.id];
    const status = runtimeItem?.status ?? normalizeWorkItemStatus(item.status);
    if (status !== 'pending') {
      return false;
    }

    const dependencies = item.dependsOn ?? [];
    return dependencies.every(dependencyId => worklist.completedItemIds.includes(dependencyId));
  });
}

function markWorkItemRunning(
  worklist: WorkflowWorklistRuntimeState,
  item: WorkflowWorkItem,
): void {
  const previous = worklist.items[item.id];
  worklist.items[item.id] = {
    itemId: item.id,
    title: item.title,
    status: 'running',
    attempts: (previous?.attempts ?? 0) + 1,
    ...(item.maxAttempts !== undefined ? { maxAttempts: item.maxAttempts } : {}),
    ...(previous?.dispatchId ? { dispatchId: previous.dispatchId } : {}),
    ...(previous?.lastSummary ? { lastSummary: previous.lastSummary } : {}),
    ...(previous?.lastFeedback ? { lastFeedback: previous.lastFeedback } : {}),
    ...(previous?.evaluatorDispatchId ? { evaluatorDispatchId: previous.evaluatorDispatchId } : {}),
    ...(previous?.commitDispatchId ? { commitDispatchId: previous.commitDispatchId } : {}),
    ...(previous?.taskControl ? { taskControl: cloneTaskControlState(previous.taskControl) } : {}),
    updatedAt: new Date().toISOString(),
  };
  worklist.lastUpdatedAt = new Date().toISOString();
}

function markWorkItemFromDispatch(
  worklist: WorkflowWorklistRuntimeState,
  item: WorkflowWorkItem,
  dispatch: TaskDispatch,
): WorkflowWorkItemStatus {
  const previous = worklist.items[item.id];
  const maxAttempts = item.maxAttempts ?? previous?.maxAttempts;
  const structuredStatus = resolveStructuredWorkItemStatus(dispatch);
  const finalStatus: WorkflowWorkItemStatus =
    structuredStatus ??
    (dispatch.status === 'completed'
      ? 'completed'
      : maxAttempts !== undefined && (previous?.attempts ?? 0) >= maxAttempts
        ? 'discarded'
        : 'failed');
  const mergedTaskControl = mergeTaskControlStates(previous?.taskControl, dispatch.taskControl);

  worklist.items[item.id] = {
    itemId: item.id,
    title: item.title,
    status: finalStatus,
    attempts: previous?.attempts ?? 1,
    ...(maxAttempts !== undefined ? { maxAttempts } : {}),
    dispatchId: dispatch.dispatchId,
    ...(dispatch.lastSummary ? { lastSummary: dispatch.lastSummary } : dispatch.resultText ? { lastSummary: summarizeResult(dispatch.resultText) } : {}),
    ...(previous?.lastFeedback ? { lastFeedback: previous.lastFeedback } : {}),
    ...(previous?.evaluatorDispatchId ? { evaluatorDispatchId: previous.evaluatorDispatchId } : {}),
    ...(previous?.commitDispatchId ? { commitDispatchId: previous.commitDispatchId } : {}),
    ...(mergedTaskControl ? { taskControl: mergedTaskControl } : {}),
    updatedAt: new Date().toISOString(),
  };

  if (finalStatus === 'completed') {
    worklist.completedItemIds = uniqueIds([...worklist.completedItemIds, item.id]);
    worklist.failedItemIds = worklist.failedItemIds.filter(value => value !== item.id);
  } else if (finalStatus === 'abandoned' || finalStatus === 'superseded') {
    worklist.failedItemIds = worklist.failedItemIds.filter(value => value !== item.id);
    worklist.completedItemIds = worklist.completedItemIds.filter(value => value !== item.id);
  } else {
    worklist.failedItemIds = uniqueIds([...worklist.failedItemIds, item.id]);
    worklist.completedItemIds = worklist.completedItemIds.filter(value => value !== item.id);
  }

  worklist.lastUpdatedAt = new Date().toISOString();
  return finalStatus;
}

function markWorkItemForRetryOrFailure(
  worklist: WorkflowWorklistRuntimeState,
  item: WorkflowWorkItem,
  dispatch: TaskDispatch,
  feedback: string,
  stage: 'evaluation' | 'commit' | 'qa',
  rejectionMode: 'retry' | 'fail' = 'retry',
): WorkflowWorkItemStatus {
  const previous = worklist.items[item.id];
  const attempts = previous?.attempts ?? Math.max(1, item.attempts ?? 1);
  const maxAttempts = item.maxAttempts ?? previous?.maxAttempts;
  const structuredStatus = resolveStructuredWorkItemStatus(dispatch);
  const finalStatus: WorkflowWorkItemStatus =
    structuredStatus === 'blocked'
      ? 'blocked'
      : rejectionMode === 'fail' || (maxAttempts !== undefined && attempts >= maxAttempts)
        ? 'failed'
        : 'pending';
  const mergedTaskControl = mergeTaskControlStates(previous?.taskControl, dispatch.taskControl);

  worklist.items[item.id] = {
    itemId: item.id,
    title: item.title,
    status: finalStatus,
    attempts,
    ...(maxAttempts !== undefined ? { maxAttempts } : {}),
    ...(previous?.dispatchId ? { dispatchId: previous.dispatchId } : {}),
    ...(stage === 'evaluation'
      ? { evaluatorDispatchId: dispatch.dispatchId }
      : stage === 'commit'
        ? { commitDispatchId: dispatch.dispatchId }
        : stage === 'qa'
          ? { qaDispatchId: dispatch.dispatchId }
          : {}),
    lastSummary: dispatch.lastSummary ?? summarizeResult(dispatch.resultText ?? feedback),
    lastFeedback: feedback,
    ...(stage !== 'evaluation' && previous?.evaluatorDispatchId
      ? { evaluatorDispatchId: previous.evaluatorDispatchId }
      : {}),
    ...(stage !== 'commit' && previous?.commitDispatchId
      ? { commitDispatchId: previous.commitDispatchId }
      : {}),
    ...(stage !== 'qa' && previous?.qaDispatchId
      ? { qaDispatchId: previous.qaDispatchId }
      : {}),
    ...(mergedTaskControl ? { taskControl: mergedTaskControl } : {}),
    updatedAt: new Date().toISOString(),
  };

  if (finalStatus === 'pending') {
    worklist.failedItemIds = worklist.failedItemIds.filter(value => value !== item.id);
    worklist.completedItemIds = worklist.completedItemIds.filter(value => value !== item.id);
  } else {
    worklist.failedItemIds = uniqueIds([...worklist.failedItemIds, item.id]);
    worklist.completedItemIds = worklist.completedItemIds.filter(value => value !== item.id);
  }
  worklist.lastUpdatedAt = new Date().toISOString();
  return finalStatus;
}

function markWorkItemAbandonedOrSuperseded(
  worklist: WorkflowWorklistRuntimeState,
  item: WorkflowWorkItem,
  dispatch: TaskDispatch,
  finalStatus: 'abandoned' | 'superseded',
): WorkflowWorkItemStatus {
  const previous = worklist.items[item.id];
  const mergedTaskControl = mergeTaskControlStates(previous?.taskControl, dispatch.taskControl);
  worklist.items[item.id] = {
    itemId: item.id,
    title: item.title,
    status: finalStatus,
    attempts: previous?.attempts ?? Math.max(1, item.attempts ?? 1),
    ...(item.maxAttempts !== undefined ? { maxAttempts: item.maxAttempts } : {}),
    dispatchId: dispatch.dispatchId,
    ...(dispatch.lastSummary
      ? { lastSummary: dispatch.lastSummary }
      : dispatch.resultText
        ? { lastSummary: summarizeResult(dispatch.resultText) }
        : {}),
    ...(previous?.lastFeedback ? { lastFeedback: previous.lastFeedback } : {}),
    ...(previous?.evaluatorDispatchId ? { evaluatorDispatchId: previous.evaluatorDispatchId } : {}),
    ...(previous?.commitDispatchId ? { commitDispatchId: previous.commitDispatchId } : {}),
    ...(mergedTaskControl ? { taskControl: mergedTaskControl } : {}),
    updatedAt: new Date().toISOString(),
  };
  worklist.failedItemIds = worklist.failedItemIds.filter(value => value !== item.id);
  worklist.completedItemIds = worklist.completedItemIds.filter(value => value !== item.id);
  worklist.lastUpdatedAt = new Date().toISOString();
  return finalStatus;
}

function extractWorkItemFeedback(dispatch: TaskDispatch): string {
  return (
    extractTaskControlFeedback(dispatch) ??
    dispatch.resultText?.trim() ??
    dispatch.lastSummary?.trim() ??
    'Task requires changes.'
  );
}

function updateDocumentItemStatus(
  document: WorkflowTaskListArtifact,
  itemId: string,
  status: WorkflowWorkItemStatus,
  attempts: number,
): void {
  const item = document.items.find(value => value.id === itemId);
  if (!item) {
    return;
  }
  item.status = status;
  item.attempts = attempts;
}

function hasBlockingFailure(
  document: WorkflowTaskListArtifact,
  worklist: WorkflowWorklistRuntimeState,
): boolean {
  return document.items.some(item => {
    const runtimeItem = worklist.items[item.id];
    const status = runtimeItem?.status ?? normalizeWorkItemStatus(item.status);
    return status === 'failed' || status === 'discarded' || status === 'blocked';
  });
}

function countPendingItems(worklist: WorkflowWorklistRuntimeState): number {
  return Object.values(worklist.items).filter(item => item.status === 'pending').length;
}

function buildDefaultCommitMessage(item: WorkflowWorkItem): string {
  return `task(${item.id}): ${item.title}`;
}

function mustFindRole(spec: WorkspaceSpec, roleId: string): RoleSpec {
  const role = spec.roles.find(value => value.id === roleId);
  if (!role) {
    throw new Error(`Unknown workflow role: ${roleId}`);
  }
  return role;
}

function inferDispatchOutcome(
  dispatch: TaskDispatch,
  availableConditions: WorkflowEdgeCondition[],
): WorkflowEdgeCondition {
  return inferWorkflowOutcome(
    {
      id: 'synthetic-review',
      type: 'review',
    },
    dispatch,
    availableConditions,
  );
}

function uniqueIds(values: string[]): string[] {
  return Array.from(new Set(values));
}

function summarizeResult(resultText: string): string {
  return resultText.trim().split(/\r?\n/).find(Boolean)?.slice(0, 240) ?? resultText.slice(0, 240);
}

function resolveCompletionStatus(
  spec: WorkspaceSpec,
  nodeId: string,
  outcome: WorkflowEdgeCondition,
): CompletionStatus | undefined {
  if (spec.completionPolicy?.successNodeIds?.includes(nodeId)) {
    return 'done';
  }
  if (spec.completionPolicy?.failureNodeIds?.includes(nodeId)) {
    return outcome === 'failure' || outcome === 'fail' ? 'crash' : 'discarded';
  }
  return undefined;
}

function resolveNextWorkflowNodeId(
  workflow: WorkflowSpec,
  nodeId: string,
  outcome: WorkflowEdgeCondition,
): string | undefined {
  const outgoing = workflow.edges.filter(edge => edge.from === nodeId);
  const exact = outgoing.find(edge => edge.when === outcome);
  if (exact) {
    return exact.to;
  }

  if (
    outcome !== 'success' &&
    ['pass', 'approved', 'improved'].includes(outcome) &&
    outgoing.some(edge => edge.when === 'success')
  ) {
    return outgoing.find(edge => edge.when === 'success')?.to;
  }

  if (
    outcome !== 'failure' &&
    ['fail', 'rejected', 'equal_or_worse', 'crash', 'timeout'].includes(outcome) &&
    outgoing.some(edge => edge.when === 'failure')
  ) {
    return outgoing.find(edge => edge.when === 'failure')?.to;
  }

  return outgoing.find(edge => edge.when === 'always')?.to;
}

function inferWorkflowOutcome(
  node: WorkflowNodeSpec,
  dispatch: TaskDispatch | undefined,
  availableConditions: WorkflowEdgeCondition[],
): WorkflowEdgeCondition {
  if (!dispatch) {
    return availableConditions.includes('success') ? 'success' : 'always';
  }

  if (dispatch.status === 'failed') {
    return pickFailureCondition(availableConditions);
  }

  if (dispatch.status === 'stopped') {
    return availableConditions.includes('timeout')
      ? 'timeout'
      : pickFailureCondition(availableConditions);
  }

  const structuredOutcome = inferStructuredOutcome(dispatch, availableConditions);
  if (structuredOutcome) {
    return structuredOutcome;
  }

  const text = `${dispatch.resultText ?? ''}\n${dispatch.lastSummary ?? ''}`.toLowerCase();
  const positive = scoreText(text, POSITIVE_PATTERNS);
  const negative = scoreText(text, NEGATIVE_PATTERNS);

  if (availableConditions.includes('approved') || availableConditions.includes('rejected')) {
    if (negative > positive) {
      return availableConditions.includes('rejected')
        ? 'rejected'
        : pickFailureCondition(availableConditions);
    }
    return availableConditions.includes('approved') ? 'approved' : 'success';
  }

  if (availableConditions.includes('pass') || availableConditions.includes('fail')) {
    if (negative > positive) {
      return availableConditions.includes('fail')
        ? 'fail'
        : pickFailureCondition(availableConditions);
    }
    return availableConditions.includes('pass') ? 'pass' : 'success';
  }

  if (availableConditions.includes('improved') || availableConditions.includes('equal_or_worse')) {
    if (negative > positive) {
      return availableConditions.includes('equal_or_worse')
        ? 'equal_or_worse'
        : pickFailureCondition(availableConditions);
    }
    return availableConditions.includes('improved') ? 'improved' : 'success';
  }

  if (node.type === 'review' && negative > positive) {
    return availableConditions.includes('rejected')
      ? 'rejected'
      : pickFailureCondition(availableConditions);
  }

  return availableConditions.includes('success') ? 'success' : 'always';
}

function pickFailureCondition(
  availableConditions: WorkflowEdgeCondition[],
): WorkflowEdgeCondition {
  for (const candidate of [
    'failure',
    'fail',
    'rejected',
    'equal_or_worse',
    'crash',
    'timeout',
    'exhausted',
  ] as const) {
    if (availableConditions.includes(candidate)) {
      return candidate;
    }
  }
  return availableConditions.includes('success') ? 'success' : 'always';
}

function scoreText(text: string, patterns: RegExp[]): number {
  return patterns.reduce((score, pattern) => score + (pattern.test(text) ? 1 : 0), 0);
}

const POSITIVE_PATTERNS = [
  /\bapprove(d)?\b/,
  /\blgtm\b/,
  /\bship it\b/,
  /\bpass(ed)?\b/,
  /\bsuccess(ful)?\b/,
  /\blooks good\b/,
  /\bimprov(ed|ement)?\b/,
];

const NEGATIVE_PATTERNS = [
  /\breject(ed|ion)?\b/,
  /\bchanges requested\b/,
  /\bneeds changes\b/,
  /\bfail(ed|ure)?\b/,
  /\berror\b/,
  /\bregression\b/,
  /\bblock(ed|er)?\b/,
  /\bworse\b/,
];
