---
id: "browser_manage"
name: "Browser Manage"
description: "Manage browser tabs and lifecycle: list tabs, switch tabs, open new tab, close tab, or close the browser entirely"
category: "system"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    action:
      type: string
      enum:
        - list_tabs
        - switch_tab
        - new_tab
        - close_tab
        - close_browser
      description: "The management action to perform"
    tab_index:
      type: integer
      description: "Tab index from list_tabs output (for switch_tab/close_tab)"
    url:
      type: string
      description: "URL for new_tab, or URL substring to match a tab (for switch_tab/close_tab)"
    text:
      type: string
      description: "Title substring to match a tab (for switch_tab/close_tab)"
    target_id:
      type: string
      description: "Raw CDP target ID (advanced, prefer tab_index or url)"
  required:
    - action
is_read_only: false
is_concurrency_safe: false
---

# Browser Manage

Manage browser tabs and the browser lifecycle. Supports cross-tab operations — list all tabs, switch between them by index or URL/title match.

## Actions

### list_tabs
List all open tabs with index, title, URL, and active marker.
```json
{"action": "list_tabs"}
```

### switch_tab
Switch to a different tab. Identify the tab by any of:
- `tab_index` — index from list_tabs output (simplest)
- `url` — URL substring match
- `text` — title substring match
- `target_id` — raw CDP target ID
```json
{"action": "switch_tab", "tab_index": 2}
{"action": "switch_tab", "url": "github.com"}
{"action": "switch_tab", "text": "Gmail"}
```

### new_tab
Open a new tab with an optional URL.
```json
{"action": "new_tab", "url": "https://example.com"}
```

### close_tab
Close a tab. Same identification methods as switch_tab.
```json
{"action": "close_tab", "tab_index": 1}
```

### close_browser
Close the browser entirely, killing the Chrome process and cleaning up the temp profile.
```json
{"action": "close_browser"}
```
