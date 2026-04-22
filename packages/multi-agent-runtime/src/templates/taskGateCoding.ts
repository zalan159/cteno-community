import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';

import type {
  AgentCapability,
  TemplateRoleSpec,
  WorkspaceTemplate,
} from '../core/templates.js';

export type TaskGateCodingTaskSource = 'planner' | 'external';

export interface TaskGateCodingTemplateOptions {
  plannerModel?: string;
  reviewerModel?: string;
  coderModel?: string;
  taskSource?: TaskGateCodingTaskSource;
  taskListPath?: string;
  sharedLessonsPath?: string;
  taskCliPath?: string;
  /**
   * Absolute path to a Claude Code subagent markdown file (e.g. `.claude/agents/cteno-qa.md`).
   * When set, an afterCommit QA phase is added: after a task is committed, the QA role
   * dispatches the subagent to run tests/eval/* cases. `qaModel` overrides the review model
   * for the QA role. `qaOnFailure` decides what happens when QA rejects or errors (default `warn`).
   */
  qaAgentPromptFile?: string;
  qaModel?: string;
  qaOnFailure?: 'retry' | 'fail' | 'warn';
}

export function createTaskGateCodingTemplate(
  options: TaskGateCodingTemplateOptions = {},
): WorkspaceTemplate {
  const reviewModel = options.reviewerModel ?? options.plannerModel ?? 'claude-opus-4-6';
  const coderModel = options.coderModel ?? 'gpt-5.4';
  const taskListPath = options.taskListPath ?? '00-management/tasks.json';
  const sharedLessonsPath = options.sharedLessonsPath ?? '00-management/shared-lessons.md';
  const taskCliPath = options.taskCliPath ?? 'packages/multi-agent-runtime/dist/cli/taskCli.js';
  const taskSource = options.taskSource ?? 'planner';
  const qaConfig = resolveQaRoleConfig(options);

  if (taskSource === 'external') {
    return createExternalTaskGateCodingTemplate({
      reviewModel,
      coderModel,
      taskListPath,
      sharedLessonsPath,
      taskCliPath,
      ...(qaConfig ? { qa: qaConfig } : {}),
    });
  }

  return createPlannerTaskGateCodingTemplate({
    plannerModel: reviewModel,
    coderModel,
    taskListPath,
    sharedLessonsPath,
    taskCliPath,
    ...(qaConfig ? { qa: qaConfig } : {}),
  });
}

interface ResolvedQaConfig {
  model: string;
  prompt: string;
  onFailure: 'retry' | 'fail' | 'warn';
}

function resolveQaRoleConfig(
  options: TaskGateCodingTemplateOptions,
): ResolvedQaConfig | undefined {
  if (!options.qaAgentPromptFile) {
    return undefined;
  }
  const absolute = path.isAbsolute(options.qaAgentPromptFile)
    ? options.qaAgentPromptFile
    : path.resolve(process.cwd(), options.qaAgentPromptFile);
  if (!existsSync(absolute)) {
    throw new Error(`qaAgentPromptFile not found: ${absolute}`);
  }
  const raw = readFileSync(absolute, 'utf8');
  const prompt = stripYamlFrontmatter(raw).trim();
  if (!prompt) {
    throw new Error(`qaAgentPromptFile is empty after frontmatter strip: ${absolute}`);
  }
  return {
    model: options.qaModel ?? options.reviewerModel ?? options.plannerModel ?? 'claude-opus-4-6',
    prompt,
    onFailure: options.qaOnFailure ?? 'retry',
  };
}

function stripYamlFrontmatter(source: string): string {
  if (!source.startsWith('---')) {
    return source;
  }
  const end = source.indexOf('\n---', 3);
  if (end < 0) {
    return source;
  }
  return source.slice(end + 4).replace(/^\r?\n/, '');
}

export function createManualTaskGateCodingTemplate(
  options: Omit<TaskGateCodingTemplateOptions, 'taskSource'> = {},
): WorkspaceTemplate {
  return createTaskGateCodingTemplate({
    ...options,
    taskSource: 'external',
  });
}

function createPlannerTaskGateCodingTemplate(options: {
  plannerModel: string;
  coderModel: string;
  taskListPath: string;
  sharedLessonsPath: string;
  taskCliPath: string;
  qa?: ResolvedQaConfig;
}): WorkspaceTemplate {
  const { plannerModel, coderModel, taskListPath, sharedLessonsPath, taskCliPath, qa } = options;

  return {
    templateId: 'task-gate-coding',
    templateName: 'Task Gate Coding',
    description:
      'Claude plans tasks from documents, Codex implements them one by one, Claude evaluates each result, and approved tasks are committed before the next task begins.',
    provider: 'claude-agent-sdk',
    model: plannerModel,
    defaultRoleId: 'planner',
    coordinatorRoleId: 'planner',
    orchestratorPrompt:
      'You coordinate a gated implementation workflow. Turn the user request into a task plan, route implementation to Codex, require evaluator approval before each commit, and only advance after the current task is safely committed.',
    claimPolicy: {
      mode: 'coordinator_only',
      maxAssignees: 1,
      fallbackRoleId: 'planner',
    },
    activityPolicy: {
      publishUserMessages: true,
      publishCoordinatorMessages: true,
      publishDispatchLifecycle: true,
      publishMemberMessages: true,
      defaultVisibility: 'public',
    },
    workflowVotePolicy: {
      minimumApprovals: 1,
      requiredApprovalRatio: 0.5,
    },
    workflow: {
      mode: 'pipeline',
      entryNodeId: 'plan_tasks',
      stages: [
        {
          id: 'planning',
          name: 'Planning',
          description: 'Read the docs, create a task plan, and produce a structured task list.',
          entryNodeId: 'plan_tasks',
          exitNodeIds: ['execute_tasks'],
        },
        {
          id: 'execution',
          name: 'Execution',
          description: 'Implement, evaluate, and commit each task before moving on.',
          entryNodeId: 'execute_tasks',
          exitNodeIds: ['complete'],
        },
      ],
      nodes: [
        {
          id: 'plan_tasks',
          type: 'assign',
          title: 'Read docs and plan tasks',
          roleId: 'planner',
          stageId: 'planning',
          producesArtifacts: ['task_overview', 'task_list'],
          prompt:
            `Read the relevant documents and repository context for the user request. Write \`00-management/task-overview.md\` with \`## Goal\`, \`## Inputs Reviewed\`, \`## Execution Strategy\`, and \`## Done Criteria\`. Then create or update \`${taskListPath}\` with the task CLI at \`${taskCliPath}\` instead of hand-editing JSON. Each item should be a small, reviewable implementation unit and include \`id\`, \`title\`, \`description\`, \`status\`, \`attempts\`, \`maxAttempts\`, \`files\`, and \`acceptanceCriteria\`. Treat \`files\` as likely/reference files to inspect first, not a hard whitelist. Prefer 2-6 tasks. Shared lessons live at \`${sharedLessonsPath}\`.`,
        },
        createExecuteTasksNode('planner', taskListPath, sharedLessonsPath, taskCliPath, qa),
        {
          id: 'complete',
          type: 'complete',
          title: 'Complete gated coding workflow',
          stageId: 'execution',
        },
      ],
      edges: [
        { from: 'plan_tasks', to: 'execute_tasks', when: 'success' },
        { from: 'execute_tasks', to: 'complete', when: 'success' },
        { from: 'execute_tasks', to: 'plan_tasks', when: 'failure' },
      ],
    },
    artifacts: [
      {
        id: 'task_overview',
        kind: 'doc',
        path: '00-management/task-overview.md',
        ownerRoleId: 'planner',
        required: true,
        description: 'Planner summary of the request, inputs, and done criteria.',
      },
      {
        id: 'task_list',
        kind: 'task_list',
        path: taskListPath,
        ownerRoleId: 'planner',
        required: true,
        description: 'Structured task queue consumed by the gated worklist runner.',
      },
    ],
    completionPolicy: {
      successNodeIds: ['complete'],
      failureNodeIds: [],
      maxIterations: 8,
      defaultStatus: 'stuck',
    },
    roles: [
      {
        id: 'planner',
        name: 'Planner',
        outputRoot: '00-management/',
        agent: {
          provider: 'claude-agent-sdk',
          model: plannerModel,
          description:
            'Reads docs, synthesizes the request into a structured task list, and evaluates each completed task before commit.',
          prompt:
            'You are the planning and evaluation lead. Read the relevant docs and code context first, decompose the request into small executable tasks with crisp done criteria and file targets, and later review each completed task rigorously before it can be committed.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      createCoderRole(coderModel),
      ...(qa ? [createQaRole(qa)] : []),
    ],
  };
}

function createExternalTaskGateCodingTemplate(options: {
  reviewModel: string;
  coderModel: string;
  taskListPath: string;
  sharedLessonsPath: string;
  taskCliPath: string;
  qa?: ResolvedQaConfig;
}): WorkspaceTemplate {
  const { reviewModel, coderModel, taskListPath, sharedLessonsPath, taskCliPath, qa } = options;

  return {
    templateId: 'task-gate-coding-manual',
    templateName: 'Task Gate Coding (Manual Tasks)',
    description:
      'Consume a hand-authored tasks.json, have Codex execute each task, require reviewer approval, and commit approved work items one by one.',
    provider: 'claude-agent-sdk',
    model: reviewModel,
    defaultRoleId: 'reviewer',
    coordinatorRoleId: 'reviewer',
    orchestratorPrompt:
      'You coordinate a gated implementation workflow from a task list ledger. Trust the current tasks.json as the source of truth, but allow coding agents to insert follow-up tasks or mark a task abandoned/superseded through the task CLI when they discover a better decomposition. Route execution to Codex, require reviewer approval before each commit, and stop on the first unresolved task failure.',
    claimPolicy: {
      mode: 'coordinator_only',
      maxAssignees: 1,
      fallbackRoleId: 'reviewer',
    },
    activityPolicy: {
      publishUserMessages: true,
      publishCoordinatorMessages: true,
      publishDispatchLifecycle: true,
      publishMemberMessages: true,
      defaultVisibility: 'public',
    },
    workflowVotePolicy: {
      minimumApprovals: 1,
      requiredApprovalRatio: 0.5,
    },
    workflow: {
      mode: 'pipeline',
      entryNodeId: 'execute_tasks',
      stages: [
        {
          id: 'execution',
          name: 'Execution',
          description: 'Execute, review, and commit each prewritten task.',
          entryNodeId: 'execute_tasks',
          exitNodeIds: ['complete'],
        },
      ],
      nodes: [
        createExecuteTasksNode('reviewer', taskListPath, sharedLessonsPath, taskCliPath, qa),
        {
          id: 'complete',
          type: 'complete',
          title: 'Complete gated coding workflow',
          stageId: 'execution',
        },
      ],
      edges: [{ from: 'execute_tasks', to: 'complete', when: 'success' }],
    },
    artifacts: [
      {
        id: 'task_list',
        kind: 'task_list',
        path: taskListPath,
        ownerRoleId: 'reviewer',
        required: true,
        description: 'Hand-authored structured task queue consumed by the gated worklist runner.',
      },
    ],
    completionPolicy: {
      successNodeIds: ['complete'],
      failureNodeIds: [],
      maxIterations: 8,
      defaultStatus: 'stuck',
    },
    roles: [
      {
        id: 'reviewer',
        name: 'Reviewer',
        outputRoot: '00-management/',
        agent: {
          provider: 'codex-sdk' as const,
          model: reviewModel,
          effort: 'medium' as const,
          description:
            'Reviews each completed task against the hand-authored task list before it can be committed.',
          prompt:
            'You are the reviewer for a gated coding workflow. Trust the provided tasks.json as the source of truth, inspect the changed files carefully, respect task updates made through the task CLI, approve only when the task is fully satisfied, and give concrete rejection feedback when it is not.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      createCoderRole(coderModel),
      ...(qa ? [createQaRole(qa)] : []),
    ],
  };
}

function createExecuteTasksNode(
  evaluationRoleId: 'planner' | 'reviewer',
  taskListPath: string,
  sharedLessonsPath: string,
  taskCliPath: string,
  qa?: ResolvedQaConfig,
) {
  return {
    id: 'execute_tasks',
    type: 'worklist' as const,
    title: 'Implement tasks with review and commit gates',
    stageId: 'execution',
    worklistArtifactId: 'task_list',
    sharedLessonsPath,
    taskCliPath,
    workerRoleId: 'coder',
    stopOnItemFailure: true,
    reloadWorklistAfterItem: true,
    revertOnRetry: true,
    revertOnExhaustedFailure: true,
    itemTimeoutMs: 60 * 60 * 1000,
    itemPromptTemplate:
      'Execute only work item "{{title}}" ({{id}}). Task description: {{description}}\nReference files to inspect first (helpful hints, not a strict allowlist): {{files}}\nAcceptance criteria:\n{{acceptance_criteria}}\n{{feedback}}\nOriginal user request: {{request}}\nTask list source: ' +
      `${taskListPath}` +
      '\nBefore changing code, read shared lessons if present: {{shared_lessons_path}}\nUse the task CLI instead of hand-editing tasks.json: {{task_cli_command}} --tasks {{task_list_path}} --cwd .\n\nSCOPE CHECK (do this before writing code). After reading the task description and relevant files, judge whether this task fits a single focused change. Decompose it if ANY of the following is true:\n  - touches more than ~5 files across unrelated modules\n  - diff would exceed ~500 lines of non-trivial code\n  - mixes independent concerns (e.g. refactor + new feature, or backend protocol + frontend UI)\n  - acceptance criteria imply multiple separately-reviewable changes\n  - you discover mid-investigation that a prerequisite task is missing\n\nHOW TO DECOMPOSE. Do not write half an implementation. Run the CLI to split {{id}} into smaller children, then supersede the parent. Concrete template (replace titles/descriptions/files/criteria with real values):\n  {{task_cli_command}} add --tasks {{task_list_path}} --cwd . --after {{id}} --id {{id}}-split-1 --title "..." --description "..." --files "a.rs,b.rs" --criteria "..." --criteria "..."\n  {{task_cli_command}} add --tasks {{task_list_path}} --cwd . --after {{id}}-split-1 --id {{id}}-split-2 --depends-on {{id}}-split-1 --title "..." --description "..." --files "c.rs" --criteria "..."\n  {{task_cli_command}} set-status {{id}} superseded --tasks {{task_list_path}} --cwd . --reason "split into {{id}}-split-1/2: <one-line rationale>"\nThen hand off with a short note explaining why you split and which child runs first. Do not mark the parent completed or commit anything for it.\n\nBefore finishing, append any reusable pitfall or workaround to the shared lessons file via the CLI.\nImplement the task, update the relevant files, and report exactly what changed. If there is previous evaluator feedback above, fix those issues before anything else.',
    itemLifecycle: {
      // QA-FIRST gate: functional correctness first. QA (loaded with the
      // cteno-qa subagent prompt) approves only when the feature actually
      // works — runs relevant tests/eval/ cases, three-layer (Store/DOM/
      // screenshot) verification where applicable, does not block on pre-
      // existing CI failures that are unrelated to this task's diff.
      evaluate: qa
        ? {
            roleId: 'qa' as const,
            onReject: 'retry' as const,
            promptTemplate:
              'Evaluate work item "{{title}}" ({{id}}) for **functional correctness only**. You are the QA gate; a reviewer will inspect code style after commit.\n\nTask description: {{description}}\nReference files: {{files}}\nAcceptance criteria (focus on the intent, not literal wording):\n{{acceptance_criteria}}\nLatest coder handoff:\n{{worker_result}}\n{{feedback}}\n\nYour job:\n- Inspect the changed files and verify the feature is actually implemented (not just declared).\n- Run the relevant cases under tests/eval/ that exercise this task and update status markers in-place ([pass] / [fail] / [flaky] / [skip] with reason).\n- Prefer script-based verification over reading code (per cteno-qa principle "能脚本判的绝不交给 AI").\n- Pre-existing failing specs that are NOT caused by this task do NOT block approval — document them instead.\n- Do not edit product runtime code during evaluation (test infrastructure / eval cases OK).\n\nStart your response with either "APPROVED:" or "REJECTED:". If rejected, list the failing cases and the concrete functional problems the coding agent must fix next. Do not reject on code style — that is the reviewer\'s job, not yours.',
          }
        : {
            // Fallback: no qa configured → keep reviewer as evaluation gate
            roleId: evaluationRoleId,
            onReject: 'retry' as const,
            promptTemplate:
              'Evaluate work item "{{title}}" ({{id}}).\nTask description: {{description}}\nReference files to inspect first: {{files}}\nAcceptance criteria:\n{{acceptance_criteria}}\nLatest coder handoff:\n{{worker_result}}\n{{feedback}}\nInspect the changed files and determine whether the task is complete. The listed files are hints, not a hard file boundary. Start your response with either "APPROVED:" or "REJECTED:". If rejected, list the concrete problems the coding agent must fix next.',
          },
      afterApprove: {
        action: 'commit' as const,
        roleId: 'coder',
        promptTemplate:
          'The evaluator approved work item "{{title}}" ({{id}}).\nStage and commit the files relevant to this task. Prefer the listed files: {{files}}\nCommit message: {{commit_message}}\nContext from the approved implementation/evaluation:\n{{worker_result}}\nCreate exactly one git commit for this task. Do not revert unrelated user changes. If there is nothing to commit, explain clearly why.',
      },
      ...(qa
        ? {
            // Post-commit code review: QA already validated functional
            // correctness; reviewer's job here is PURELY code quality —
            // naming, readability, dead code, subtle bugs, style. Failures
            // DO NOT revert the commit or block the next task — they land
            // as advisory warnings because the feature ships regardless.
            afterCommit: {
              roleId: evaluationRoleId, // 'reviewer' (Codex GPT-5.4)
              onFailure: 'warn' as const,
              promptTemplate:
                'Perform a **post-commit code review** of work item "{{title}}" ({{id}}). The feature was already validated by QA — do NOT re-check functional correctness, do NOT rerun tests, and do NOT block on acceptance criteria (that is QA\'s domain, already passed).\n\nCommitted change context:\n{{worker_result}}\nReference files: {{files}}\n\nFocus strictly on:\n- Naming clarity and consistency with surrounding code\n- Readability, code structure, obvious refactor opportunities\n- Dead code, unused imports, left-behind debug artifacts\n- Subtle logic bugs or edge cases the coder might have missed (note them but do not reject — QA approved)\n- Documentation / comment quality where WHY is non-obvious\n\nStart your response with either "APPROVED:" or "REJECTED:".\n- "APPROVED:" means the code quality is acceptable.\n- "REJECTED:" does NOT revert anything — it surfaces advisory notes for follow-up. List concrete code-quality suggestions (one per line). The workflow will continue to the next task regardless.',
            },
          }
        : {}),
    },
  };
}

function createQaRole(qa: ResolvedQaConfig): TemplateRoleSpec {
  const capabilities: AgentCapability[] = ['read', 'write', 'edit', 'glob', 'grep', 'shell'];
  return {
    id: 'qa',
    name: 'QA',
    outputRoot: '00-management/',
    agent: {
      provider: 'claude-agent-sdk' as const,
      model: qa.model,
      description:
        'End-to-end QA agent. Runs tests/eval/* cases against the just-committed change, updates status markers, and reports pass/fail. Does not modify product runtime code.',
      prompt: qa.prompt,
      capabilities,
    },
  };
}

function createCoderRole(coderModel: string): TemplateRoleSpec {
  const capabilities: AgentCapability[] = ['read', 'write', 'edit', 'glob', 'grep', 'shell'];
  return {
    id: 'coder',
    name: 'Coder',
    outputRoot: '40-code/',
    agent: {
      provider: 'codex-sdk' as const,
      model: coderModel,
      effort: 'high' as const,
      description: 'Implements one approved task at a time and commits it after evaluator approval.',
      prompt:
        'You are the coding agent. Execute exactly one task at a time, make the smallest complete change that satisfies the task, and commit only after the evaluator approves.',
      capabilities,
      requiresEditAccess: true,
    },
  };
}
