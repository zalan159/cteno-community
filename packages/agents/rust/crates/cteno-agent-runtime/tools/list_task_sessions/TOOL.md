---
id: "list_task_sessions"
name: "List Task Sessions"
description: "List all task sessions dispatched by this persona"
category: "persona"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties: {}
  required: []
is_read_only: true
is_concurrency_safe: true
---

# List Task Sessions Tool

List all task sessions that this persona has dispatched.

## When to Use

Use this tool to:
- Check the status of dispatched tasks
- Find session IDs for `get_session_output` or `send_to_session`
- Get an overview of active and completed tasks
