---
id: "browser_cdp"
name: "Browser CDP"
description: "Send raw Chrome DevTools Protocol commands to the browser. Use for file upload (DOM.setFileInputFiles), key events (Input.dispatchKeyEvent), rich text input (Input.insertText), permissions (Browser.grantPermissions), and any CDP operation not covered by other browser tools."
category: "system"
version: "1.0.0"
supports_background: false
should_defer: true
search_hint: "chrome devtools protocol raw CDP command"
input_schema:
  type: object
  properties:
    method:
      type: string
      description: "CDP method name (e.g. DOM.setFileInputFiles, Input.dispatchKeyEvent, Accessibility.getFullAXTree, Target.createTarget)"
    params:
      type: object
      description: "CDP method parameters as JSON object. See https://chromedevtools.github.io/devtools-protocol/ for reference."
    timeout:
      type: integer
      description: "Timeout in seconds (default: 30)"
  required:
    - method
is_read_only: true
is_concurrency_safe: false
---

# Browser CDP

Send raw Chrome DevTools Protocol (CDP) commands directly to the browser. This is the most powerful and flexible browser tool — it can do anything CDP supports.

## Common Commands

### DOM Operations
```json
{"method": "DOM.getDocument"}
{"method": "DOM.querySelectorAll", "params": {"nodeId": 1, "selector": "input[type=file]"}}
{"method": "DOM.setFileInputFiles", "params": {"files": ["/path/to/file"], "nodeId": 123}}
```

### Input
```json
{"method": "Input.dispatchKeyEvent", "params": {"type": "keyDown", "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13}}
{"method": "Input.insertText", "params": {"text": "Hello world"}}
{"method": "Input.dispatchMouseEvent", "params": {"type": "mousePressed", "x": 100, "y": 200, "button": "left", "clickCount": 1}}
```

### Page
```json
{"method": "Page.captureScreenshot", "params": {"format": "png"}}
{"method": "Page.navigate", "params": {"url": "https://example.com"}}
{"method": "Page.printToPDF"}
```

### Accessibility
```json
{"method": "Accessibility.getFullAXTree", "params": {"depth": -1}}
```

### Target (Tab Management)
```json
{"method": "Target.getTargets"}
{"method": "Target.createTarget", "params": {"url": "https://example.com"}}
{"method": "Target.closeTarget", "params": {"targetId": "..."}}
```

### Permissions
```json
{"method": "Browser.grantPermissions", "params": {"permissions": ["geolocation", "notifications"]}}
```

## Notes

- Requires an active browser session (call browser_navigate first)
- Returns the raw CDP JSON response
- For network monitoring, use browser_network instead (it manages CDP Network events automatically)
- CDP protocol reference: https://chromedevtools.github.io/devtools-protocol/
