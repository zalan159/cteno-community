import type {
  TaskControlEvidenceRecord,
  TaskControlState,
  TaskControlStatus,
  TaskControlToolCall,
  TaskControlToolName,
  TaskDispatch,
  TaskRecordEvidenceInput,
  TaskSetStatusInput,
  TaskSubmitReviewInput,
  TaskWriteHandoffInput,
  WorkflowEdgeCondition,
  WorkflowWorkItemStatus,
} from './types.js';

export interface TaskControlToolSpec {
  name: TaskControlToolName;
  description: string;
  inputSchema: Record<string, unknown>;
}

export interface ApplyTaskControlOptions {
  actorRoleId?: string;
  now?: () => string;
}

export const TASK_CONTROL_TOOL_NAMES: TaskControlToolName[] = [
  'task.set_status',
  'task.write_handoff',
  'task.submit_review',
  'task.record_evidence',
];

export const TASK_CONTROL_TOOL_SPECS: TaskControlToolSpec[] = [
  {
    name: 'task.set_status',
    description: 'Update the structured runtime status for a workflow task or work item.',
    inputSchema: {
      type: 'object',
      additionalProperties: false,
      properties: {
        taskId: { type: 'string' },
        status: {
          type: 'string',
          enum: ['pending', 'running', 'completed', 'failed', 'blocked', 'retry_requested'],
        },
        summary: { type: 'string' },
        reason: { type: 'string' },
        metadata: { type: 'object' },
      },
      required: ['status'],
    },
  },
  {
    name: 'task.write_handoff',
    description: 'Write a structured handoff from the current agent to the next workflow stage.',
    inputSchema: {
      type: 'object',
      additionalProperties: false,
      properties: {
        taskId: { type: 'string' },
        summary: { type: 'string' },
        details: { type: 'string' },
        toRoleId: { type: 'string' },
        metadata: { type: 'object' },
      },
      required: ['summary'],
    },
  },
  {
    name: 'task.submit_review',
    description: 'Submit a structured review verdict for a workflow task.',
    inputSchema: {
      type: 'object',
      additionalProperties: false,
      properties: {
        taskId: { type: 'string' },
        verdict: { type: 'string', enum: ['approved', 'rejected'] },
        summary: { type: 'string' },
        issues: { type: 'array', items: { type: 'string' } },
        retryable: { type: 'boolean' },
        metadata: { type: 'object' },
      },
      required: ['verdict', 'summary'],
    },
  },
  {
    name: 'task.record_evidence',
    description: 'Attach structured evidence for a workflow task, such as tests, diffs, or logs.',
    inputSchema: {
      type: 'object',
      additionalProperties: false,
      properties: {
        taskId: { type: 'string' },
        evidenceId: { type: 'string' },
        kind: { type: 'string', enum: ['note', 'log', 'command', 'test', 'diff', 'file', 'report'] },
        summary: { type: 'string' },
        path: { type: 'string' },
        content: { type: 'string' },
        metadata: { type: 'object' },
      },
      required: ['kind', 'summary'],
    },
  },
];

export function supportsTaskControlTool(name: string): name is TaskControlToolName {
  return TASK_CONTROL_TOOL_NAMES.includes(name as TaskControlToolName);
}

export function applyTaskControlToolCall(
  dispatch: TaskDispatch,
  call: TaskControlToolCall,
  options: ApplyTaskControlOptions = {},
): TaskDispatch {
  switch (call.toolName) {
    case 'task.set_status':
      return applyTaskStatusUpdate(dispatch, call.input, options);
    case 'task.write_handoff':
      return applyTaskHandoff(dispatch, call.input, options);
    case 'task.submit_review':
      return applyTaskReview(dispatch, call.input, options);
    case 'task.record_evidence':
      return applyTaskEvidence(dispatch, call.input, options);
    default:
      return dispatch;
  }
}

export function applyTaskStatusUpdate(
  dispatch: TaskDispatch,
  input: TaskSetStatusInput,
  options: ApplyTaskControlOptions = {},
): TaskDispatch {
  const taskId = resolveTaskId(dispatch, input.taskId);
  const updatedAt = resolveNow(options);
  return {
    ...dispatch,
    taskControl: {
      ...cloneTaskControlState(dispatch.taskControl),
      latestStatus: {
        taskId,
        status: input.status,
        updatedAt,
        ...(input.summary ? { summary: input.summary } : {}),
        ...(input.reason ? { reason: input.reason } : {}),
        ...(options.actorRoleId ? { actorRoleId: options.actorRoleId } : {}),
        ...(input.metadata ? { metadata: input.metadata } : {}),
      },
    },
  };
}

export function applyTaskHandoff(
  dispatch: TaskDispatch,
  input: TaskWriteHandoffInput,
  options: ApplyTaskControlOptions = {},
): TaskDispatch {
  const taskId = resolveTaskId(dispatch, input.taskId);
  const updatedAt = resolveNow(options);
  return {
    ...dispatch,
    taskControl: {
      ...cloneTaskControlState(dispatch.taskControl),
      latestHandoff: {
        taskId,
        summary: input.summary,
        updatedAt,
        ...(input.details ? { details: input.details } : {}),
        ...(options.actorRoleId ? { fromRoleId: options.actorRoleId } : {}),
        ...(input.toRoleId ? { toRoleId: input.toRoleId } : {}),
        ...(input.metadata ? { metadata: input.metadata } : {}),
      },
    },
  };
}

export function applyTaskReview(
  dispatch: TaskDispatch,
  input: TaskSubmitReviewInput,
  options: ApplyTaskControlOptions = {},
): TaskDispatch {
  const taskId = resolveTaskId(dispatch, input.taskId);
  const updatedAt = resolveNow(options);
  return {
    ...dispatch,
    taskControl: {
      ...cloneTaskControlState(dispatch.taskControl),
      latestReview: {
        taskId,
        verdict: input.verdict,
        summary: input.summary,
        updatedAt,
        ...(input.issues?.length ? { issues: [...input.issues] } : {}),
        ...(input.retryable !== undefined ? { retryable: input.retryable } : {}),
        ...(options.actorRoleId ? { actorRoleId: options.actorRoleId } : {}),
        ...(input.metadata ? { metadata: input.metadata } : {}),
      },
    },
  };
}

export function applyTaskEvidence(
  dispatch: TaskDispatch,
  input: TaskRecordEvidenceInput,
  options: ApplyTaskControlOptions = {},
): TaskDispatch {
  const taskId = resolveTaskId(dispatch, input.taskId);
  const recordedAt = resolveNow(options);
  const previous = cloneTaskControlState(dispatch.taskControl);
  const evidenceId = input.evidenceId ?? `${taskId}:evidence:${previous.evidence.length + 1}`;
  const evidence: TaskControlEvidenceRecord = {
    taskId,
    evidenceId,
    kind: input.kind,
    summary: input.summary,
    recordedAt,
    ...(input.path ? { path: input.path } : {}),
    ...(input.content ? { content: input.content } : {}),
    ...(options.actorRoleId ? { actorRoleId: options.actorRoleId } : {}),
    ...(input.metadata ? { metadata: input.metadata } : {}),
  };

  return {
    ...dispatch,
    taskControl: {
      ...previous,
      evidence: mergeEvidence(previous.evidence, [evidence]),
    },
  };
}

export function cloneTaskControlState(
  state: TaskControlState | undefined,
): TaskControlState {
  return {
    ...(state?.latestStatus ? { latestStatus: { ...state.latestStatus } } : {}),
    ...(state?.latestHandoff ? { latestHandoff: { ...state.latestHandoff } } : {}),
    ...(state?.latestReview
      ? {
          latestReview: {
            ...state.latestReview,
            ...(state.latestReview.issues ? { issues: [...state.latestReview.issues] } : {}),
          },
        }
      : {}),
    evidence: (state?.evidence ?? []).map(entry => ({ ...entry })),
  };
}

export function mergeTaskControlStates(
  base: TaskControlState | undefined,
  incoming: TaskControlState | undefined,
): TaskControlState | undefined {
  if (!base && !incoming) {
    return undefined;
  }

  const left = cloneTaskControlState(base);
  const right = cloneTaskControlState(incoming);
  return {
    ...(left.latestStatus || right.latestStatus
      ? { latestStatus: right.latestStatus ?? left.latestStatus }
      : {}),
    ...(left.latestHandoff || right.latestHandoff
      ? { latestHandoff: right.latestHandoff ?? left.latestHandoff }
      : {}),
    ...(left.latestReview || right.latestReview
      ? { latestReview: right.latestReview ?? left.latestReview }
      : {}),
    evidence: mergeEvidence(left.evidence, right.evidence),
  };
}

export function inferStructuredOutcome(
  dispatch: TaskDispatch | undefined,
  availableConditions: WorkflowEdgeCondition[],
): WorkflowEdgeCondition | undefined {
  const reviewVerdict = dispatch?.taskControl?.latestReview?.verdict;
  if (reviewVerdict) {
    if (reviewVerdict === 'approved' && availableConditions.includes('approved')) {
      return 'approved';
    }
    if (reviewVerdict === 'rejected' && availableConditions.includes('rejected')) {
      return 'rejected';
    }
  }

  const status = dispatch?.taskControl?.latestStatus?.status;
  if (!status) {
    return undefined;
  }

  if (
    status === 'completed' &&
    (availableConditions.includes('pass') || availableConditions.includes('success'))
  ) {
    return availableConditions.includes('pass') ? 'pass' : 'success';
  }

  if (
    (status === 'failed' || status === 'blocked') &&
    (availableConditions.includes('fail') || availableConditions.includes('failure'))
  ) {
    return availableConditions.includes('fail') ? 'fail' : 'failure';
  }

  return undefined;
}

export function resolveStructuredWorkItemStatus(
  dispatch: TaskDispatch | undefined,
): WorkflowWorkItemStatus | undefined {
  const status = dispatch?.taskControl?.latestStatus?.status;
  switch (status) {
    case 'completed':
      return 'completed';
    case 'failed':
      return 'failed';
    case 'blocked':
      return 'blocked';
    case 'retry_requested':
    case 'pending':
    case 'running':
      return 'pending';
    default:
      return undefined;
  }
}

export function extractTaskControlFeedback(dispatch: TaskDispatch | undefined): string | undefined {
  const review = dispatch?.taskControl?.latestReview;
  if (review) {
    if (review.issues?.length) {
      return [review.summary, ...review.issues].join('\n');
    }
    return review.summary;
  }

  const status = dispatch?.taskControl?.latestStatus;
  if (status?.reason) {
    return status.reason;
  }
  if (status?.summary) {
    return status.summary;
  }

  const handoff = dispatch?.taskControl?.latestHandoff;
  if (handoff?.details) {
    return handoff.details;
  }
  if (handoff?.summary) {
    return handoff.summary;
  }

  return undefined;
}

function resolveTaskId(dispatch: TaskDispatch, providedTaskId: string | undefined): string {
  const taskId = providedTaskId ?? dispatch.workItemId;
  if (!taskId) {
    throw new Error('Task control tools require a task id or dispatch workItemId.');
  }
  if (dispatch.workItemId && providedTaskId && providedTaskId !== dispatch.workItemId) {
    throw new Error(
      `Task control update targets "${providedTaskId}" but dispatch is scoped to "${dispatch.workItemId}".`,
    );
  }
  return taskId;
}

function mergeEvidence(
  left: TaskControlEvidenceRecord[],
  right: TaskControlEvidenceRecord[],
): TaskControlEvidenceRecord[] {
  const byId = new Map<string, TaskControlEvidenceRecord>();
  for (const entry of [...left, ...right]) {
    byId.set(entry.evidenceId, { ...entry });
  }
  return Array.from(byId.values());
}

function resolveNow(options: ApplyTaskControlOptions): string {
  return options.now?.() ?? new Date().toISOString();
}
