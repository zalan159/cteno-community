---
id: "write"
name: "File Write"
description: "Create or overwrite a file with the given content. Automatically creates parent directories if they don't exist."
category: "system"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    path:
      type: string
      description: "File path to write (supports ~ expansion)"
    workdir:
      type: string
      description: "Working directory used to resolve relative path (default: ~)"
    content:
      type: string
      description: "The full content to write to the file"
  required:
    - path
    - content
is_read_only: false
is_concurrency_safe: false
---

# File Write Tool

Create a new file or overwrite an existing file with the specified content.

## When to Use

- **Creating new files** that don't exist yet
- **Overwriting entire files** when the content changes significantly
- For small, targeted edits to existing files, prefer the `edit` tool instead

## Important: Read Before Write

If the target file already exists, you must read it first using the `read` tool before writing. If the file has been modified since your last read (e.g., by a linter, formatter, or the user), the write will be rejected with a `FILE_MODIFIED_SINCE_READ` error. In that case, read the file again to get the latest content, then retry the write.

## Features

- Automatically creates parent directories if they don't exist
- Supports `~` expansion for home directory paths
- Reports whether a file was created or overwritten
- Staleness detection prevents accidental overwrites of externally modified files

## Examples

```javascript
// Create a new file
write({
  path: "~/project/src/utils.ts",
  content: "export function add(a: number, b: number): number {\n  return a + b;\n}\n"
})

// Create file in new directory (auto-creates directories)
write({
  path: "~/project/src/new_module/index.ts",
  content: "export * from './utils';\n"
})
```
