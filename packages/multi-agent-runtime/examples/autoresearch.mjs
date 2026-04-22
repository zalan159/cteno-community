import path from 'node:path';
import {
  ClaudeAgentWorkspace,
  createAutoresearchTemplate,
  createClaudeWorkspaceProfile,
  instantiateWorkspace,
} from '../dist/index.js';
import { attachConsoleLogger, createScratchDir, printFileIfExists } from './_shared.mjs';

const cwd = await createScratchDir('cteno-autoresearch');
const workspace = new ClaudeAgentWorkspace({
  spec: instantiateWorkspace(
    createAutoresearchTemplate(),
    {
      id: `autoresearch-${Date.now()}`,
      name: 'Autoresearch Smoke',
      cwd,
    },
    createClaudeWorkspaceProfile(),
  ),
});

const stopLogging = attachConsoleLogger(workspace, 'autoresearch');

try {
  await workspace.start();
  const turn = await workspace.runWorkspaceTurn({
    message:
      'Research how collaboration tools like Slack and GitHub handle @mentions and directed attention, then write a concise brief to research/10-scout/mention-patterns.md with three short source links and a final section called Implications for Cteno.',
  }, { timeoutMs: 240000, resultTimeoutMs: 30000 });

  console.log('\nWORKSPACE TURN');
  console.log(JSON.stringify(turn, null, 2));
  await printFileIfExists(path.join(cwd, 'research/10-scout/mention-patterns.md'));
} finally {
  stopLogging();
  await workspace.close();
}
