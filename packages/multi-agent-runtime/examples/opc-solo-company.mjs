import path from 'node:path';
import {
  ClaudeAgentWorkspace,
  createClaudeWorkspaceProfile,
  createOpcSoloCompanyTemplate,
  instantiateWorkspace,
} from '../dist/index.js';
import { attachConsoleLogger, createScratchDir, printFileIfExists } from './_shared.mjs';

const cwd = await createScratchDir('cteno-opc');
const workspace = new ClaudeAgentWorkspace({
  spec: instantiateWorkspace(
    createOpcSoloCompanyTemplate(),
    {
      id: `opc-${Date.now()}`,
      name: 'OPC Solo Company Smoke',
      cwd,
    },
    createClaudeWorkspaceProfile(),
  ),
});

const stopLogging = attachConsoleLogger(workspace, 'opc');

try {
  await workspace.start();
  const turn = await workspace.runWorkspaceTurn({
    message:
      'Please prepare a practical monthly close checklist for a solo SaaS founder and write it to company/10-finance/monthly-close-checklist.md. Include cash review, invoices, subscriptions, payroll or contractors, tax prep handoff, and KPI review.',
  }, { timeoutMs: 180000, resultTimeoutMs: 20000 });

  console.log('\nWORKSPACE TURN');
  console.log(JSON.stringify(turn, null, 2));
  await printFileIfExists(path.join(cwd, 'company/10-finance/monthly-close-checklist.md'));
} finally {
  stopLogging();
  await workspace.close();
}
