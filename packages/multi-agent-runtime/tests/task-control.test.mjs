import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  applyTaskControlToolCall,
  applyTaskHandoff,
  applyTaskReview,
  applyTaskStatusUpdate,
  applyTaskControlProtocolFromText,
  executeWorkflow,
  parseTaskControlProtocol,
} from '../dist/index.js';

test('task control tools accumulate structured status, handoff, review, and evidence on a dispatch', async () => {
  const now = () => '2026-04-15T00:00:00.000Z';
  let dispatch = {
    dispatchId: 'dispatch-1',
    workspaceId: 'workspace-1',
    roleId: 'coder',
    instruction: 'Implement task.',
    workItemId: 'task-1',
    status: 'completed',
    createdAt: now(),
  };

  dispatch = applyTaskControlToolCall(
    dispatch,
    {
      toolName: 'task.set_status',
      input: {
        status: 'running',
        summary: 'Started implementation.',
      },
    },
    { actorRoleId: 'coder', now },
  );
  dispatch = applyTaskHandoff(
    dispatch,
    {
      summary: 'Implementation is ready for review.',
      details: 'Updated the main code path and left notes in the diff.',
      toRoleId: 'reviewer',
    },
    { actorRoleId: 'coder', now },
  );
  dispatch = applyTaskReview(
    dispatch,
    {
      verdict: 'approved',
      summary: 'Looks good after validation.',
      issues: [],
      retryable: false,
    },
    { actorRoleId: 'reviewer', now },
  );
  dispatch = applyTaskControlToolCall(
    dispatch,
    {
      toolName: 'task.record_evidence',
      input: {
        kind: 'test',
        summary: 'Unit smoke passed.',
        path: 'tmp/test.log',
      },
    },
    { actorRoleId: 'coder', now },
  );

  assert.equal(dispatch.taskControl?.latestStatus?.status, 'running');
  assert.equal(dispatch.taskControl?.latestStatus?.actorRoleId, 'coder');
  assert.equal(dispatch.taskControl?.latestHandoff?.toRoleId, 'reviewer');
  assert.equal(dispatch.taskControl?.latestReview?.verdict, 'approved');
  assert.equal(dispatch.taskControl?.latestReview?.actorRoleId, 'reviewer');
  assert.equal(dispatch.taskControl?.evidence.length, 1);
  assert.equal(dispatch.taskControl?.evidence[0]?.evidenceId, 'task-1:evidence:1');
});

test('task control protocol parses fenced blocks and strips them from final text', async () => {
  const text = [
    'Implemented the requested change and ran focused validation.',
    '',
    '```task-control',
    '{"toolName":"task.set_status","input":{"status":"completed","summary":"Implementation complete."}}',
    '{"toolName":"task.write_handoff","input":{"summary":"Ready for review.","details":"Ran focused checks.","toRoleId":"reviewer"}}',
    '```',
  ].join('\n');

  const parsed = parseTaskControlProtocol(text);
  assert.equal(parsed.calls.length, 2);
  assert.equal(parsed.cleanedText, 'Implemented the requested change and ran focused validation.');

  const applied = applyTaskControlProtocolFromText(
    {
      dispatchId: 'dispatch-2',
      workspaceId: 'workspace-1',
      roleId: 'coder',
      instruction: 'Implement task.',
      workItemId: 'task-1',
      status: 'completed',
      createdAt: '2026-04-15T00:00:00.000Z',
    },
    text,
  );

  assert.equal(applied.dispatch.taskControl?.latestStatus?.status, 'completed');
  assert.equal(applied.dispatch.taskControl?.latestHandoff?.toRoleId, 'reviewer');
});

test('worklist execution prefers structured review verdicts over ambiguous prose', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-task-control-'));
  const taskListPath = path.join(cwd, 'tasks.json');
  await writeFile(
    taskListPath,
    `${JSON.stringify(
      {
        version: 1,
        items: [
          {
            id: 'task-1',
            title: 'Ambiguous review handling',
            description: 'Exercise structured task review.',
            status: 'pending',
            attempts: 0,
            maxAttempts: 1,
          },
        ],
      },
      null,
      2,
    )}\n`,
    'utf8',
  );

  const spec = {
    id: 'task-control-review',
    name: 'Task Control Review',
    provider: 'codex-sdk',
    cwd,
    roles: [
      {
        id: 'coder',
        name: 'Coder',
        agent: {
          description: 'Implements a work item.',
          prompt: 'Implement the task.',
        },
      },
      {
        id: 'reviewer',
        name: 'Reviewer',
        agent: {
          description: 'Reviews a work item.',
          prompt: 'Review the task.',
        },
      },
    ],
    defaultRoleId: 'coder',
    coordinatorRoleId: 'coder',
    workflow: {
      mode: 'pipeline',
      entryNodeId: 'execute_tasks',
      nodes: [
        {
          id: 'execute_tasks',
          type: 'worklist',
          roleId: 'coder',
          workerRoleId: 'coder',
          worklistArtifactId: 'task_list',
          stopOnItemFailure: true,
          itemLifecycle: {
            evaluate: {
              roleId: 'reviewer',
              onReject: 'fail',
            },
          },
        },
        {
          id: 'complete',
          type: 'complete',
        },
      ],
      edges: [{ from: 'execute_tasks', to: 'complete', when: 'success' }],
    },
    artifacts: [
      {
        id: 'task_list',
        kind: 'task_list',
        path: 'tasks.json',
      },
    ],
  };

  const runAssignment = async assignment => {
    const baseDispatch = {
      dispatchId: `${assignment.roleId}-${assignment.workItemId ?? 'workflow'}`,
      workspaceId: spec.id,
      roleId: assignment.roleId,
      instruction: assignment.instruction,
      summary: assignment.summary,
      workItemId: assignment.workItemId,
      workflowNodeId: assignment.workflowNodeId,
      stageId: assignment.stageId,
      status: 'completed',
      createdAt: '2026-04-15T00:00:00.000Z',
      completedAt: '2026-04-15T00:00:01.000Z',
    };

    if (assignment.roleId === 'coder') {
      return applyTaskHandoff(
        applyTaskStatusUpdate(
          {
            ...baseDispatch,
            resultText: 'Patched the workflow file and updated validation.',
            lastSummary: 'Coder finished implementation.',
          },
          {
            status: 'completed',
            summary: 'Implementation complete.',
          },
          { actorRoleId: 'coder', now: () => '2026-04-15T00:00:02.000Z' },
        ),
        {
          summary: 'Ready for structured review.',
          details: 'Please verify the reject path and persisted status.',
          toRoleId: 'reviewer',
        },
        { actorRoleId: 'coder', now: () => '2026-04-15T00:00:03.000Z' },
      );
    }

    return applyTaskReview(
      {
        ...baseDispatch,
        resultText:
          'REJECTED: blocked for now, but it looks good and should pass after a small fix.',
        lastSummary: 'Reviewer found one remaining gap.',
      },
      {
        verdict: 'rejected',
        summary: 'Structured review rejects the task until the remaining gap is fixed.',
        issues: ['Missing end-to-end proof for the reject path.'],
        retryable: false,
      },
      { actorRoleId: 'reviewer', now: () => '2026-04-15T00:00:04.000Z' },
    );
  };

  await executeWorkflow(spec, { message: 'Run the structured review task.' }, runAssignment);

  const persisted = JSON.parse(await readFile(taskListPath, 'utf8'));
  assert.equal(persisted.items[0].status, 'failed');
  assert.equal(persisted.items[0].attempts, 1);
});
