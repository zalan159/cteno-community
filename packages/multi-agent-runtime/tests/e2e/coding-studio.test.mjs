import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';

import {
  ClaudeAgentWorkspace,
  createClaudeWorkspaceProfile,
  createCodingStudioTemplate,
  instantiateWorkspace,
} from '../../dist/index.js';
import {
  createScratchDir,
  resolveClaudeTestModel,
  runWorkspaceTurnScenario,
} from './_shared.mjs';

test('coding studio e2e routes a workspace turn to the prd role and generates a usable PRD', { timeout: 240_000 }, async () => {
  const cwd = await createScratchDir('cteno-e2e-coding');
  const outputFile = path.join(cwd, '10-prd/group-mentions.md');
  const workspace = new ClaudeAgentWorkspace({
    spec: instantiateWorkspace(
      createCodingStudioTemplate(),
      {
        id: `coding-e2e-${Date.now()}`,
        name: 'Coding E2E',
        cwd,
      },
      createClaudeWorkspaceProfile({
        model: resolveClaudeTestModel(),
      }),
    ),
  });

  const { dispatch, turn, fileText } = await runWorkspaceTurnScenario({
    workspace,
    message:
      'We need a short PRD for a group-chat mention feature. Please create it at 10-prd/group-mentions.md with sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria. Keep it under 250 words.',
    expectedRoleId: 'prd',
    outputFile,
    expectClaimWindow: true,
  });

  assert.match(turn.plan.responseText, /@prd|PRD/i);
  assert.match(dispatch.resultText, /PRD|group mentions|acceptance/i);
  assert.match(fileText, /^(# |## Goal)/m);
  assert.match(fileText, /## Goal/i);
  assert.match(fileText, /## User Story/i);
  assert.match(fileText, /## Scope/i);
  assert.match(fileText, /(## Non-Goals|\*\*Out of Scope:\*\*|## Out of Scope)/i);
  assert.match(fileText, /## Acceptance Criteria/i);
  assert.match(fileText, /@/);
  assert.ok(fileText.split(/\s+/).length <= 320, 'Expected concise PRD output');
});
