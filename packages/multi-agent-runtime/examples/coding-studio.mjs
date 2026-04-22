import path from 'node:path';
import {
  ClaudeAgentWorkspace,
  createClaudeWorkspaceProfile,
  createCodingStudioTemplate,
  instantiateWorkspace,
} from '../dist/index.js';
import { attachConsoleLogger, createScratchDir, printFileIfExists } from './_shared.mjs';

const cwd = await createScratchDir('cteno-coding-studio');
const workspace = new ClaudeAgentWorkspace({
  spec: instantiateWorkspace(
    createCodingStudioTemplate(),
    {
      id: `coding-studio-${Date.now()}`,
      name: 'Coding Studio Smoke',
      cwd,
    },
    createClaudeWorkspaceProfile(),
  ),
});

const stopLogging = attachConsoleLogger(workspace, 'coding');

try {
  await workspace.start();
  const turn = await workspace.runWorkspaceTurn({
    message:
      'We need a short PRD for a group-chat mention feature. Please create it at 10-prd/group-mentions.md with sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria. Keep it under 250 words.',
  }, { timeoutMs: 180000, resultTimeoutMs: 20000 });

  console.log('\nWORKSPACE TURN');
  console.log(JSON.stringify(turn, null, 2));
  await printFileIfExists(path.join(cwd, '10-prd/group-mentions.md'));
} finally {
  stopLogging();
  await workspace.close();
}
