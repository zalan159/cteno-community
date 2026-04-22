---
id: "list_scheduled_tasks"
name: "List Scheduled Tasks"
description: "List all scheduled tasks with their status and next run times"
category: "system"
version: "1.0.0"
supports_background: false
should_defer: true
search_hint: "list scheduled tasks timer status"
input_schema:
  type: object
  properties:
    enabled_only:
      type: boolean
      description: "Only show enabled tasks (default: false, shows all)"
  required: []
is_read_only: true
is_concurrency_safe: true
---

# List Scheduled Tasks Tool

List all scheduled tasks created for this session.

## When to Use

Use when user asks:
- "What scheduled tasks do I have?"
- "Show me my reminders"
- "What's running automatically?"

## Return Value

Returns a JSON array of tasks with id, name, schedule, next_run_at, last_status, enabled.
