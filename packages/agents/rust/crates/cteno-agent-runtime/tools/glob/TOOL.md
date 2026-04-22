---
id: "glob"
name: "Glob"
description: "Fast file pattern matching tool that works with any codebase size"
category: "system"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    pattern:
      type: string
      description: "The glob pattern to match files against (e.g. \"**/*.rs\", \"src/**/*.ts\")"
    path:
      type: string
      description: "The directory to search in. If not specified, the working directory is used. Must be a valid directory path if provided."
  required:
    - pattern
is_read_only: true
is_concurrency_safe: true
---

# Glob Tool

Fast file pattern matching tool that works with any codebase size.

## When to Use

- Supports glob patterns like `**/*.rs` or `src/**/*.ts`
- Returns matching file paths sorted by modification time (newest first)
- Use this tool when you need to find files by name patterns
- **Do NOT use shell commands like `find` or `ls` to search for files — use this tool instead**

## Parameters

- `pattern` (string, required): The glob pattern to match files against
- `path` (string, optional): The directory to search in. Defaults to the working directory.

## Glob Pattern Syntax

| Pattern | Matches |
|---------|---------|
| `*` | Any sequence of characters in a file name |
| `**` | Any sequence of directories |
| `?` | Any single character |
| `[abc]` | Any character in the set |
| `[!abc]` | Any character NOT in the set |

## Examples

**Find all Rust files:**
```json
{ "pattern": "**/*.rs" }
```

**Find all TypeScript files in src:**
```json
{ "pattern": "src/**/*.ts" }
```

**Find all test files:**
```json
{ "pattern": "**/*_test.*" }
```

**Find files in a specific directory:**
```json
{ "pattern": "*.md", "path": "/Users/user/project/docs" }
```

## Output

Returns one file path per line, sorted by modification time (newest first). Paths are relative to the search directory.

If more than 100 files match, results are truncated and a note is appended:
```
[Truncated: showing 100 of 523 files. Use a more specific pattern to narrow results.]
```

## Excluded Directories

The following directories are automatically excluded from search:
- `.git`
- `node_modules`
- `target`
- `.next`
- `dist`
- `build`
- `.cache`
- `__pycache__`
