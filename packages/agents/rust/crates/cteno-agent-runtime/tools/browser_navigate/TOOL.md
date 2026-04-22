---
id: "browser_navigate"
name: "Browser Navigate"
description: "Open a URL in a browser with CDP debugging. Launches Chrome if not already running, copying the user's profile for login state."
category: "system"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    url:
      type: string
      description: "The URL to navigate to"
    headless:
      type: boolean
      description: "Run in headless mode (default: false)"
    wait_seconds:
      type: number
      description: "Seconds to wait after navigation for page load (default: 3)"
  required:
    - url
is_read_only: true
is_concurrency_safe: false
---

# Browser Navigate

Open a URL in a Chrome browser controlled via CDP. On first call, launches Chrome with a copy of the user's profile (preserving login state).

## When to Use

- Opening any web page for interaction or reading
- Starting a browser automation session
- Navigating to a new URL within an existing session

## Parameters

- `url` (string, required): The URL to navigate to
- `headless` (boolean, optional): Run headless, no visible window (default: false)
- `wait_seconds` (number, optional): Wait time after navigation (default: 3)

## Returns

URL and title of the loaded page, plus a brief AX tree summary.

## Notes

- Chrome profile is copied to a temp directory — original profile is never modified
- First call may take a few seconds to launch Chrome
- Subsequent calls in the same session reuse the existing Chrome instance

## Important: Clean Up When Done

When you have finished all browser tasks, **always** call `browser_manage` with `{"action": "close_browser"}` to close the browser and clean up the Chrome process. Each session launches its own Chrome instance — failing to close it wastes system resources.
