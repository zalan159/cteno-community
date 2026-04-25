# Skill 系统架构

> 调研日期: 2026-04-06

## 概述

Skill 是 Markdown 指导模块（SKILL.md + 可选 scripts/），指导 Agent 组合使用 Tools 完成复杂任务。Skill **不是可调用的工具**，而是激活后返回给 LLM 的指令文本。

---

## 1. Skill 格式

### 目录结构

```
skills/
├── orchestration/
│   ├── SKILL.md          # 主文件（YAML frontmatter + Markdown）
│   └── scripts/          # 可选辅助脚本
├── xlsx/
│   ├── SKILL.md
│   └── scripts/
├── browser-automation/
│   └── SKILL.md
└── ...
```

### SKILL.md 格式

```yaml
---
id: skill-identifier          # 可选；默认用目录名
name: Display Name            # 必填
description: What it does     # 必填
version: 1.0.0               # 可选
when_to_use: 使用时机描述
allowed-tools: [tool1, tool2] # 激活时额外可用的工具
user-invocable: true          # 用户是否可直接调用（默认 true）
disable-model-invocation: false # 是否对模型隐藏（默认 false）
context: inline | fork        # 执行上下文（默认 inline）
agent: worker                 # fork 模式的 AgentKind
model: proxy-deepseek-reasoner # 模型覆盖
effort: high                  # 思考力度覆盖
---

# Markdown 正文
这里是 LLM 激活后会读到的指令内容。
支持 ${SKILL_DIR}、${SESSION_ID}、${SKILL_ID}、${SKILL_NAME}、$ARGS、$1 变量替换。
支持嵌入 shell 块：```! command ``` 和行内 !`command`
```

### SkillConfig 结构体

定义在 `src/service_init.rs:527`：

```rust
pub struct SkillConfig {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub instructions: Option<String>,        // SKILL.md 正文（懒加载）
    pub when_to_use: Option<String>,
    pub allowed_tools: Option<StringOrVec>,
    pub user_invocable: bool,
    pub disable_model_invocation: bool,
    pub context: Option<SkillContext>,       // Inline | Fork
    pub agent: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    // 运行时字段：
    pub is_bundled: bool,
    pub path: Option<PathBuf>,
    pub source: Option<String>,             // "builtin" / "global" / "workspace"
}
```

---

## 2. 三层加载体系

**优先级从低到高（后者覆盖前者）：**

| 层级 | 目录 | 说明 |
|------|------|------|
| Builtin | `apps/desktop/src-tauri/skills/` | 随 App 发布 |
| Global | `~/.agents/skills/` | 用户全局 |
| Workspace | `{workdir}/.cteno/skills/` | 项目级 |

### 加载流程

`service_init.rs` 中的 `load_all_skills(builtin_dir, global_dir, workspace_dir)`：

1. 读取每层目录下的子目录
2. 跳过直接位于根目录的 `SKILL.md`（那是系统说明，不是 skill）
3. 每个子目录调用 `load_skill_from_dir(path, dir_name)`
4. 解析 YAML frontmatter + Markdown 正文
5. 以 skill ID 为 key 合并到 HashMap（后加载的覆盖先加载的）
6. 返回 `Vec<SkillConfig>`

### Builtin Skill 同步

`sync_builtin_skills()` 在启动时：
- 将 builtin skills 同步到 global 目录（若不存在）
- 如 app 版本更高，升级已有 builtin skills（semver 比较）
- 清理标有 `.cteno-source.json` 但 builtin 中已删除的孤儿 skill

---

## 3. 运行时注入

### Skill 索引注入

`build_skill_index_message()` 在每次会话中：

1. 过滤掉 `disable_model_invocation: true` 的 skill
2. 预算控制：不超过 context window 的 1%
3. 始终包含 bundled skills
4. 生成格式化 Markdown：

```markdown
## Available Skills
Use the `skill` tool with `activate` operation to load a skill's full instructions.

- orchestration: 多 Agent 编排 -- 需要多 Agent 协作时使用
- xlsx: Excel 创建/编辑 -- 处理电子表格时使用
```

5. 作为 runtime context message 注入（不是 system prompt，以保持 prompt caching）

### 预激活

Session 配置可指定 `pre_activated_skill_ids`，在会话创建时自动注入完整 `<activated_skill>` 块。

**关键代码** `session.rs:3064-3086`：
```rust
// 轻量索引：始终注入
if let Some(skill_index) = build_skill_index_message(&enabled_skills, context_window_tokens) {
    runtime_context_messages.push(skill_index);
}

// 预激活：注入完整内容
if let Some(ref pre_skill_ids) = config.pre_activated_skill_ids {
    for skill_id in pre_skill_ids {
        // 找到并注入 <activated_skill> 块
    }
}
```

---

## 4. 统一 `skill` 工具

### 工具定义

`tools/skill/TOOL.md` 定义了统一的 `skill` 工具，支持 10 个 operation：

| Operation | 说明 |
|-----------|------|
| `list` | 列出所有已安装 skill |
| `activate` | 加载 skill 指令到上下文 |
| `deactivate` | 标记 skill 为未激活 |
| `search` | 搜索 SkillHub 注册表 |
| `browse` / `featured` | 查看热门/推荐 skill |
| `install` | 从 SkillHub 下载安装 |
| `create` | 初始化新 skill 结构 |
| `validate` | 检查 skill 格式 |
| `package` | 创建可分发的 .skill 文件 |
| `delete` | 删除 skill |

### 重构历史

之前拆成两个工具，现已合并为一个：

```
删除:
  - tools/skill_context/TOOL.md    → 合并到 skill
  - tools/skill_manager/TOOL.md    → 合并到 skill
  - tool_executors/skill_context.rs → 合并到 skill.rs
  - tool_executors/skill_manager.rs → 合并到 skill.rs
保留:
  - tools/skill/TOOL.md            ← 统一入口
  - tool_executors/skill.rs        ← 统一实现
```

---

## 5. 激活流程（核心）

### Inline 模式（默认）

`skill.rs` 中的 `activate_skill()` (line 77)：

```
LLM 调用 skill tool (operation: activate, id: "xxx")
    ↓
SkillExecutor::activate_skill()
    ↓
从三层目录加载 skills → 找到目标 skill
    ↓
变量替换:
  ${SKILL_DIR}  → skill 目录绝对路径
  ${SESSION_ID} → 当前 session ID
  ${SKILL_ID}   → skill ID
  ${SKILL_NAME} → skill 名称
  $ARGS / $1    → 传入的参数
    ↓
Shell 块执行:
  ```! command ``` → 替换为命令输出
  !`command`      → 行内替换为命令输出
  - 在 skill 目录中执行
  - 输出限制 10KB
    ↓
返回 XML:
  <activated_skill id="..." name="...">
    <description>...</description>
    <instructions>处理后的完整指令</instructions>
    <available_resources>skill 目录文件树</available_resources>
  </activated_skill>
    ↓
LLM 阅读指令，使用基础 tools 执行
```

### Fork 模式

当 `context: fork` 时，目标语义是：fork 出来的工作仍属于当前 agent session 的内部子任务，由当前 vendor runtime 管理其 subagent / DAG / wait / merge 生命周期。Host 只提供必要的 session 元数据、权限闭环和事件传输，不拥有 fork 状态机。

当前 legacy 实现里，`activate_fork_skill()` (line 143) 仍通过 PersonaManager 创建独立 task session：

1. 从 input 提取 owner/persona ID
2. 通过 PersonaManager dispatch_task 创建独立 agent session
3. 返回 `{ sessionId, status }` 给调用方
4. Fork 出的 agent 在独立 session 中执行 skill 指令
5. 结果通过 `notify_task_result()` 推回原 session

迁移方向：

1. Cteno fork/subagent 能力收敛到 `cteno-agent-runtime`
2. Claude / Codex / Gemini 由各自 adapter 映射到原生子任务能力或明确降级
3. `dispatch_task`/PersonaManager 保留为兼容入口，不继续扩展为新的通用 DAG 引擎

---

## 6. System Prompt 中的说明

`system_prompt.rs` 中关于 Skill 的说明（line 140, 328-344）：

> Skills are guidance modules — each SKILL.md provides instructions for you to follow using basic tools (shell, read, file, edit, etc.). Skills are NOT callable tools themselves.
>
> When you activate a skill, its SKILL.md instructions are returned in the tool result via `<activated_skill>` — do NOT re-read the SKILL.md file.

---

## 7. 工具路由

`llm.rs` 和 `tool_executors/mod.rs` 中的路由逻辑：

```
LLM tool call
    ↓
Persona/dispatch_task → 直接处理
    ↓ (否则)
Tools API 查找 → 找到则执行
    ↓ (失败)
Skills API 回退 → 按 skill ID 查找并激活
```

---

## 8. 架构全景

```
┌─────────────────────────────────────────────────┐
│                  System Prompt                   │
│  "Skills are guidance modules..."               │
│  + Skill 索引（轻量摘要）                         │
│  + 预激活的 <activated_skill> 块                  │
└─────────────────────┬───────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────┐
│                    LLM                          │
│  1. 阅读 Skill 索引                              │
│  2. 决定是否需要激活某 skill                      │
│  3. 调用 skill tool (activate)                   │
│  4. 阅读返回的 <activated_skill> 指令             │
│  5. 使用基础 tools 执行指令                       │
└─────────────────────┬───────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────┐
│              SkillExecutor                       │
│                                                  │
│  load_skills():                                  │
│    builtin/ → global/ → workspace/ (三层合并)     │
│                                                  │
│  activate_skill():                               │
│    ├─ inline: 变量替换 → shell 执行 → 返回 XML    │
│    └─ fork:   vendor runtime subtask              │
│              (legacy: dispatch_task → session)    │
│                                                  │
│  list/create/search/install/delete/...           │
└─────────────────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────┐
│              三层 Skill 目录                      │
│                                                  │
│  1. src-tauri/skills/        (Builtin, 最低)     │
│  2. ~/.agents/skills/        (Global)            │
│  3. {workdir}/.cteno/skills/ (Workspace, 最高)   │
└─────────────────────────────────────────────────┘
```

---

## 9. 当前 Builtin Skills

| Skill | 用途 |
|-------|------|
| `orchestration` | 多 Agent 编排模式 |
| `xlsx` | Excel 创建/编辑 |
| `browser-automation` | CDP 浏览器自动化 |

---

## 10. 关键文件索引

| 文件 | 作用 |
|------|------|
| `src/service_init.rs` | SkillConfig 定义、加载、索引构建 |
| `src/tool_executors/skill.rs` | SkillExecutor 统一实现 |
| `src/tool_executors/mod.rs` | 工具注册 |
| `tools/skill/TOOL.md` | Skill 工具定义 |
| `src/system_prompt.rs` | LLM 可见的 Skill 说明 |
| `src/happy_client/session.rs` | 运行时 Skill 注入 |
| `src/happy_client/manager.rs` | 预激活配置 |
| `skills/*/SKILL.md` | 各 Builtin Skill |
