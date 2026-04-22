---
id: "computer_use"
name: "Computer Use"
description: "Control the desktop by taking screenshots and simulating mouse/keyboard input"
category: "system"
version: "1.0.0"
supports_background: false
should_defer: true
search_hint: "desktop control mouse keyboard screenshot input"
input_schema:
  type: object
  properties:
    action:
      type: string
      description: "The action to perform"
      enum:
        - screenshot
        - click
        - double_click
        - right_click
        - type
        - keypress
        - scroll
        - drag
        - move
        - cursor_position
    x:
      type: integer
      description: "X coordinate (for click, double_click, right_click, scroll, move)"
    y:
      type: integer
      description: "Y coordinate (for click, double_click, right_click, scroll, move)"
    text:
      type: string
      description: "Text to type (for type action)"
    keys:
      type: array
      items:
        type: string
      description: "Keys to press (for keypress action), e.g. ['ctrl', 'c'] or ['enter']"
    scroll_x:
      type: integer
      description: "Horizontal scroll amount (for scroll action, positive = right)"
    scroll_y:
      type: integer
      description: "Vertical scroll amount (for scroll action, positive = down)"
    start_x:
      type: integer
      description: "Drag start X coordinate (for drag action)"
    start_y:
      type: integer
      description: "Drag start Y coordinate (for drag action)"
    end_x:
      type: integer
      description: "Drag end X coordinate (for drag action)"
    end_y:
      type: integer
      description: "Drag end Y coordinate (for drag action)"
  required:
    - action
is_read_only: false
is_concurrency_safe: false
---

# Computer Use Tool

Control the desktop through screenshots and simulated mouse/keyboard input. This enables visual UI interaction — you see the screen, decide what to do, and issue actions.

## Workflow

1. Take a `screenshot` to see the current screen state.
2. Analyze the screenshot to identify UI elements and their positions.
3. Perform actions (click, type, scroll, etc.) based on coordinates.
4. Take another `screenshot` to verify the result.
5. Repeat until the task is complete.

## Actions

### screenshot
Capture the full screen. Returns a base64-encoded PNG image. Always start with this to understand the current state.

### click / double_click / right_click
Click at screen coordinates. Requires `x` and `y`.

### type
Type text at the current cursor position. Requires `text`. Click on the target input field first.

### keypress
Press key combinations. Requires `keys` array. Examples:
- `["enter"]` — press Enter
- `["ctrl", "c"]` — Ctrl+C (copy on Windows/Linux)
- `["ctrl", "v"]` — Ctrl+V (paste on Windows/Linux)
- `["meta", "c"]` — Cmd+C (copy on macOS)
- `["meta", "v"]` — Cmd+V (paste on macOS)
- `["alt", "tab"]` — Alt+Tab (switch app on Windows/Linux)
- `["meta", "tab"]` — Cmd+Tab (switch app on macOS)
- `["meta", "space"]` — Spotlight search (macOS) / Start menu (Windows)
- `["ctrl", "a"]` — Select all

### scroll
Scroll at a position. Requires `x`, `y`, and at least one of `scroll_x`/`scroll_y`.

### drag
Drag from one point to another. Requires `start_x`, `start_y`, `end_x`, `end_y`.

### move
Move the cursor without clicking. Requires `x` and `y`.

### cursor_position
Get the current cursor position. No parameters needed.

## Important Notes

- Coordinates are in screen pixels (absolute, not relative to any window).

- On macOS, use `meta` (Cmd) for most shortcuts (copy, paste, etc.).
- On Windows/Linux, use `ctrl` for most shortcuts (copy, paste, etc.).
- `meta` maps to Cmd on macOS, Win key on Windows, Super on Linux.
- After clicking a text field, wait briefly before typing.
- For complex workflows, take screenshots frequently to verify state.
