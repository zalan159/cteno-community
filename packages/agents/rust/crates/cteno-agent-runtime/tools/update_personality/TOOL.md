---
id: "update_personality"
name: "Update Personality"
description: "Update your own personality notes based on user interactions"
category: "persona"
version: "1.0.0"
supports_background: false
should_defer: true
search_hint: "update personality notes persona character"
input_schema:
  type: object
  properties:
    notes:
      type: string
      description: "Updated personality notes (replaces existing notes entirely)"
  required:
    - notes
is_read_only: false
is_concurrency_safe: false
---

# Update Personality Tool

Update your personality notes based on user feedback, preferences, and interaction patterns.

## When to Use

Use this tool when:
- The user shares preferences about communication style
- You learn important traits to maintain consistency
- The user explicitly asks you to remember behavioral preferences

## Format

Write personality notes as concise bullet points:
- Communication style preferences
- Domain expertise areas
- Decision-making principles
- Response format preferences
