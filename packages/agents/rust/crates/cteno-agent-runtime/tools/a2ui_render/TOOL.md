---
id: "a2ui_render"
name: "A2UI Render"
description: "Send A2UI protocol messages to update the user-facing interface with declarative components"
category: "system"
version: "1.0.0"
supports_background: false
should_defer: true
search_hint: "UI render declarative components interface display"
input_schema:
  type: object
  required:
    - messages
  properties:
    messages:
      type: array
      description: |
        Array of A2UI protocol messages. Each message should contain exactly one of:
        - createSurface: Initialize a rendering surface
        - updateComponents: Add or update components on a surface
        - updateDataModel: Update the data model for data binding
        - deleteSurface: Remove a surface
      items:
        type: object
        properties:
          createSurface:
            type: object
            properties:
              surfaceId:
                type: string
                description: "Unique surface identifier (e.g. 'main')"
              catalogId:
                type: string
                description: "Component catalog ID (default: 'cteno/v1')"
          updateComponents:
            type: object
            properties:
              surfaceId:
                type: string
                description: "Target surface ID"
              components:
                type: array
                description: "Components to add or update (matched by id)"
                items:
                  type: object
                  required:
                    - id
                    - component
                  properties:
                    id:
                      type: string
                      description: "Unique component ID"
                    component:
                      type: string
                      description: "Component type from catalog (e.g. 'Text', 'Progress', 'Card')"
                    children:
                      type: array
                      description: "Child component IDs"
                      items:
                        type: string
          updateDataModel:
            type: object
            properties:
              surfaceId:
                type: string
                description: "Target surface ID"
              data:
                type: object
                description: "Data to merge into the surface data model"
          deleteSurface:
            type: object
            properties:
              surfaceId:
                type: string
                description: "Surface ID to delete"
is_read_only: false
is_concurrency_safe: false
---

# A2UI Render

Send declarative UI messages to update the user-facing interface. Components are rendered natively (not in a WebView).

## Quick Start

First call: create a surface and add components in one batch.

```json
{
  "messages": [
    {"createSurface": {"surfaceId": "main", "catalogId": "cteno/v1"}},
    {"updateComponents": {"surfaceId": "main", "components": [
      {"id": "status", "component": "StatusIndicator", "status": "active", "text": "运行中"},
      {"id": "title", "component": "Text", "text": "抖音涨粉看板", "variant": "heading"},
      {"id": "progress", "component": "Progress", "value": 0.52, "label": "总进度 52%"},
      {"id": "metrics", "component": "MetricsGrid", "metrics": {"粉丝数": 5172, "目标": 10000, "昨日播放": 8664}},
      {"id": "feed", "component": "ActivityFeed", "items": [
        {"text": "完成竞品分析", "timestamp": "10:30"},
        {"text": "发布观点视频", "timestamp": "14:15"}
      ]}
    ]}}
  ]
}
```

Subsequent calls: only send updateComponents with changed components.

```json
{
  "messages": [
    {"updateComponents": {"surfaceId": "main", "components": [
      {"id": "progress", "component": "Progress", "value": 0.65, "label": "总进度 65%"},
      {"id": "metrics", "component": "MetricsGrid", "metrics": {"粉丝数": 5230, "目标": 10000, "昨日播放": 9102}}
    ]}}
  ]
}
```

## Available Components (cteno/v1)

**Layout**: Container, Row, Column, Card, Divider
**Data Display**: Text, Progress, MetricCard, StatusIndicator, Badge
**Lists**: List, ListItem, ChecklistItem
**Interactive**: Button, ButtonGroup
**Media**: Image, Icon
**Composite**: MetricsGrid, ActivityFeed
