---
id: "shell"
name: "Shell Command"
description: "Execute shell commands (PowerShell on Windows, bash/zsh on macOS/Linux) for file operations, system queries, and process management"
category: "system"
version: "1.0.0"
supports_background: true
input_schema:
  type: object
  properties:
    command:
      type: string
      description: "The shell command to execute"
    workdir:
      type: string
      description: "Working directory (default: ~)"
    timeout:
      type: number
      description: "Timeout in seconds. REQUIRED when background is false/unset. Must be a positive integer."
    background:
      type: boolean
      description: "If true, run as a background run and return a run_id"
    wait_timeout_secs:
      type: number
      description: "Only when background=true: wait up to N seconds for completion before returning run_id (default: 0)"
    hard_timeout_secs:
      type: number
      description: "Only when background=true: stop the run after N seconds (0 = no hard timeout, default: 0)"
    notify:
      type: boolean
      description: "Only when background=true: notify the agent when the run finishes (default: true)"
  required:
    - command
    - timeout
is_read_only: false
is_concurrency_safe: false
---

# Shell Command Tool

Execute shell commands with safety checks and timeout controls.

## IMPORTANT: 避免使用 shell 执行以下操作

当有专用工具可用时，禁止使用 shell 执行这些操作。使用专用工具可以让用户更好地理解和审查你的工作：

- **文件搜索**: 使用 **glob** 工具 (禁止 find 或 ls)
- **内容搜索**: 使用 **grep** 工具 (禁止 grep 或 rg)
- **读取文件**: 使用 **read** 工具 (禁止 cat/head/tail)
- **编辑文件**: 使用 **edit** 工具 (禁止 sed/awk)
- **创建文件**: 使用 **write** 工具 (禁止 echo >/cat <<EOF)

shell 工具仅用于系统命令和需要 shell 执行的终端操作。如果不确定，默认使用专用工具。

## Usage

This tool allows you to run any shell command on the user's system. Always:
- Verify the command is safe before execution
- Use appropriate timeouts
- Handle errors gracefully

## Parameters

- `command` (string, required): The shell command to execute
- `workdir` (string, optional): Working directory (default: ~)
- `timeout` (number, **required** when not background): Timeout in seconds. Prevents long-running commands from blocking the conversation.
- `background` (boolean, optional): If true, run in background and return a `run_id`
- `wait_timeout_secs` (number, optional): Only when `background: true`. Wait N seconds for completion; if still running, return `run_id`.
- `hard_timeout_secs` (number, optional): Only when `background: true`. Stop after N seconds. Use `0` for infinite (will still be stopped when session is archived or app closes).
- `notify` (boolean, optional): Only when `background: true`. If true, the agent will be notified when the run finishes.

## Examples

Use this tool to execute shell commands. For example:

**macOS/Linux:**
- List files: `ls -la ~/Downloads`
- Find recent PDFs: `find ~/Documents -name '*.pdf' -mtime -7`
- Show largest files: `du -sh ~/Downloads/* | sort -hr | head -10`

**Windows (PowerShell):**
- List files: `Get-ChildItem ~/Downloads`
- Find recent PDFs: `Get-ChildItem ~/Documents -Recurse -Filter '*.pdf' | Where-Object {$_.LastWriteTime -gt (Get-Date).AddDays(-7)}`
- Show largest files: `Get-ChildItem ~/Downloads | Sort-Object Length -Descending | Select-Object -First 10 Name, @{N='Size';E={'{0:N2} MB' -f ($_.Length/1MB)}}`
- Start a dev server in background: `npm run dev` with `background: true`, and `hard_timeout_secs: 0`
- Check logs: use `run_manager` with `op: "logs"`

## Safety Restrictions

Dangerous commands are blocked:
- `rm -rf /` - System-wide deletion
- `curl|sh` - Pipe execution
- Other high-risk operations

The system enforces a blacklist of dangerous command patterns.
