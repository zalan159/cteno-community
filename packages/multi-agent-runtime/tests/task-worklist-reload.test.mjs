import assert from 'node:assert/strict';
import { execFile as execFileCallback } from 'node:child_process';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import test from 'node:test';

import {
  addTask,
  createClaudeWorkspaceProfile,
  createManualTaskGateCodingTemplate,
  executeWorkflow,
  instantiateWorkspace,
  setTaskStatus,
} from '../dist/index.js';

const execFile = promisify(execFileCallback);

function makeDispatch(spec, assignment, sequence, resultText) {
  return {
    dispatchId: `dispatch-${sequence}`,
    workspaceId: spec.id,
    roleId: assignment.roleId,
    provider: assignment.provider,
    ...(assignment.model ? { model: assignment.model } : {}),
    instruction: assignment.instruction,
    status: 'completed',
    createdAt: new Date().toISOString(),
    completedAt: new Date().toISOString(),
    resultText,
    lastSummary: resultText,
    ...(assignment.workflowNodeId ? { workflowNodeId: assignment.workflowNodeId } : {}),
    ...(assignment.stageId ? { stageId: assignment.stageId } : {}),
    ...(assignment.workItemId ? { workItemId: assignment.workItemId } : {}),
  };
}

test('worklist reload picks up a task inserted after the current item completes', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-worklist-reload-'));
  await mkdir(path.join(cwd, '00-management'), { recursive: true });
  await writeFile(
    path.join(cwd, '00-management', 'tasks.json'),
    `${JSON.stringify(
      {
        version: 1,
        items: [
          {
            id: 'task-1',
            title: 'First task',
            description: 'Finish the first item.',
            status: 'pending',
            attempts: 0,
            maxAttempts: 2,
            files: ['apps/client/desktop/src/lib.rs'],
            acceptanceCriteria: ['Task 1 is completed.'],
          },
        ],
      },
      null,
      2,
    )}\n`,
  );

  const spec = instantiateWorkspace(
    createManualTaskGateCodingTemplate({
      reviewerModel: 'claude-opus-4-6',
      coderModel: 'gpt-5.4',
    }),
    { id: 'worklist-reload', name: 'Worklist Reload', cwd },
    createClaudeWorkspaceProfile({
      model: 'claude-opus-4-6',
      permissionMode: 'bypassPermissions',
    }),
  );

  let sequence = 0;
  const result = await executeWorkflow(
    spec,
    { message: 'Run the finite worklist.' },
    async assignment => {
      sequence += 1;
      if (
        assignment.roleId === 'coder' &&
        assignment.summary.includes('Commit work item task-1')
      ) {
        await addTask(
          { cwd, taskListPath: '00-management/tasks.json' },
          {
            id: 'task-2',
            title: 'Inserted follow-up',
            description: 'Run after task-1 completes.',
            afterId: 'task-1',
            files: ['apps/client/desktop/src/happy_client/manager.rs'],
            acceptanceCriteria: ['Task 2 is completed.'],
          },
        );
        return makeDispatch(spec, assignment, sequence, 'Committed task(task-1): First task');
      }

      const resultText =
        assignment.roleId === 'reviewer'
          ? `APPROVED: ${assignment.workItemId} is complete.`
          : assignment.summary.includes('Commit work item')
            ? `Committed task(${assignment.workItemId}): ${assignment.summary}`
            : `Implemented ${assignment.workItemId}.`;
      return makeDispatch(spec, assignment, sequence, resultText);
    },
  );

  const savedTaskList = JSON.parse(
    await readFile(path.join(cwd, '00-management', 'tasks.json'), 'utf8'),
  );

  assert.equal(result.completionStatus, 'done');
  assert.deepEqual(
    result.dispatches.map(dispatch => [dispatch.roleId, dispatch.workItemId]),
    [
      ['coder', 'task-1'],
      ['reviewer', 'task-1'],
      ['coder', 'task-1'],
      ['coder', 'task-2'],
      ['reviewer', 'task-2'],
      ['coder', 'task-2'],
    ],
  );
  assert.equal(savedTaskList.items.length, 2);
  assert.equal(savedTaskList.items[0].status, 'completed');
  assert.equal(savedTaskList.items[1].status, 'completed');
});

test('worklist can abandon a task and continue with inserted follow-up tasks', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-worklist-abandon-'));
  await mkdir(path.join(cwd, '00-management'), { recursive: true });
  await writeFile(
    path.join(cwd, '00-management', 'tasks.json'),
    `${JSON.stringify(
      {
        version: 1,
        items: [
          {
            id: 'task-big',
            title: 'Too large task',
            description: 'Split this work into a narrower follow-up.',
            status: 'pending',
            attempts: 0,
            maxAttempts: 2,
            files: ['apps/client/desktop/src/happy_client/manager.rs'],
            acceptanceCriteria: ['Task is split safely.'],
          },
        ],
      },
      null,
      2,
    )}\n`,
  );

  const spec = instantiateWorkspace(
    createManualTaskGateCodingTemplate({
      reviewerModel: 'claude-opus-4-6',
      coderModel: 'gpt-5.4',
    }),
    { id: 'worklist-abandon', name: 'Worklist Abandon', cwd },
    createClaudeWorkspaceProfile({
      model: 'claude-opus-4-6',
      permissionMode: 'bypassPermissions',
    }),
  );

  let sequence = 0;
  const result = await executeWorkflow(
    spec,
    { message: 'Run the finite worklist.' },
    async assignment => {
      sequence += 1;
      if (assignment.roleId === 'coder' && assignment.workItemId === 'task-big') {
        await addTask(
          { cwd, taskListPath: '00-management/tasks.json' },
          {
            id: 'task-big-part-1',
            title: 'Split follow-up',
            description: 'Complete the smaller extracted task.',
            afterId: 'task-big',
            files: ['apps/client/desktop/src/happy_client/manager.rs'],
            acceptanceCriteria: ['Follow-up task is completed.'],
          },
        );
        await setTaskStatus(
          { cwd, taskListPath: '00-management/tasks.json' },
          'task-big',
          'abandoned',
          'Split into a narrower follow-up task.',
        );
        return makeDispatch(
          spec,
          assignment,
          sequence,
          'Split task-big into task-big-part-1 and abandoned the original task with handoff.',
        );
      }

      const resultText =
        assignment.roleId === 'reviewer'
          ? `APPROVED: ${assignment.workItemId} is complete.`
          : assignment.summary.includes('Commit work item')
            ? `Committed task(${assignment.workItemId}): ${assignment.summary}`
            : `Implemented ${assignment.workItemId}.`;
      return makeDispatch(spec, assignment, sequence, resultText);
    },
  );

  const savedTaskList = JSON.parse(
    await readFile(path.join(cwd, '00-management', 'tasks.json'), 'utf8'),
  );

  assert.equal(result.completionStatus, 'done');
  assert.deepEqual(
    result.dispatches.map(dispatch => [dispatch.roleId, dispatch.workItemId]),
    [
      ['coder', 'task-big'],
      ['coder', 'task-big-part-1'],
      ['reviewer', 'task-big-part-1'],
      ['coder', 'task-big-part-1'],
    ],
  );
  assert.equal(savedTaskList.items[0].status, 'abandoned');
  assert.equal(savedTaskList.items[1].status, 'completed');
});

test('task CLI updates the task ledger and appends shared lessons markdown', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-task-cli-'));
  await mkdir(path.join(cwd, '00-management'), { recursive: true });
  await writeFile(
    path.join(cwd, '00-management', 'tasks.json'),
    `${JSON.stringify({ version: 1, items: [] }, null, 2)}\n`,
  );

  const cliPath = path.resolve(
    path.dirname(new URL(import.meta.url).pathname),
    '../dist/cli/taskCli.js',
  );

  await execFile('node', [
    cliPath,
    'add',
    '--cwd',
    cwd,
    '--tasks',
    '00-management/tasks.json',
    '--id',
    'task-cli-1',
    '--title',
    'CLI task',
    '--description',
    'Created via CLI.',
    '--criteria',
    'Task is present in the ledger.',
  ]);
  await execFile('node', [
    cliPath,
    'set-status',
    'task-cli-1',
    'blocked',
    '--cwd',
    cwd,
    '--tasks',
    '00-management/tasks.json',
    '--reason',
    'Waiting on external environment.',
  ]);
  await execFile('node', [
    cliPath,
    'lessons:add',
    '--cwd',
    cwd,
    '--lessons',
    '00-management/shared-lessons.md',
    '--title',
    'Avoid stale daemon locks',
    '--body',
    'If the daemon root is synthetic, clear stale lock files before assuming the host runtime is broken.',
    '--task-id',
    'task-cli-1',
    '--role',
    'coder',
  ]);

  const savedTaskList = JSON.parse(
    await readFile(path.join(cwd, '00-management', 'tasks.json'), 'utf8'),
  );
  const lessons = await readFile(path.join(cwd, '00-management', 'shared-lessons.md'), 'utf8');

  assert.equal(savedTaskList.items[0].status, 'blocked');
  assert.equal(savedTaskList.items[0].metadata.statusReason, 'Waiting on external environment.');
  assert.match(lessons, /# Shared Lessons/);
  assert.match(lessons, /Avoid stale daemon locks/);
  assert.match(lessons, /task-cli-1/);
});
