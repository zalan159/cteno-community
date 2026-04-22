import type { WorkspaceTemplate } from '../core/templates.js';

export interface PlanCodingEvalTemplateOptions {
  templateId?: string;
  templateName?: string;
  description?: string;
  planRoot?: string;
  evalRoot?: string;
  codeRoot?: string;
  verificationCommands?: string[];
}

const DEFAULT_VERIFICATION_COMMANDS = [
  'cargo check --manifest-path apps/client/desktop/Cargo.toml',
  'cargo check --manifest-path apps/client/desktop/Cargo.toml --no-default-features',
];

/**
 * Plan -> Coding -> Eval 三阶段 workspace template.
 *
 * Roles:
 *   - planner: 读目标并产出实施计划
 *   - coder:   按计划实施代码修改
 *   - evaluator: 跑验证命令并输出结论
 *
 * 适合派发单个明确 refactor / code-mod 小任务的场景。
 */
export function createPlanCodingEvalTemplate(
  options: PlanCodingEvalTemplateOptions = {},
): WorkspaceTemplate {
  const planRoot = normalizeDirectory(options.planRoot ?? 'plans/');
  const evalRoot = normalizeDirectory(options.evalRoot ?? 'eval/');
  const codeRoot = normalizeDirectory(options.codeRoot ?? '.');
  const verificationCommands =
    options.verificationCommands && options.verificationCommands.length > 0
      ? options.verificationCommands
      : DEFAULT_VERIFICATION_COMMANDS;
  const verificationBlock = verificationCommands
    .map(command => `- \`${command}\``)
    .join('\n');

  return {
    templateId: options.templateId ?? 'plan-coding-eval',
    templateName: options.templateName ?? 'Plan Coding Eval',
    description:
      options.description ??
      'Three-role workflow for executing a single well-scoped code change: planner drafts, coder implements, evaluator verifies.',
    defaultRoleId: 'planner',
    coordinatorRoleId: 'planner',
    orchestratorPrompt:
      'You coordinate a three-stage pipeline: (1) planner produces a concrete implementation plan, (2) coder applies the plan to the codebase, (3) evaluator verifies by running compile / smoke checks. Keep output tight, preserve the planner -> coder -> evaluator handoff, and route work via the fixed workflow nodes.',
    claimPolicy: {
      mode: 'direct',
      claimTimeoutMs: 15000,
      maxAssignees: 1,
      allowSupportingClaims: false,
      fallbackRoleId: 'planner',
    },
    activityPolicy: {
      publishUserMessages: true,
      publishCoordinatorMessages: true,
      publishDispatchLifecycle: true,
      publishMemberMessages: true,
      defaultVisibility: 'public',
    },
    workflow: {
      mode: 'pipeline',
      entryNodeId: 'plan',
      stages: [
        {
          id: 'plan',
          name: 'Plan',
          description: 'Planner analyzes the goal and writes a concrete plan.',
          entryNodeId: 'plan',
          exitNodeIds: ['implement'],
        },
        {
          id: 'code',
          name: 'Code',
          description: 'Coder applies the plan and keeps edits focused.',
          entryNodeId: 'implement',
          exitNodeIds: ['evaluate'],
        },
        {
          id: 'eval',
          name: 'Eval',
          description: 'Evaluator runs verification commands and reports.',
          entryNodeId: 'evaluate',
          exitNodeIds: ['complete'],
        },
      ],
      nodes: [
        {
          id: 'plan',
          type: 'assign',
          title: 'Draft implementation plan',
          roleId: 'planner',
          producesArtifacts: ['plan_doc'],
          stageId: 'plan',
        },
        {
          id: 'implement',
          type: 'assign',
          title: 'Implement per plan',
          roleId: 'coder',
          requiresArtifacts: ['plan_doc'],
          producesArtifacts: ['code_patch'],
          stageId: 'code',
        },
        {
          id: 'evaluate',
          type: 'assign',
          title: 'Run verification commands + smoke',
          roleId: 'evaluator',
          requiresArtifacts: ['code_patch'],
          producesArtifacts: ['eval_report'],
          stageId: 'eval',
        },
        {
          id: 'complete',
          type: 'complete',
          title: 'Finalize',
          stageId: 'eval',
        },
      ],
      edges: [
        { from: 'plan', to: 'implement', when: 'success' },
        { from: 'implement', to: 'evaluate', when: 'success' },
        { from: 'evaluate', to: 'complete', when: 'pass' },
      ],
    },
    artifacts: [
      {
        id: 'plan_doc',
        kind: 'doc',
        path: planRoot,
        ownerRoleId: 'planner',
        required: true,
        description: 'Concrete plan with file-level edits, risks, and test strategy.',
      },
      {
        id: 'code_patch',
        kind: 'code',
        path: codeRoot,
        ownerRoleId: 'coder',
        required: true,
        description: 'Applied code changes for the requested task.',
      },
      {
        id: 'eval_report',
        kind: 'report',
        path: evalRoot,
        ownerRoleId: 'evaluator',
        required: true,
        description: 'Verification report with command evidence and residual risk notes.',
      },
    ],
    roles: [
      {
        id: 'planner',
        name: 'Planner',
        description: 'Reads the goal and codebase, then writes a concrete implementation plan.',
        agent: {
          description:
            'Senior engineer who produces a file-level implementation plan with risk notes and test strategy. Does NOT write code itself.',
          prompt:
            `You are the planner. Your only output is a Markdown plan. Read the repo structure, relevant files named in the goal, and propose concrete edits: exact file paths, function names, logic summary, risks, and testing approach. Do NOT apply code. Write the plan to \`${planRoot}<slug>.md\`. Assume the coder and evaluator will only read your plan plus the original task, so make it implementation-ready and verification-aware. Reply with the plan path and a short summary.`,
          capabilities: ['read', 'glob', 'grep'],
          maxTurns: 15,
        },
      },
      {
        id: 'coder',
        name: 'Coder',
        description: 'Implements the plan. Writes code and keeps diffs focused.',
        agent: {
          description:
            'Pragmatic coder. Applies the plan unless blocked, respects existing code style, and keeps changes minimal.',
          prompt:
            `You are the coder. Read the plan from \`${planRoot}\` and apply it inside \`${codeRoot}\`. Make the minimum focused edits needed to satisfy the task. Run narrow, incremental verification while coding when it helps, but do not turn evaluation into your main job. Do NOT git commit. Reply with a diff summary, list of modified files, and any blockers left for the evaluator.`,
          capabilities: ['read', 'write', 'edit', 'glob', 'grep', 'shell'],
          requiresEditAccess: true,
          maxTurns: 30,
        },
      },
      {
        id: 'evaluator',
        name: 'Evaluator',
        description: 'Verifies the code change. Runs required checks and reports.',
        agent: {
          description:
            'Verifier. Runs the required verification commands plus any task-specific smoke checks described by the plan, then writes a pass/fail report.',
          prompt:
            `You are the evaluator. Run the following verification commands from the workspace root:\n${verificationBlock}\nIf the plan specifies extra smoke checks, run the narrowest useful ones after the required commands. Write \`${evalRoot}<slug>.md\` with: commands run, truncated outputs (up to 20 lines each), pass/fail verdict, and any observed anomalies or residual risks.`,
          capabilities: ['read', 'glob', 'grep', 'shell'],
          maxTurns: 15,
        },
      },
    ],
    completionPolicy: {
      successNodeIds: ['complete'],
      maxIterations: 50,
    },
  };
}

function normalizeDirectory(value: string): string {
  const trimmed = value.trim();
  if (!trimmed || trimmed === '.') {
    return '.';
  }

  return trimmed.endsWith('/') ? trimmed : `${trimmed}/`;
}
