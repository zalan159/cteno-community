import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  ClaudeAgentWorkspace,
  CodexSdkWorkspace,
  createClaudeWorkspaceProfile,
  createCodexWorkspaceProfile,
  createPlanCodingEvalTemplate,
  instantiateWorkspace,
} from '../dist/index.js';
import { attachConsoleLogger } from './_shared.mjs';

function resolveProvider() {
  const provider = (process.env.MULTI_AGENT_PROVIDER || 'codex').trim().toLowerCase();
  if (provider === 'claude' || provider === 'codex') {
    return provider;
  }

  throw new Error(`Unsupported MULTI_AGENT_PROVIDER: ${provider}`);
}

function resolveTaskMessage() {
  const cliMessage = process.argv.slice(2).join(' ').trim();
  if (cliMessage) {
    return cliMessage;
  }

  const envMessage = process.env.MULTI_AGENT_TASK?.trim();
  if (envMessage) {
    return envMessage;
  }

  throw new Error('Missing task message. Pass it as CLI args or set MULTI_AGENT_TASK.');
}

function parseCommands() {
  const raw = process.env.MULTI_AGENT_VERIFY_COMMANDS?.trim();
  if (!raw) {
    return [
      'npm run typecheck --prefix packages/multi-agent-runtime',
      'cargo check --manifest-path apps/client/desktop/Cargo.toml --no-default-features',
    ];
  }

  return raw
    .split('\n')
    .map(line => line.trim())
    .filter(Boolean);
}

const exampleDir = fileURLToPath(new URL('.', import.meta.url));
const repoRoot = path.resolve(exampleDir, '../../..');
const provider = resolveProvider();
const message = resolveTaskMessage();
const workspaceCwd = process.env.MULTI_AGENT_WORKSPACE_CWD || repoRoot;
const taskSlug = process.env.MULTI_AGENT_TASK_SLUG || `task-${Date.now()}`;
const template = createPlanCodingEvalTemplate({
  planRoot: process.env.MULTI_AGENT_PLAN_ROOT || '00-management/task-templates/plans',
  evalRoot: process.env.MULTI_AGENT_EVAL_ROOT || '00-management/task-templates/eval',
  codeRoot: process.env.MULTI_AGENT_CODE_ROOT || '.',
  verificationCommands: parseCommands(),
});

const spec =
  provider === 'claude'
    ? instantiateWorkspace(
        template,
        {
          id: `cteno-plan-coding-eval-${taskSlug}`,
          name: 'Cteno Plan Coding Eval',
          cwd: workspaceCwd,
        },
        createClaudeWorkspaceProfile({
          model: process.env.MULTI_AGENT_MODEL || 'claude-sonnet-4-5',
        }),
      )
    : instantiateWorkspace(
        template,
        {
          id: `cteno-plan-coding-eval-${taskSlug}`,
          name: 'Cteno Plan Coding Eval',
          cwd: workspaceCwd,
        },
        createCodexWorkspaceProfile({
          model: process.env.MULTI_AGENT_MODEL || 'gpt-5.4',
        }),
      );

const workspace =
  provider === 'claude'
    ? new ClaudeAgentWorkspace({ spec })
    : new CodexSdkWorkspace({
        spec,
        skipGitRepoCheck: true,
        approvalPolicy: 'never',
        sandboxMode: 'workspace-write',
      });

const stopLogging = attachConsoleLogger(workspace, 'cteno-plan-coding-eval');

try {
  await workspace.start();
  const turn = await workspace.runWorkspaceTurn(
    { message, workflowEntry: 'direct' },
    { timeoutMs: 300_000, resultTimeoutMs: 30_000 },
  );

  console.log('\nWORKSPACE TURN');
  console.log(
    JSON.stringify(
      {
        plan: turn.plan,
        dispatches: turn.dispatches.map(dispatch => ({
          roleId: dispatch.roleId,
          status: dispatch.status,
          summary: dispatch.summary,
          resultText: dispatch.resultText,
        })),
        persistenceRoot:
          typeof workspace.getPersistenceRoot === 'function'
            ? workspace.getPersistenceRoot()
            : undefined,
      },
      null,
      2,
    ),
  );
} finally {
  stopLogging();
  await workspace.close();
}
