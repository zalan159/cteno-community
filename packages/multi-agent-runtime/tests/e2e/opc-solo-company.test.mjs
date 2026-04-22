import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';

import {
  ClaudeAgentWorkspace,
  createClaudeWorkspaceProfile,
  createOpcSoloCompanyTemplate,
  instantiateWorkspace,
} from '../../dist/index.js';
import {
  createScratchDir,
  resolveClaudeTestModel,
  runWorkspaceTurnScenario,
} from './_shared.mjs';

test('opc e2e routes a workspace turn to finance and generates a monthly close checklist', { timeout: 300_000 }, async () => {
  const cwd = await createScratchDir('cteno-e2e-opc');
  const outputFile = path.join(cwd, 'company/10-finance/monthly-close-checklist.md');
  const workspace = new ClaudeAgentWorkspace({
    spec: instantiateWorkspace(
      createOpcSoloCompanyTemplate(),
      {
        id: `opc-e2e-${Date.now()}`,
        name: 'OPC E2E',
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
      'Please prepare a compact monthly close checklist for a solo SaaS founder and write it to company/10-finance/monthly-close-checklist.md. Keep it concise, around 12-18 actionable checklist items total, while still covering cash review, invoices, subscriptions, payroll or contractors, tax prep handoff, and KPI review.',
    expectedRoleId: 'finance',
    outputFile,
    timeoutMs: 270_000,
  });

  assert.match(turn.plan.responseText, /@finance|finance/i);
  assert.match(dispatch.resultText, /finance|checklist|monthly close/i);
  assert.match(fileText, /(Cash Review|Cash & Bank Reconciliation|Cash & Banking|cash balance|cash runway)/i);
  assert.match(fileText, /(Invoices|Receivables|overdue|invoice)/i);
  assert.match(fileText, /(Subscriptions|subscription renewals|MRR|Revenue & Receivables)/i);
  assert.match(fileText, /(Payroll|contractor payments|contractors)/i);
  assert.match(fileText, /(Tax Prep Handoff|Tax Preparation|estimated tax|sales tax|VAT)/i);
  assert.match(fileText, /(KPI Review|MRR|burn rate|churn rate)/i);
  const checkboxCount = (fileText.match(/- \[ \]/g) ?? []).length;
  assert.ok(checkboxCount >= 8, 'Expected a practical checklist with multiple actionable items');
});
