---
id: "update_plan"
name: "Update Plan"
description: "Create or update a step-by-step plan for the current task."
category: "system"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    todos:
      type: array
      description: "The full plan as an ordered list of steps."
      items:
        type: object
        properties:
          content:
            type: string
            description: "A concise step description (5-10 words)."
          status:
            type: string
            enum: ["pending", "in_progress", "completed"]
        required: [content, status]
    explanation:
      type: string
      description: "Optional explanation of why the plan was created or changed."
  required: [todos]
is_read_only: false
is_concurrency_safe: false
---

# Update Plan Tool

Create or update a structured step-by-step plan for the current task.

## Rules

1. **Exactly one step** should have `status: "in_progress"` at any time — the step you are about to work on.
2. **Send the full array** every time — not just the changed items.
3. After finishing the last step, mark all as `"completed"`.
4. You may add, remove, or reorder steps as the task evolves.
5. **Do not** repeat plan contents in your text response — the plan is rendered separately by the UI.

## When to Use

- Tasks with 3+ distinct steps
- Multi-file changes or multi-stage workflows
- Tasks with dependencies between steps

## When NOT to Use

- Simple questions or one-step tasks
- Quick lookups or single edits
