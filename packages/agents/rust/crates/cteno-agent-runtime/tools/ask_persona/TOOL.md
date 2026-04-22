---
id: "ask_persona"
name: "Ask Persona"
description: "Ask the persona that dispatched this task for clarification or guidance"
category: "persona"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    question:
      type: string
      description: "The question or request for the dispatching persona"
  required:
    - question
is_read_only: false
is_concurrency_safe: false
---

# Ask Persona Tool

Ask the persona that dispatched this task for clarification, guidance, or additional information.

## When to Use

Use this tool when you need:
- Clarification on task requirements
- A decision that requires the persona's judgment
- Additional context not provided in the original task

## Note

This tool only works in task sessions that were dispatched by a persona.
The persona will receive your question and can respond via `send_to_session`.
