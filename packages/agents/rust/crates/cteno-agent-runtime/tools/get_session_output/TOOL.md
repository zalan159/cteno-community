---
id: "get_session_output"
name: "Get Session Output"
description: "Get the latest messages from a task session"
category: "persona"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    session_id:
      type: string
      description: "The task session ID to read output from"
    last_n:
      type: integer
      description: "Number of recent messages to retrieve (default: 5)"
  required:
    - session_id
is_read_only: true
is_concurrency_safe: true
---

# Get Session Output Tool

Read the latest messages from a task session to check its progress or results.

## When to Use

Use this tool to:
- Check if a dispatched task has completed
- Review the output and results of a task
- Decide whether to send follow-up instructions or close the session
