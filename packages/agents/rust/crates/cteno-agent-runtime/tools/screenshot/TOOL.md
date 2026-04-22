---
id: "screenshot"
name: "Screenshot"
description: "Capture a screenshot of the current desktop screen"
category: "system"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties: {}
is_read_only: true
is_concurrency_safe: true
---

# Screenshot Tool

Capture the full desktop screen and return the image.

## Important

- The screenshot is **automatically displayed** to the user in the chat UI. Do NOT repeat the image URL or download link in your text response — it creates a duplicate display.
- If you can see the image (vision-capable model), describe what you observe.
- If you cannot see the image, simply confirm the screenshot was taken and let the user view it in the displayed result.
