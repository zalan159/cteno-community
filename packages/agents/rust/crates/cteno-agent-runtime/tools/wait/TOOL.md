---
id: "wait"
name: "Wait"
description: "Wait for a specified duration, returning early if a new message arrives"
category: "system"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  required: []
  properties:
    seconds:
      type: integer
      description: "How long to wait. Default: 30."
    reason:
      type: string
      description: "Why you are waiting (for logging)."
is_read_only: true
is_concurrency_safe: true
---

# Wait

Blocks execution for up to N seconds. Returns early if a new message (e.g. `[Task Complete]`) arrives in the queue.

## Return values

- `"status": "message_arrived"` — a message arrived during the wait; it will be delivered automatically
- `"status": "timeout"` — no messages arrived; report progress to user then stop output

## When to use

- After dispatching a task, when you want to wait for the result before proceeding

## When NOT to use

- Do NOT call wait repeatedly in a loop
- If timeout, report progress and stop — don't wait again
