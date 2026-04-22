import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';

import {
  CodexSdkWorkspace,
  createCodexWorkspaceProfile,
  createOpcSoloCompanyTemplate,
  instantiateWorkspace,
} from '../../dist/index.js';
import {
  createScratchDir,
  resolveCodexTestModel,
  runWorkspaceTurnScenario,
} from './_shared.mjs';

test('codex sdk e2e routes an opc workspace turn to finance and generates a monthly close checklist', { timeout: 300_000 }, async () => {
  const cwd = await createScratchDir('cteno-e2e-codex-opc');
  const outputFile = path.join(cwd, 'company/10-finance/monthly-close-checklist.md');
  const workspace = new CodexSdkWorkspace({
    spec: instantiateWorkspace(
      createOpcSoloCompanyTemplate(),
      {
        id: `codex-opc-e2e-${Date.now()}`,
        name: 'Codex OPC E2E',
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
      'Please prepare a compact monthly close checklist for a solo SaaS founder and write it to company/10-finance/monthly-close-checklist.md. Keep it concise, around 12-18 actionable checklist items total, while still covering cash review, invoices, subscriptions, payroll or contractors, tax prep handoff, and KPI review.',
    expectedRoleId: 'finance',
    outputFile,
    timeoutMs: 270_000,
  });

  assert.match(turn.plan.responseText, /@finance|finance/i);
  assert.match(dispatch.resultText, /finance|checklist|monthly close/i);
  assert.match(fileText, /(cash|bank|runway)/i);
  assert.match(fileText, /(invoice|receivables|overdue)/i);
  assert.match(fileText, /(subscription|MRR|revenue)/i);
  assert.match(fileText, /(payroll|contractor)/i);
  assert.match(fileText, /(tax|sales tax|VAT|1099)/i);
  assert.match(fileText, /(KPI|MRR|burn|churn|CAC)/i);
});
