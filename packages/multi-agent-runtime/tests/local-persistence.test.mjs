import assert from 'node:assert/strict';
import { access, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  ClaudeAgentWorkspace,
  CodexSdkWorkspace,
  HybridWorkspace,
  LocalWorkspacePersistence,
  createClaudeWorkspaceProfile,
  createCodexWorkspaceProfile,
  createCodingStudioTemplate,
  createTaskGateCodingTemplate,
  instantiateWorkspace,
} from '../dist/index.js';

function makeRuntimeState(spec) {
  return {
    workspaceId: spec.id,
    status: 'running',
    provider: spec.provider,
    sessionId: 'root-session',
    startedAt: new Date().toISOString(),
    roles: Object.fromEntries(spec.roles.map(role => [role.id, role])),
    dispatches: {},
    members: Object.fromEntries(
      spec.roles.map(role => [
        role.id,
        {
          memberId: role.id,
          workspaceId: spec.id,
          roleId: role.id,
          roleName: role.name,
          ...(role.direct !== undefined ? { direct: role.direct } : {}),
          status: 'idle',
        },
      ]),
    ),
    activities: [],
    workflowRuntime: {
      mode: 'group_chat',
    },
  };
}

test('claude adapter restores and deletes a persisted workspace', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-claude-'));
  const template = createCodingStudioTemplate();
  const spec = instantiateWorkspace(
    template,
    { id: 'claude-restore', name: 'Claude Restore', cwd },
    createClaudeWorkspaceProfile(),
  );
  const persistence = LocalWorkspacePersistence.fromSpec(spec);
  await persistence.initializeWorkspace(spec);
  await persistence.persistRuntime({
    state: makeRuntimeState(spec),
    events: [],
    providerState: {
      workspaceId: spec.id,
      provider: spec.provider,
      rootConversationId: 'claude-root-session',
      memberBindings: {},
      updatedAt: new Date().toISOString(),
    },
  });

  const workspace = await ClaudeAgentWorkspace.restoreFromLocal({ cwd, workspaceId: spec.id });
  assert.equal(workspace.getSnapshot().sessionId, 'root-session');
  assert.equal(workspace.getPersistenceRoot(), path.join(cwd, '.multi-agent-runtime', spec.id));

  await workspace.deleteWorkspace();
  await assert.rejects(access(path.join(cwd, '.multi-agent-runtime', spec.id)));
});

test('codex adapter restores and deletes a persisted workspace', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-codex-'));
  const template = createCodingStudioTemplate();
  const spec = instantiateWorkspace(
    template,
    { id: 'codex-restore', name: 'Codex Restore', cwd },
    createCodexWorkspaceProfile(),
  );
  const persistence = LocalWorkspacePersistence.fromSpec(spec);
  await persistence.initializeWorkspace(spec);
  await persistence.persistRuntime({
    state: makeRuntimeState(spec),
    events: [],
    providerState: {
      workspaceId: spec.id,
      provider: spec.provider,
      rootConversationId: 'codex-root-thread',
      memberBindings: {
        prd: {
          roleId: 'prd',
          providerConversationId: 'thread-prd-123',
          kind: 'thread',
          updatedAt: new Date().toISOString(),
        },
      },
      updatedAt: new Date().toISOString(),
    },
  });

  const workspace = await CodexSdkWorkspace.restoreFromLocal({ cwd, workspaceId: spec.id });
  assert.equal(workspace.getSnapshot().sessionId, 'root-session');
  assert.equal(workspace.getPersistenceRoot(), path.join(cwd, '.multi-agent-runtime', spec.id));

  await workspace.deleteWorkspace();
  await assert.rejects(access(path.join(cwd, '.multi-agent-runtime', spec.id)));
});

test('hybrid adapter restores from persisted child workspaces', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-hybrid-'));
  const hybridSpec = instantiateWorkspace(
    createTaskGateCodingTemplate({
      plannerModel: 'claude-opus-4-6',
      coderModel: 'gpt-5.4',
    }),
    { id: 'hybrid-restore', name: 'Hybrid Restore', cwd },
    createClaudeWorkspaceProfile({
      model: 'claude-opus-4-6',
      permissionMode: 'bypassPermissions',
    }),
  );

  const claudeSpec = {
    ...hybridSpec,
    id: `${hybridSpec.id}--claude`,
    provider: 'claude-agent-sdk',
    defaultProvider: 'claude-agent-sdk',
    defaultModel: 'claude-opus-4-6',
    model: 'claude-opus-4-6',
    roles: [hybridSpec.roles.find(role => role.id === 'planner')],
    defaultRoleId: 'planner',
    coordinatorRoleId: 'planner',
  };
  const codexSpec = {
    ...hybridSpec,
    id: `${hybridSpec.id}--codex`,
    provider: 'codex-sdk',
    defaultProvider: 'codex-sdk',
    defaultModel: 'gpt-5.4',
    model: 'gpt-5.4',
    roles: [hybridSpec.roles.find(role => role.id === 'coder')],
    defaultRoleId: 'coder',
    coordinatorRoleId: 'coder',
    claimPolicy: {
      ...hybridSpec.claimPolicy,
      fallbackRoleId: 'coder',
    },
  };

  const claudePersistence = LocalWorkspacePersistence.fromSpec(claudeSpec);
  await claudePersistence.initializeWorkspace(claudeSpec);
  await claudePersistence.persistRuntime({
    state: {
      ...makeRuntimeState(claudeSpec),
      dispatches: {
        'dispatch-planner': {
          dispatchId: 'dispatch-planner',
          workspaceId: claudeSpec.id,
          roleId: 'planner',
          provider: 'claude-agent-sdk',
          model: 'claude-opus-4-6',
          instruction: 'Plan tasks.',
          status: 'completed',
          summary: 'Planner finished.',
          createdAt: new Date().toISOString(),
          completedAt: new Date().toISOString(),
        },
      },
      members: {
        planner: {
          memberId: 'planner',
          workspaceId: claudeSpec.id,
          roleId: 'planner',
          roleName: 'Planner',
          provider: 'claude-agent-sdk',
          status: 'idle',
        },
      },
      activities: [
        {
          activityId: 'activity-planner',
          workspaceId: claudeSpec.id,
          kind: 'member_delivered',
          visibility: 'public',
          text: 'Planner finished.',
          createdAt: new Date().toISOString(),
          roleId: 'planner',
          memberId: 'planner',
          dispatchId: 'dispatch-planner',
        },
      ],
    },
    events: [],
    providerState: {
      workspaceId: claudeSpec.id,
      provider: 'claude-agent-sdk',
      rootConversationId: 'claude-root-session',
      memberBindings: {},
      updatedAt: new Date().toISOString(),
    },
  });

  const codexPersistence = LocalWorkspacePersistence.fromSpec(codexSpec);
  await codexPersistence.initializeWorkspace(codexSpec);
  await codexPersistence.persistRuntime({
    state: {
      ...makeRuntimeState(codexSpec),
      dispatches: {
        'dispatch-coder': {
          dispatchId: 'dispatch-coder',
          workspaceId: codexSpec.id,
          roleId: 'coder',
          provider: 'codex-sdk',
          model: 'gpt-5.4',
          instruction: 'Implement task.',
          status: 'running',
          summary: 'Coder is implementing.',
          createdAt: new Date().toISOString(),
        },
      },
      members: {
        coder: {
          memberId: 'coder',
          workspaceId: codexSpec.id,
          roleId: 'coder',
          roleName: 'Coder',
          provider: 'codex-sdk',
          status: 'active',
        },
      },
      activities: [
        {
          activityId: 'activity-coder',
          workspaceId: codexSpec.id,
          kind: 'dispatch_started',
          visibility: 'public',
          text: 'Coder is implementing.',
          createdAt: new Date().toISOString(),
          roleId: 'coder',
          memberId: 'coder',
          dispatchId: 'dispatch-coder',
        },
      ],
    },
    events: [],
    providerState: {
      workspaceId: codexSpec.id,
      provider: 'codex-sdk',
      rootConversationId: 'codex-root-thread',
      memberBindings: {
        coder: {
          roleId: 'coder',
          providerConversationId: 'thread-coder-123',
          kind: 'thread',
          updatedAt: new Date().toISOString(),
        },
      },
      updatedAt: new Date().toISOString(),
    },
  });

  const workspace = await HybridWorkspace.restoreFromLocal({
    cwd,
    workspaceId: hybridSpec.id,
    defaultModels: {
      'claude-agent-sdk': 'claude-opus-4-6',
      'codex-sdk': 'gpt-5.4',
    },
  });
  const snapshot = workspace.getSnapshot();

  assert.equal(snapshot.provider, 'hybrid');
  assert.equal(snapshot.status, 'running');
  assert.deepEqual(Object.keys(snapshot.roles).sort(), ['coder', 'planner']);
  assert.deepEqual(Object.keys(snapshot.dispatches).sort(), ['dispatch-coder', 'dispatch-planner']);
  assert.equal(snapshot.dispatches['dispatch-planner'].workspaceId, hybridSpec.id);
  assert.equal(snapshot.dispatches['dispatch-coder'].workspaceId, hybridSpec.id);
  assert.equal(snapshot.members.planner.provider, 'claude-agent-sdk');
  assert.equal(snapshot.members.coder.provider, 'codex-sdk');

  await workspace.deleteWorkspace();
  await assert.rejects(access(path.join(cwd, '.multi-agent-runtime', claudeSpec.id)));
  await assert.rejects(access(path.join(cwd, '.multi-agent-runtime', codexSpec.id)));
});

test('hybrid preserves group-chat children but restarts workflow dispatches statelessly', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-hybrid-lazy-'));
  const hybridSpec = instantiateWorkspace(
    createTaskGateCodingTemplate({
      plannerModel: 'claude-opus-4-6',
      coderModel: 'gpt-5.4',
    }),
    { id: 'hybrid-lazy', name: 'Hybrid Lazy', cwd },
    createClaudeWorkspaceProfile({
      model: 'claude-opus-4-6',
      permissionMode: 'bypassPermissions',
    }),
  );

  const claudeSpec = {
    ...hybridSpec,
    id: `${hybridSpec.id}--claude`,
    provider: 'claude-agent-sdk',
    defaultProvider: 'claude-agent-sdk',
    defaultModel: 'claude-opus-4-6',
    model: 'claude-opus-4-6',
    roles: [hybridSpec.roles.find(role => role.id === 'planner')],
    defaultRoleId: 'planner',
    coordinatorRoleId: 'planner',
  };
  const codexSpec = {
    ...hybridSpec,
    id: `${hybridSpec.id}--codex`,
    provider: 'codex-sdk',
    defaultProvider: 'codex-sdk',
    defaultModel: 'gpt-5.4',
    model: 'gpt-5.4',
    roles: [hybridSpec.roles.find(role => role.id === 'coder')],
    defaultRoleId: 'coder',
    coordinatorRoleId: 'coder',
    claimPolicy: {
      ...hybridSpec.claimPolicy,
      fallbackRoleId: 'coder',
    },
  };

  class FakeChildWorkspace {
    constructor(spec, provider) {
      this.spec = spec;
      this.provider = provider;
      this.startCount = 0;
      this.assignCount = 0;
      this.snapshot = {
        ...makeRuntimeState(spec),
        status: 'closed',
      };
    }

    onEvent() {
      return () => {};
    }

    getSnapshot() {
      return {
        ...this.snapshot,
        roles: { ...this.snapshot.roles },
        dispatches: { ...this.snapshot.dispatches },
        members: { ...this.snapshot.members },
        activities: [...this.snapshot.activities],
        workflowRuntime: { ...this.snapshot.workflowRuntime },
      };
    }

    async start() {
      if (this.snapshot.status === 'running') {
        return;
      }
      this.startCount += 1;
      this.snapshot.status = 'running';
    }

    async assignRoleTask(request) {
      this.assignCount += 1;
      return {
        dispatchId: `${request.roleId}-dispatch-${this.assignCount}`,
        workspaceId: this.spec.id,
        roleId: request.roleId,
        provider: this.provider,
        model: this.provider === 'codex-sdk' ? 'gpt-5.4' : 'claude-opus-4-6',
        instruction: request.instruction,
        status: 'completed',
        createdAt: new Date().toISOString(),
        completedAt: new Date().toISOString(),
        resultText: `${request.roleId} completed`,
        lastSummary: `${request.roleId} completed`,
      };
    }

    async close() {
      this.snapshot.status = 'closed';
    }

    async deleteWorkspace() {}
  }

  const fakeClaude = new FakeChildWorkspace(claudeSpec, 'claude-agent-sdk');
  const fakeCodex = new FakeChildWorkspace(codexSpec, 'codex-sdk');
  const workspace = new HybridWorkspace({
    spec: hybridSpec,
    restoredChildren: {
      'claude-agent-sdk': fakeClaude,
      'codex-sdk': fakeCodex,
    },
    restoredChildSpecs: {
      'claude-agent-sdk': claudeSpec,
      'codex-sdk': codexSpec,
    },
  });

  await workspace.start();
  assert.equal(fakeClaude.startCount, 0);
  assert.equal(fakeCodex.startCount, 0);

  await workspace.assignRoleTask({
    roleId: 'coder',
    instruction: 'Implement the next task.',
  });
  assert.equal(fakeCodex.startCount, 1);
  assert.equal(fakeCodex.assignCount, 1);

  await workspace.assignRoleTask({
    roleId: 'coder',
    instruction: 'Continue the coordinator-led group chat task.',
  });
  assert.equal(fakeCodex.startCount, 1);
  assert.equal(fakeCodex.assignCount, 2);

  await workspace.assignRoleTask({
    roleId: 'coder',
    instruction: 'Implement the workflow task with a fresh worker.',
    workflowNodeId: 'execute_tasks',
    stageId: 'execution',
    workItemId: 'task-ephemeral',
  });
  assert.equal(fakeCodex.startCount, 2);
  assert.equal(fakeCodex.assignCount, 3);
});

test('hybrid adapter resumes a persisted worklist from the active node', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-hybrid-resume-'));
  const hybridSpec = instantiateWorkspace(
    createTaskGateCodingTemplate({
      plannerModel: 'claude-opus-4-6',
      coderModel: 'gpt-5.4',
    }),
    { id: 'hybrid-resume', name: 'Hybrid Resume', cwd },
    createClaudeWorkspaceProfile({
      model: 'claude-opus-4-6',
      permissionMode: 'bypassPermissions',
    }),
  );

  const claudeSpec = {
    ...hybridSpec,
    id: `${hybridSpec.id}--claude`,
    provider: 'claude-agent-sdk',
    defaultProvider: 'claude-agent-sdk',
    defaultModel: 'claude-opus-4-6',
    model: 'claude-opus-4-6',
    roles: [hybridSpec.roles.find(role => role.id === 'planner')],
    defaultRoleId: 'planner',
    coordinatorRoleId: 'planner',
  };
  const codexSpec = {
    ...hybridSpec,
    id: `${hybridSpec.id}--codex`,
    provider: 'codex-sdk',
    defaultProvider: 'codex-sdk',
    defaultModel: 'gpt-5.4',
    model: 'gpt-5.4',
    roles: [hybridSpec.roles.find(role => role.id === 'coder')],
    defaultRoleId: 'coder',
    coordinatorRoleId: 'coder',
    claimPolicy: {
      ...hybridSpec.claimPolicy,
      fallbackRoleId: 'coder',
    },
  };

  const userMessageActivity = {
    activityId: 'activity-user',
    workspaceId: claudeSpec.id,
    kind: 'user_message',
    visibility: 'public',
    text: 'Resume the gated coding workflow.',
    createdAt: new Date().toISOString(),
  };

  const claudePersistence = LocalWorkspacePersistence.fromSpec(claudeSpec);
  await claudePersistence.initializeWorkspace(claudeSpec);
  await claudePersistence.persistRuntime({
    state: {
      ...makeRuntimeState(claudeSpec),
      status: 'closed',
      activities: [userMessageActivity],
      workflowRuntime: {
        mode: 'workflow_running',
        activeNodeId: 'execute_tasks',
      },
    },
    events: [],
    providerState: {
      workspaceId: claudeSpec.id,
      provider: 'claude-agent-sdk',
      rootConversationId: 'claude-root-session',
      memberBindings: {},
      updatedAt: new Date().toISOString(),
    },
  });

  const codexPersistence = LocalWorkspacePersistence.fromSpec(codexSpec);
  await codexPersistence.initializeWorkspace(codexSpec);
  await codexPersistence.persistRuntime({
    state: {
      ...makeRuntimeState(codexSpec),
      status: 'closed',
      workflowRuntime: {
        mode: 'workflow_running',
        activeNodeId: 'execute_tasks',
      },
    },
    events: [],
    providerState: {
      workspaceId: codexSpec.id,
      provider: 'codex-sdk',
      rootConversationId: 'codex-root-thread',
      memberBindings: {
        coder: {
          roleId: 'coder',
          providerConversationId: 'thread-coder-123',
          kind: 'thread',
          updatedAt: new Date().toISOString(),
        },
      },
      updatedAt: new Date().toISOString(),
    },
  });

  await mkdir(path.join(cwd, '00-management'), { recursive: true });
  await writeFile(
    path.join(cwd, '00-management', 'tasks.json'),
    `${JSON.stringify(
      {
        version: 1,
        items: [
          {
            id: 'task-1',
            title: 'Resume me',
            description: 'Finish the interrupted task.',
            status: 'running',
            attempts: 1,
            maxAttempts: 3,
            files: ['apps/client/desktop/src/lib.rs'],
            acceptanceCriteria: ['Task completes cleanly.'],
          },
        ],
      },
      null,
      2,
    )}\n`,
    'utf8',
  );

  const workspace = await HybridWorkspace.restoreFromLocal({
    cwd,
    workspaceId: hybridSpec.id,
    defaultModels: {
      'claude-agent-sdk': 'claude-opus-4-6',
      'codex-sdk': 'gpt-5.4',
    },
  });
  workspace.active = true;
  workspace.initialized = true;

  let sequence = 0;
  workspace.assignRoleTask = async request => {
    sequence += 1;
    const resultText =
      request.roleId === 'planner'
        ? 'APPROVED: The task is complete.'
        : request.instruction.includes('Create exactly one git commit')
          ? 'Committed task(task-1): Resume me'
          : 'Implemented task-1 successfully.';
    return {
      dispatchId: `dispatch-${sequence}`,
      workspaceId: hybridSpec.id,
      roleId: request.roleId,
      provider:
        request.roleId === 'planner' ? 'claude-agent-sdk' : 'codex-sdk',
      model:
        request.roleId === 'planner' ? 'claude-opus-4-6' : 'gpt-5.4',
      instruction: request.instruction,
      status: 'completed',
      createdAt: new Date().toISOString(),
      completedAt: new Date().toISOString(),
      resultText,
      lastSummary: resultText,
      ...(request.workItemId ? { workItemId: request.workItemId } : {}),
    };
  };
  workspace.runDispatch = async dispatchPromise => dispatchPromise;

  const dispatches = await workspace.resumePendingWork();
  const savedTaskList = JSON.parse(
    await readFile(path.join(cwd, '00-management', 'tasks.json'), 'utf8'),
  );

  assert.equal(dispatches.length, 3);
  assert.deepEqual(
    dispatches.map(dispatch => dispatch.roleId),
    ['coder', 'planner', 'coder'],
  );
  assert.equal(savedTaskList.items[0].status, 'completed');
  assert.equal(savedTaskList.items[0].attempts, 2);

  await workspace.deleteWorkspace();
  await assert.rejects(access(path.join(cwd, '.multi-agent-runtime', claudeSpec.id)));
  await assert.rejects(access(path.join(cwd, '.multi-agent-runtime', codexSpec.id)));
});

test('hybrid resume skips work items replanned as done and restarts codex on the pending item', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-hybrid-replan-'));
  const hybridSpec = instantiateWorkspace(
    createTaskGateCodingTemplate({
      plannerModel: 'claude-opus-4-6',
      coderModel: 'gpt-5.4',
    }),
    { id: 'hybrid-replan', name: 'Hybrid Replan', cwd },
    createClaudeWorkspaceProfile({
      model: 'claude-opus-4-6',
      permissionMode: 'bypassPermissions',
    }),
  );

  const claudeSpec = {
    ...hybridSpec,
    id: `${hybridSpec.id}--claude`,
    provider: 'claude-agent-sdk',
    defaultProvider: 'claude-agent-sdk',
    defaultModel: 'claude-opus-4-6',
    model: 'claude-opus-4-6',
    roles: [hybridSpec.roles.find(role => role.id === 'planner')],
    defaultRoleId: 'planner',
    coordinatorRoleId: 'planner',
  };
  const codexSpec = {
    ...hybridSpec,
    id: `${hybridSpec.id}--codex`,
    provider: 'codex-sdk',
    defaultProvider: 'codex-sdk',
    defaultModel: 'gpt-5.4',
    model: 'gpt-5.4',
    roles: [hybridSpec.roles.find(role => role.id === 'coder')],
    defaultRoleId: 'coder',
    coordinatorRoleId: 'coder',
    claimPolicy: {
      ...hybridSpec.claimPolicy,
      fallbackRoleId: 'coder',
    },
  };

  const claudePersistence = LocalWorkspacePersistence.fromSpec(claudeSpec);
  await claudePersistence.initializeWorkspace(claudeSpec);
  await claudePersistence.persistRuntime({
    state: {
      ...makeRuntimeState(claudeSpec),
      status: 'closed',
      activities: [
        {
          activityId: 'activity-user-replan',
          workspaceId: claudeSpec.id,
          kind: 'user_message',
          visibility: 'public',
          text: 'Resume from the replanned worklist.',
          createdAt: new Date().toISOString(),
        },
      ],
      workflowRuntime: {
        mode: 'workflow_running',
        activeNodeId: 'execute_tasks',
      },
    },
    events: [],
    providerState: {
      workspaceId: claudeSpec.id,
      provider: 'claude-agent-sdk',
      rootConversationId: 'claude-root-session',
      memberBindings: {},
      updatedAt: new Date().toISOString(),
    },
  });

  const codexPersistence = LocalWorkspacePersistence.fromSpec(codexSpec);
  await codexPersistence.initializeWorkspace(codexSpec);
  await codexPersistence.persistRuntime({
    state: {
      ...makeRuntimeState(codexSpec),
      status: 'closed',
      sessionId: 'stale-codex-thread',
      workflowRuntime: {
        mode: 'workflow_running',
        activeNodeId: 'execute_tasks',
      },
    },
    events: [],
    providerState: {
      workspaceId: codexSpec.id,
      provider: 'codex-sdk',
      rootConversationId: 'stale-codex-thread',
      memberBindings: {
        coder: {
          roleId: 'coder',
          providerConversationId: 'thread-coder-stale',
          kind: 'thread',
          updatedAt: new Date().toISOString(),
        },
      },
      updatedAt: new Date().toISOString(),
    },
  });

  await mkdir(path.join(cwd, '00-management'), { recursive: true });
  await writeFile(
    path.join(cwd, '00-management', 'tasks.json'),
    `${JSON.stringify(
      {
        version: 2,
        items: [
          {
            id: 'task-06',
            title: 'Already done',
            description: 'This task should not be replayed.',
            status: 'done',
            attempts: 3,
            maxAttempts: 3,
            files: ['apps/client/desktop/src/happy_client/reconnect.rs'],
            acceptanceCriteria: ['Already complete.'],
          },
          {
            id: 'task-09',
            title: 'Do the next pending task',
            description: 'This is the next item that should run.',
            status: 'pending',
            attempts: 0,
            maxAttempts: 3,
            files: ['apps/client/desktop/src/happy_client/manager.rs'],
            acceptanceCriteria: ['Task completes cleanly.'],
          },
        ],
      },
      null,
      2,
    )}\n`,
    'utf8',
  );

  const workspace = await HybridWorkspace.restoreFromLocal({
    cwd,
    workspaceId: hybridSpec.id,
    defaultModels: {
      'claude-agent-sdk': 'claude-opus-4-6',
      'codex-sdk': 'gpt-5.4',
    },
  });
  workspace.active = true;
  workspace.initialized = true;

  const codexWorkspace = workspace.childWorkspaces.get('codex-sdk');
  assert.equal(codexWorkspace.roleThreadIds.get('coder'), 'thread-coder-stale');

  let sequence = 0;
  workspace.assignRoleTask = async request => {
    sequence += 1;
    const resultText =
      request.roleId === 'planner'
        ? 'APPROVED: The task is complete.'
        : request.instruction.includes('Create a git commit')
          ? 'Committed task(task-09): Do the next pending task'
          : 'Implemented task-09 successfully.';
    return {
      dispatchId: `dispatch-replan-${sequence}`,
      workspaceId: hybridSpec.id,
      roleId: request.roleId,
      provider:
        request.roleId === 'planner' ? 'claude-agent-sdk' : 'codex-sdk',
      model:
        request.roleId === 'planner' ? 'claude-opus-4-6' : 'gpt-5.4',
      instruction: request.instruction,
      status: 'completed',
      createdAt: new Date().toISOString(),
      completedAt: new Date().toISOString(),
      resultText,
      lastSummary: resultText,
      ...(request.workItemId ? { workItemId: request.workItemId } : {}),
    };
  };
  workspace.runDispatch = async dispatchPromise => dispatchPromise;

  const dispatches = await workspace.resumePendingWork();
  const savedTaskList = JSON.parse(
    await readFile(path.join(cwd, '00-management', 'tasks.json'), 'utf8'),
  );

  assert.equal(codexWorkspace.roleThreadIds.size, 0);
  assert.deepEqual(
    dispatches.map(dispatch => [dispatch.roleId, dispatch.workItemId]),
    [
      ['coder', 'task-09'],
      ['planner', 'task-09'],
      ['coder', 'task-09'],
    ],
  );
  assert.ok(['done', 'completed'].includes(savedTaskList.items[0].status));
  assert.equal(savedTaskList.items[1].status, 'completed');

  await workspace.deleteWorkspace();
  await assert.rejects(access(path.join(cwd, '.multi-agent-runtime', claudeSpec.id)));
  await assert.rejects(access(path.join(cwd, '.multi-agent-runtime', codexSpec.id)));
});

test('hybrid resume can retry an exhausted failed work item when explicitly requested', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-hybrid-retry-exhausted-'));
  const hybridSpec = instantiateWorkspace(
    createTaskGateCodingTemplate({
      plannerModel: 'claude-opus-4-6',
      coderModel: 'gpt-5.4',
    }),
    { id: 'hybrid-retry-exhausted', name: 'Hybrid Retry Exhausted', cwd },
    createClaudeWorkspaceProfile({
      model: 'claude-opus-4-6',
      permissionMode: 'bypassPermissions',
    }),
  );

  const claudeSpec = {
    ...hybridSpec,
    id: `${hybridSpec.id}--claude`,
    provider: 'claude-agent-sdk',
    defaultProvider: 'claude-agent-sdk',
    defaultModel: 'claude-opus-4-6',
    model: 'claude-opus-4-6',
    roles: [hybridSpec.roles.find(role => role.id === 'planner')],
    defaultRoleId: 'planner',
    coordinatorRoleId: 'planner',
  };
  const codexSpec = {
    ...hybridSpec,
    id: `${hybridSpec.id}--codex`,
    provider: 'codex-sdk',
    defaultProvider: 'codex-sdk',
    defaultModel: 'gpt-5.4',
    model: 'gpt-5.4',
    roles: [hybridSpec.roles.find(role => role.id === 'coder')],
    defaultRoleId: 'coder',
    coordinatorRoleId: 'coder',
    claimPolicy: {
      ...hybridSpec.claimPolicy,
      fallbackRoleId: 'coder',
    },
  };

  const claudePersistence = LocalWorkspacePersistence.fromSpec(claudeSpec);
  await claudePersistence.initializeWorkspace(claudeSpec);
  await claudePersistence.persistRuntime({
    state: {
      ...makeRuntimeState(claudeSpec),
      status: 'closed',
      activities: [
        {
          activityId: 'activity-user-retry-exhausted',
          workspaceId: claudeSpec.id,
          kind: 'user_message',
          visibility: 'public',
          text: 'Resume the exhausted failed task.',
          createdAt: new Date().toISOString(),
        },
      ],
      workflowRuntime: {
        mode: 'workflow_running',
        activeNodeId: 'execute_tasks',
      },
    },
    events: [],
    providerState: {
      workspaceId: claudeSpec.id,
      provider: 'claude-agent-sdk',
      rootConversationId: 'claude-root-session',
      memberBindings: {},
      updatedAt: new Date().toISOString(),
    },
  });

  const codexPersistence = LocalWorkspacePersistence.fromSpec(codexSpec);
  await codexPersistence.initializeWorkspace(codexSpec);
  await codexPersistence.persistRuntime({
    state: {
      ...makeRuntimeState(codexSpec),
      status: 'closed',
      workflowRuntime: {
        mode: 'workflow_running',
        activeNodeId: 'execute_tasks',
      },
    },
    events: [],
    providerState: {
      workspaceId: codexSpec.id,
      provider: 'codex-sdk',
      rootConversationId: 'codex-root-thread',
      memberBindings: {
        coder: {
          roleId: 'coder',
          providerConversationId: 'thread-coder-exhausted',
          kind: 'thread',
          updatedAt: new Date().toISOString(),
        },
      },
      updatedAt: new Date().toISOString(),
    },
  });

  await mkdir(path.join(cwd, '00-management'), { recursive: true });
  await writeFile(
    path.join(cwd, '00-management', 'tasks.json'),
    `${JSON.stringify(
      {
        version: 1,
        items: [
          {
            id: 'task-exhausted',
            title: 'Retry me once more',
            description: 'This task previously failed because the reviewer API errored.',
            status: 'failed',
            attempts: 2,
            maxAttempts: 2,
            files: ['apps/client/desktop/src/happy_client/manager.rs'],
            acceptanceCriteria: ['Task completes cleanly on resume.'],
          },
        ],
      },
      null,
      2,
    )}\n`,
    'utf8',
  );

  const workspace = await HybridWorkspace.restoreFromLocal({
    cwd,
    workspaceId: hybridSpec.id,
    defaultModels: {
      'claude-agent-sdk': 'claude-opus-4-6',
      'codex-sdk': 'gpt-5.4',
    },
  });
  workspace.active = true;
  workspace.initialized = true;

  let sequence = 0;
  workspace.assignRoleTask = async request => {
    sequence += 1;
    const resultText =
      request.roleId === 'planner'
        ? 'APPROVED: The retried task is complete.'
        : request.instruction.includes('Create exactly one git commit')
          ? 'Committed task(task-exhausted): Retry me once more'
          : 'Implemented task-exhausted successfully.';
    return {
      dispatchId: `dispatch-retry-exhausted-${sequence}`,
      workspaceId: hybridSpec.id,
      roleId: request.roleId,
      provider:
        request.roleId === 'planner' ? 'claude-agent-sdk' : 'codex-sdk',
      model:
        request.roleId === 'planner' ? 'claude-opus-4-6' : 'gpt-5.4',
      instruction: request.instruction,
      status: 'completed',
      createdAt: new Date().toISOString(),
      completedAt: new Date().toISOString(),
      resultText,
      lastSummary: resultText,
      ...(request.workItemId ? { workItemId: request.workItemId } : {}),
    };
  };
  workspace.runDispatch = async dispatchPromise => dispatchPromise;

  const dispatches = await workspace.resumePendingWork({
    retryExhaustedItems: true,
    resetAttemptsForRetriedItems: true,
  });
  const savedTaskList = JSON.parse(
    await readFile(path.join(cwd, '00-management', 'tasks.json'), 'utf8'),
  );

  assert.deepEqual(
    dispatches.map(dispatch => [dispatch.roleId, dispatch.workItemId]),
    [
      ['coder', 'task-exhausted'],
      ['planner', 'task-exhausted'],
      ['coder', 'task-exhausted'],
    ],
  );
  assert.equal(savedTaskList.items[0].status, 'completed');
  assert.equal(savedTaskList.items[0].attempts, 1);

  await workspace.deleteWorkspace();
  await assert.rejects(access(path.join(cwd, '.multi-agent-runtime', claudeSpec.id)));
  await assert.rejects(access(path.join(cwd, '.multi-agent-runtime', codexSpec.id)));
});
