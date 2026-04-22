import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';

import {
  CodexSdkWorkspace,
  createCodingStudioTemplate,
  createCodexWorkspaceProfile,
  instantiateWorkspace,
} from '../../dist/index.js';
import {
  createScratchDir,
  resolveCodexTestModel,
  runWorkspaceTurnScenario,
} from './_shared.mjs';

test('codex sdk e2e routes a workspace turn to a reusable prd role thread', { timeout: 240_000 }, async () => {
  const cwd = await createScratchDir('cteno-e2e-codex-coding');
  const outputFile = path.join(cwd, '10-prd/group-mentions.md');
  const workspace = new CodexSdkWorkspace({
    spec: instantiateWorkspace(
      createCodingStudioTemplate(),
      {
        id: `codex-coding-e2e-${Date.now()}`,
        name: 'Codex Coding E2E',
        cwd,
      },
      createCodexWorkspaceProfile({
        model: resolveCodexTestModel(),
      }),
    ),
    skipGitRepoCheck: true,
    approvalPolicy: 'never',
    sandboxMode: 'workspace-write',
  });

  const { dispatch, turn, fileText } = await runWorkspaceTurnScenario({
    workspace,
    message:
      'We need a short PRD for a group-chat mention feature. Please create it at 10-prd/group-mentions.md with sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria. Keep it under 250 words.',
    expectedRoleId: 'prd',
    outputFile,
    timeoutMs: 180_000,
    resultTimeoutMs: 20_000,
    expectClaimWindow: true,
  });

  assert.match(turn.plan.responseText, /@prd|PRD/i);
  assert.match(dispatch.resultText, /PRD|group mentions|acceptance/i);
  assert.match(fileText, /^#{1,2}\s+Goal/im);
  assert.match(fileText, /^#{1,2}\s+User Story/im);
  assert.match(fileText, /^#{1,2}\s+Scope/im);
  assert.match(fileText, /(^#{1,2}\s+Non-Goals|\*\*Out of Scope:\*\*|^#{1,2}\s+Out of Scope)/im);
  assert.match(fileText, /^#{1,2}\s+Acceptance Criteria/im);
});
