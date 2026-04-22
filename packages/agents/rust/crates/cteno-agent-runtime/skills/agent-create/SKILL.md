---
id: agent-create
name: "Agent 创建"
description: "创建自定义 Agent（AGENT.md），定义专属身份、工具和行为"
when_to_use: "用户想创建新的自定义 Agent 类型时使用"
version: "1.0.0"
tags:
  - agent
  - custom-agent
  - agent-creation
user_invocable: true
disable_model_invocation: false
---

# Agent 创建

## 流程

1. **确认需求** — 询问用户：
   - Agent 做什么？（名称 + 一句话描述）
   - 项目级（仅当前工作目录）还是全局（所有项目可用）？

2. **设计 Agent** — 根据需求确定：
   - `id`: 小写字母 + 连字符，2-4 词（如 `code-reviewer`、`test-runner`）
   - 工具限制：`allowed_tools`（白名单）或 `excluded_tools`（黑名单），不设则继承 Worker 全量工具
   - System Prompt：用第二人称（"你是..."），写清行为边界和方法论

3. **写入文件** — 用 `shell` 工具创建目录和 AGENT.md：
   - **项目级**: `{workdir}/.cteno/agents/{id}/AGENT.md`
   - **全局**: `~/.cteno/agents/{id}/AGENT.md`（macOS 实际路径约 `~/Library/Application Support/com.frontfidelity.cteno/agents/{id}/AGENT.md`，通过 `~/.cteno/agents/` 软链接访问）

4. **确认** — 告知用户 Agent 已创建，可通过 `dispatch_task` 的 `agent_type` 参数调用

## AGENT.md 格式

```markdown
---
name: "Agent 显示名称"
description: "一句话描述用途"
version: "1.0.0"
type: "autonomous"
# 可选字段：
# model: "deepseek-chat"              # 模型覆盖
# temperature: 0.2                     # 温度
# max_tokens: 4096                     # 最大 token
# allowed_tools: ["shell", "read", "edit"]   # 工具白名单
# excluded_tools: ["browser_navigate"]       # 工具黑名单
# expose_as_tool: false                # 是否暴露为父 Agent 可调用的工具
---

（这里写 system prompt，Markdown 格式）

# Agent 名称

## 核心身份
你是 ...

## 行为准则
1. ...
2. ...

## 可用工具
- **shell** — ...
- **read** — ...
```

## 设计原则

- **具体 > 泛化**：每条指令必须有价值，不要写空洞的"请确保质量"
- **工具最小化**：只给 Agent 完成任务所需的工具，不要全量授权
- **边界清晰**：写清"做什么"和"不做什么"
- **包含示例**：在 system prompt 中用具体示例说明期望行为
- **面向自主执行**：Agent 会独立运行，prompt 是它的完整操作手册

## 内置 Agent Kind

创建自定义 Agent 前，先考虑是否已有合适的内置类型：

| Kind | 说明 | 工具集 |
|------|------|--------|
| `worker` | 通用任务执行 | 全量（排除 persona-only 和 browser-only） |
| `browser` | 浏览器自动化 | browser_* + websearch + read/write/edit/shell |

如果内置类型不能满足需求（如需要不同的工具集或专属 prompt），再创建自定义 Agent。
