/**
 * Offline smoke test for rpc-parity-task-gate.mjs fixes.
 *
 * Validates without calling any LLM:
 *   (1) reloadWorklistAfterItem is set on the execute_tasks node
 *   (2) worktree is created, 00-management + tests/eval are symlinked, tasks.json visible from both sides
 *   (3) QA role is injected, afterCommit lifecycle is populated when qaAgentPromptFile is provided
 *
 * Teardown: removes the worktree + branch at the end. Writes a pass/fail report to stdout.
 */

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  createManualTaskGateCodingTemplate,
  instantiateWorkspace,
  createClaudeWorkspaceProfile,
  executeWorkflow,
} from '../dist/index.js';

const exampleDir = fileURLToPath(new URL('.', import.meta.url));
const repoRoot = path.resolve(exampleDir, '../../..');
const qaAgentPromptFile = path.join(repoRoot, '.claude', 'agents', 'cteno-qa.md');

const results = [];
function check(name, cond, detail) {
  results.push({ name, ok: !!cond, detail: detail ?? '' });
}

function git(args, opts = {}) {
  return execFileSync('git', args, { cwd: repoRoot, stdio: 'pipe', encoding: 'utf8', ...opts }).trim();
}

function ensureSymlink(linkPath, targetAbsolute) {
  if (fs.existsSync(linkPath) || fs.lstatSync(linkPath, { throwIfNoEntry: false })) {
    fs.rmSync(linkPath, { recursive: true, force: true });
  }
  fs.mkdirSync(path.dirname(linkPath), { recursive: true });
  fs.symlinkSync(targetAbsolute, linkPath);
}

const ts = new Date().toISOString().replace(/[-:.TZ]/g, '').slice(0, 14);
const worktreeDir = path.join(repoRoot, '.claude', 'worktrees', `smoke-${ts}`);
const branchName = `task-gate-smoke/${ts}`;

let createdWorktree = false;
try {
  fs.mkdirSync(path.dirname(worktreeDir), { recursive: true });
  git(['worktree', 'add', '-b', branchName, worktreeDir, 'HEAD']);
  createdWorktree = true;

  const mgmtSource = path.join(repoRoot, '00-management');
  ensureSymlink(path.join(worktreeDir, '00-management'), mgmtSource);

  const evalSource = path.join(repoRoot, 'tests', 'eval');
  if (fs.existsSync(evalSource)) {
    ensureSymlink(path.join(worktreeDir, 'tests', 'eval'), evalSource);
  }

  // --- Fix 2: worktree + symlinks ---
  const worktreeExists = fs.existsSync(worktreeDir) && fs.statSync(worktreeDir).isDirectory();
  check('fix2.worktree_created', worktreeExists, worktreeDir);

  const mgmtLink = path.join(worktreeDir, '00-management');
  const mgmtIsLink = fs.lstatSync(mgmtLink).isSymbolicLink();
  const mgmtTarget = fs.readlinkSync(mgmtLink);
  check(
    'fix2.management_symlink',
    mgmtIsLink && path.resolve(path.dirname(mgmtLink), mgmtTarget) === mgmtSource,
    `link→${mgmtTarget}`,
  );

  const probeFile = path.join(mgmtSource, `.smoke-probe-${ts}.txt`);
  fs.writeFileSync(probeFile, 'hello from main', 'utf8');
  try {
    const viaWorktree = fs.readFileSync(path.join(worktreeDir, '00-management', path.basename(probeFile)), 'utf8');
    check('fix2.bidirectional_visibility', viaWorktree === 'hello from main', viaWorktree);
  } finally {
    fs.unlinkSync(probeFile);
  }

  // --- Fix 1 + 3: build a spec with qaAgentPromptFile and inspect ---
  const template = createManualTaskGateCodingTemplate({
    reviewerModel: 'gpt-5.4',
    coderModel: 'gpt-5.4',
    taskListPath: '00-management/tasks.json',
    sharedLessonsPath: '00-management/shared-lessons.md',
    taskCliPath: 'packages/multi-agent-runtime/dist/cli/taskCli.js',
    qaAgentPromptFile,
    qaModel: 'claude-opus-4-6',
  });

  const spec = instantiateWorkspace(
    template,
    { id: 'smoke', name: 'smoke', cwd: worktreeDir },
    createClaudeWorkspaceProfile({ model: 'claude-opus-4-6' }),
  );

  const execNode = spec.workflow?.nodes?.find(n => n.id === 'execute_tasks');
  check('fix1.reloadWorklistAfterItem_set', execNode?.reloadWorklistAfterItem === true, JSON.stringify(execNode?.reloadWorklistAfterItem));

  const qaRole = spec.roles?.find(r => r.id === 'qa');
  check('fix3.qa_role_present', !!qaRole, qaRole ? `provider=${qaRole.agent?.provider} model=${qaRole.agent?.model}` : 'missing');
  check(
    'fix3.qa_prompt_loaded_from_claude_agents',
    !!qaRole?.agent?.prompt && qaRole.agent.prompt.includes('Cteno QA'),
    qaRole?.agent?.prompt ? `prompt_len=${qaRole.agent.prompt.length}` : 'no prompt',
  );
  check(
    'fix3.afterCommit_wired',
    execNode?.itemLifecycle?.afterCommit?.roleId === 'qa',
    JSON.stringify(execNode?.itemLifecycle?.afterCommit ?? null),
  );
  check(
    'fix3.afterCommit_onFailure_default_retry',
    execNode?.itemLifecycle?.afterCommit?.onFailure === 'retry',
    execNode?.itemLifecycle?.afterCommit?.onFailure ?? 'missing',
  );

  // --- Fix 5: revert on retry + exhausted ---
  check('fix5.revertOnRetry_set', execNode?.revertOnRetry === true, String(execNode?.revertOnRetry));
  check('fix5.revertOnExhaustedFailure_set', execNode?.revertOnExhaustedFailure === true, String(execNode?.revertOnExhaustedFailure));

  // --- Fix 6: reviewer on codex-sdk / gpt-5.4 ---
  const reviewerRole = spec.roles?.find(r => r.id === 'reviewer');
  check(
    'fix6.reviewer_provider_codex',
    reviewerRole?.agent?.provider === 'codex-sdk',
    `provider=${reviewerRole?.agent?.provider}`,
  );
  check(
    'fix6.reviewer_model_gpt54',
    reviewerRole?.agent?.model === 'gpt-5.4',
    `model=${reviewerRole?.agent?.model}`,
  );

  // --- Fix 7: itemTimeoutMs default wired ---
  check(
    'fix7.itemTimeoutMs_default_1h',
    execNode?.itemTimeoutMs === 60 * 60 * 1000,
    String(execNode?.itemTimeoutMs),
  );

  // --- Fix 7b: end-to-end timeout path — stub a runAssignment that hangs past itemTimeoutMs ---
  {
    const timeoutMs = 80;
    const ts2 = Date.now();
    const fakeTasksDir = path.join('/tmp', `task-gate-smoke-${ts2}`);
    fs.mkdirSync(fakeTasksDir, { recursive: true });
    const fakeTasks = path.join(fakeTasksDir, 'tasks.json');
    fs.writeFileSync(
      fakeTasks,
      JSON.stringify({
        version: 1,
        mode: 'finite',
        items: [
          { id: 't-slow', title: 'slow task', description: 'will hang', status: 'pending', attempts: 0, maxAttempts: 1 },
        ],
      }),
    );

    const fakeSpec = {
      id: 'timeout-smoke',
      name: 'timeout-smoke',
      provider: 'claude-agent-sdk',
      cwd: fakeTasksDir,
      defaultRoleId: 'reviewer',
      coordinatorRoleId: 'reviewer',
      roles: [
        { id: 'reviewer', name: 'Reviewer', agent: { provider: 'claude-agent-sdk', description: 'x', prompt: 'x' } },
        { id: 'coder', name: 'Coder', agent: { provider: 'codex-sdk', description: 'x', prompt: 'x' } },
      ],
      workflow: {
        mode: 'pipeline',
        entryNodeId: 'execute_tasks',
        nodes: [
          {
            id: 'execute_tasks',
            type: 'worklist',
            worklistArtifactId: 'task_list',
            workerRoleId: 'coder',
            stopOnItemFailure: true,
            itemTimeoutMs: timeoutMs,
            itemLifecycle: { evaluate: { roleId: 'reviewer', onReject: 'retry' } },
          },
          { id: 'complete', type: 'complete' },
        ],
        edges: [{ from: 'execute_tasks', to: 'complete', when: 'success' }],
      },
      artifacts: [{ id: 'task_list', kind: 'task_list', path: 'tasks.json' }],
      completionPolicy: { successNodeIds: ['complete'], failureNodeIds: [], maxIterations: 2 },
    };

    const request = { message: 'test', visibility: 'public' };
    const started = Date.now();
    const result = await executeWorkflow(
      fakeSpec,
      request,
      async () => {
        // Hang longer than itemTimeoutMs — force the framework timeout path.
        await new Promise(resolve => setTimeout(resolve, timeoutMs * 5));
        return { dispatchId: 'd-1', roleId: 'coder', status: 'completed', summary: 'never' };
      },
    );
    const elapsed = Date.now() - started;
    check(
      'fix7b.timeout_fires_before_hang_completes',
      elapsed < timeoutMs * 4,
      `elapsed=${elapsed}ms (< ${timeoutMs * 4}ms)`,
    );
    check(
      'fix7b.timeout_treated_as_failure',
      result.completionStatus !== 'done',
      `completionStatus=${result.completionStatus}`,
    );

    fs.rmSync(fakeTasksDir, { recursive: true, force: true });
  }

  // --- Negative control: without qaAgentPromptFile, no QA role, no afterCommit ---
  const noQaTemplate = createManualTaskGateCodingTemplate({
    reviewerModel: 'claude-opus-4-6',
    coderModel: 'gpt-5.4',
  });
  const noQaSpec = instantiateWorkspace(
    noQaTemplate,
    { id: 'smoke-no-qa', name: 'smoke', cwd: worktreeDir },
    createClaudeWorkspaceProfile({ model: 'claude-opus-4-6' }),
  );
  check('fix3.negative.no_qa_role', !noQaSpec.roles?.find(r => r.id === 'qa'), 'roles w/o qa');
  const noQaExec = noQaSpec.workflow?.nodes?.find(n => n.id === 'execute_tasks');
  check('fix3.negative.no_afterCommit', !noQaExec?.itemLifecycle?.afterCommit, JSON.stringify(noQaExec?.itemLifecycle ?? null));

  console.log('\n=== SMOKE RESULTS ===');
  for (const r of results) {
    console.log(`${r.ok ? '[PASS]' : '[FAIL]'} ${r.name}  ${r.detail ? '— ' + r.detail : ''}`);
  }
  const failed = results.filter(r => !r.ok);
  console.log(`\n${results.length - failed.length}/${results.length} passed`);
  if (failed.length > 0) process.exitCode = 1;
} catch (err) {
  console.error('Smoke test crashed:', err);
  process.exitCode = 2;
} finally {
  if (createdWorktree) {
    try {
      git(['worktree', 'remove', '--force', worktreeDir]);
    } catch (err) {
      console.warn('worktree remove failed:', err.message);
    }
    try {
      git(['branch', '-D', branchName]);
    } catch (err) {
      console.warn('branch delete failed:', err.message);
    }
  }
}
