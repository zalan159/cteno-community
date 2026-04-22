---
id: "delete_scheduled_task"
name: "Delete Scheduled Task"
description: "Delete a scheduled task by ID or name"
category: "system"
version: "1.0.0"
supports_background: false
should_defer: true
search_hint: "delete remove scheduled task cancel"
input_schema:
  type: object
  properties:
    task_id:
      type: string
      description: "Task ID to delete"
  required:
    - task_id
is_read_only: false
is_concurrency_safe: false
---

# Delete Scheduled Task Tool

Delete a scheduled task permanently.

## When to Use

User asks to "cancel", "remove", "delete", "stop" a scheduled task.

## Workflow

1. If user doesn't know the task ID, first call `list_scheduled_tasks` to find it
2. Call this tool with the task_id
3. Confirm: "Task '[name]' deleted."
