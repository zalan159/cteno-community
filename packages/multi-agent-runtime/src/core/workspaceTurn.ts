import type {
  ClaimDecision,
  CoordinatorWorkflowDecision,
  RoleSpec,
  WorkflowNodeSpec,
  WorkflowVoteDecision,
  WorkflowNodeType,
  WorkspaceClaimResponse,
  WorkspaceSpec,
  WorkspaceTurnAssignment,
  WorkspaceTurnPlan,
  WorkspaceTurnRequest,
  WorkspaceVisibility,
  WorkspaceWorkflowVoteResponse,
} from './types.js';
import { resolveWorkflowNodeModel, resolveWorkflowNodeProvider } from './providerResolution.js';

export function buildCoordinatorDecisionPrompt(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
): string {
  const coordinatorRoleId = spec.coordinatorRoleId ?? spec.defaultRoleId ?? spec.roles[0]?.id;
  if (!coordinatorRoleId) {
    throw new Error('Workspace has no coordinator role.');
  }

  const maxAssignments = Math.max(
    1,
    request.maxAssignments ?? spec.claimPolicy?.maxAssignees ?? 1,
  );

  const roleLines = spec.roles.map(role => describeRole(role)).join('\n');
  const fallbackRoleId =
    spec.claimPolicy?.fallbackRoleId ?? spec.defaultRoleId ?? coordinatorRoleId;

  const preferredRoleLine = request.preferRoleId
    ? `Bias toward @${request.preferRoleId} if it is a strong fit, but do not force it if another role is clearly better.`
    : 'Choose the best role or small set of roles based on the actual work required.';

  return [
    `You are @${coordinatorRoleId}, the coordinator for the workspace "${spec.name}".`,
    'A user just sent one workspace-level message. Treat it as visible to the whole team.',
    'Your job is to choose exactly one of three paths:',
    '1. `respond`: you answer directly without delegating and without starting workflow.',
    '2. `delegate`: you direct one specialist role to take this next, still staying in group-chat mode.',
    '3. `propose_workflow`: you think this needs programmatic workflow execution, so you ask the team to vote on entering workflow mode.',
    preferredRoleLine,
    `You may assign at most ${maxAssignments} role(s).`,
    `Fallback role if no specialist clearly fits: @${fallbackRoleId}.`,
    'Return strict JSON only. Do not wrap it in markdown fences. Do not add prose before or after the JSON.',
    'JSON schema:',
    JSON.stringify(
      {
        kind: 'respond | delegate | propose_workflow',
        responseText: 'A short public coordinator response.',
        targetRoleId:
          'one valid workspace role id when kind=delegate, otherwise empty string',
        workflowVoteReason:
          'short reason for entering workflow mode when kind=propose_workflow, otherwise empty string',
        rationale: 'One short sentence explaining the routing decision.',
      },
      null,
      2,
    ),
    'Available roles:',
    roleLines,
    'User workspace message:',
    request.message,
    'Rules:',
    '- Use only valid roleId values from the available roles. Return bare ids like `prd`, not `@prd`.',
    '- Prefer `respond` or `delegate` by default.',
    '- Use `propose_workflow` only when the task clearly needs staged programmatic flow, loops, gates, or formal deliverable progression.',
    '- If kind=`delegate`, choose one specialist role instead of opening workflow vote.',
    '- Keep responseText concise and public-facing.',
  ].join('\n\n');
}

export function buildWorkspaceTurnPrompt(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
): string {
  return buildCoordinatorDecisionPrompt(spec, request);
}

export function parseCoordinatorDecision(
  rawText: string,
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
): CoordinatorWorkflowDecision {
  const coordinatorRoleId = spec.coordinatorRoleId ?? spec.defaultRoleId ?? spec.roles[0]?.id;
  if (!coordinatorRoleId) {
    throw new Error('Workspace has no coordinator role.');
  }

  const parsed = extractJsonObject(rawText);
  const kind =
    parsed?.kind === 'respond' || parsed?.kind === 'delegate' || parsed?.kind === 'propose_workflow'
      ? parsed.kind
      : 'delegate';
  const targetRoleId =
    typeof parsed?.targetRoleId === 'string' && spec.roles.some(role => role.id === parsed.targetRoleId)
      ? parsed.targetRoleId
      : undefined;
  const workflowVoteReason =
    typeof parsed?.workflowVoteReason === 'string' && parsed.workflowVoteReason.trim().length > 0
      ? parsed.workflowVoteReason.trim()
      : undefined;
  const responseText =
    typeof parsed?.responseText === 'string' && parsed.responseText.trim().length > 0
      ? parsed.responseText.trim()
      : kind === 'respond'
        ? `@${coordinatorRoleId} will handle this directly.`
        : kind === 'propose_workflow'
          ? `@${coordinatorRoleId} proposes entering workflow mode for this request.`
          : `@${targetRoleId ?? coordinatorRoleId} will take this next.`;
  const normalizedKind =
    kind === 'propose_workflow' && spec.workflow
      ? 'propose_workflow'
      : shouldProposeWorkflowHeuristically(spec, request.message)
        ? 'propose_workflow'
        : kind === 'delegate' && targetRoleId
          ? 'delegate'
          : 'respond';

  return {
    kind: normalizedKind,
    responseText,
    ...(targetRoleId ? { targetRoleId } : {}),
    ...((workflowVoteReason || normalizedKind === 'propose_workflow')
      ? {
          workflowVoteReason:
            workflowVoteReason ??
            'This request appears to need staged workflow execution with formal flow control.',
        }
      : {}),
    ...(typeof parsed?.rationale === 'string' && parsed.rationale.trim().length > 0
      ? { rationale: parsed.rationale.trim() }
      : {}),
  };
}

export function buildWorkspaceClaimPrompt(
  spec: WorkspaceSpec,
  role: RoleSpec,
  request: WorkspaceTurnRequest,
): string {
  return [
    `You are @${role.id} (${role.name}) in the workspace "${spec.name}".`,
    role.description ? `Role description: ${role.description}` : null,
    role.agent.description ? `Agent description: ${role.agent.description}` : null,
    role.outputRoot ? `Preferred output root: ${role.outputRoot}` : null,
    'A new workspace message is visible to the whole team.',
    'Your job right now is only to decide whether you should claim this task, support another owner, or decline.',
    'Do not perform the work yet. Do not use tools. Do not write files. Do not answer with prose outside JSON.',
    'Return strict JSON with this schema:',
    JSON.stringify(
      {
        decision: 'claim | support | decline',
        confidence: 0.0,
        rationale: 'one short sentence explaining the decision',
        publicResponse:
          'short public update that can appear in the workspace timeline, or an empty string if none',
        proposedInstruction:
          'concrete next step you would execute if chosen as owner or supporter, or an empty string if none',
      },
      null,
      2,
    ),
    'Decision rules:',
    '- Use `claim` when you should be a primary owner for this request.',
    '- Use `support` when you can contribute meaningfully but should not be the main owner.',
    '- Use `decline` when another role is a better fit or the task is outside your lane.',
    '- Set confidence between 0 and 1.',
    '- Keep rationale and publicResponse concise.',
    '- Always include every JSON field. Use an empty string for publicResponse or proposedInstruction when you have nothing useful to add.',
    'Workspace message:',
    request.message,
  ]
    .filter(Boolean)
    .join('\n\n');
}

export function buildWorkflowVotePrompt(
  spec: WorkspaceSpec,
  role: RoleSpec,
  request: WorkspaceTurnRequest,
  coordinatorDecision: CoordinatorWorkflowDecision,
): string {
  return [
    `You are @${role.id} (${role.name}) in the workspace "${spec.name}".`,
    role.description ? `Role description: ${role.description}` : null,
    role.agent.description ? `Agent description: ${role.agent.description}` : null,
    'The coordinator is considering switching this request from normal group-chat handling into workflow mode.',
    `Coordinator public message: ${coordinatorDecision.responseText}`,
    coordinatorDecision.workflowVoteReason
      ? `Workflow proposal reason: ${coordinatorDecision.workflowVoteReason}`
      : null,
    'Your job is only to vote on whether this request should enter workflow mode now.',
    'Return strict JSON only. Do not use tools. Do not do the work yet.',
    'Return strict JSON with this schema:',
    JSON.stringify(
      {
        decision: 'approve | reject | abstain',
        confidence: 0.0,
        rationale: 'one short sentence explaining your vote',
        publicResponse: 'short public update for the workspace timeline, or an empty string',
      },
      null,
      2,
    ),
    'Decision rules:',
    '- Use `approve` when formal workflow execution is the best next step.',
    '- Use `reject` when normal group-chat delegation is enough.',
    '- Use `abstain` only when you genuinely cannot judge whether workflow mode is warranted.',
    '- If the request clearly needs staged execution, loops, gates, review, or keep/discard control, do not abstain.',
    role.id === (spec.coordinatorRoleId ?? spec.defaultRoleId)
      ? '- You are the coordinator who proposed workflow mode. Do not abstain unless you explicitly changed your mind.'
      : null,
    '- Keep rationale and publicResponse concise.',
    'Workspace message:',
    request.message,
  ]
    .filter(Boolean)
    .join('\n\n');
}

export function parseWorkspaceClaimResponse(
  rawInput: unknown,
  role: RoleSpec,
  request: WorkspaceTurnRequest,
): WorkspaceClaimResponse {
  const parsed = extractClaimPayload(rawInput);
  const decision = normalizeDecision(parsed?.decision, role, request.message);
  const confidence = normalizeConfidence(parsed?.confidence, decision);
  const rationale = normalizeText(parsed?.rationale) ?? fallbackRationale(role, decision, request.message);
  const publicResponse = normalizeText(parsed?.publicResponse);
  const proposedInstruction = normalizeText(parsed?.proposedInstruction);

  return {
    roleId: role.id,
    decision,
    confidence,
    rationale,
    ...(publicResponse ? { publicResponse } : {}),
    ...(proposedInstruction ? { proposedInstruction } : {}),
  };
}

export function parseWorkflowVoteResponse(
  rawInput: unknown,
  role: RoleSpec,
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  coordinatorDecision: CoordinatorWorkflowDecision,
): WorkspaceWorkflowVoteResponse {
  const parsed = extractClaimPayload(rawInput);
  const rawDecision = typeof parsed?.decision === 'string' ? parsed.decision.trim().toLowerCase() : '';
  const normalizedDecision =
    rawDecision === 'approve' || rawDecision === 'reject' || rawDecision === 'abstain'
      ? rawDecision
      : 'abstain';
  const decision = normalizeWorkflowVoteDecision(
    normalizedDecision,
    role,
    spec,
    request,
    coordinatorDecision,
  );
  const confidence = normalizeConfidence(parsed?.confidence, decision === 'approve' ? 'claim' : 'decline');
  const rationale =
    normalizeText(parsed?.rationale) ?? `@${role.id} voted ${decision} on entering workflow mode.`;
  const publicResponse =
    normalizeText(parsed?.publicResponse) ??
    (decision === 'approve'
      ? `@${role.id} approves entering workflow mode.`
      : decision === 'reject'
        ? `@${role.id} prefers to stay in group-chat mode.`
        : undefined);

  return {
    roleId: role.id,
    decision,
    confidence,
    rationale,
    ...(publicResponse ? { publicResponse } : {}),
  };
}

export function buildPlanFromClaimResponses(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  responses: WorkspaceClaimResponse[],
): WorkspaceTurnPlan {
  const coordinatorRoleId = spec.coordinatorRoleId ?? spec.defaultRoleId ?? spec.roles[0]?.id;
  if (!coordinatorRoleId) {
    throw new Error('Workspace has no coordinator role.');
  }

  const maxAssignments = Math.max(
    1,
    request.maxAssignments ?? spec.claimPolicy?.maxAssignees ?? 1,
  );
  const fallbackRoleId =
    spec.claimPolicy?.fallbackRoleId ?? spec.defaultRoleId ?? coordinatorRoleId;
  const fallbackAssignment = buildFallbackAssignment(
    spec,
    guessFallbackRoleId(spec, request.message, fallbackRoleId),
    request.message,
  );
  const rolesById = new Map(spec.roles.map(role => [role.id, role]));

  const claims = responses
    .filter(response => response.decision === 'claim')
    .sort((left, right) =>
      compareClaimCandidates(left, right, rolesById, coordinatorRoleId, request.message),
    );
  const supports = responses
    .filter(response => response.decision === 'support')
    .sort((left, right) =>
      compareClaimCandidates(left, right, rolesById, coordinatorRoleId, request.message),
    );

  const assignments: WorkspaceTurnAssignment[] = [];
  for (const response of claims.slice(0, maxAssignments)) {
    assignments.push(
      buildAssignmentFromClaimResponse(spec, request, response, 'public'),
    );
  }

  if (assignments.length < maxAssignments && spec.claimPolicy?.allowSupportingClaims) {
    for (const response of supports) {
      if (assignments.length >= maxAssignments) {
        break;
      }
      assignments.push(
        buildAssignmentFromClaimResponse(spec, request, response, 'public'),
      );
    }
  }

  const finalAssignments = assignments.length > 0 ? uniqueAssignments(assignments) : [fallbackAssignment];
  const selectedResponses = responses.filter(response =>
    finalAssignments.some(assignment => assignment.roleId === response.roleId),
  );

  return {
    coordinatorRoleId,
    responseText: buildClaimResponseText(finalAssignments, selectedResponses, spec.roles),
    assignments: finalAssignments,
    rationale:
      selectedResponses.length > 0
        ? 'Assignments were resolved from member claim/support responses.'
        : 'No member claimed the task, so runtime fell back to heuristic routing.',
  };
}

export function parseWorkspaceTurnPlan(
  rawText: string,
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
): WorkspaceTurnPlan {
  const coordinatorRoleId = spec.coordinatorRoleId ?? spec.defaultRoleId ?? spec.roles[0]?.id;
  if (!coordinatorRoleId) {
    throw new Error('Workspace has no coordinator role.');
  }

  const parsed = extractJsonObject(rawText);
  const fallbackRoleId =
    spec.claimPolicy?.fallbackRoleId ?? spec.defaultRoleId ?? coordinatorRoleId;
  const maxAssignments = Math.max(
    1,
    request.maxAssignments ?? spec.claimPolicy?.maxAssignees ?? 1,
  );

  const allowedRoleIds = new Set(spec.roles.map(role => role.id));
  const assignmentsInput = Array.isArray(parsed?.assignments) ? parsed.assignments : [];
  const assignments = assignmentsInput
    .flatMap(value => normalizeAssignment(value, request))
    .filter(assignment => allowedRoleIds.has(assignment.roleId))
    .slice(0, maxAssignments);

  const finalAssignments = assignments.length > 0
    ? assignments
    : [buildFallbackAssignment(spec, guessFallbackRoleId(spec, request.message, fallbackRoleId), request.message)];

  const responseText =
    typeof parsed?.responseText === 'string' && parsed.responseText.trim().length > 0
      ? parsed.responseText.trim()
      : buildFallbackResponseText(finalAssignments, spec.roles);

  return {
    coordinatorRoleId,
    responseText,
    assignments: finalAssignments,
    ...(typeof parsed?.rationale === 'string' && parsed.rationale.trim().length > 0
      ? { rationale: parsed.rationale.trim() }
      : {}),
  };
}

export function planWorkspaceTurnHeuristically(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
): WorkspaceTurnPlan {
  const coordinatorRoleId = spec.coordinatorRoleId ?? spec.defaultRoleId ?? spec.roles[0]?.id;
  if (!coordinatorRoleId) {
    throw new Error('Workspace has no coordinator role.');
  }

  const fallbackRoleId =
    spec.claimPolicy?.fallbackRoleId ?? spec.defaultRoleId ?? coordinatorRoleId;
  const maxAssignments = Math.max(
    1,
    request.maxAssignments ?? spec.claimPolicy?.maxAssignees ?? 1,
  );

  const assignments = request.preferRoleId
    ? [buildFallbackAssignment(spec, request.preferRoleId, request.message)]
    : buildHeuristicAssignments(spec, request.message, fallbackRoleId, maxAssignments);

  return {
    coordinatorRoleId,
    responseText: buildFallbackResponseText(assignments, spec.roles),
    assignments: assignments.map(assignment => {
      const visibility =
        assignment.visibility ??
        request.visibility ??
        (spec.claimPolicy?.mode === 'claim' ? 'public' : undefined);
      return {
        ...assignment,
        ...(visibility ? { visibility } : {}),
      };
    }),
    rationale: 'Heuristic runtime routing selected claim candidates from role/message affinity.',
  };
}

export function buildWorkflowEntryPlan(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
): WorkspaceTurnPlan {
  const coordinatorRoleId = spec.coordinatorRoleId ?? spec.defaultRoleId ?? spec.roles[0]?.id;
  if (!coordinatorRoleId) {
    throw new Error('Workspace has no coordinator role.');
  }

  if (!spec.workflow) {
    return planWorkspaceTurnHeuristically(spec, request);
  }

  const entryNode = getWorkflowEntryNode(spec);
  if (!entryNode) {
    return planWorkspaceTurnHeuristically(spec, request);
  }

  const assignment = buildWorkflowDispatchAssignment(spec, request, entryNode);
  if (!assignment) {
    return {
      coordinatorRoleId,
      responseText: `@${coordinatorRoleId} opened workflow mode, but the entry node does not require a direct specialist dispatch yet.`,
      assignments: [],
      rationale: `Workflow mode entered at node ${entryNode.id}.`,
    };
  }

  return {
    coordinatorRoleId,
    responseText: `Workflow mode approved. Starting at "${entryNode.title ?? entryNode.id}" with @${assignment.roleId}.`,
    assignments: [assignment],
    rationale: `Workflow mode entered at node ${entryNode.id}.`,
  };
}

export function resolveClaimCandidateRoleIds(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
): string[] {
  const maxAssignments = Math.max(
    1,
    request.maxAssignments ?? spec.claimPolicy?.maxAssignees ?? 1,
  );
  const workflowCandidates = resolveWorkflowCandidates(spec, request.message);
  if (workflowCandidates.length === 0) {
    return spec.roles.map(role => role.id);
  }

  const roleIds: string[] = [];
  for (const candidate of workflowCandidates) {
    const primaryRoleId = chooseBestCandidateRole(spec, request.message, candidate.roleIds);
    if (!primaryRoleId) {
      continue;
    }
    if (!roleIds.includes(primaryRoleId)) {
      roleIds.push(primaryRoleId);
    }
    if (roleIds.length >= maxAssignments) {
      return roleIds;
    }
  }

  return roleIds.length > 0 ? roleIds : spec.roles.map(role => role.id);
}

export function resolveWorkflowVoteCandidateRoleIds(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  _coordinatorDecision: CoordinatorWorkflowDecision,
): string[] {
  const configured = spec.workflowVotePolicy?.candidateRoleIds?.filter(roleId =>
    spec.roles.some(role => role.id === roleId),
  );
  if (configured && configured.length > 0) {
    return configured;
  }
  return spec.roles.map(role => role.id);
}

export function shouldApproveWorkflowVote(
  spec: WorkspaceSpec,
  responses: WorkspaceWorkflowVoteResponse[],
): boolean {
  const approvals = responses.filter(response => response.decision === 'approve');
  const rejections = responses.filter(response => response.decision === 'reject');
  const decisive = approvals.length + rejections.length;
  const minimumApprovals = Math.max(1, spec.workflowVotePolicy?.minimumApprovals ?? 1);
  const requiredRatio = Math.max(0.5, spec.workflowVotePolicy?.requiredApprovalRatio ?? 0.5);

  if (approvals.length < minimumApprovals || decisive === 0) {
    return false;
  }

  return approvals.length / decisive >= requiredRatio;
}

function buildAssignmentFromClaimResponse(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  response: WorkspaceClaimResponse,
  defaultVisibility: WorkspaceVisibility,
): WorkspaceTurnAssignment {
  const workflowCandidate = findBestWorkflowCandidateForRole(
    spec,
    request.message,
    response.roleId,
  );
  const requestedOutputPath = extractRequestedOutputPath(request.message);
  const proposedInstruction = response.proposedInstruction?.trim();
  const instruction =
    proposedInstruction &&
    (!requestedOutputPath || proposedInstruction.includes(requestedOutputPath))
      ? proposedInstruction
      : request.message.trim();
  const workflowNode = workflowCandidate && spec.workflow
    ? spec.workflow.nodes.find(node => node.id === workflowCandidate.nodeId)
    : undefined;
  const role = spec.roles.find(value => value.id === response.roleId);

  return {
    roleId: response.roleId,
    summary:
      response.publicResponse ??
      `Handle workspace request as @${response.roleId}`,
    instruction,
    ...(workflowNode && role
      ? {
          provider: resolveWorkflowNodeProvider(spec, role, workflowNode),
          model: resolveWorkflowNodeModel(spec, role, workflowNode),
        }
      : {}),
    visibility: request.visibility ?? spec.activityPolicy?.defaultVisibility ?? defaultVisibility,
    ...(workflowCandidate ? { workflowNodeId: workflowCandidate.nodeId } : {}),
    ...(workflowCandidate?.stageId ? { stageId: workflowCandidate.stageId } : {}),
  };
}

export function buildWorkflowDispatchAssignment(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  node: WorkflowNodeSpec,
): WorkspaceTurnAssignment | undefined {
  const roleId = resolveWorkflowNodeRoleId(spec, request.message, node);
  if (!roleId) {
    return undefined;
  }
  const role = spec.roles.find(value => value.id === roleId);
  if (!role) {
    return undefined;
  }

  const artifactHints = (node.producesArtifacts ?? [])
    .map(artifactId => spec.artifacts?.find(artifact => artifact.id === artifactId))
    .filter(Boolean)
    .map(artifact => [artifact?.id, artifact?.path].filter(Boolean).join(' -> '));

  return {
    roleId,
    summary: node.title ? `${node.title} (${node.type})` : `Run workflow node ${node.id}`,
    provider: resolveWorkflowNodeProvider(spec, role, node),
    model: resolveWorkflowNodeModel(spec, role, node),
    instruction: [
      `You are executing workflow node "${node.title ?? node.id}" (${node.type}).`,
      node.stageId ? `Current stage: ${node.stageId}.` : null,
      node.prompt ? `Node-specific prompt: ${node.prompt}` : null,
      node.command ? `Node command to prepare for or execute: ${node.command}` : null,
      artifactHints.length > 0
        ? `Artifacts to produce or update: ${artifactHints.join(', ')}.`
        : null,
      `Original user request: ${request.message}`,
      'Focus only on this workflow step. Do not try to finish the entire workflow in one turn.',
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

export function getWorkflowEntryNode(spec: WorkspaceSpec): WorkflowNodeSpec | undefined {
  return spec.workflow?.nodes.find(node => node.id === spec.workflow?.entryNodeId);
}

function resolveWorkflowNodeRoleId(
  spec: WorkspaceSpec,
  message: string,
  node: WorkflowNodeSpec,
): string | undefined {
  const evaluationRoleId = node.itemLifecycle?.evaluate?.roleId;
  const afterApproveRoleId = node.itemLifecycle?.afterApprove?.roleId;
  if (node.roleId && spec.roles.some(role => role.id === node.roleId)) {
    return node.roleId;
  }
  if (node.workerRoleId && spec.roles.some(role => role.id === node.workerRoleId)) {
    return node.workerRoleId;
  }
  if (node.plannerRoleId && spec.roles.some(role => role.id === node.plannerRoleId)) {
    return node.plannerRoleId;
  }
  if (evaluationRoleId && spec.roles.some(role => role.id === evaluationRoleId)) {
    return evaluationRoleId;
  }
  if (afterApproveRoleId && spec.roles.some(role => role.id === afterApproveRoleId)) {
    return afterApproveRoleId;
  }
  if (node.reviewerRoleId && spec.roles.some(role => role.id === node.reviewerRoleId)) {
    return node.reviewerRoleId;
  }
  if (node.candidateRoleIds && node.candidateRoleIds.length > 0) {
    return chooseBestCandidateRole(spec, message, node.candidateRoleIds) ?? node.candidateRoleIds[0];
  }
  return undefined;
}

function normalizeDecision(
  value: unknown,
  role: RoleSpec,
  message: string,
): ClaimDecision {
  const normalized = typeof value === 'string' ? value.trim().toLowerCase() : '';
  if (normalized === 'claim' || normalized === 'support' || normalized === 'decline') {
    return normalized;
  }

  return scoreRoleForMessage(role, message.toLowerCase()) > 0 ? 'claim' : 'decline';
}

function normalizeConfidence(value: unknown, decision: ClaimDecision): number {
  if (typeof value === 'number' && Number.isFinite(value)) {
    return Math.max(0, Math.min(1, value));
  }

  return decision === 'claim' ? 0.75 : decision === 'support' ? 0.5 : 0.2;
}

function normalizeText(value: unknown): string | undefined {
  if (typeof value !== 'string') {
    return undefined;
  }
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

function fallbackRationale(role: RoleSpec, decision: ClaimDecision, message: string): string {
  if (decision === 'claim') {
    return `@${role.id} appears to be a strong fit for: ${message}`;
  }
  if (decision === 'support') {
    return `@${role.id} can contribute but should not own this request.`;
  }
  return `@${role.id} is not the best primary owner for this request.`;
}

function compareResponses(left: WorkspaceClaimResponse, right: WorkspaceClaimResponse): number {
  return right.confidence - left.confidence || left.roleId.localeCompare(right.roleId);
}

function compareClaimCandidates(
  left: WorkspaceClaimResponse,
  right: WorkspaceClaimResponse,
  rolesById: Map<string, RoleSpec>,
  coordinatorRoleId: string,
  message: string,
): number {
  const leftScore = scoreClaimCandidate(left, rolesById, coordinatorRoleId, message);
  const rightScore = scoreClaimCandidate(right, rolesById, coordinatorRoleId, message);
  return rightScore - leftScore || compareResponses(left, right);
}

function scoreClaimCandidate(
  response: WorkspaceClaimResponse,
  rolesById: Map<string, RoleSpec>,
  coordinatorRoleId: string,
  message: string,
): number {
  const role = rolesById.get(response.roleId);
  const loweredMessage = message.toLowerCase();
  const affinity = role ? scoreRoleForMessage(role, loweredMessage) : 0;
  const coordinatorPenalty = response.roleId === coordinatorRoleId && affinity > 0 ? 3 : 0;
  const specialistBonus = response.roleId !== coordinatorRoleId && affinity > 0 ? 1 : 0;
  return response.confidence * 100 + affinity * 10 + specialistBonus - coordinatorPenalty;
}

function uniqueAssignments(assignments: WorkspaceTurnAssignment[]): WorkspaceTurnAssignment[] {
  const seen = new Set<string>();
  return assignments.filter(assignment => {
    if (seen.has(assignment.roleId)) {
      return false;
    }
    seen.add(assignment.roleId);
    return true;
  });
}

function buildClaimResponseText(
  assignments: WorkspaceTurnAssignment[],
  responses: WorkspaceClaimResponse[],
  roles: RoleSpec[],
): string {
  if (responses.length === 0) {
    return buildFallbackResponseText(assignments, roles);
  }

  const selectedClaims = responses.filter(response => response.decision === 'claim');
  const selectedSupports = responses.filter(response => response.decision === 'support');

  if (selectedClaims.length === 0 && selectedSupports.length === 0) {
    return buildFallbackResponseText(assignments, roles);
  }

  const owners = selectedClaims.length > 0 ? selectedClaims : selectedSupports;
  const ownerLabels = owners.map(response => `@${response.roleId}`);
  const supportLabels = selectedSupports
    .filter(response => !owners.some(owner => owner.roleId === response.roleId))
    .map(response => `@${response.roleId}`);

  if (supportLabels.length > 0) {
    return `${ownerLabels.join(' and ')} will take this next, with support from ${supportLabels.join(' and ')}.`;
  }

  if (ownerLabels.length === 1) {
    return `${ownerLabels[0]} will take this next.`;
  }

  return `${ownerLabels.join(' and ')} will split this work.`;
}

function describeRole(role: RoleSpec): string {
  return [
    `- @${role.id} (${role.name})`,
    role.description ? `description: ${role.description}` : null,
    role.agent.description ? `agent: ${role.agent.description}` : null,
    role.outputRoot ? `output_root: ${role.outputRoot}` : null,
  ]
    .filter(Boolean)
    .join(' | ');
}

function extractJsonObject(rawText: string): Record<string, unknown> | null {
  const trimmed = rawText.trim();
  const fencedMatch = trimmed.match(/```(?:json)?\s*([\s\S]*?)```/i);
  const candidate = fencedMatch?.[1] ?? trimmed;
  const direct = tryParseJson(candidate);
  if (direct && typeof direct === 'object' && !Array.isArray(direct)) {
    return direct as Record<string, unknown>;
  }

  const start = candidate.indexOf('{');
  const end = candidate.lastIndexOf('}');
  if (start >= 0 && end > start) {
    const sliced = candidate.slice(start, end + 1);
    const parsed = tryParseJson(sliced);
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>;
    }
  }

  return null;
}

function extractClaimPayload(rawInput: unknown): Record<string, unknown> | null {
  if (rawInput && typeof rawInput === 'object' && !Array.isArray(rawInput)) {
    return rawInput as Record<string, unknown>;
  }

  if (typeof rawInput !== 'string') {
    return null;
  }

  return extractJsonObject(rawInput);
}

function tryParseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function normalizeAssignment(
  value: unknown,
  request: WorkspaceTurnRequest,
): WorkspaceTurnAssignment[] {
  if (!value || typeof value !== 'object') {
    return [];
  }

  const roleId = typeof (value as { roleId?: unknown }).roleId === 'string'
    ? (value as { roleId: string }).roleId.trim().replace(/^@+/, '')
    : '';
  const instruction = typeof (value as { instruction?: unknown }).instruction === 'string'
    ? (value as { instruction: string }).instruction.trim()
    : request.message.trim();

  if (!roleId || !instruction) {
    return [];
  }

  const assignment: WorkspaceTurnAssignment = {
    roleId,
    instruction,
  };

  const provider = typeof (value as { provider?: unknown }).provider === 'string'
    ? (value as { provider: string }).provider.trim()
    : '';
  if (provider === 'claude-agent-sdk' || provider === 'codex-sdk') {
    assignment.provider = provider;
  }

  const model = typeof (value as { model?: unknown }).model === 'string'
    ? (value as { model: string }).model.trim()
    : '';
  if (model) {
    assignment.model = model;
  }

  const summary = typeof (value as { summary?: unknown }).summary === 'string'
    ? (value as { summary: string }).summary.trim()
    : '';
  if (summary) {
    assignment.summary = summary;
  }

  const workflowNodeId = typeof (value as { workflowNodeId?: unknown }).workflowNodeId === 'string'
    ? (value as { workflowNodeId: string }).workflowNodeId.trim()
    : '';
  if (workflowNodeId) {
    assignment.workflowNodeId = workflowNodeId;
  }

  const stageId = typeof (value as { stageId?: unknown }).stageId === 'string'
    ? (value as { stageId: string }).stageId.trim()
    : '';
  if (stageId) {
    assignment.stageId = stageId;
  }

  return [assignment];
}

function buildFallbackAssignment(
  spec: WorkspaceSpec,
  roleId: string,
  message: string,
  workflowCandidate?: WorkflowCandidate,
): WorkspaceTurnAssignment {
  const workflowNode = workflowCandidate && spec.workflow
    ? spec.workflow.nodes.find(node => node.id === workflowCandidate.nodeId)
    : undefined;
  const role = spec.roles.find(value => value.id === roleId);

  return {
    roleId,
    summary: `Handle workspace request as @${roleId}`,
    instruction: message.trim(),
    ...(workflowNode && role
      ? {
          provider: resolveWorkflowNodeProvider(spec, role, workflowNode),
          model: resolveWorkflowNodeModel(spec, role, workflowNode),
        }
      : {}),
    ...(workflowCandidate ? { workflowNodeId: workflowCandidate.nodeId } : {}),
    ...(workflowCandidate?.stageId ? { stageId: workflowCandidate.stageId } : {}),
  };
}

function buildHeuristicAssignments(
  spec: WorkspaceSpec,
  message: string,
  fallbackRoleId: string,
  maxAssignments: number,
): WorkspaceTurnAssignment[] {
  const workflowCandidates = resolveWorkflowCandidates(spec, message);
  if (workflowCandidates.length > 0) {
    const selected: WorkspaceTurnAssignment[] = [];
    for (const candidate of workflowCandidates) {
      const roleId = chooseBestCandidateRole(spec, message, candidate.roleIds);
      if (!roleId || selected.some(entry => entry.roleId === roleId)) {
        continue;
      }
      selected.push(buildFallbackAssignment(spec, roleId, message, candidate));
      if (selected.length >= maxAssignments) {
        return selected;
      }
    }
    if (selected.length > 0) {
      return selected;
    }
  }

  const lowered = message.toLowerCase();
  const scored = spec.roles
    .map(role => ({ roleId: role.id, score: scoreRoleForMessage(role, lowered) }))
    .sort((left, right) => right.score - left.score || left.roleId.localeCompare(right.roleId));

  const selected = scored
    .filter(entry => entry.score > 0)
    .slice(0, maxAssignments)
    .map(entry => entry.roleId);

  const finalRoles = selected.length > 0 ? selected : [guessFallbackRoleId(spec, message, fallbackRoleId)];
  return Array.from(new Set(finalRoles)).map(roleId => buildFallbackAssignment(spec, roleId, message));
}

function guessFallbackRoleId(
  spec: WorkspaceSpec,
  message: string,
  fallbackRoleId: string,
): string {
  const lowered = message.toLowerCase();
  let bestRoleId = fallbackRoleId;
  let bestScore = 0;

  for (const role of spec.roles) {
    const score = scoreRoleForMessage(role, lowered);
    if (score > bestScore) {
      bestRoleId = role.id;
      bestScore = score;
    }
  }

  return bestRoleId;
}

function scoreRoleForMessage(role: RoleSpec, loweredMessage: string): number {
  const corpus = [
    role.id,
    role.name,
    role.description,
    role.agent.description,
    role.outputRoot,
    ...(ROLE_HINTS[role.id] ?? []),
  ]
    .filter(Boolean)
    .join(' ')
    .toLowerCase();

  let score = 0;
  for (const token of buildSearchTokens(corpus)) {
    if (token.length < 3) {
      continue;
    }
    if (loweredMessage.includes(token)) {
      score += token.length > 8 ? 3 : 1;
    }
  }

  return score;
}

interface WorkflowCandidate {
  nodeId: string;
  nodeType: WorkflowNodeType;
  stageId?: string;
  roleIds: string[];
  score: number;
}

function resolveWorkflowCandidates(
  spec: WorkspaceSpec,
  message: string,
): WorkflowCandidate[] {
  if (!spec.workflow) {
    return [];
  }

  const lowered = message.toLowerCase();
  const stageById = new Map((spec.workflow.stages ?? []).map(stage => [stage.id, stage]));
  const artifactById = new Map((spec.artifacts ?? []).map(artifact => [artifact.id, artifact]));

  const candidates: WorkflowCandidate[] = [];
  for (const node of spec.workflow.nodes) {
      const evaluationRoleId = node.itemLifecycle?.evaluate?.roleId;
      const afterApproveRoleId = node.itemLifecycle?.afterApprove?.roleId;
      const roleIds = Array.from(
        new Set(
          [
            node.roleId,
            node.workerRoleId,
            node.plannerRoleId,
            evaluationRoleId,
            afterApproveRoleId,
            node.reviewerRoleId,
            ...(node.candidateRoleIds ?? []),
          ].filter(
            (value): value is string => Boolean(value),
          ),
        ),
      );
      if (roleIds.length === 0) {
        continue;
      }

      const stage = node.stageId ? stageById.get(node.stageId) : undefined;
      const roleCorpus = roleIds
        .map(roleId => spec.roles.find(role => role.id === roleId))
        .filter((role): role is RoleSpec => Boolean(role))
        .map(role => [
          role.id,
          role.name,
          role.description,
          role.agent.description,
          role.outputRoot,
        ].filter(Boolean).join(' '));
      const artifactCorpus = [...(node.requiresArtifacts ?? []), ...(node.producesArtifacts ?? [])]
        .map(artifactId => artifactById.get(artifactId))
        .filter(Boolean)
        .map(artifact => [artifact?.id, artifact?.path, artifact?.description].filter(Boolean).join(' '));
      const corpus = [
        node.id,
        node.title,
        stage?.name,
        stage?.description,
        ...roleCorpus,
        ...artifactCorpus,
      ]
        .filter(Boolean)
        .join(' ')
        .toLowerCase();
      const score =
        buildSearchTokens(corpus)
          .filter(token => token.length >= 3 && lowered.includes(token))
          .reduce((sum, token) => sum + (token.length > 8 ? 3 : 1), 0) +
        roleIds.reduce((sum, roleId) => {
          const role = spec.roles.find(value => value.id === roleId);
          return sum + (role ? scoreRoleForMessage(role, lowered) : 0);
        }, 0) +
        workflowNodePriority(node.type);

      if (score > 0) {
        candidates.push({
          nodeId: node.id,
          nodeType: node.type,
          ...(node.stageId ? { stageId: node.stageId } : {}),
          roleIds,
          score,
        });
      }
  }

  return candidates.sort((left, right) => right.score - left.score || left.nodeId.localeCompare(right.nodeId));
}

function workflowNodePriority(nodeType: WorkflowNodeType): number {
  switch (nodeType) {
    case 'assign':
      return 6;
    case 'review':
      return 5;
    case 'worklist':
      return 5;
    case 'shell':
      return 4;
    case 'evaluate':
      return 3;
    case 'claim':
      return 1;
    default:
      return 0;
  }
}

function shouldProposeWorkflowHeuristically(
  spec: WorkspaceSpec,
  message: string,
): boolean {
  if (!spec.workflow) {
    return false;
  }

  const candidates = resolveWorkflowCandidates(spec, message);
  if (
    candidates.some(candidate =>
      ['shell', 'evaluate', 'loop', 'commit', 'revert', 'merge'].includes(candidate.nodeType),
    )
  ) {
    return true;
  }

  if (spec.workflow.mode === 'loop') {
    const lowered = message.toLowerCase();
    return ['research', 'experiment', 'iteration', 'loop', 'hypothesis', 'benchmark', 'evaluate']
      .some(token => lowered.includes(token));
  }

  return false;
}

function normalizeWorkflowVoteDecision(
  decision: WorkflowVoteDecision,
  role: RoleSpec,
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  coordinatorDecision: CoordinatorWorkflowDecision,
): WorkflowVoteDecision {
  if (decision === 'approve' || decision === 'reject') {
    return decision;
  }

  const coordinatorRoleId = spec.coordinatorRoleId ?? spec.defaultRoleId;
  if (
    role.id === coordinatorRoleId &&
    coordinatorDecision.kind === 'propose_workflow'
  ) {
    return 'approve';
  }

  if (
    coordinatorDecision.kind === 'propose_workflow' &&
    roleParticipatesInWorkflow(spec, role.id)
  ) {
    return 'approve';
  }

  if (!shouldProposeWorkflowHeuristically(spec, request.message)) {
    return 'abstain';
  }

  const hasWorkflowLane = resolveWorkflowCandidates(spec, request.message).some(candidate =>
    candidate.roleIds.includes(role.id),
  );
  return hasWorkflowLane ? 'approve' : 'abstain';
}

function roleParticipatesInWorkflow(spec: WorkspaceSpec, roleId: string): boolean {
  if (!spec.workflow) {
    return false;
  }

  return spec.workflow.nodes.some(node =>
    ('roleId' in node && node.roleId === roleId) ||
    ('reviewerRoleId' in node && node.reviewerRoleId === roleId) ||
    ('candidateRoleIds' in node && Array.isArray(node.candidateRoleIds) && node.candidateRoleIds.includes(roleId)),
  );
}

function chooseBestCandidateRole(
  spec: WorkspaceSpec,
  message: string,
  roleIds: string[],
): string | null {
  const lowered = message.toLowerCase();
  const scored = roleIds
    .map(roleId => ({
      roleId,
      score: spec.roles.find(role => role.id === roleId)
        ? scoreRoleForMessage(spec.roles.find(role => role.id === roleId)!, lowered)
        : 0,
    }))
    .sort((left, right) => right.score - left.score || left.roleId.localeCompare(right.roleId));
  return scored[0]?.roleId ?? null;
}

function findBestWorkflowCandidateForRole(
  spec: WorkspaceSpec,
  message: string,
  roleId: string,
): WorkflowCandidate | undefined {
  return resolveWorkflowCandidates(spec, message).find(candidate =>
    candidate.roleIds.includes(roleId),
  );
}

function buildSearchTokens(text: string): string[] {
  return Array.from(
    new Set(
      text
        .split(/[^a-z0-9@-]+/i)
        .map(value => value.trim().toLowerCase())
        .filter(Boolean),
    ),
  );
}

function extractRequestedOutputPath(message: string): string | null {
  const normalized = message.replace(/\r\n/g, '\n');
  const directPathMatch = normalized.match(/(?:^|\s)([A-Za-z0-9._/-]+\/[A-Za-z0-9._-]+\.(?:md|txt|json|ya?ml|csv|ts|tsx|js|jsx|rs|py|sh))(?:\s|$)/);
  if (directPathMatch?.[1]) {
    return directPathMatch[1];
  }

  const toPathMatch = normalized.match(/\bto\s+([A-Za-z0-9._/-]+\/[A-Za-z0-9._-]+\.(?:md|txt|json|ya?ml|csv|ts|tsx|js|jsx|rs|py|sh))\b/i);
  if (toPathMatch?.[1]) {
    return toPathMatch[1];
  }

  return null;
}

const ROLE_HINTS: Record<string, string[]> = {
  pm: ['plan', 'milestone', 'scope', 'coordination'],
  prd: ['prd', 'requirement', 'requirements', 'spec', 'user story', 'acceptance criteria'],
  architect: ['architecture', 'design', 'interface', 'data model', 'technical plan'],
  coder: ['implement', 'implementation', 'code', 'patch', 'bug fix'],
  tester: ['test', 'qa', 'regression', 'verification'],
  reviewer: ['review', 'audit', 'bug finding'],
  ceo: ['priority', 'decision', 'approval', 'strategy'],
  finance: ['finance', 'monthly close', 'cash', 'invoice', 'invoices', 'subscription', 'revenue', 'kpi', 'burn', 'runway', 'budget'],
  tax: ['tax', 'filing', 'sales tax', 'vat', 'estimated tax'],
  admin: ['admin', 'vendor', 'operations', 'sop', 'checklist'],
  recruiter: ['recruit', 'candidate', 'hiring', 'interview'],
  lead: ['research lead', 'hypothesis', 'question framing'],
  scout: ['research', 'sources', 'web', 'compare', 'brief'],
  experimenter: ['experiment', 'metric', 'measure', 'test design'],
  critic: ['critique', 'risk', 'skeptic', 'assumption'],
  shangshu: ['coordination', 'governance', 'routing'],
  zhongshu: ['brief', 'mandate', 'task order', 'draft'],
  menxia: ['review', 'challenge', 'risk', 'red team'],
  gongbu: ['implementation', 'build', 'execution', 'tooling'],
  hubu: ['budget', 'resources', 'capacity', 'allocation'],
  libu: ['documentation', 'communication', 'release notes'],
  xingbu: ['compliance', 'safety', 'policy'],
  bingbu: ['release', 'incident', 'operations', 'rollout'],
};

function buildFallbackResponseText(
  assignments: WorkspaceTurnAssignment[],
  roles: RoleSpec[],
): string {
  const roleNames = assignments.map(assignment => {
    const role = roles.find(value => value.id === assignment.roleId);
    return role ? `@${role.id}` : assignment.roleId;
  });

  if (roleNames.length === 1) {
    return `${roleNames[0]} will take this next.`;
  }

  return `${roleNames.join(' and ')} will split this work.`;
}
