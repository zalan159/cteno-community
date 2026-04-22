---
id: "tool_search"
name: "Tool Search"
description: "Fetches full schema definitions for deferred tools so they can be called."
category: "system"
version: "1.0.0"
should_defer: false
input_schema:
  type: object
  properties:
    query:
      type: string
      description: 'Query to find deferred tools. Use "select:<tool_name>" for direct selection, or keywords to search.'
    max_results:
      type: number
      description: "Maximum number of results to return (default: 5)"
  required:
    - query
is_read_only: true
is_concurrency_safe: true
---

# Tool Search

Fetches full schema definitions for deferred tools so they can be called.

Deferred tools appear by name in the system prompt's deferred tools list. Until fetched, only the name and a short description are known -- there is no parameter schema, so the tool cannot be invoked. This tool takes a query, matches it against the deferred tool list, and returns the matched tools' complete definitions inside a <functions> block.

Once a tool's schema appears in the result, it is callable exactly like any tool defined at the start of the conversation.

## Query forms

- `select:browser_cdp,image_generation` -- fetch these exact tools by name (comma-separated)
- `browser automation` -- keyword search, up to max_results best matches
- `mcp slack` -- search for MCP tools by server name or action
