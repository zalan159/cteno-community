---
id: "close_task_session"
name: "Close Task Session"
description: "Close a completed task session after reviewing its output"
category: "persona"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    session_id:
      type: string
      description: "The task session ID to close"
  required:
    - session_id
is_read_only: false
is_concurrency_safe: false
---

# Close Task Session Tool

Close a task session after you have reviewed its output and are satisfied with the results.

## When to Use

Use this tool when:
- A task has completed successfully
- You have reviewed the output with `get_session_output`
- You want to free up resources

## Important

Always save lessons learned via `memory_save` before closing a task session.
