---
id: "upload_artifact"
name: "Upload Artifact"
description: "Upload a local file (agent work product) to the server-managed object storage and return a file reference for the user to download/preview on other devices."
category: "system"
version: "1.0.0"
supports_background: true
should_defer: true
search_hint: "upload file artifact object storage download"
input_schema:
  type: object
  properties:
    path:
      type: string
      description: "Local file path to upload (supports ~ expansion; relative paths are resolved against workdir)."
    filename:
      type: string
      description: "Optional display filename override (defaults to basename(path))."
    mime:
      type: string
      description: "Optional MIME type (defaults to application/octet-stream)."
    ttl_days:
      type: number
      description: "Retention in days on object storage: 7 or 30 (default: 7)."
    background:
      type: boolean
      description: "If true, upload in the background and return a run_id (default: true)."
    notify:
      type: boolean
      description: "Only when background=true: notify the agent when upload finishes (default: true)."
    hard_timeout_secs:
      type: number
      description: "Only when background=true: stop the upload after N seconds (0 = no hard timeout, default: 0)."
  required:
    - path
is_read_only: false
is_concurrency_safe: false
---

# Upload Artifact Tool

This tool uploads a local file produced by the agent (reports, PDFs, images, etc.) so the user can access it from other devices.

## Behavior

- Default behavior is `background: true` because uploads can be large (up to ~2GB).
- When running in background, the tool returns a `run_id` immediately.
- When the background upload finishes, the agent will receive a system message whose log tail includes one of:
  - `[artifact-upload-complete] file_id=... url=...`
  - `[artifact-upload-failed] file_id=... reason=...`

## Agent Responsibility

**CRITICAL**: Do NOT send a download link until you receive the background task completion notification.
- The tool returns a `run_id` immediately — this is NOT the file_id. Never use it in download links.
- Wait for the notification, then read the `file_id` and `filename` values from its header.

When you receive the completion notification, you MUST:

**REQUIRED ACTIONS**:
1. Read the `file_id` and `filename` values from the notification header (they are clearly labeled)
2. Respond with: `✅ [message] [filename](cteno-file://<file_id>)` — replace `<file_id>` with the ACTUAL value from the notification
3. Do NOT copy any example values from these instructions — use ONLY the real values from the notification

If you receive `[artifact-upload-failed]`, inform the user and suggest the next step (e.g. try again, reduce file size, check login/binding).

## Safety

- Only upload files that are directly related to the user's request/work product.
- Do NOT upload secrets (keys, tokens), system configuration, or unrelated personal files.

