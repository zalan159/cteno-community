/**
 * Task-gated workflow: Execute tasks.json in an isolated git worktree.
 *
 * Guarantees provided here (on top of the framework loop):
 *   - Worktree isolation: Codex edits a .claude/worktrees/task-gate-<ts>/ branch, not main.
 *   - Shared ledger: 00-management/ and tests/eval/ are symlinked from main so edits to
 *     tasks.json mid-run (by the coder via taskCli, or by the user in main) are picked up
 *     by the per-iteration ledger reload.
 *   - Review gate: reviewer evaluates each task before commit. On rejection the coder
 *     retries; on retry OR exhausted failure the worktree is hard-reset to the pre-task
 *     SHA (framework-level). No separate QA phase — coding + review only.
 *   - Single-run lock: .task-gate.lock prevents concurrent runs on the same tasks.json.
 *   - Resume: state is persisted to .task-gate-state.json; relaunching with TASK_GATE_RESUME=1
 *     reuses the previous worktree + branch instead of making a new one.
 *   - Per-task timeout: framework caps each item's lifecycle at 30 min by default; on timeout
 *     the item is reverted + retried or failed.
 *   - Graceful SIGINT / SIGTERM: workspace is closed, lockfile removed, state kept for resume.
 *   - Worktree prune: stale (>7 days) worktrees whose branch is merged or gone are cleaned
 *     on launch so .claude/worktrees/ does not accumulate.
 *
 * Roles / models:
 *   - Reviewer: Codex GPT-5.4 (codex-sdk)
 *   - Coder:    Codex GPT-5.4
 *
 * Usage:
 *   node packages/multi-agent-runtime/examples/rpc-parity-task-gate.mjs
 *
 * Environment overrides:
 *   REVIEWER_MODEL            default gpt-5.4
 *   CODER_MODEL               default gpt-5.4
 *   COORDINATOR_MODEL         default claude-opus-4-6 (workspace coordinator / claude-agent-sdk default)
 *   TASK_GATE_USE_WORKTREE    default '1'   (set '0' to run in the main checkout)
 *   TASK_GATE_RESUME          set '1' to reuse the previous worktree+branch recorded in state
 *   TASK_GATE_PRUNE_DAYS      default '7'   (age threshold for auto-prune; set '0' to disable)
 *   TASK_GATE_ITEM_TIMEOUT_MS override per-item timeout (milliseconds)
 *   TASK_GATE_TASKS_PATH      default '00-management/tasks.json'
 *   TASK_GATE_SHARED_LESSONS_PATH default '00-management/shared-lessons.md'
 *   TASK_GATE_RUN_ID          optional stable run id; defaults to task file basename
 *   MULTI_AGENT_WORKSPACE_CWD overrides cwd when worktree is disabled
 */

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  HybridWorkspace,
  createManualTaskGateCodingTemplate,
  instantiateWorkspace,
  createClaudeWorkspaceProfile,
} from '../dist/index.js';
import { attachConsoleLogger } from './_shared.mjs';

const exampleDir = fileURLToPath(new URL('.', import.meta.url));
const repoRoot = path.resolve(exampleDir, '../../..');

const reviewerModel = process.env.REVIEWER_MODEL || 'gpt-5.4';
const coderModel = process.env.CODER_MODEL || 'gpt-5.4';
const coordinatorModel = process.env.COORDINATOR_MODEL || 'claude-opus-4-6';
const useWorktree = process.env.TASK_GATE_USE_WORKTREE !== '0';
const wantsResume = process.env.TASK_GATE_RESUME === '1';
const pruneDays = Number(process.env.TASK_GATE_PRUNE_DAYS ?? '7');
const taskListPath = process.env.TASK_GATE_TASKS_PATH || '00-management/tasks.json';
const sharedLessonsPath =
  process.env.TASK_GATE_SHARED_LESSONS_PATH || '00-management/shared-lessons.md';
const itemTimeoutMsOverride = process.env.TASK_GATE_ITEM_TIMEOUT_MS
  ? Number(process.env.TASK_GATE_ITEM_TIMEOUT_MS)
  : undefined;
const requestedRunId = process.env.TASK_GATE_RUN_ID;

function slugify(value) {
  return value
    .replace(/\\/g, '/')
    .replace(/^\.\//, '')
    .replace(/[^a-zA-Z0-9._/-]+/g, '-')
    .replace(/\//g, '--')
    .replace(/^-+|-+$/g, '');
}

const defaultRunId = path.basename(taskListPath, path.extname(taskListPath));
const runId = slugify(requestedRunId || defaultRunId || 'task-gate');

const lockPath = path.join(repoRoot, '00-management', `.task-gate.${runId}.lock`);
const statePath = path.join(repoRoot, '00-management', `.task-gate-state.${runId}.json`);
const worktreeRoot = path.join(repoRoot, '.claude', 'worktrees');

function git(args, opts = {}) {
  return execFileSync('git', args, { cwd: repoRoot, stdio: 'pipe', encoding: 'utf8', ...opts }).trim();
}

function gitSafe(args, opts = {}) {
  try {
    return git(args, opts);
  } catch {
    return undefined;
  }
}

function ensureSymlink(linkPath, targetAbsolute) {
  if (fs.existsSync(linkPath) || fs.lstatSync(linkPath, { throwIfNoEntry: false })) {
    fs.rmSync(linkPath, { recursive: true, force: true });
  }
  fs.mkdirSync(path.dirname(linkPath), { recursive: true });
  fs.symlinkSync(targetAbsolute, linkPath);
}

function isPidAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    return err.code === 'EPERM';
  }
}

// ---------------------------------------------------------------- (1) lockfile
function acquireLock() {
  if (fs.existsSync(lockPath)) {
    let existing;
    try {
      existing = JSON.parse(fs.readFileSync(lockPath, 'utf8'));
    } catch {
      existing = null;
    }
    if (existing && isPidAlive(existing.pid)) {
      console.error(
        `[task-gate] another run is live: pid ${existing.pid}, started ${existing.startedAt}, worktree ${existing.worktreeDir ?? '(n/a)'}.`,
      );
      console.error(`[task-gate] remove ${lockPath} if you are sure no process holds it.`);
      process.exit(2);
    }
    console.warn(`[task-gate] cleaning stale lockfile (pid ${existing?.pid ?? '?'} gone)`);
    fs.rmSync(lockPath, { force: true });
  }
  fs.mkdirSync(path.dirname(lockPath), { recursive: true });
}

function writeLock(extra = {}) {
  const payload = {
    pid: process.pid,
    startedAt: new Date().toISOString(),
    tasksPath: taskListPath,
    runId,
    ...extra,
  };
  fs.writeFileSync(lockPath, JSON.stringify(payload, null, 2));
}

function releaseLock() {
  if (fs.existsSync(lockPath)) {
    try {
      const payload = JSON.parse(fs.readFileSync(lockPath, 'utf8'));
      if (payload.pid && payload.pid !== process.pid) return; // not ours; leave
    } catch {
      /* ignore parse errors, fall through and remove */
    }
    fs.rmSync(lockPath, { force: true });
  }
}

// ------------------------------------------------------------- (2) state/resume
function readState() {
  if (!fs.existsSync(statePath)) return undefined;
  try {
    return JSON.parse(fs.readFileSync(statePath, 'utf8'));
  } catch {
    return undefined;
  }
}

function writeState(partial) {
  const prev = readState() ?? {};
  const next = { ...prev, ...partial, updatedAt: new Date().toISOString() };
  fs.mkdirSync(path.dirname(statePath), { recursive: true });
  fs.writeFileSync(statePath, JSON.stringify(next, null, 2));
}

function clearState() {
  if (fs.existsSync(statePath)) fs.rmSync(statePath, { force: true });
}

function isWorktreeRegistered(worktreeDir) {
  const out = gitSafe(['worktree', 'list', '--porcelain']) ?? '';
  return out.split('\n').some(line => line === `worktree ${worktreeDir}`);
}

// ------------------------------------------------------------- (5) worktree prune
function pruneStaleWorktrees() {
  if (pruneDays <= 0) return;
  if (!fs.existsSync(worktreeRoot)) return;
  const cutoffMs = Date.now() - pruneDays * 24 * 60 * 60 * 1000;
  const mergedList = (gitSafe(['branch', '--merged', 'main']) ?? '')
    .split('\n')
    .map(line => line.replace(/^\*?\s*/, '').trim())
    .filter(Boolean);
  const merged = new Set(mergedList);
  for (const entry of fs.readdirSync(worktreeRoot)) {
    if (!entry.startsWith('task-gate-')) continue;
    const dir = path.join(worktreeRoot, entry);
    const stat = fs.statSync(dir, { throwIfNoEntry: false });
    if (!stat) continue;
    if (stat.mtimeMs >= cutoffMs) continue;
    const branch = `task-gate/${entry.replace(/^task-gate-/, '')}`;
    const branchExists = (gitSafe(['branch', '--list', branch]) ?? '').trim().length > 0;
    if (branchExists && !merged.has(branch)) continue; // unmerged work; leave it
    console.log(`[task-gate] pruning stale worktree ${dir} (branch ${branch} ${branchExists ? 'merged' : 'gone'})`);
    gitSafe(['worktree', 'remove', '--force', dir]);
    if (branchExists) gitSafe(['branch', '-D', branch]);
  }
  gitSafe(['worktree', 'prune']);
}

function wireSymlinks(worktreeDir) {
  const mgmtSource = path.join(repoRoot, '00-management');
  if (fs.existsSync(mgmtSource)) {
    ensureSymlink(path.join(worktreeDir, '00-management'), mgmtSource);
  }
  const evalSource = path.join(repoRoot, 'tests', 'eval');
  if (fs.existsSync(evalSource)) {
    ensureSymlink(path.join(worktreeDir, 'tests', 'eval'), evalSource);
  }
  // Link per-app node_modules from the main repo so tsc / vitest / yarn run
  // scripts work inside the worktree without needing a fresh `yarn install`
  // per turn. node_modules is gitignored, so `git worktree add` does not
  // populate it — linking is faster and avoids disk blow-up.
  wireNodeModulesSymlinks(worktreeDir);
}

function wireNodeModulesSymlinks(worktreeDir) {
  // Mirror main-repo node_modules into the worktree at the same relative
  // path via COPY (not symlink). Symlinking breaks Codex sandbox: writes
  // that hit a symlink resolve to the main repo, which is NOT inside
  // writable_roots, so prisma generate / yarn add / next build etc. get
  // EPERM. An in-worktree copy stays inside writable_roots so generators
  // that mutate node_modules work normally.
  //
  // On macOS APFS we use `cp -Rc` for copy-on-write clones (near-zero time
  // and disk cost, fully diverge-able). On other platforms we fall back to
  // fs.cpSync. Call is safe to repeat — skips if destination already a
  // real directory, only rebuilds if it's a stale symlink or missing.
  const candidateDirs = [
    '',                  // repo root node_modules (yarn workspace root, if any)
    'apps',              // apps/<name>/node_modules
    'packages',          // packages/<name>/node_modules (flat)
    'packages/agents/rust/crates', // unlikely but cheap to scan
  ];
  const roots = new Set();
  if (fs.existsSync(path.join(repoRoot, 'node_modules'))) roots.add('');
  for (const dir of candidateDirs) {
    if (!dir) continue;
    const abs = path.join(repoRoot, dir);
    if (!fs.existsSync(abs)) continue;
    let entries;
    try {
      entries = fs.readdirSync(abs, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const ent of entries) {
      if (!ent.isDirectory()) continue;
      const rel = path.join(dir, ent.name);
      if (fs.existsSync(path.join(repoRoot, rel, 'node_modules'))) {
        roots.add(rel);
      }
    }
  }
  for (const rel of roots) {
    const src = path.join(repoRoot, rel, 'node_modules');
    const dst = path.join(worktreeDir, rel, 'node_modules');
    try {
      cloneOrCopyDir(src, dst);
    } catch (err) {
      console.warn(`[task-gate] failed to materialize node_modules at ${rel}: ${err.message}`);
    }
  }
}

function cloneOrCopyDir(src, dst) {
  // If dst is a real directory already (previous clone), leave it alone.
  let statRes;
  try {
    statRes = fs.lstatSync(dst);
  } catch {
    statRes = undefined;
  }
  if (statRes?.isSymbolicLink()) {
    fs.unlinkSync(dst);
    statRes = undefined;
  }
  if (statRes?.isDirectory()) {
    return; // already materialized
  }
  fs.mkdirSync(path.dirname(dst), { recursive: true });

  if (process.platform === 'darwin') {
    // APFS copy-on-write clone — near-zero disk and wall-clock cost.
    try {
      execFileSync('cp', ['-Rc', src, dst], { stdio: 'ignore' });
      return;
    } catch {
      // Fall through to fs.cpSync below.
    }
  }
  // Cross-platform fallback.
  fs.cpSync(src, dst, { recursive: true, force: true, preserveTimestamps: true });
}

function createWorktree() {
  const ts = new Date().toISOString().replace(/[-:.TZ]/g, '').slice(0, 14);
  const worktreeDir = path.join(worktreeRoot, `${runId}-${ts}`);
  const branchName = `task-gate/${runId}/${ts}`;
  fs.mkdirSync(worktreeRoot, { recursive: true });
  git(['worktree', 'add', '-b', branchName, worktreeDir, 'HEAD']);
  wireSymlinks(worktreeDir);
  return { worktreeDir, branchName };
}

function resolveWorktree() {
  if (!useWorktree) return null;
  if (wantsResume) {
    const prev = readState();
    if (!prev?.worktreeDir || !prev?.branchName) {
      console.error('[task-gate] TASK_GATE_RESUME=1 set but no valid state file found.');
      process.exit(3);
    }
    if (!isWorktreeRegistered(prev.worktreeDir)) {
      console.error(`[task-gate] state references worktree ${prev.worktreeDir} but git does not list it.`);
      process.exit(3);
    }
    // Re-wire symlinks (main's 00-management may have been re-created).
    wireSymlinks(prev.worktreeDir);
    console.log(`[task-gate] resuming worktree: ${prev.worktreeDir}`);
    console.log(`[task-gate] resuming branch:   ${prev.branchName}`);
    return { worktreeDir: prev.worktreeDir, branchName: prev.branchName, resumed: true };
  }
  pruneStaleWorktrees();
  const created = createWorktree();
  console.log(`[task-gate] worktree: ${created.worktreeDir}`);
  console.log(`[task-gate] branch:   ${created.branchName}`);
  return { ...created, resumed: false };
}

// ----------------------------------------------------- boot: lock + state + wt
acquireLock();
const worktreeInfo = resolveWorktree();
writeLock({
  worktreeDir: worktreeInfo?.worktreeDir,
  branchName: worktreeInfo?.branchName,
});
if (worktreeInfo) {
  writeState({
    worktreeDir: worktreeInfo.worktreeDir,
    branchName: worktreeInfo.branchName,
    pid: process.pid,
    tasksPath: taskListPath,
    runId,
  });
}

const cwd = worktreeInfo?.worktreeDir
  ?? process.env.MULTI_AGENT_WORKSPACE_CWD
  ?? repoRoot;

const template = createManualTaskGateCodingTemplate({
  reviewerModel,
  coderModel,
  taskListPath,
  sharedLessonsPath,
  taskCliPath: 'packages/multi-agent-runtime/dist/cli/taskCli.js',
});

// Optional per-item timeout override (env).
if (itemTimeoutMsOverride !== undefined && Number.isFinite(itemTimeoutMsOverride)) {
  const execNode = template.workflow?.nodes?.find(n => n.id === 'execute_tasks');
  if (execNode) execNode.itemTimeoutMs = itemTimeoutMsOverride;
}

const spec = instantiateWorkspace(
  template,
  {
    id: `rpc-parity-${Date.now()}`,
    name: 'RPC Parity Task Gate',
    cwd,
  },
  createClaudeWorkspaceProfile({ model: coordinatorModel }),
);

const workspace = new HybridWorkspace({
  spec,
  defaultModels: {
    'claude-agent-sdk': coordinatorModel,
    'codex-sdk': coderModel,
  },
  codex: {
    skipGitRepoCheck: true,
    approvalPolicy: 'never',
    sandboxMode: 'workspace-write',
    // Git worktrees store their metadata under <main-repo>/.git/worktrees/<name>/,
    // which lives outside the worktree cwd. Codex `workspace-write` sandbox blocks
    // writes there unless we pass the path explicitly. Listing `.git` alone is not
    // enough — verified in smoke tests: the sandbox requires the subpath to be
    // called out so git can write HEAD/index.lock/logs during commit.
    additionalDirectories: worktreeInfo
      ? [
          path.join(repoRoot, '.git', 'worktrees', path.basename(worktreeInfo.worktreeDir)),
          path.join(repoRoot, '.git'),
        ]
      : [path.join(repoRoot, '.git')],
  },
});

const stopLogging = attachConsoleLogger(workspace, 'rpc-parity');

// ---------------------------------------------------------- (4) SIGINT/SIGTERM
let shuttingDown = false;
async function gracefulShutdown(signal) {
  if (shuttingDown) return;
  shuttingDown = true;
  console.warn(`\n[task-gate] received ${signal}; attempting graceful shutdown (state kept for resume)...`);
  try {
    await Promise.race([
      workspace.close(),
      new Promise(resolve => setTimeout(resolve, 10_000)),
    ]);
  } catch (err) {
    console.warn('[task-gate] workspace.close() errored:', err?.message ?? err);
  }
  stopLogging();
  releaseLock();
  // Intentionally DO NOT clear state — user can TASK_GATE_RESUME=1 next launch.
  process.exit(130);
}
process.on('SIGINT', () => { gracefulShutdown('SIGINT'); });
process.on('SIGTERM', () => { gracefulShutdown('SIGTERM'); });

try {
  await workspace.start();

  const turn = await workspace.runWorkspaceTurn(
    {
      message: `Execute the task list in ${taskListPath}. Each task goes through coder → reviewer → commit.`,
      workflowEntry: 'direct',
    },
    { timeoutMs: 3_600_000, resultTimeoutMs: 120_000 },
  );

  console.log('\nWORKFLOW RESULT');
  console.log(JSON.stringify({
    completionStatus: turn.completionStatus,
    dispatches: turn.dispatches?.map(d => ({
      roleId: d.roleId,
      status: d.status,
      summary: d.summary,
    })),
  }, null, 2));

  if (worktreeInfo) {
    console.log(`\n[task-gate] worktree left in place for review: ${worktreeInfo.worktreeDir}`);
    console.log(`[task-gate] merge with: git merge ${worktreeInfo.branchName}`);
    console.log(`[task-gate] drop with:  git worktree remove --force ${worktreeInfo.worktreeDir} && git branch -D ${worktreeInfo.branchName}`);
  }

  // Normal completion — drop the resume state so next launch starts fresh.
  if (turn.completionStatus === 'done') {
    clearState();
  } else {
    writeState({ lastCompletionStatus: turn.completionStatus });
  }
} finally {
  stopLogging();
  try { await workspace.close(); } catch { /* already closed in shutdown path */ }
  releaseLock();
}
