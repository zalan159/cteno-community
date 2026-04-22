---
id: "dispatch_task"
name: "Dispatch Task"
description: "Dispatch a single task or a task graph (DAG) to worker sessions"
category: "persona"
version: "2.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    task:
      type: string
      description: "Full task instruction for the worker agent (single task mode)"
    tasks:
      type: array
      description: "Task graph with dependencies (multi-task mode). Each task runs in an independent worker session. Upstream results are auto-injected into downstream tasks."
      items:
        type: object
        properties:
          id:
            type: string
            description: "Unique task ID within this group (e.g. 'crawl', 'analyze', 'report')"
          task:
            type: string
            description: "Full task instruction for this step"
          depends_on:
            type: array
            items:
              type: string
            description: "IDs of tasks that must complete first. Their results are auto-injected as context."
          profile_id:
            type: string
            description: "LLM profile for this task. Use [视觉] tagged models for image tasks, [计算机操作] for browser tasks."
          skill_ids:
            type: array
            items:
              type: string
            description: "Skill IDs to pre-activate in this task's worker session"
          workdir:
            type: string
            description: "Working directory for this task"
          agent_type:
            type: string
            description: "Agent type: 'worker' (default), 'browser' for web browsing, or a custom agent ID from AGENT.md (e.g. 'qa-engineer')"
        required:
          - id
          - task
    workdir:
      type: string
      description: "Working directory (single task mode)"
    profile_id:
      type: string
      description: "LLM profile ID (single task mode). Use [视觉] tagged models for image tasks."
    skill_ids:
      type: array
      items:
        type: string
      description: "Skill IDs to pre-activate (single task mode)"
    agent_type:
      type: string
      description: "Agent type: 'worker' (default), 'browser' for web browsing, or a custom agent ID from AGENT.md (e.g. 'qa-engineer')"
    agent_flavor:
      type: string
      description: "Optional vendor override for the worker session. Defaults to the persona's own agent/vendor (`cteno`, `claude`, `codex`, `gemini`)."
is_read_only: false
is_concurrency_safe: false
---

# Dispatch Task Tool

Dispatch a single task or a task graph (DAG of tasks with dependencies) to worker sessions.

## Two Modes

### Single Task: `task` parameter
Dispatches one task to one worker. Same as before.

### Task Graph: `tasks` parameter
Dispatches multiple tasks as a DAG. The system automatically:
1. Validates the dependency graph (no cycles allowed)
2. Starts all root tasks (no dependencies) in parallel
3. When a task completes, checks which downstream tasks have all dependencies met
4. Starts ready downstream tasks with upstream results auto-injected
5. Sends `[Task Group Complete]` when all tasks finish

## When to Use

**Single task (`task`):**
- One independent background task
- Simple delegation

**Task graph (`tasks`):**
- Multiple tasks with dependencies (e.g. crawl → analyze → report)
- Parallel execution of independent steps
- Pipeline workflows where later steps need earlier results

## Agent Type

Use `agent_type: "browser"` for web browsing tasks. Browser Agent sessions get:
- Specialized browser perception-reasoning-action loop prompt
- Whitelisted tool set: browser_*, computer_use, screenshot, websearch, read, memory, shell
- Optimized for web navigation, data extraction, and form filling

Example: `dispatch_task({ task: "打开 example.com 并提取产品列表", agent_type: "browser", profile_id: "proxy-gemini-2.5-flash" })`

## Vendor Selection

By default, dispatched workers inherit the persona's `agent` vendor. Use
`agent_flavor` only when you intentionally want the child task to run on a
different vendor than the owning persona.

## Model Selection for Task Graph

Each task in the graph can specify its own `profile_id`. Choose based on task requirements:
- **Image/visual tasks** → use a model tagged [视觉]
- **Browser/computer tasks** → use a model tagged [计算机操作]
- **Text-only tasks** → any model works, prefer cheaper ones for simple tasks

## Workflow

1. Call `memory_recall` FIRST to check for relevant past experience
2. Formulate clear task instructions (include file paths, expected outputs)
3. Call this tool with `task` (single) or `tasks` (graph)
4. **Wait for results** — workers auto-push `[Task Complete]` messages
5. For task graphs, you'll also receive `[Task Group Complete]` with a summary
6. Use `send_to_session` to provide additional instructions if needed
7. When satisfied, use `close_task_session` to close completed tasks

## Example: Task Graph

```json
{
  "tasks": [
    { "id": "crawl", "task": "爬取 example.com 的产品数据，保存到 /tmp/data.json" },
    { "id": "screenshot", "task": "截取 example.com 首页截图", "profile_id": "proxy-gemini-2.5-flash", "depends_on": [] },
    { "id": "analyze", "task": "分析爬取的数据，生成统计摘要", "depends_on": ["crawl"] },
    { "id": "report", "task": "基于数据分析和截图写一份完整报告", "depends_on": ["crawl", "analyze", "screenshot"] }
  ]
}
```

This starts `crawl` and `screenshot` in parallel, then `analyze` after `crawl`, then `report` after all three.
