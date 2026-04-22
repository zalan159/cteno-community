import { appendFile, mkdir, readFile } from 'node:fs/promises';
import path from 'node:path';

import type { WorkspaceEvent, WorkspaceSpec } from '../index.js';
import { HybridWorkspace } from '../index.js';

function formatExecutionEvent(event: WorkspaceEvent): string | undefined {
  if (event.type === 'dispatch.started') {
    return `${event.timestamp} | ${event.dispatch.roleId} | started | ${event.description}`;
  }

  if (event.type === 'dispatch.progress') {
    return `${event.timestamp} | ${event.dispatch.roleId} | progress | ${event.summary ?? event.description}`;
  }

  if (
    event.type === 'dispatch.completed' ||
    event.type === 'dispatch.failed' ||
    event.type === 'dispatch.stopped'
  ) {
    return `${event.timestamp} | ${event.dispatch.roleId} | ${event.type} | ${event.summary}`;
  }

  if (event.type === 'dispatch.result') {
    return `${event.timestamp} | ${event.dispatch.roleId} | result | ${oneLine(event.resultText, 240)}`;
  }

  return undefined;
}

function shouldRefreshTracker(
  event: WorkspaceEvent,
  state: {
    lastProgressSummaryByDispatch: Map<string, string>;
    lastProgressRefreshAt: number;
  },
): boolean {
  if (
    event.type === 'dispatch.started' ||
    event.type === 'dispatch.completed' ||
    event.type === 'dispatch.failed' ||
    event.type === 'dispatch.stopped' ||
    event.type === 'dispatch.result'
  ) {
    return true;
  }

  if (event.type !== 'dispatch.progress') {
    return false;
  }

  const summary = event.summary ?? event.description;
  const key = event.dispatch.dispatchId;
  const previous = state.lastProgressSummaryByDispatch.get(key);
  if (previous === summary) {
    return false;
  }

  const now = Date.parse(event.timestamp);
  if (Number.isFinite(now) && now - state.lastProgressRefreshAt < 20_000) {
    return false;
  }

  state.lastProgressSummaryByDispatch.set(key, summary);
  state.lastProgressRefreshAt = Number.isFinite(now) ? now : Date.now();
  return true;
}

async function appendProgressLine(filePath: string, line: string): Promise<void> {
  await mkdir(path.dirname(filePath), { recursive: true });
  await appendFile(filePath, `${line}\n`, 'utf8');
}

async function printFileIfPresent(filePath: string): Promise<void> {
  try {
    const text = await readFile(filePath, 'utf8');
    console.log(`\nFILE ${filePath}\n${text}`);
  } catch {
    console.log(`\nFILE ${filePath} was not created.`);
  }
}

function oneLine(text: string, maxLength: number): string {
  const compact = text.replace(/\s+/g, ' ').trim();
  if (compact.length <= maxLength) {
    return compact;
  }

  return `${compact.slice(0, Math.max(0, maxLength - 3))}...`;
}

async function main(): Promise<void> {
  const cwd = process.argv[2] ? path.resolve(process.argv[2]) : process.cwd();
  const task =
    process.argv.slice(3).join(' ') ||
    'Implement a small repository change using Claude for planning and Codex for execution.';
  const claudeModel = process.env.CLAUDE_MODEL ?? 'claude-sonnet-4-5';
  const codexModel = process.env.CODEX_MODEL ?? 'gpt-5.4';
  const planPath = '00-management/plan.md';
  const briefPath = '10-prd/execution-brief.md';
  const statusPath = '00-management/status.md';
  const verificationPath = '50-test/verification.md';
  const progressLogPath = path.join(cwd, '00-management/codex-progress.log');

  const spec: WorkspaceSpec = {
    id: `hybrid-claude-codex-${Date.now()}`,
    name: 'Hybrid Claude + Codex Workspace',
    provider: 'hybrid',
    defaultProvider: 'claude-agent-sdk',
    defaultModel: claudeModel,
    model: claudeModel,
    cwd,
    roles: [
      {
        id: 'planner',
        name: 'Planner',
        outputRoot: '00-management/',
        agent: {
          description: 'Creates plans and implementation briefs.',
          prompt:
            'You are a planning lead. Break work into a concise execution plan, write explicit acceptance criteria, and leave implementation details to downstream coding agents.',
          tools: ['Read', 'Write', 'Edit', 'Glob', 'Grep'],
        },
      },
      {
        id: 'tracker',
        name: 'Tracker',
        outputRoot: '00-management/',
        agent: {
          description: 'Maintains concise progress status and executive overviews.',
          prompt:
            'You are a delivery tracker. Keep status up to date, summarize progress crisply, call out blockers early, and maintain a concise executive view of the run.',
          tools: ['Read', 'Write', 'Edit', 'Glob', 'Grep'],
        },
      },
      {
        id: 'coder',
        name: 'Coder',
        outputRoot: '40-code/',
        agent: {
          provider: 'codex-sdk',
          model: codexModel,
          description: 'Implements code changes with focused diffs.',
          prompt:
            'You are an implementation specialist. Follow the approved plan, make focused code changes, and explain assumptions briefly when the repository leaves gaps.',
          tools: ['Read', 'Write', 'Edit', 'Glob', 'Grep', 'Bash'],
        },
      },
      {
        id: 'tester',
        name: 'Tester',
        outputRoot: '50-test/',
        agent: {
          provider: 'codex-sdk',
          model: codexModel,
          description: 'Validates changes and records residual risk.',
          prompt:
            'You are a verification specialist. Prefer the narrowest useful checks first, report failures clearly, and document residual risks when full validation is not possible.',
          tools: ['Read', 'Write', 'Edit', 'Glob', 'Grep', 'Bash'],
        },
      },
    ],
  };

  const workspace = new HybridWorkspace({
    spec,
    defaultModels: {
      'claude-agent-sdk': claudeModel,
      'codex-sdk': codexModel,
    },
    codex: {
      skipGitRepoCheck: true,
      approvalPolicy: 'never',
      sandboxMode: 'workspace-write',
    },
  });

  const trackerState = {
    lastProgressSummaryByDispatch: new Map<string, string>(),
    lastProgressRefreshAt: 0,
  };
  let trackerChain = Promise.resolve();

  workspace.onEvent(event => {
    if (
      (event.type === 'dispatch.started' ||
        event.type === 'dispatch.progress' ||
        event.type === 'dispatch.completed' ||
        event.type === 'dispatch.failed' ||
        event.type === 'dispatch.stopped' ||
        event.type === 'dispatch.result') &&
      event.dispatch.roleId !== 'coder' &&
      event.dispatch.roleId !== 'tester'
    ) {
      return;
    }

    const line = formatExecutionEvent(event);
    if (!line) {
      return;
    }

    trackerChain = trackerChain
      .then(async () => {
        await appendProgressLine(progressLogPath, line);

        if (!shouldRefreshTracker(event, trackerState)) {
          return;
        }

        await workspace.runRoleTask(
          {
            roleId: 'tracker',
            summary: 'Refresh hybrid delivery status',
            visibility: 'coordinator',
            instruction: [
              `Update \`${statusPath}\` to reflect the latest Codex execution event.`,
              `New event: ${line}`,
              `Use the existing status file if present, keep it concise, and preserve these sections when possible:`,
              '`## Status`, `## Active Work`, `## Completed`, `## Blockers`, `## Next Step`.',
              `Source plan: \`${planPath}\``,
              `Source brief: \`${briefPath}\``,
              'Source progress log: `00-management/codex-progress.log`',
            ].join('\n'),
          },
          { timeoutMs: 120_000, resultTimeoutMs: 10_000 },
        );
      })
      .catch(error => {
        console.error('[hybrid] tracker update failed:', error);
      });
  });

  await workspace.start();

  try {
    const planningDispatch = await workspace.runRoleTask(
      {
        roleId: 'planner',
        summary: 'Create implementation plan and execution brief',
        visibility: 'coordinator',
        instruction: [
          'Prepare the workspace for a hybrid Claude + Codex delivery run.',
          `Task: ${task}`,
          `Create \`${planPath}\` with these sections:`,
          '`## Goal`, `## Scope`, `## Constraints`, `## Work Plan`, `## Acceptance Criteria`.',
          `Create \`${briefPath}\` for the coding agent with these sections:`,
          '`## Objective`, `## Required Changes`, `## Constraints`, `## Validation Expectations`.',
          'Make the plan concise but implementation-ready.',
        ].join('\n'),
      },
      { timeoutMs: 240_000, resultTimeoutMs: 20_000 },
    );

    await workspace.runRoleTask(
      {
        roleId: 'tracker',
        summary: 'Initialize delivery status',
        visibility: 'coordinator',
        instruction: [
          `Create \`${statusPath}\` as the executive progress board for this run.`,
          `Reference \`${planPath}\` and \`${briefPath}\`.`,
          'State clearly that planning is complete and Codex implementation has not started yet.',
          'Use these sections: `## Status`, `## Active Work`, `## Completed`, `## Blockers`, `## Next Step`.',
        ].join('\n'),
      },
      { timeoutMs: 180_000, resultTimeoutMs: 10_000 },
    );

    const coderDispatch = await workspace.runRoleTask(
      {
        roleId: 'coder',
        summary: 'Implement approved plan',
        visibility: 'public',
        instruction: [
          'Implement the requested change in this repository.',
          `Task: ${task}`,
          `Follow \`${planPath}\` and \`${briefPath}\` as the source of truth.`,
          'Keep the diff focused, preserve repository conventions, and explain any assumptions briefly in the final response.',
        ].join('\n'),
      },
      { timeoutMs: 900_000, resultTimeoutMs: 30_000 },
    );

    const testerDispatch = await workspace.runRoleTask(
      {
        roleId: 'tester',
        summary: 'Validate implementation',
        visibility: 'public',
        instruction: [
          'Validate the code changes produced by the coder.',
          `Write a concise verification report to \`${verificationPath}\`.`,
          'Prefer the narrowest useful checks first and record residual risks if full validation is not possible.',
        ].join('\n'),
      },
      { timeoutMs: 600_000, resultTimeoutMs: 20_000 },
    );

    await trackerChain;

    const finalTrackingDispatch = await workspace.runRoleTask(
      {
        roleId: 'tracker',
        summary: 'Write final executive overview',
        visibility: 'coordinator',
        instruction: [
          `Refresh \`${statusPath}\` one final time for an executive audience.`,
          `Summarize the delivery using \`${planPath}\`, \`00-management/codex-progress.log\`, and \`${verificationPath}\` if it exists.`,
          'Call out outcome, completed work, validation status, remaining risks, and the most important next step.',
        ].join('\n'),
      },
      { timeoutMs: 240_000, resultTimeoutMs: 20_000 },
    );

    console.log('\nHYBRID RUN');
    console.log(
      JSON.stringify(
        {
          cwd,
          task,
          planningDispatch,
          coderDispatch,
          testerDispatch,
          finalTrackingDispatch,
        },
        null,
        2,
      ),
    );

    await printFileIfPresent(path.join(cwd, planPath));
    await printFileIfPresent(path.join(cwd, statusPath));
    await printFileIfPresent(path.join(cwd, verificationPath));
  } finally {
    await workspace.close();
  }
}

main().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
