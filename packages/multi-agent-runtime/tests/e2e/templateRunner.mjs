import assert from 'node:assert/strict';
import test from 'node:test';

import {
  ClaudeAgentWorkspace,
  CodexSdkWorkspace,
  HybridWorkspace,
  createClaudeWorkspaceProfile,
  createCodexWorkspaceProfile,
  instantiateWorkspace,
} from '../../dist/index.js';
import {
  createScratchDir,
  readRequiredFile,
  resolveClaudeTestModel,
  resolveCodexTestModel,
} from './_shared.mjs';

const selectedTemplates = splitCsv(process.env.MULTI_AGENT_E2E_TEMPLATE);
const selectedProviders = splitCsv(process.env.MULTI_AGENT_E2E_PROVIDER);

export function registerTemplateTurnCases(cases) {
  for (const templateCase of cases) {
    for (const provider of templateCase.providers) {
      if (!shouldRunCase(templateCase.id, provider)) {
        continue;
      }

      const timeoutMs = templateCase.timeoutMs ?? 300_000;
      test(
        `${templateCase.id} [${provider}]`,
        { timeout: timeoutMs + 300_000 },
        async () => {
          const cwd = await createScratchDir(`${templateCase.scratchPrefix}-${provider}`);
          const models = {
            claude: resolveClaudeTestModel(),
            codex: resolveCodexTestModel(),
          };
          if (templateCase.setup) {
            await templateCase.setup({ cwd, provider, models });
          }
          const workspace = createWorkspaceForProvider(templateCase, provider, cwd, models);
          const events = [];
          const stopListening = workspace.onEvent(event => {
            events.push(event);
          });

          try {
            await workspace.start();
            const turn = await workspace.runWorkspaceTurn(
              { message: resolveValue(templateCase.request, { cwd, provider, models }) },
              {
                timeoutMs,
                resultTimeoutMs: templateCase.resultTimeoutMs ?? 20_000,
              },
            );

            const files = await readOutputFiles(templateCase.outputFiles, cwd);
            const initializedEvents = events.filter(event => event.type === 'workspace.initialized');
            assert.ok(initializedEvents.length >= 1, 'Expected at least one workspace.initialized event');

            const userMessageEvent = events.find(
              event => event.type === 'activity.published' && event.activity.kind === 'user_message',
            );
            const coordinatorActivity = events.find(
              event =>
                event.type === 'activity.published' &&
                event.activity.kind === 'coordinator_message',
            );
            assert.ok(userMessageEvent, 'Expected a public user_message activity');
            assert.ok(coordinatorActivity, 'Expected a public coordinator_message activity');

            if (templateCase.expectClaimWindow) {
              assert.ok(
                events.find(event => event.type === 'claim.window.opened'),
                'Expected claim.window.opened event',
              );
              assert.ok(
                events.some(event => event.type === 'claim.response'),
                'Expected claim.response events',
              );
              assert.ok(
                events.find(event => event.type === 'claim.window.closed'),
                'Expected claim.window.closed event',
              );
            }

            if (templateCase.expectWorkflowVote) {
              assert.ok(
                events.find(event => event.type === 'workflow.vote.opened'),
                'Expected workflow.vote.opened event',
              );
              assert.ok(
                events.some(event => event.type === 'workflow.vote.response'),
                'Expected workflow.vote.response events',
              );
              assert.ok(
                events.find(event => event.type === 'workflow.vote.closed'),
                'Expected workflow.vote.closed event',
              );
            }

            if (templateCase.expectWorkflowStart) {
              assert.ok(
                events.find(event => event.type === 'workflow.started'),
                'Expected workflow.started event',
              );
            }

            const expectedDispatchRoleIds = templateCase.expectedDispatchRoleIds ?? [];
            for (const roleId of expectedDispatchRoleIds) {
              assert.ok(
                turn.dispatches.some(dispatch => dispatch.roleId === roleId),
                `Expected a dispatch for role "${roleId}"`,
              );
            }

            const primaryRoleId = templateCase.expectedPrimaryRoleId;
            const primaryDispatch = primaryRoleId
              ? turn.dispatches.find(dispatch => dispatch.roleId === primaryRoleId)
              : turn.dispatches[0];
            assert.ok(primaryDispatch, 'Expected at least one role dispatch from workspace turn');

            if (primaryRoleId) {
              assert.equal(primaryDispatch.roleId, primaryRoleId);
            }

            const completedEvent = events.find(
              event =>
                event.type === 'dispatch.completed' &&
                event.dispatch.dispatchId === primaryDispatch.dispatchId,
            );
            const resultEvent = events.find(
              event =>
                event.type === 'dispatch.result' &&
                event.dispatch.dispatchId === primaryDispatch.dispatchId,
            );
            assert.ok(completedEvent, 'Expected the primary dispatch to complete');
            assert.ok(resultEvent, 'Expected the primary dispatch to return final result text');
            assert.equal(primaryDispatch.claimStatus, 'claimed');
            assert.ok(
              primaryDispatch.resultText && primaryDispatch.resultText.trim().length > 0,
              'Expected non-empty primary dispatch resultText',
            );

            for (const [name, fileText] of Object.entries(files)) {
              assert.ok(fileText.trim().length > 0, `Expected non-empty output file: ${name}`);
            }

            if (templateCase.assert) {
              await templateCase.assert({
                provider,
                cwd,
                models,
                turn,
                events,
                files,
                primaryDispatch,
              });
            }
          } finally {
            stopListening();
            await workspace.close();
          }
        },
      );
    }
  }
}

function shouldRunCase(templateId, provider) {
  const templateAllowed =
    selectedTemplates.length === 0 || selectedTemplates.includes(templateId);
  const providerAllowed =
    selectedProviders.length === 0 || selectedProviders.includes(provider);
  return templateAllowed && providerAllowed;
}

function createWorkspaceForProvider(templateCase, provider, cwd, models) {
  const template = templateCase.templateFactory({ cwd, provider, models });
  const instance = {
    id: `${templateCase.id}-${provider}-e2e-${Date.now()}`,
    name: `${template.templateName} ${provider.toUpperCase()} E2E`,
    cwd,
  };

  if (provider === 'claude') {
    const profile = createClaudeWorkspaceProfile({
      model: models.claude,
      ...(templateCase.claudePermissionMode
        ? { permissionMode: templateCase.claudePermissionMode }
        : {}),
    });
    return new ClaudeAgentWorkspace({
      spec: instantiateWorkspace(template, instance, profile),
    });
  }

  if (provider === 'codex') {
    return new CodexSdkWorkspace({
      spec: instantiateWorkspace(
        template,
        instance,
        createCodexWorkspaceProfile({
          model: models.codex,
        }),
      ),
      skipGitRepoCheck: true,
      approvalPolicy: 'never',
      sandboxMode: 'workspace-write',
      ...(templateCase.codexWorkspaceOptions ?? {}),
    });
  }

  if (provider === 'hybrid') {
    const profile = createClaudeWorkspaceProfile({
      model: models.claude,
      permissionMode: templateCase.hybridPermissionMode ?? 'bypassPermissions',
    });
    return new HybridWorkspace({
      spec: instantiateWorkspace(template, instance, profile),
      defaultModels: {
        'claude-agent-sdk': models.claude,
        'codex-sdk': models.codex,
      },
      codex: {
        skipGitRepoCheck: true,
        approvalPolicy: 'never',
        sandboxMode: 'workspace-write',
        ...(templateCase.codexWorkspaceOptions ?? {}),
      },
    });
  }

  throw new Error(`Unsupported provider: ${provider}`);
}

async function readOutputFiles(outputFiles, cwd) {
  const entries = await Promise.all(
    Object.entries(outputFiles).map(async ([name, resolver]) => {
      const filePath = resolveValue(resolver, { cwd });
      return [name, await readRequiredFile(filePath)];
    }),
  );
  return Object.fromEntries(entries);
}

function resolveValue(value, context) {
  return typeof value === 'function' ? value(context.cwd, context) : value;
}

function splitCsv(value) {
  return value
    ? value
        .split(',')
        .map(item => item.trim())
        .filter(Boolean)
    : [];
}
