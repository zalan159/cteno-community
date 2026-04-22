---
id: "schedule_task"
name: "Schedule Task"
description: "Create a scheduled task that runs automatically at specified times"
category: "system"
version: "1.1.0"
supports_background: false
should_defer: true
search_hint: "schedule task cron timer recurring automatic"
input_schema:
  type: object
  properties:
    name:
      type: string
      description: "Task name (short human-readable description)"
    task_prompt:
      type: string
      description: "Full instruction for the Agent (dispatch), shell command (script), or message to send (hypothesis)"
    task_type:
      type: string
      enum: ["dispatch", "script", "hypothesis"]
      description: "Execution type. dispatch=Agent task via Persona (default), script=shell command, hypothesis=message to hypothesis agent"
    hypothesis_agent_id:
      type: string
      description: "Hypothesis agent ID (required when task_type=hypothesis)"
    schedule_kind:
      type: string
      enum: ["at", "every", "cron"]
      description: "Schedule type: at=one-time, every=interval, cron=cron expression"
    schedule_at:
      type: string
      description: "One-time execution time (ISO-8601 with timezone, e.g. 2026-02-21T09:00:00+08:00). Required when schedule_kind=at"
    schedule_in_seconds:
      type: integer
      description: "Relative one-time delay in seconds from NOW (e.g. 60 for 'in 1 minute'). Preferred for relative requests when schedule_kind=at"
    schedule_every_seconds:
      type: integer
      description: "Repeat interval in seconds (minimum 60). Required when schedule_kind=every"
    schedule_cron:
      type: string
      description: "Cron expression (5 fields: minute hour day month weekday). Required when schedule_kind=cron"
    timezone:
      type: string
      description: "IANA timezone (default: Asia/Shanghai)"
    delete_after_run:
      type: boolean
      description: "Auto-delete after first successful run (default: false, set to true for one-time reminders)"
  required:
    - name
    - task_prompt
    - schedule_kind
is_read_only: false
is_concurrency_safe: false
---

# Schedule Task Tool

Create a scheduled task that runs automatically at specified times.

## Task Types

| task_type | Behavior | Requirements |
|-----------|----------|-------------|
| `dispatch` (default) | Dispatches `task_prompt` as Agent task via Persona | Requires active Persona context |
| `script` | Executes `task_prompt` as shell command (`sh -c`) | No Persona needed |
| `hypothesis` | Sends `task_prompt` as message to hypothesis agent session | Requires `hypothesis_agent_id`, agent must be running |

## When to Use

Use this tool when the user asks to:
- "Remind me every day at..."
- "Every morning/evening..."
- "At [specific time]..."
- "Every [interval]..."
- "Remind me on [date]..."
- "Run this script every hour..."
- "Check hypothesis progress every 30 minutes..."

## Time Conversion Guide

Convert natural language to schedule parameters:

| Natural Language | schedule_kind | Parameters |
|---|---|---|
| "Every day at 9am" | cron | schedule_cron: "0 9 * * *" |
| "Every Monday 10am" | cron | schedule_cron: "0 10 * * 1" |
| "Weekdays 6pm" | cron | schedule_cron: "0 18 * * 1-5" |
| "Every hour" | every | schedule_every_seconds: 3600 |
| "Every 30 minutes" | every | schedule_every_seconds: 1800 |
| "Tomorrow 3pm" | at | schedule_at: [computed ISO-8601] |
| "In 10 minutes" | at | schedule_in_seconds: 600 |

## ⚠️ IMPORTANT: Time Calculation

For relative requests ("in N minutes/seconds/hours"), prefer **`schedule_in_seconds`** so backend computes the absolute timestamp.
Use `schedule_at` only for explicit calendar requests ("tomorrow 3pm", specific dates/times).

**You MUST read the current time from the "Current Date & Time" section of the system prompt** when you need calendar/date arithmetic.
Do NOT guess or assume the current date/time.

**Common mistakes to avoid:**
- Using wrong year, month, or day
- Hallucinating a time instead of computing from the system-provided current time
- Forgetting to carry over when adding minutes crosses an hour/day boundary

## Workflow

1. If user gives a relative delay ("in 1 minute"), pass `schedule_in_seconds` (no date math needed)
2. For explicit calendar times, **read** current time from system prompt and **compute** `schedule_at`
3. **Verify** computed `schedule_at` is in the future and year/month/day are correct
4. Call this tool with the computed parameters
5. Confirm to user with: "Task '[name]' created. Next run: [time]."
