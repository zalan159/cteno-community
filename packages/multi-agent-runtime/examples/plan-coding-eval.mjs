// Plan-Coding-Eval launcher.
//
// 用法：
//   node ./examples/plan-coding-eval.mjs [goal-file]
//
// goal-file 默认 ./goals/t13-recovery.md；需要是 markdown 描述一个具体 refactor 目标。
// 会在仓库根的 /tmp/plan-coding-eval-<ts>/ 新建 scratch 工作区，
// 并在 cwd 下产出 plans/*.md / eval/*.md 供评审。

import path from 'node:path';
import fs from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import {
  ClaudeAgentWorkspace,
  createClaudeWorkspaceProfile,
  createPlanCodingEvalTemplate,
  instantiateWorkspace,
} from '../dist/index.js';
import { attachConsoleLogger } from './_shared.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '../../..');

const goalArg = process.argv[2] ?? path.join(__dirname, 'goals/t13-recovery.md');
const goalPath = path.resolve(goalArg);
const goalText = await fs.readFile(goalPath, 'utf8').catch(() => {
  console.error(`Goal file not found: ${goalPath}`);
  process.exit(2);
});

const goalSlug = path
  .basename(goalPath, path.extname(goalPath))
  .replace(/[^a-z0-9]+/gi, '-')
  .toLowerCase();

// 重点：让 agent 在 Cteno 仓库真实工作目录里操作（改代码 / 跑 cargo check）。
// 如果想要 sandbox 可改成 createScratchDir。
const cwd = REPO_ROOT;

const workspace = new ClaudeAgentWorkspace({
  spec: instantiateWorkspace(
    createPlanCodingEvalTemplate(),
    {
      id: `pce-${goalSlug}-${Date.now()}`,
      name: `PCE: ${goalSlug}`,
      cwd,
    },
    createClaudeWorkspaceProfile(),
  ),
});

const stopLogging = attachConsoleLogger(workspace, goalSlug);

const message = `# Goal

${goalText}

# Working instructions

- All edits happen in cwd (${cwd}). 不要离开此目录。
- Planner writes plan to \`plans/${goalSlug}.md\` (create if missing).
- Coder applies plan; runs \`cd apps/client/desktop && cargo check --message-format=short 2>&1 | tail -10\` and \`cargo check --no-default-features --message-format=short 2>&1 | tail -10\`; does NOT git commit.
- Evaluator runs the same cargo check double forms, plus any smoke command the plan specifies. Writes \`eval/${goalSlug}.md\` with pass/fail.

Begin with planner.`;

console.log(`\n=== Launching plan-coding-eval for goal: ${goalSlug} ===`);
console.log(`cwd: ${cwd}`);
console.log(`goal doc: ${goalPath}`);

try {
  await workspace.start();
  const turn = await workspace.runWorkspaceTurn(
    { message },
    { timeoutMs: 60 * 60 * 1000, resultTimeoutMs: 5 * 60 * 1000 },
  );
  console.log('\n=== WORKSPACE TURN COMPLETE ===');
  console.log(JSON.stringify(turn, null, 2));
} catch (err) {
  console.error('\n=== WORKSPACE ERROR ===');
  console.error(err);
  process.exitCode = 1;
} finally {
  stopLogging();
  await workspace.close();
}
