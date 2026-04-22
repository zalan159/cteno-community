---
id: "browser_action"
name: "Browser Action"
description: "Perform actions on the browser page: click elements, type text, scroll, evaluate JavaScript, or take screenshots"
category: "system"
version: "2.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    action:
      type: string
      enum:
        - click
        - type
        - scroll
        - evaluate
        - screenshot
      description: "The action to perform"
    element_index:
      type: integer
      description: "Element index from previous state output (for click/type)"
    selector:
      type: string
      description: "CSS selector (for click/type)"
    text:
      type: string
      description: "Text to type (for type action) or JS expression (for evaluate)"
    scroll_y:
      type: integer
      description: "Pixels to scroll vertically (positive=down, negative=up). Default: 500"
    full_page:
      type: boolean
      description: "Capture full page screenshot (for screenshot action). Default: false"
  required:
    - action
is_read_only: true
is_concurrency_safe: false
---

# Browser Action

Core browser interaction: click, type, evaluate JS, scroll, screenshot.

For other operations use `browser_cdp`:
- File upload: `DOM.setFileInputFiles`
- Key press: `Input.dispatchKeyEvent`
- Rich text: `Input.insertText`
- Select dropdown: evaluate JS to find option + click

## Actions

### click
Click an element by `element_index` or `selector`.

### type
Type text into an input element. Uses React-compatible nativeInputValueSetter.

### evaluate
Execute JavaScript and return the result. The most powerful action — can do anything JS can do.

### scroll
Scroll the page vertically. `scroll_y` in pixels (default: 500, negative for up).

### screenshot
Capture a screenshot of the current page. Returns image data for vision models.
