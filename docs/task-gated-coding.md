# Task-Gated Coding Workflow

> Claude 计划/评审 + Codex 实现，逐任务审批提交的门控编码工作流。

## 概念

Task-Gated Coding 是一套将大型开发任务拆解为离散工作项、逐项实现并门控审批的工作流框架。核心保证：**只有通过评审的代码才能 commit**。

```
任务列表(tasks.json)
    ↓
For each task:
    Coder(Codex GPT-5.4) 实现
        ↓
    Reviewer(Claude) 评审
        ├─ APPROVED → commit → 下一个 task
        └─ REJECTED → 反馈 → Coder 修复 → 重新评审
```

---

## 两种模式

### Planner 模式（默认）

Claude 先读文档 + 代码，自动生成 tasks.json，再逐个执行。

```
Planning Stage  →  Execution Stage
(Claude 生成任务)   (Codex 实现 + Claude 评审 + commit)
```

### Manual/External 模式

人工预写 tasks.json，直接进入执行阶段。适合任务已经明确的场景。

```
Execution Stage
(Codex 实现 + Claude 评审 + commit)
```

---

## 快速上手

### 1. 准备 tasks.json

```json
{
  "version": 1,
  "mode": "finite",
  "summary": "简述这批任务的目标",
  "items": [
    {
      "id": "task-01",
      "title": "简短标题",
      "description": "详细描述：做什么、为什么、怎么做",
      "status": "pending",
      "attempts": 0,
      "maxAttempts": 2,
      "dependsOn": [],
      "files": ["src/foo.rs", "src/bar.rs"],
      "acceptanceCriteria": [
        "foo.rs 中新增 xxx 函数",
        "cargo check 通过"
      ]
    },
    {
      "id": "task-02",
      "title": "第二个任务",
      "description": "...",
      "status": "pending",
      "attempts": 0,
      "maxAttempts": 2,
      "dependsOn": ["task-01"],
      "files": ["src/baz.rs"],
      "acceptanceCriteria": ["..."]
    }
  ]
}
```

### 2. 编写执行脚本

```javascript
import {
  HybridWorkspace,
  createManualTaskGateCodingTemplate,
  instantiateWorkspace,
  createClaudeWorkspaceProfile,
} from '@anthropic-ai/multi-agent-runtime';

const template = createManualTaskGateCodingTemplate({
  reviewerModel: 'claude-opus-4-6',
  coderModel: 'gpt-5.4',
  taskListPath: '00-management/tasks.json',
  sharedLessonsPath: '00-management/shared-lessons.md',
});

const spec = instantiateWorkspace(template, {
  id: `my-workflow-${Date.now()}`,
  name: 'My Task Gate Workflow',
  cwd: process.cwd(),
}, createClaudeWorkspaceProfile({ model: 'claude-opus-4-6' }));

const workspace = new HybridWorkspace({
  spec,
  defaultModels: {
    'claude-agent-sdk': 'claude-opus-4-6',
    'codex-sdk': 'gpt-5.4',
  },
  codex: {
    skipGitRepoCheck: true,
    approvalPolicy: 'never',
    sandboxMode: 'workspace-write',
  },
});

await workspace.start();
const turn = await workspace.runWorkspaceTurn(
  { message: '执行 tasks.json 中的任务', workflowEntry: 'direct' },
  { timeoutMs: 600_000, resultTimeoutMs: 60_000 },
);
await workspace.close();
```

### 3. 运行

```bash
node my-workflow.mjs
```

---

## tasks.json 格式

### 顶层结构

| 字段 | 类型 | 说明 |
|------|------|------|
| `version` | `1` | 固定 |
| `mode` | `"finite"` \| `"replenishing"` | `finite` = 执行完就结束；`replenishing` = 执行间可追加新任务 |
| `summary` | `string` | 这批任务的整体描述 |
| `items` | `WorkItem[]` | 任务列表（按执行顺序） |

### WorkItem 字段

| 字段 | 类型 | 必填 | 说明 |
|------|------|:---:|------|
| `id` | `string` | Yes | 唯一标识，建议用 `kebab-case` |
| `title` | `string` | Yes | 简短标题（commit message 会用） |
| `description` | `string` | Yes | 详细说明，给 Coder 看的 |
| `status` | `string` | No | 初始状态，默认 `"pending"` |
| `attempts` | `number` | No | 已尝试次数，默认 `0` |
| `maxAttempts` | `number` | No | 最大重试次数，超出则标 `discarded` |
| `dependsOn` | `string[]` | No | 依赖的 task ID，前置任务完成才执行 |
| `files` | `string[]` | No | 参考文件（提示，非硬限制） |
| `acceptanceCriteria` | `string[]` | No | 验收标准，Reviewer 据此评审 |
| `metadata` | `object` | No | 自定义元数据（如 `statusReason`） |

### 状态流转

```
pending ──→ running ──→ completed
                │
                ├──→ pending    (评审拒绝，retry)
                ├──→ failed     (评审拒绝，不重试)
                ├──→ discarded  (超出 maxAttempts)
                ├──→ abandoned  (Coder 通过 CLI 放弃)
                └──→ superseded (Coder 通过 CLI 替换为新任务)
```

---

## 角色分工

| 角色 | Provider | 职责 |
|------|----------|------|
| **Planner** (planner 模式) | claude-agent-sdk | 读文档、拆任务、生成 tasks.json、评审实现 |
| **Reviewer** (manual 模式) | claude-agent-sdk | 评审实现是否满足验收标准 |
| **Coder** | codex-sdk (GPT-5.4) | 逐个实现任务、提交代码 |

### 执行生命周期（每个 task）

```
1. Coder 实现
   ↓ (读 shared-lessons → 改代码 → 报告变更)
2. Reviewer 评审
   ↓ (检查文件变更 vs acceptanceCriteria)
   ├─ APPROVED: → 3. Coder commit
   └─ REJECTED: → 反馈写入 feedback → 回到 1 (retry)
3. Coder commit
   ↓ (git add + git commit)
4. 标记 completed → 下一个 task
```

---

## Task CLI

工作流中 Coder 可以通过 Task CLI 动态管理任务列表，无需手编 JSON。

```bash
# 基础用法（在工作流 prompt 中自动注入路径）
node packages/multi-agent-runtime/dist/cli/taskCli.js <command> --tasks <path> --cwd .
```

### 命令一览

| 命令 | 说明 | 示例 |
|------|------|------|
| `list` | 列出所有任务 | `taskCli list --tasks tasks.json` |
| `get <id>` | 查看单个任务详情 | `taskCli get task-01 --tasks tasks.json` |
| `add` | 新增任务 | 见下方 |
| `update <id>` | 修改任务字段 | 见下方 |
| `set-status <id> <status>` | 修改状态 | `taskCli set-status task-01 abandoned --reason "拆分为更小任务"` |
| `lessons:add` | 追加共享教训 | 见下方 |

### add — 新增任务

```bash
taskCli add \
  --tasks 00-management/tasks.json \
  --id follow-up-01 \
  --title "修复遗留的 lint 错误" \
  --description "上一个任务引入了 3 个 clippy warning，需要修复" \
  --after task-02 \
  --files "src/foo.rs,src/bar.rs" \
  --criteria "cargo clippy 零 warning" \
  --criteria "不改变功能行为" \
  --max-attempts 2
```

### update — 修改任务

```bash
taskCli update task-01 \
  --tasks 00-management/tasks.json \
  --title "新标题" \
  --description "更新后的描述"
```

### set-status — 改状态

```bash
# 放弃当前任务，改用更细的拆分
taskCli set-status task-01 superseded \
  --tasks 00-management/tasks.json \
  --reason "任务太大，已拆分为 task-01a 和 task-01b"
```

允许的状态值：`pending`, `running`, `completed`, `failed`, `blocked`, `discarded`, `abandoned`, `superseded`

### lessons:add — 记录教训

```bash
taskCli lessons:add \
  --lessons 00-management/shared-lessons.md \
  --title "cargo check 前检查进程" \
  --body "跑 cargo check 前先 pgrep cargo，有进程则跳过，避免锁冲突" \
  --task-id task-01 \
  --role coder
```

---

## Shared Lessons（共享教训）

跨任务的经验积累文件，Coder 每个任务开始前读、完成后写。

### 文件格式

```markdown
# Shared Lessons

## 2026-04-16T10:23:45.123Z - cargo check 前检查进程
- Task: task-01
- Role: coder

跑 cargo check 前先 pgrep cargo，有进程则跳过，避免锁冲突。

## 2026-04-16T11:00:00.000Z - base64 crate 版本
- Task: task-03
- Role: coder

本项目用 base64 0.13 (encode/decode 函数)，不是 0.21 (Engine trait)。
```

### 工作流中的集成

1. **Coder 收到的 prompt** 包含：`"Before changing code, read shared lessons if present: {{shared_lessons_path}}"`
2. **Coder 完成任务前** 被要求：`"Append any reusable pitfall or workaround to the shared lessons file via the CLI"`
3. 教训在整个 workflow 生命周期内持续积累

---

## Task Control Protocol

Agent 在响应末尾可以包含结构化的 `task-control` 代码块，让运行时提取机器可读的状态更新。

### 格式

````
```task-control
{"toolName":"task.set_status","input":{"status":"completed","summary":"实现完成"}}
{"toolName":"task.write_handoff","input":{"summary":"准备评审","toRoleId":"reviewer"}}
```
````

### 可用工具

| 工具名 | 用途 | 关键参数 |
|--------|------|----------|
| `task.set_status` | 更新任务状态 | `status`, `summary`, `reason` |
| `task.write_handoff` | 结构化交接 | `summary`, `details`, `toRoleId` |
| `task.submit_review` | 提交评审结论 | `verdict: "approved"\|"rejected"`, `summary`, `issues[]` |
| `task.record_evidence` | 附加证据 | `kind`, `summary`, `content`, `path` |

---

## 模板配置参考

### createManualTaskGateCodingTemplate(options)

用于已有 tasks.json 的场景（跳过规划阶段）。

```typescript
interface TaskGateCodingTemplateOptions {
  plannerModel?: string;       // Planner 模式下的规划模型
  reviewerModel?: string;      // 评审模型，默认 'claude-opus-4-6'
  coderModel?: string;         // 实现模型，默认 'gpt-5.4'
  taskSource?: 'planner' | 'external';  // 默认 'planner'
  taskListPath?: string;       // 默认 '00-management/tasks.json'
  sharedLessonsPath?: string;  // 默认 '00-management/shared-lessons.md'
  taskCliPath?: string;        // 默认 'packages/multi-agent-runtime/dist/cli/taskCli.js'
}
```

### createTaskGateCodingTemplate(options)

完整版，根据 `taskSource` 选择 planner 或 external 模式。

### HybridWorkspace 配置

```typescript
new HybridWorkspace({
  spec,                          // instantiateWorkspace() 的输出
  defaultModels: {
    'claude-agent-sdk': 'claude-opus-4-6',
    'codex-sdk': 'gpt-5.4',
  },
  codex: {
    skipGitRepoCheck: true,      // 不检查是否在 git repo 中
    approvalPolicy: 'never',     // Codex 不弹人工审批
    sandboxMode: 'workspace-write', // 允许写工作区文件
  },
});
```

### runWorkspaceTurn 参数

```typescript
workspace.runWorkspaceTurn(
  {
    message: '执行任务描述',
    workflowEntry: 'direct',    // 跳过 coordinator 直接执行工作流
  },
  {
    timeoutMs: 600_000,          // 整体超时 10 分钟
    resultTimeoutMs: 60_000,     // 单次结果等待超时
  },
);
```

---

## 仓库内示例脚本

| 脚本 | 模式 | 说明 |
|------|------|------|
| `examples/rpc-parity-task-gate.mjs` | Manual | 手写 tasks.json + Claude 评审 + Codex 实现 |
| `examples/cteno-plan-coding-eval.mjs` | Planner | Claude/Codex 规划 + 实现 + 评估 |
| `src/examples/hybridClaudeCodex.ts` | 自定义 | Claude 规划 + Codex 实现 + Codex 测试 |

### 运行示例

```bash
# Manual 模式（已有 tasks.json）
node packages/multi-agent-runtime/examples/rpc-parity-task-gate.mjs

# 环境变量覆盖
REVIEWER_MODEL=claude-opus-4-6 \
CODER_MODEL=gpt-5.4 \
node packages/multi-agent-runtime/examples/rpc-parity-task-gate.mjs

# Planner 模式
MULTI_AGENT_TASK="实现 xxx 功能" \
node packages/multi-agent-runtime/examples/cteno-plan-coding-eval.mjs
```

---

## 最佳实践

1. **任务粒度**：2-6 个任务为一批，每个任务应该是一个可独立 review 的变更
2. **acceptanceCriteria 要具体**：不写"代码质量好"，写"cargo check 通过且无新 warning"
3. **files 是提示不是限制**：Coder 可以改 files 之外的文件，但 files 帮助 Reviewer 聚焦
4. **善用 dependsOn**：有依赖关系的任务串行，无依赖的可并行（当前实现按顺序逐个）
5. **maxAttempts 设 2-3**：给评审拒绝后的修复机会，但避免无限循环
6. **Shared Lessons 持续写**：每个任务完成后记录踩坑点，后续任务受益
7. **大任务及时拆分**：Coder 发现任务太大时，通过 CLI `add` + `set-status superseded` 拆分
