---
id: "grep"
name: "Grep"
description: "Search file contents using ripgrep — supports regex, glob/type filters, multiline, and pagination"
category: "system"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    pattern:
      type: string
      description: "The regular expression pattern to search for in file contents"
    path:
      type: string
      description: "File or directory to search in. Defaults to workdir."
    glob:
      type: string
      description: "Glob pattern to filter files (e.g. \"*.rs\", \"**/*.ts\", \"*.{ts,tsx}\") — maps to rg --glob"
    type:
      type: string
      description: "File type to search (rg --type). Common types: js, py, rust, go, java, ts, css, html, md. More efficient than glob for standard file types."
    output_mode:
      type: string
      description: "Output mode: \"content\" shows matching lines with context, \"files_with_matches\" shows only file paths (default), \"count\" shows match counts per file."
      enum:
        - content
        - files_with_matches
        - count
    context_before:
      type: integer
      description: "Number of lines to show before each match (rg -B). Only effective in content mode."
    context_after:
      type: integer
      description: "Number of lines to show after each match (rg -A). Only effective in content mode."
    context:
      type: integer
      description: "Number of lines to show before and after each match (rg -C). Overrides context_before/context_after. Only effective in content mode."
    case_insensitive:
      type: boolean
      description: "Case insensitive search (rg -i). Default: false."
    head_limit:
      type: integer
      description: "Limit output to first N lines/entries. Works across all output modes. Defaults to 250. Pass 0 for unlimited (use sparingly)."
    offset:
      type: integer
      description: "Skip first N lines/entries before applying head_limit. Defaults to 0."
    multiline:
      type: boolean
      description: "Enable multiline mode where . matches newlines and patterns can span lines (rg -U --multiline-dotall). Default: false."
    line_numbers:
      type: boolean
      description: "Show line numbers in output (rg -n). Only effective in content mode. Default: true."
  required:
    - pattern
is_read_only: true
is_concurrency_safe: true
---

# Grep Tool

A powerful search tool built on ripgrep (rg).

## Usage

- ALWAYS use the Grep tool for content search tasks. NEVER invoke `grep` or `rg` via the shell tool. The Grep tool handles path resolution, output pagination, and safe defaults.
- Supports full regex syntax (e.g., `log.*Error`, `function\s+\w+`)
- Filter files with `glob` parameter (e.g., `"*.js"`, `"**/*.tsx"`) or `type` parameter (e.g., `"js"`, `"py"`, `"rust"`)
- Output modes: `"content"` shows matching lines, `"files_with_matches"` shows only file paths (default), `"count"` shows match counts
- Pattern syntax: Uses ripgrep (not grep) — literal braces need escaping (use `interface\{\}` to find `interface{}` in Go code)
- Multiline matching: By default patterns match within single lines only. For cross-line patterns like `struct \{[\s\S]*?field`, use `multiline: true`
- `head_limit` defaults to 250 to prevent context explosion. Pass `0` explicitly for unlimited (use sparingly — large result sets waste context).
- Use `offset` together with `head_limit` for pagination through large result sets.

## Examples

- Search for a function definition: `pattern: "fn resolve_workdir"`, `type: "rust"`
- Find all TODO comments in TypeScript: `pattern: "TODO|FIXME"`, `glob: "*.ts"`
- Count occurrences: `pattern: "import React"`, `output_mode: "count"`, `type: "tsx"`
- Context around matches: `pattern: "panic!"`, `output_mode: "content"`, `context: 3`
- Multiline struct search: `pattern: "struct Config \\{[\\s\\S]*?\\}"`, `multiline: true`, `output_mode: "content"`
