import assert from 'node:assert/strict';
import { mkdtemp } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { WorkspaceRuntime } from '../dist/core/runtime.js';
import {
  ClaudeAgentWorkspace,
  CodexSdkWorkspace,
  LocalWorkspacePersistence,
  createClaudeWorkspaceProfile,
  createCodexWorkspaceProfile,
  createCodingStudioTemplate,
  instantiateWorkspace,
} from '../dist/index.js';

class TimeoutCleanupRuntime extends WorkspaceRuntime {
  constructor() {
    super();
    this.state = {
      workspaceId: 'timeout-runtime',
      status: 'running',
      provider: 'codex-sdk',
      roles: {},
      dispatches: {
        'dispatch-1': {
          dispatchId: 'dispatch-1',
          workspaceId: 'timeout-runtime',
          roleId: 'coder',
          provider: 'codex-sdk',
          model: 'gpt-5.4',
          instruction: 'Do the thing.',
          status: 'running',
          createdAt: new Date().toISOString(),
          startedAt: new Date().toISOString(),
        },
      },
      members: {},
      activities: [],
      workflowRuntime: {
        mode: 'group_chat',
      },
    };
  }

  getSnapshot() {
    return {
      ...this.state,
      dispatches: { ...this.state.dispatches },
    };
  }

  async handleDispatchTimeout(dispatchId, error) {
    const dispatch = this.state.dispatches[dispatchId];
    dispatch.status = 'stopped';
    dispatch.completedAt = new Date().toISOString();
    dispatch.lastSummary = `timeout cleanup: ${error.message}`;
  }
}

function makeRunningState(spec, roleId, provider, model) {
  return {
    workspaceId: spec.id,
    status: 'running',
    provider,
    sessionId: 'root-session',
    startedAt: new Date().toISOString(),
    roles: Object.fromEntries(spec.roles.map(role => [role.id, role])),
    dispatches: {
      'dispatch-1': {
        dispatchId: 'dispatch-1',
        workspaceId: spec.id,
        roleId,
        provider,
        model,
        instruction: 'Continue the running task.',
        status: 'running',
        createdAt: new Date().toISOString(),
        startedAt: new Date().toISOString(),
        providerTaskId: 'provider-task-1',
      },
    },
    members: Object.fromEntries(
      spec.roles.map(role => [
        role.id,
        {
          memberId: role.id,
          workspaceId: spec.id,
          roleId: role.id,
          roleName: role.name,
          ...(role.direct !== undefined ? { direct: role.direct } : {}),
          status: role.id === roleId ? 'active' : 'idle',
        },
      ]),
    ),
    activities: [],
    workflowRuntime: {
      mode: 'group_chat',
    },
  };
}

test('runDispatch timeout cleanup returns the stopped dispatch snapshot', async () => {
  const runtime = new TimeoutCleanupRuntime();
  const dispatch = runtime.getSnapshot().dispatches['dispatch-1'];

  const result = await runtime.runDispatch(Promise.resolve(dispatch), { timeoutMs: 5 });

  assert.equal(result.status, 'stopped');
  assert.match(result.lastSummary, /timeout cleanup/i);
});

test('codex close clears a stale running dispatch from persisted state', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-codex-close-'));
  const spec = instantiateWorkspace(
    createCodingStudioTemplate(),
    { id: 'codex-close', name: 'Codex Close', cwd },
    createCodexWorkspaceProfile(),
  );
  const persistence = LocalWorkspacePersistence.fromSpec(spec);
  await persistence.initializeWorkspace(spec);
  await persistence.persistRuntime({
    state: makeRunningState(spec, spec.roles[0].id, 'codex-sdk', 'gpt-5.4'),
    events: [],
    providerState: {
      workspaceId: spec.id,
      provider: 'codex-sdk',
      rootConversationId: 'codex-root-thread',
      memberBindings: {},
      updatedAt: new Date().toISOString(),
    },
  });

  const workspace = await CodexSdkWorkspace.restoreFromLocal({ cwd, workspaceId: spec.id });
  await workspace.close();

  assert.equal(workspace.getSnapshot().status, 'closed');
  assert.equal(workspace.getSnapshot().dispatches['dispatch-1'].status, 'stopped');

  const persisted = await persistence.loadWorkspaceState();
  assert.equal(persisted.status, 'closed');
  assert.equal(persisted.dispatches['dispatch-1'].status, 'stopped');
});

test('claude close clears a stale running dispatch from persisted state', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-claude-close-'));
  const spec = instantiateWorkspace(
    createCodingStudioTemplate(),
    { id: 'claude-close', name: 'Claude Close', cwd },
    createClaudeWorkspaceProfile(),
  );
  const persistence = LocalWorkspacePersistence.fromSpec(spec);
  await persistence.initializeWorkspace(spec);
  await persistence.persistRuntime({
    state: makeRunningState(spec, spec.roles[0].id, 'claude-agent-sdk', 'claude-sonnet-4-5'),
    events: [],
    providerState: {
      workspaceId: spec.id,
      provider: 'claude-agent-sdk',
      rootConversationId: 'claude-root-session',
      memberBindings: {},
      updatedAt: new Date().toISOString(),
    },
  });

  const workspace = await ClaudeAgentWorkspace.restoreFromLocal({ cwd, workspaceId: spec.id });
  await workspace.close();

  assert.equal(workspace.getSnapshot().status, 'closed');
  assert.equal(workspace.getSnapshot().dispatches['dispatch-1'].status, 'stopped');

  const persisted = await persistence.loadWorkspaceState();
  assert.equal(persisted.status, 'closed');
  assert.equal(persisted.dispatches['dispatch-1'].status, 'stopped');
});
