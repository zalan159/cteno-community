import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';

import {
  ClaudeAgentWorkspace,
  createAutoresearchTemplate,
  createClaudeWorkspaceProfile,
  instantiateWorkspace,
} from '../../dist/index.js';
import {
  createScratchDir,
  resolveClaudeTestModel,
  runWorkspaceTurnScenario,
} from './_shared.mjs';

test('autoresearch e2e enters workflow mode and starts at the lead entry node', { timeout: 360_000 }, async () => {
  const cwd = await createScratchDir('cteno-e2e-autoresearch');
  const outputFile = path.join(cwd, 'research/00-lead/mention-hypothesis.md');
  const workspace = new ClaudeAgentWorkspace({
    spec: instantiateWorkspace(
      createAutoresearchTemplate(),
      {
        id: `autoresearch-e2e-${Date.now()}`,
        name: 'Autoresearch E2E',
        cwd,
      },
      createClaudeWorkspaceProfile({
        model: resolveClaudeTestModel(),
      }),
    ),
  });

  const { dispatch, turn, events, fileText } = await runWorkspaceTurnScenario({
    workspace,
    message:
      'Start the autoresearch workflow for group-chat mention semantics. Frame the current hypothesis for how collaboration tools like Slack and GitHub handle @mentions, and write the initial hypothesis brief to research/00-lead/mention-hypothesis.md with sections for Hypothesis, Success Criteria, and Next Experiment.',
    expectedRoleId: 'lead',
    outputFile,
    timeoutMs: 240_000,
    resultTimeoutMs: 30_000,
    expectWorkflowVote: true,
    expectWorkflowStart: true,
  });

  assert.match(turn.plan.responseText, /workflow|@lead|hypothesis/i);
  assert.match(dispatch.resultText, /hypothesis|experiment|mention/i);
  assert.match(fileText, /#|##/);
  assert.match(fileText, /Hypothesis/i);
  assert.match(fileText, /Success Criteria/i);
  assert.match(fileText, /Next Experiment/i);
  assert.match(fileText, /Slack/i);
  assert.match(fileText, /GitHub/i);
  assert.ok(fileText.split(/\s+/).length >= 40, 'Expected a substantive workflow-entry brief');
});
