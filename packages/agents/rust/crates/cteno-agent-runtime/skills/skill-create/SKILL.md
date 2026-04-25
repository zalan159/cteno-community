---
id: skill-create
name: "Skill 创建"
description: "创建自定义 Skill（SKILL.md），定义可复用的工作流程和指导"
when_to_use: "用户想创建新的自定义 Skill 时使用"
version: "1.0.0"
tags:
  - skill
  - skill-creation
  - workflow
user_invocable: true
disable_model_invocation: false
---

# Skill 创建

## 流程

1. **确认需求** — 询问用户：
   - 这个 Skill 做什么？（名称 + 一句话描述）
   - 项目级还是全局？
     - **项目级**: `{workdir}/.cteno/skills/{id}/SKILL.md` — 仅当前项目可用
     - **全局**: `~/.cteno/skills/{id}/SKILL.md` — 所有项目可用

2. **设计 Skill** — 根据需求确定：
   - `id`: 小写字母 + 连字符（如 `code-review`、`deploy-staging`）
   - 触发条件（`when_to_use`）：何时应该激活，包含触发短语和示例
   - 步骤拆解：每步必须有**成功标准**
   - 是否需要参数（`$arg_name` 替换）

3. **写入文件** — 用 `shell` 工具创建目录和 SKILL.md

4. **确认** — 告知用户 Skill 已创建，如何激活使用

## SKILL.md 格式

```markdown
---
id: skill-name
name: "显示名称"
description: "一句话描述"
version: "1.0.0"
when_to_use: "详细说明何时触发。以'用户需要...'开头，包含触发短语。例：'用户需要部署到 staging 环境时使用。触发词：部署、发布、上线'"
tags:
  - tag1
  - tag2
# 可选字段：
# user_invocable: true            # 用户可通过 skill tool 手动激活（默认 true）
# disable_model_invocation: false  # 禁止模型自动激活（默认 false）
# argument_hint: "[file] [target]" # 参数提示
# model: "deepseek-chat"          # 模型覆盖
# context: fork                    # 执行上下文：inline（默认）或 fork（子 agent）
# agent: worker                    # fork 时使用的 AgentKind
# allowed_tools:                   # 允许的额外工具
#   - browser_navigate
---

# Skill 标题

简要描述这个 Skill 做什么。

## 输入（如有参数）
- `$filename`: 要处理的文件
- `$target`: 部署目标

## 目标
明确的目标和完成标准。最好有具体的交付物。

## 步骤

### 1. 步骤名称
具体说明要做什么。包含命令示例。

**成功标准**: 如何确认此步完成，可以进入下一步。

### 2. 第二步
...

### 3a. 并行步骤 A（与 3b 同时进行）
...

### 3b. 并行步骤 B
...

### 4. [用户] 确认并审批
需要用户介入的步骤用 `[用户]` 标注。用于不可逆操作（合并、发布、删除）。
```

## 设计原则

- **`when_to_use` 是最重要的字段** — 决定模型何时自动建议激活。写清触发条件和示例短语
- **每步必须有成功标准** — 模型需要知道何时可以进入下一步
- **简单 Skill 保持简单** — 2 步的 Skill 不需要在每步加复杂注解
- **可并行步骤用子编号** — 3a, 3b 表示可同时执行
- **不可逆操作加 `[用户]`** — 合并 PR、发送消息、删除文件等

## 内置 Skill 参考

默认启用的内置 Skill（避免重复创建）：

| ID | 名称 | 用途 |
|----|------|------|
| `orchestration` | 编排系统 | 多 Agent 编排脚本 |
| `browser-automation` | 浏览器自动化 | CDP 脚本编写 |
| `mcp` | MCP Skill | 解析 MCP 文档并自动安装/注册 MCP Server |
| `pptx` | PPTX | PowerPoint 创建/编辑 |
| `xlsx` | XLSX | Excel 读取/生成 |
| `agent-create` | Agent 创建 | 创建自定义 Agent |
