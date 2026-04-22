---
id: "read"
name: "Read File"
description: "Read file contents with pagination, BOM detection, and multi-format support"
category: "system"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    file_path:
      type: string
      description: "Path to the file to read (absolute or relative)"
    workdir:
      type: string
      description: "Working directory used to resolve relative file_path (default: ~)"
    offset:
      type: number
      description: "Line number to start reading from (0-based, optional)"
    limit:
      type: number
      description: "Maximum number of lines to read (optional, default: 2000)"
  required:
    - file_path
is_read_only: true
is_concurrency_safe: true
---

Reads a file from the local filesystem. You can access any file directly by using this tool.
Assume this tool is able to read all files on the machine. If the User provides a path to a file assume that path is valid. It is okay to read a file that does not exist; an error will be returned.

Usage:
- The file_path parameter can be an absolute path, a relative path (resolved against workdir), or use ~ expansion
- By default, it reads up to 2000 lines starting from the beginning of the file
- You can optionally specify offset and limit to read a specific range (especially useful for large files)
- When you already know which part of the file you need, only read that part
- Results are returned using cat -n format, with line numbers starting at 1
- This tool can read images (eg PNG, JPG, GIF, WEBP, BMP). When reading an image file the contents are presented visually as the LLM is multimodal.
- This tool can only read files, not directories. To read a directory, use the shell tool with an ls command.
- If you read a file that exists but has empty contents you will receive a system reminder warning in place of file contents.
