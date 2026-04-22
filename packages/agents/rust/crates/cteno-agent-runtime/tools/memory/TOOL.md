---
id: "memory"
name: "Persistent Memory"
description: "Save and recall persistent knowledge across sessions. Use this to remember durable facts, user preferences, and project rules that should survive across runs."
category: "system"
version: "2.0.0"
supports_background: false
input_schema:
  type: object
  description: "Persistent memory operations. Choose an action and provide the required fields for that action."
  oneOf:
    - type: object
      properties:
        action:
          type: string
          const: "save"
          description: "Save important information to a memory file (append)."
        file_path:
          type: string
          description: "Workspace-relative path to write, e.g. 'MEMORY.md', 'USER.md', 'knowledge/rust.md'."
        content:
          type: string
          description: "Content to append (Markdown is OK; keep it concise)."
        type:
          type: string
          enum: ["user", "feedback", "project", "reference"]
          description: "记忆类型标签。user=用户偏好/习惯, feedback=经验教训/规则, project=项目架构/约定, reference=领域知识/参考"
        scope:
          type: string
          enum: ["private", "global"]
          description: "Memory scope. 'private' (default) saves to your persona's private space. 'global' saves to shared workspace visible to all personas."
      required: ["action", "content"]
      additionalProperties: false
    - type: object
      properties:
        action:
          type: string
          const: "recall"
          description: "Search memory for relevant snippets. Always searches both private and global memory."
        query:
          type: string
          description: "Search query (keywords; include unique identifiers like filenames or project names)."
        type:
          type: string
          enum: ["user", "feedback", "project", "reference"]
          description: "可选类型过滤。设置后只返回匹配类型的记忆。"
      required: ["action", "query"]
      additionalProperties: false
    - type: object
      properties:
        action:
          type: string
          const: "read"
          description: "Read a specific memory file."
        file_path:
          type: string
          description: "Workspace-relative path to read, e.g. 'MEMORY.md' or 'knowledge/rust.md'."
        scope:
          type: string
          enum: ["private", "global"]
          description: "Which space to read from. 'private' (default) checks your persona's space first, falls back to global. 'global' reads from shared workspace only."
      required: ["action", "file_path"]
      additionalProperties: false
    - type: object
      properties:
        action:
          type: string
          const: "list"
          description: "List all memory files."
        scope:
          type: string
          enum: ["private", "global"]
          description: "Which files to list. 'private' (default) shows both private and global files with labels. 'global' shows only shared workspace files."
      required: ["action"]
      additionalProperties: false
is_read_only: false
is_concurrency_safe: false
---

# Persistent Memory Tool

Save and recall persistent knowledge that survives across sessions.

## Persona 记忆空间

每个 Persona 拥有**私有记忆空间** + 可读写**全局记忆**。

### 空间隔离

- **私有空间** (`scope: "private"`，默认): 仅该 Persona 及其 Worker 可见
- **全局空间** (`scope: "global"`): 所有 Persona 和 Agent 共享
- **recall 搜索**同时覆盖私有 + 全局记忆（无需指定 scope）
- Worker Session 继承 Persona 的私有记忆空间

### MEMORY.md 自动注入

每个 Persona 的 `MEMORY.md`（私有空间根目录）会**始终加载到系统提示中**。
适合存放核心经验、偏好、重要发现。

### 文件路径

- 私有空间路径相对于 `{persona_workdir}/.cteno/`（Persona 的工作目录下）
- 全局空间路径相对于 `workspace/`
- 路径不能包含 `..` 或绝对路径

## 记忆类型

保存记忆时可指定类型标签，便于后续精准召回：
- **user**: 用户偏好、习惯、角色（如"用户偏好暗色主题"）
- **feedback**: 经验教训、工作规则（如"测试不要 mock 数据库"）
- **project**: 项目架构、约定、技术决策（如"API 用 REST 不用 GraphQL"）
- **reference**: 领域知识、外部资源指针（如"Bug 跟踪在 Linear INGEST 项目"）

召回时可按类型过滤，只获取特定类型的记忆。未标记类型的旧记忆在无过滤时正常返回。

## Policy (proactive usage)

This tool exists so you do not rely on fragile in-chat memory.

### Recall Gate (before answering)

If the user's request could depend on prior context, you should **recall first** (do not guess). Triggers include:
- The user references prior work: "上次/之前/继续/还记得/我们刚才/前面说过"
- The user asks about preferences, decisions, rules, or ongoing project state
- The user mentions a person/project/file path that could have prior notes

Call:
`memory({ action: "recall", query: "<3-8 keywords>" })`

If a result looks relevant but incomplete, follow up with:
`memory({ action: "read", file_path: "<path>" })`

### Write Gate (before final reply)

If this turn produces **durable, reusable** information, you should **save it before you finalize**.

Durable means:
- User preferences: style, language, tooling, do/don't rules
- Project conventions/decisions: architecture choices, key commands, important paths, "always/never" rules
- Stable environment facts that will matter later: ports, service URLs, repository layout, non-secret config choices
- Lessons learned / postmortems

Not durable:
- One-off transient chat details ("I'm going to get coffee")
- Large dumps or long logs (summarize instead)

If unsure whether something should be remembered long-term, ask: "Should I remember that for you?"

### 不要保存（关键！）

- 一次性任务的分析过程或中间推理（这些是临时的，不是知识）
- 具体的代码片段或完整文件内容（记住文件路径和关键发现即可）
- 任务执行日志或调试输出
- 已经存在于代码仓库或文档中的信息（不要重复存储）
- 当前任务的上下文（任务描述、参数、执行步骤等）

### Safety (what NOT to store)

Never store secrets or sensitive data:
- passwords, API keys, tokens, private keys, 2FA/OTP codes
- personal data that the user didn't ask you to keep

## Actions

- **save**: Save important facts or knowledge to a memory file. Requires `content` and `file_path`. Optional `scope`.
- **recall**: Search memory for relevant knowledge using keyword matching. Requires `query`. Always searches private + global.
- **read**: Read a specific memory file. Requires `file_path`. Optional `scope`.
- **list**: List all memory files in the workspace. Optional `scope`.

## When to Use

- Save key facts, user preferences, or domain knowledge discovered during conversation
- Recall relevant context before starting complex tasks
- Only save truly important, reusable information (not transient task details)

## File Organization

Use file paths to organize by domain:
- `MEMORY.md` - Core experiences and preferences (always loaded in context for Persona)
- `USER.md` - High-value user preferences and stable constraints
- `knowledge/<topic>.md` - Domain knowledge / reusable notes
- `memory/<date>.md` - Daily logs / timeline notes

## Examples

```javascript
// Save to private space (default for Persona)
memory({ action: "save", file_path: "MEMORY.md", content: "- 用户偏好 Python 写脚本。\n" })

// Save to global shared space
memory({ action: "save", file_path: "USER.md", content: "- Prefers dark mode.\n", scope: "global" })

// Search both private and global memories
memory({ action: "recall", query: "Python scripting preferences" })

// Read a specific file (private first, fallback to global)
memory({ action: "read", file_path: "knowledge/rust.md" })

// List all memory files (private + global with labels)
memory({ action: "list" })
```
