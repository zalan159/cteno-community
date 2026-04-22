---
id: "browser_network"
name: "Browser Network"
description: "Monitor all network requests in the browser via CDP Network domain. Captures fetch, XHR, WebSocket, and all other HTTP requests including those from WebWorkers."
category: "system"
version: "2.0.0"
supports_background: false
should_defer: true
search_hint: "browser network monitor CDP requests XHR fetch"
input_schema:
  type: object
  properties:
    action:
      type: string
      enum:
        - start_capture
        - stop_capture
        - get_requests
        - clear
      description: "The network monitoring action to perform"
    filter:
      type: string
      description: "URL pattern to filter captured requests (substring match, case-insensitive)"
    method_filter:
      type: string
      description: "HTTP method filter: GET, POST, PUT, DELETE, PATCH, or ALL (default: ALL)"
    max_requests:
      type: integer
      description: "Maximum number of requests to return (default: 200)"
  required:
    - action
is_read_only: true
is_concurrency_safe: false
---

# Browser Network

Monitor all network requests made by the browser using CDP Network domain. Unlike JS-based capture, this catches everything — including requests from WebWorkers, Service Workers, and pre-initialized fetch/XHR instances.

## Actions

### start_capture
Start capturing network requests via CDP.
```json
{"action": "start_capture"}
{"action": "start_capture", "filter": "api.example.com", "method_filter": "POST"}
```

### get_requests
Get captured requests (with optional filtering).
```json
{"action": "get_requests"}
{"action": "get_requests", "filter": "/api/", "method_filter": "GET"}
```

### clear
Clear all captured requests without stopping capture.

### stop_capture
Stop capturing and disable network monitoring.

## Notes

- Uses CDP Network.enable — survives page navigation (unlike JS injection)
- Captures request bodies (POST data) automatically
- Captures all request types including WebWorker requests
- Must call start_capture before other actions
