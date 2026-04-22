---
id: "send_to_session"
name: "Send to Session"
description: "Send a message to a running task session"
category: "persona"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    session_id:
      type: string
      description: "The task session ID to send message to"
    message:
      type: string
      description: "The instruction or follow-up message"
  required:
    - session_id
    - message
is_read_only: false
is_concurrency_safe: false
---

# Send to Session Tool

Send a follow-up message or additional instructions to a running task session.

## When to Use

Use this tool to:
- Provide additional context or corrections to an in-progress task
- Ask the worker agent to adjust its approach
- Request specific modifications to partial results
