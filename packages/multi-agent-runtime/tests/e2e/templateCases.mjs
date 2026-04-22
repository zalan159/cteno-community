import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import path from 'node:path';

import {
  createAutoresearchTemplate,
  createCodingStudioTemplate,
  createOpcSoloCompanyTemplate,
  createTaskGateCodingTemplate,
} from '../../dist/index.js';

function createHybridWorkflowTemplate() {
  return {
    templateId: 'hybrid-workflow',
    templateName: 'Hybrid Workflow',
    description: 'Cross-provider workflow with Claude planning/review and Codex implementation.',
    provider: 'claude-agent-sdk',
    defaultRoleId: 'lead',
    coordinatorRoleId: 'lead',
    orchestratorPrompt:
      'You are coordinating a deterministic test workspace. If the user asks to run the workflow or mentions [workflow], choose propose_workflow.',
    claimPolicy: {
      mode: 'coordinator_only',
      maxAssignees: 1,
      fallbackRoleId: 'lead',
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
      entryNodeId: 'plan',
      stages: [
        { id: 'plan', name: 'Plan', entryNodeId: 'plan', exitNodeIds: ['implement'] },
        { id: 'build', name: 'Build', entryNodeId: 'implement', exitNodeIds: ['review', 'complete'] },
      ],
      nodes: [
        {
          id: 'plan',
          type: 'assign',
          title: 'Write plan',
          roleId: 'lead',
          stageId: 'plan',
          prompt:
            'Create `00-management/hybrid-plan.md` with `## Goal`, `## Steps`, and `## Acceptance Criteria`. Mention the implementation file path.',
        },
        {
          id: 'implement',
          type: 'assign',
          title: 'Implement note',
          roleId: 'coder',
          provider: 'codex-sdk',
          stageId: 'build',
          prompt:
            'Create `40-code/hybrid-note.md` with a short heading and a bullet list summarizing the implemented output. Mention that the workflow used Codex for implementation.',
        },
        {
          id: 'review',
          type: 'review',
          title: 'Review output',
          reviewerRoleId: 'reviewer',
          stageId: 'build',
          prompt:
            'Check whether `00-management/hybrid-plan.md` and `40-code/hybrid-note.md` both exist and are coherent. Write `60-review/hybrid-review.md` with a short approval note. If both files exist and look correct, approve.',
        },
        {
          id: 'complete',
          type: 'complete',
          title: 'Complete',
          stageId: 'build',
        },
      ],
      edges: [
        { from: 'plan', to: 'implement', when: 'success' },
        { from: 'implement', to: 'review', when: 'success' },
        { from: 'review', to: 'complete', when: 'approved' },
        { from: 'review', to: 'implement', when: 'rejected' },
      ],
    },
    completionPolicy: {
      successNodeIds: ['complete'],
      failureNodeIds: [],
      maxIterations: 4,
      defaultStatus: 'stuck',
    },
    roles: [
      {
        id: 'lead',
        name: 'Lead',
        outputRoot: '00-management/',
        agent: {
          description: 'Coordinates and plans the workflow.',
          prompt:
            'You are the lead. When a user asks to run the workflow or includes [workflow], enter workflow mode and complete planning steps concisely.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'coder',
        name: 'Coder',
        outputRoot: '40-code/',
        agent: {
          provider: 'codex-sdk',
          description: 'Implements requested deliverables.',
          prompt:
            'You are the implementation role. Create the requested deliverable with minimal extra text.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep', 'shell'],
          requiresEditAccess: true,
        },
      },
      {
        id: 'reviewer',
        name: 'Reviewer',
        outputRoot: '60-review/',
        agent: {
          description: 'Reviews workflow outputs.',
          prompt:
            'You review outputs with a correctness-first mindset. Approve when the requested files exist and satisfy the instructions.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
    ],
  };
}

function createHybridTaskWorklistTemplate() {
  return {
    templateId: 'hybrid-task-worklist',
    templateName: 'Hybrid Task Worklist',
    description: 'Claude plans a task list and Codex executes each work item in sequence.',
    provider: 'claude-agent-sdk',
    defaultRoleId: 'lead',
    coordinatorRoleId: 'lead',
    orchestratorPrompt:
      'You coordinate a deterministic hybrid worklist workflow. If the user says [workflow] or asks to run the task loop, propose the workflow.',
    claimPolicy: {
      mode: 'coordinator_only',
      maxAssignees: 1,
      fallbackRoleId: 'lead',
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
        { id: 'plan', name: 'Plan', entryNodeId: 'plan_tasks', exitNodeIds: ['implement_tasks'] },
        { id: 'build', name: 'Build', entryNodeId: 'implement_tasks', exitNodeIds: ['review', 'complete'] },
      ],
      nodes: [
        {
          id: 'plan_tasks',
          type: 'assign',
          title: 'Plan task list',
          roleId: 'lead',
          stageId: 'plan',
          producesArtifacts: ['task_plan', 'task_list'],
          prompt:
            'Create `00-management/task-plan.md` with `## Goal`, `## Tasks`, and `## Acceptance Criteria`. Also create `00-management/tasks.json` as valid JSON with exactly two items. Item 1 must create `40-code/feature-a.md`; item 2 must create `40-code/feature-b.md`. Use an `items` array with fields `id`, `title`, `description`, `status`, `attempts`, `maxAttempts`, `files`, and `acceptanceCriteria`.',
        },
        {
          id: 'implement_tasks',
          type: 'worklist',
          title: 'Implement planned tasks',
          stageId: 'build',
          worklistArtifactId: 'task_list',
          workerRoleId: 'coder',
          stopOnItemFailure: true,
        },
        {
          id: 'review',
          type: 'review',
          title: 'Review implemented tasks',
          reviewerRoleId: 'reviewer',
          stageId: 'build',
          prompt:
            'Check that `00-management/task-plan.md`, `00-management/tasks.json`, `40-code/feature-a.md`, and `40-code/feature-b.md` exist. Write `60-review/worklist-review.md` with a short approval note. Approve if all files exist and the tasks file shows completed items.',
        },
        {
          id: 'complete',
          type: 'complete',
          title: 'Complete',
          stageId: 'build',
        },
      ],
      edges: [
        { from: 'plan_tasks', to: 'implement_tasks', when: 'success' },
        { from: 'implement_tasks', to: 'review', when: 'success' },
        { from: 'implement_tasks', to: 'plan_tasks', when: 'failure' },
        { from: 'review', to: 'complete', when: 'approved' },
        { from: 'review', to: 'implement_tasks', when: 'rejected' },
      ],
    },
    artifacts: [
      {
        id: 'task_plan',
        kind: 'doc',
        path: '00-management/task-plan.md',
        ownerRoleId: 'lead',
        required: true,
      },
      {
        id: 'task_list',
        kind: 'task_list',
        path: '00-management/tasks.json',
        ownerRoleId: 'lead',
        required: true,
      },
    ],
    completionPolicy: {
      successNodeIds: ['complete'],
      failureNodeIds: [],
      maxIterations: 4,
      defaultStatus: 'stuck',
    },
    roles: [
      {
        id: 'lead',
        name: 'Lead',
        outputRoot: '00-management/',
        agent: {
          description: 'Plans and reviews worklist-based workflows.',
          prompt:
            'You are the planning lead. Produce crisp plans and valid JSON worklists, then review completion accurately.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'coder',
        name: 'Coder',
        outputRoot: '40-code/',
        agent: {
          provider: 'codex-sdk',
          description: 'Executes one work item at a time.',
          prompt:
            'You are the implementation role. Complete exactly the current work item and keep your result concise.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep', 'shell'],
          requiresEditAccess: true,
        },
      },
      {
        id: 'reviewer',
        name: 'Reviewer',
        outputRoot: '60-review/',
        agent: {
          description: 'Reviews workflow outputs.',
          prompt:
            'You review workflow outputs with a correctness-first mindset. Approve when the requested files exist and the task list is completed.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
    ],
  };
}

function createAutoresearchWorklistTemplate() {
  return {
    templateId: 'autoresearch-worklist',
    templateName: 'Autoresearch Worklist',
    description: 'Autoresearch-style replenishing worklist with Claude planning and Codex execution.',
    provider: 'claude-agent-sdk',
    defaultRoleId: 'lead',
    coordinatorRoleId: 'lead',
    orchestratorPrompt:
      'You coordinate a deterministic autoresearch loop. If the user asks to kick off the loop or includes [workflow], propose the workflow.',
    claimPolicy: {
      mode: 'coordinator_only',
      maxAssignees: 1,
      fallbackRoleId: 'lead',
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
      mode: 'loop',
      entryNodeId: 'frame_hypothesis',
      stages: [
        { id: 'research', name: 'Research', entryNodeId: 'frame_hypothesis', exitNodeIds: ['experiment_loop', 'complete'] },
      ],
      nodes: [
        {
          id: 'frame_hypothesis',
          type: 'assign',
          title: 'Frame hypothesis',
          roleId: 'lead',
          stageId: 'research',
          producesArtifacts: ['hypothesis_brief'],
          prompt:
            'Create `research/00-lead/hypothesis.md` with `## Hypothesis`, `## Success Criteria`, and `## Next Experiment`. Keep it concise and mention collaboration mention semantics.',
        },
        {
          id: 'experiment_loop',
          type: 'worklist',
          title: 'Run replenishing experiment loop',
          stageId: 'research',
          worklistArtifactId: 'experiment_queue',
          plannerRoleId: 'lead',
          plannerPrompt:
            'Create or update `research/10-experiments/experiment-queue.json`. Generate exactly one pending item per planning pass, up to two total unique items across the whole run. Each item should instruct the worker to create `research/20-experiments/<item-id>.md` with a concise local experiment-design note tied to the hypothesis. Keep the task self-contained, do not require web research, and do not recreate completed item ids.',
          workerRoleId: 'experimenter',
          worklistMode: 'replenishing',
          replenish: 'when_empty',
          maxBatches: 2,
          stopOnItemFailure: true,
          itemPromptTemplate:
            'Execute only this experiment-design task for "{{title}}" ({{id}}). Create `research/20-experiments/{{id}}.md` as a concise note with sections `## Objective`, `## Method`, and `## Expected Signal`, using only the local hypothesis and this description: {{description}}. Do not browse the web or run exploratory shell commands unless strictly necessary to write the file. Original user request: {{request}}',
        },
        {
          id: 'complete',
          type: 'complete',
          title: 'Complete',
          stageId: 'research',
        },
      ],
      edges: [
        { from: 'frame_hypothesis', to: 'experiment_loop', when: 'success' },
        { from: 'experiment_loop', to: 'complete', when: 'success' },
        { from: 'experiment_loop', to: 'frame_hypothesis', when: 'failure' },
      ],
    },
    artifacts: [
      {
        id: 'hypothesis_brief',
        kind: 'doc',
        path: 'research/00-lead/hypothesis.md',
        ownerRoleId: 'lead',
        required: true,
      },
      {
        id: 'experiment_queue',
        kind: 'task_list',
        path: 'research/10-experiments/experiment-queue.json',
        ownerRoleId: 'lead',
        required: true,
      },
    ],
    completionPolicy: {
      successNodeIds: ['complete'],
      failureNodeIds: [],
      maxIterations: 6,
      defaultStatus: 'stuck',
    },
    roles: [
      {
        id: 'lead',
        name: 'Lead',
        outputRoot: 'research/00-lead/',
        agent: {
          description: 'Frames hypotheses and replenishes experiment queues.',
          prompt:
            'You are the research lead. Frame the hypothesis, maintain a structured experiment queue, and review outcomes concisely.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'experimenter',
        name: 'Experimenter',
        outputRoot: 'research/20-experiments/',
        agent: {
          provider: 'codex-sdk',
          description: 'Executes one experiment note at a time.',
          prompt:
            'You are the experimenter. Execute the current experiment item only, create the requested note, and summarize the outcome briefly.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep', 'shell'],
          requiresEditAccess: true,
        },
      },
    ],
  };
}

function setupGitRepo(cwd) {
  mkdirSync(path.join(cwd, 'docs'), { recursive: true });
  writeFileSync(
    path.join(cwd, 'docs', 'requirements.md'),
    [
      '# Feature Pack',
      '',
      'Create two short docs:',
      '- `docs/feature-alpha.md` titled `Feature Alpha` with sections `Overview` and `Acceptance`.',
      '- `docs/feature-beta.md` titled `Feature Beta` with sections `Overview` and `Acceptance`.',
      '',
      'Keep each file concise and implementation-focused.',
      '',
    ].join('\n'),
    'utf8',
  );
  execFileSync('git', ['init', '-b', 'main'], { cwd, stdio: 'ignore' });
  execFileSync('git', ['config', 'user.name', 'Cteno E2E'], { cwd, stdio: 'ignore' });
  execFileSync('git', ['config', 'user.email', 'e2e@cteno.local'], { cwd, stdio: 'ignore' });
  execFileSync('git', ['add', '.'], { cwd, stdio: 'ignore' });
  execFileSync('git', ['commit', '--allow-empty', '-m', 'Initial commit'], {
    cwd,
    stdio: 'ignore',
  });
}

export const templateE2ECases = [
  {
    id: 'coding-studio',
    providers: ['claude', 'codex'],
    templateFactory: createCodingStudioTemplate,
    scratchPrefix: 'cteno-e2e-coding',
    timeoutMs: 240_000,
    resultTimeoutMs: 20_000,
    request:
      'We need a short PRD for a group-chat mention feature. Please create it at 10-prd/group-mentions.md with sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria. Keep it under 250 words.',
    outputFiles: {
      prd: cwd => path.join(cwd, '10-prd/group-mentions.md'),
    },
    expectedPrimaryRoleId: 'prd',
    expectClaimWindow: true,
    assert({ turn, primaryDispatch, files }) {
      const fileText = files.prd;
      assertMatch(turn.plan.responseText, /@prd|PRD/i);
      assertMatch(primaryDispatch.resultText, /PRD|group mentions|acceptance/i);
      assertMatch(fileText, /## Goal/i);
      assertMatch(fileText, /## User Story/i);
      assertMatch(fileText, /## Scope/i);
      assertMatch(fileText, /(## Non-Goals|\*\*Out of Scope:\*\*|## Out of Scope)/i);
      assertMatch(fileText, /## Acceptance Criteria/i);
    },
  },
  {
    id: 'opc-solo-company',
    providers: ['claude', 'codex'],
    templateFactory: createOpcSoloCompanyTemplate,
    scratchPrefix: 'cteno-e2e-opc',
    timeoutMs: 300_000,
    resultTimeoutMs: 20_000,
    request:
      'Please prepare a compact monthly close checklist for a solo SaaS founder and write it to company/10-finance/monthly-close-checklist.md. Keep it concise, around 12-18 actionable checklist items total, while still covering cash review, invoices, subscriptions, payroll or contractors, tax prep handoff, and KPI review.',
    outputFiles: {
      checklist: cwd => path.join(cwd, 'company/10-finance/monthly-close-checklist.md'),
    },
    expectedPrimaryRoleId: 'finance',
    assert({ turn, primaryDispatch, files }) {
      const fileText = files.checklist;
      assertMatch(turn.plan.responseText, /@finance|finance/i);
      assertMatch(primaryDispatch.resultText, /finance|checklist|monthly close/i);
      assertMatch(fileText, /(cash|bank|runway)/i);
      assertMatch(fileText, /(invoice|receivables|overdue)/i);
      assertMatch(fileText, /(subscription|MRR|revenue)/i);
      assertMatch(fileText, /(payroll|contractor)/i);
      assertMatch(fileText, /(tax|sales tax|VAT|1099)/i);
      assertMatch(fileText, /(KPI|MRR|burn|churn|CAC)/i);
    },
  },
  {
    id: 'autoresearch',
    providers: ['claude', 'codex'],
    templateFactory: createAutoresearchTemplate,
    scratchPrefix: 'cteno-e2e-autoresearch',
    timeoutMs: 360_000,
    resultTimeoutMs: 30_000,
    codexWorkspaceOptions: {
      networkAccessEnabled: true,
      webSearchMode: 'live',
    },
    request:
      'Start the autoresearch workflow for group-chat mention semantics. Frame the current hypothesis for how collaboration tools like Slack and GitHub handle @mentions, and write the initial hypothesis brief to research/00-lead/mention-hypothesis.md with sections for Hypothesis, Success Criteria, and Next Experiment.',
    outputFiles: {
      brief: cwd => path.join(cwd, 'research/00-lead/mention-hypothesis.md'),
    },
    expectedPrimaryRoleId: 'lead',
    expectWorkflowVote: true,
    expectWorkflowStart: true,
    assert({ turn, primaryDispatch, files }) {
      const fileText = files.brief;
      assertMatch(turn.plan.responseText, /workflow|@lead|hypothesis/i);
      assertMatch(primaryDispatch.resultText, /hypothesis|experiment|mention/i);
      assertMatch(fileText, /Hypothesis/i);
      assertMatch(fileText, /Success Criteria/i);
      assertMatch(fileText, /Next Experiment/i);
      assertMatch(fileText, /Slack/i);
      assertMatch(fileText, /GitHub/i);
    },
  },
  {
    id: 'hybrid-workflow',
    providers: ['hybrid'],
    templateFactory: createHybridWorkflowTemplate,
    scratchPrefix: 'cteno-e2e-hybrid-workflow',
    timeoutMs: 600_000,
    resultTimeoutMs: 30_000,
    hybridPermissionMode: 'bypassPermissions',
    request:
      '[workflow] Run the full workflow end-to-end. Create the plan, implement the note, review it, and finish the workflow.',
    outputFiles: {
      plan: cwd => path.join(cwd, '00-management/hybrid-plan.md'),
      code: cwd => path.join(cwd, '40-code/hybrid-note.md'),
      review: cwd => path.join(cwd, '60-review/hybrid-review.md'),
    },
    expectedPrimaryRoleId: 'lead',
    expectedDispatchRoleIds: ['lead', 'coder', 'reviewer'],
    expectWorkflowVote: true,
    expectWorkflowStart: true,
    assert({ turn, events, files }) {
      assertTruthy(turn.workflowVoteWindow, 'Expected workflow vote to open');
      assertTruthy(turn.workflowVoteResponses?.length, 'Expected workflow vote responses');
      assertTruthy(turn.dispatches.length >= 3, 'Expected workflow to dispatch multiple steps');
      assertTruthy(
        turn.dispatches.some(dispatch => dispatch.roleId === 'lead'),
        'Expected Claude planning dispatch',
      );
      assertTruthy(
        turn.dispatches.some(dispatch => dispatch.roleId === 'coder'),
        'Expected Codex implementation dispatch',
      );
      assertTruthy(
        turn.dispatches.some(dispatch => dispatch.roleId === 'reviewer'),
        'Expected Claude review dispatch',
      );

      const workflowStageStarted = events.filter(event => event.type === 'workflow.stage.started');
      const workflowStageCompleted = events.filter(event => event.type === 'workflow.stage.completed');
      const workflowCompleted = events.find(
        event =>
          event.type === 'activity.published' &&
          event.activity.kind === 'workflow_completed',
      );
      const coderDispatch = turn.dispatches.find(dispatch => dispatch.roleId === 'coder');

      assertTruthy(workflowStageStarted.length >= 2, 'Expected workflow stage start events');
      assertTruthy(workflowStageCompleted.length >= 2, 'Expected workflow stage complete events');
      assertTruthy(workflowCompleted, 'Expected workflow_completed activity');
      assertEqual(coderDispatch?.provider, 'codex-sdk');

      assertMatch(files.plan, /## Goal/i);
      assertMatch(files.plan, /## Steps/i);
      assertMatch(files.code, /Codex/i);
      assertMatch(files.review, /approv|looks good|approved/i);
    },
  },
  {
    id: 'hybrid-task-worklist',
    providers: ['hybrid'],
    templateFactory: createHybridTaskWorklistTemplate,
    scratchPrefix: 'cteno-e2e-hybrid-task-worklist',
    timeoutMs: 600_000,
    resultTimeoutMs: 30_000,
    hybridPermissionMode: 'bypassPermissions',
    request:
      '[workflow] Run the task worklist end-to-end. Plan the work, create the tasks.json file, execute each task, review the result, and finish the workflow.',
    outputFiles: {
      plan: cwd => path.join(cwd, '00-management/task-plan.md'),
      tasks: cwd => path.join(cwd, '00-management/tasks.json'),
      featureA: cwd => path.join(cwd, '40-code/feature-a.md'),
      featureB: cwd => path.join(cwd, '40-code/feature-b.md'),
      review: cwd => path.join(cwd, '60-review/worklist-review.md'),
    },
    expectedPrimaryRoleId: 'lead',
    expectedDispatchRoleIds: ['lead', 'coder', 'reviewer'],
    expectWorkflowVote: true,
    expectWorkflowStart: true,
    assert({ turn, files }) {
      const coderDispatches = turn.dispatches.filter(dispatch => dispatch.roleId === 'coder');
      const taskDoc = JSON.parse(files.tasks);

      assertTruthy(coderDispatches.length >= 2, 'Expected at least two coder work-item dispatches');
      assertTruthy(
        coderDispatches.every(dispatch => Boolean(dispatch.workItemId)),
        'Expected coder dispatches to carry workItemId metadata',
      );
      assertTruthy(Array.isArray(taskDoc.items), 'Expected tasks.json to contain an items array');
      assertEqual(taskDoc.items.length, 2);
      assertTruthy(
        taskDoc.items.every(item => item.status === 'completed'),
        'Expected all worklist items to be marked completed',
      );
      assertMatch(files.plan, /## Goal/i);
      assertMatch(files.plan, /## Tasks/i);
      assertMatch(files.featureA, /feature a|task/i);
      assertMatch(files.featureB, /feature b|task/i);
      assertMatch(files.review, /approv|looks good|approved/i);
    },
  },
  {
    id: 'task-gate-coding',
    providers: ['hybrid'],
    templateFactory: ({ models }) =>
      createTaskGateCodingTemplate({
        plannerModel: models.claude,
        coderModel: models.codex,
      }),
    scratchPrefix: 'cteno-e2e-task-gate-coding',
    timeoutMs: 720_000,
    resultTimeoutMs: 30_000,
    hybridPermissionMode: 'bypassPermissions',
    codexWorkspaceOptions: {
      sandboxMode: 'danger-full-access',
    },
    setup: ({ cwd }) => {
      setupGitRepo(cwd);
    },
    request:
      '[workflow] Read the docs in docs/ and implement the requested changes through the gated task workflow. Plan tasks, execute them one by one, evaluate each result, commit approved tasks, and finish the workflow.',
    outputFiles: {
      overview: cwd => path.join(cwd, '00-management/task-overview.md'),
      tasks: cwd => path.join(cwd, '00-management/tasks.json'),
      alpha: cwd => path.join(cwd, 'docs/feature-alpha.md'),
      beta: cwd => path.join(cwd, 'docs/feature-beta.md'),
    },
    expectedPrimaryRoleId: 'planner',
    expectedDispatchRoleIds: ['planner', 'coder'],
    expectWorkflowVote: true,
    expectWorkflowStart: true,
    assert({ turn, cwd, files }) {
      const coderDispatches = turn.dispatches.filter(dispatch => dispatch.roleId === 'coder');
      const evaluatorDispatches = turn.dispatches.filter(
        dispatch =>
          dispatch.roleId === 'planner' && dispatch.workflowNodeId === 'execute_tasks',
      );
      const commitDispatches = turn.dispatches.filter(
        dispatch =>
          dispatch.roleId === 'coder' &&
          dispatch.workflowNodeId === 'execute_tasks' &&
          dispatch.summary?.startsWith('Commit work item '),
      );
      const taskDoc = JSON.parse(files.tasks);
      const gitLog = execFileSync(
        'git',
        ['log', '--oneline', '--format=%s', '--max-count=5'],
        { cwd, encoding: 'utf8' },
      );

      assertTruthy(Array.isArray(taskDoc.items), 'Expected tasks.json to contain an items array');
      assertTruthy(taskDoc.items.length >= 2, 'Expected at least two planned tasks');
      assertTruthy(
        taskDoc.items.every(item => item.status === 'completed'),
        'Expected every planned task to complete',
      );
      assertTruthy(
        coderDispatches.length >= taskDoc.items.length,
        'Expected at least one coder dispatch per task',
      );
      assertTruthy(
        evaluatorDispatches.length >= taskDoc.items.length,
        'Expected at least one evaluator dispatch per task',
      );
      assertTruthy(
        coderDispatches.every(dispatch => Boolean(dispatch.workItemId)),
        'Expected coder dispatches to carry workItemId metadata',
      );
      assertTruthy(
        evaluatorDispatches.every(dispatch => Boolean(dispatch.workItemId)),
        'Expected evaluator dispatches to carry workItemId metadata',
      );
      assertTruthy(
        commitDispatches.length >= taskDoc.items.length,
        'Expected at least one commit dispatch per task',
      );
      assertTruthy(
        commitDispatches.every(dispatch => Boolean(dispatch.workItemId)),
        'Expected commit dispatches to carry workItemId metadata',
      );
      assertMatch(files.overview, /## Goal/i);
      assertMatch(files.overview, /## Execution Strategy/i);
      assertMatch(files.alpha, /Feature Alpha/i);
      assertMatch(files.beta, /Feature Beta/i);
      assertTruthy(
        /task\(.+\):/.test(gitLog),
        `Expected task-scoped commits in git log, received:\n${gitLog}`,
      );
    },
  },
  {
    id: 'autoresearch-worklist',
    providers: ['hybrid'],
    templateFactory: createAutoresearchWorklistTemplate,
    scratchPrefix: 'cteno-e2e-autoresearch-worklist',
    timeoutMs: 600_000,
    resultTimeoutMs: 30_000,
    hybridPermissionMode: 'bypassPermissions',
    request:
      '[workflow] Kick off the autoresearch loop for mention semantics. Frame the hypothesis, plan experiments in a replenishing queue, execute them, and finish the workflow.',
    outputFiles: {
      hypothesis: cwd => path.join(cwd, 'research/00-lead/hypothesis.md'),
      queue: cwd => path.join(cwd, 'research/10-experiments/experiment-queue.json'),
    },
    expectedPrimaryRoleId: 'lead',
    expectedDispatchRoleIds: ['lead', 'experimenter'],
    expectWorkflowVote: true,
    expectWorkflowStart: true,
    assert({ turn, cwd, files }) {
      const experimentDispatches = turn.dispatches.filter(dispatch => dispatch.roleId === 'experimenter');
      const queueDoc = JSON.parse(files.queue);
      const experimentFiles = queueDoc.items.map(item =>
        path.join(cwd, 'research/20-experiments', `${item.id}.md`),
      );

      assertTruthy(experimentDispatches.length >= 2, 'Expected at least two experiment work-item dispatches');
      assertTruthy(
        experimentDispatches.every(dispatch => Boolean(dispatch.workItemId)),
        'Expected experiment dispatches to carry workItemId metadata',
      );
      assertTruthy(Array.isArray(queueDoc.items), 'Expected experiment queue to contain an items array');
      assertEqual(queueDoc.items.length, 2);
      assertTruthy(
        queueDoc.items.every(item => item.status === 'completed'),
        'Expected replenished experiment items to be completed',
      );
      assertMatch(files.hypothesis, /## Hypothesis/i);
      assertMatch(files.hypothesis, /## Success Criteria/i);
      assertMatch(files.hypothesis, /## Next Experiment/i);

      for (const experimentFile of experimentFiles) {
        const experimentText = readFileSync(experimentFile, 'utf8');
        assertTruthy(
          experimentText.trim().length > 0,
          `Expected non-empty experiment note: ${experimentFile}`,
        );
        assertMatch(experimentText, /## Objective/i);
        assertMatch(experimentText, /## Method/i);
        assertMatch(experimentText, /## Expected Signal/i);
      }
    },
  },
];

function assertEqual(actual, expected) {
  if (actual !== expected) {
    throw new Error(`Expected ${JSON.stringify(expected)} but received ${JSON.stringify(actual)}`);
  }
}

function assertMatch(value, pattern) {
  if (typeof value !== 'string' || !pattern.test(value)) {
    throw new Error(`Expected value to match ${pattern}, received: ${JSON.stringify(value)}`);
  }
}

function assertTruthy(value, message) {
  if (!value) {
    throw new Error(message);
  }
}
