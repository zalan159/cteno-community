---
id: a2ui
name: "A2UI Interface Builder"
version: "1.0.0"
description: "Guide for composing A2UI declarative UI components to build user-facing dashboards and interfaces"
when_to_use: "需要向用户展示结构化 UI 组件（表单、卡片、列表等）时使用"
tags:
  - ui
  - rendering
  - dashboard
  - a2ui
---

# A2UI Interface Builder

A2UI 是声明式 UI 协议（基于 Google A2A 规范 v0.9），Agent 通过 `a2ui_render` 工具发送 JSON 消息，前端原生渲染组件（非 WebView）。

## 协议概述

- Agent 调用 `a2ui_render` 工具，传入 JSON messages 数组
- 前端接收后原生渲染为 React Native 组件
- 所有组件以扁平数组传输，通过 `children` 引用子组件 ID 构建树形结构
- 支持增量更新：只需发送变化的组件，ID 匹配则替换，新 ID 则追加

---

## 消息类型

A2UI 协议定义了四种消息类型，每次调用 `a2ui_render` 时传入 `messages` 数组：

### createSurface — 创建渲染面

初始化一个渲染面，必须在发送组件之前调用。

```json
{"createSurface": {"surfaceId": "main", "catalogId": "cteno/v1"}}
```

| 字段 | 说明 |
|------|------|
| `surfaceId` | 渲染面唯一标识，后续所有操作引用此 ID |
| `catalogId` | 组件目录，固定使用 `cteno/v1` |

### updateComponents — 添加/更新组件

发送组件数组，ID 匹配的组件会被替换，新 ID 的组件追加到渲染面。

```json
{"updateComponents": {"surfaceId": "main", "components": [
  {"id": "root", "component": "Container", "children": ["title"]},
  {"id": "title", "component": "Text", "text": "你好", "variant": "heading"}
]}}
```

| 字段 | 说明 |
|------|------|
| `surfaceId` | 目标渲染面 ID |
| `components` | 组件数组，每个组件必须包含 `id` 和 `component` 字段 |

### updateDataModel — 更新数据模型

更新渲染面的数据模型，用于数据绑定场景。

```json
{"updateDataModel": {"surfaceId": "main", "data": {"score": 0.52, "followers": 5172}}}
```

| 字段 | 说明 |
|------|------|
| `surfaceId` | 目标渲染面 ID |
| `data` | 键值对数据对象 |

### deleteSurface — 删除渲染面

移除整个渲染面及其所有组件。

```json
{"deleteSurface": {"surfaceId": "main"}}
```

---

## 组件参考（cteno/v1 目录）

### 布局组件

#### Container — 根布局容器

页面的根容器，所有组件树的起点。

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `padding` | number | 否 | 内边距（像素） |
| `maxWidth` | number | 否 | 最大宽度 |
| `background` | string | 否 | 背景色（CSS 颜色值） |
| `children` | string[] | 否 | 子组件 ID 列表 |

#### Row — 水平布局

水平排列子组件（flex-direction: row）。

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `gap` | number | 否 | 子组件间距 |
| `align` | string | 否 | 垂直对齐方式（flex alignItems） |
| `justify` | string | 否 | 水平分布方式（flex justifyContent） |
| `children` | string[] | 否 | 子组件 ID 列表 |

#### Column — 垂直布局

垂直排列子组件（flex-direction: column）。

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `gap` | number | 否 | 子组件间距 |
| `align` | string | 否 | 水平对齐方式 |
| `children` | string[] | 否 | 子组件 ID 列表 |

#### Card — 卡片容器

带边框和圆角的容器，用于分组关联内容。

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `title` | string | 否 | 卡片标题 |
| `children` | string[] | 否 | 子组件 ID 列表 |

#### Divider — 分割线

细线分隔符，无属性。

---

### 数据展示组件

#### Text — 文本

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `text` | string | 是 | 文本内容 |
| `variant` | string | 否 | 样式变体：`"heading"` / `"subheading"` / `"body"` / `"caption"` / `"code"` |
| `markdown` | bool | 否 | 是否启用 Markdown 渲染 |

#### Progress — 进度条

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `value` | number | 是 | 进度值，范围 0.0 ~ 1.0 |
| `label` | string | 否 | 进度说明文本 |

#### MetricCard — 指标卡片

单个关键指标的展示卡片。

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `value` | string/number | 是 | 指标值 |
| `label` | string | 是 | 指标名称 |
| `trend` | string | 否 | 趋势变化（如 `"+58"`, `"-0.1%"`） |
| `trendDirection` | string | 否 | 趋势方向：`"up"` / `"down"` |

#### StatusIndicator — 状态指示器

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `status` | string | 是 | 状态：`"active"` / `"idle"` / `"error"` |
| `text` | string | 是 | 状态说明文本 |

#### Badge — 徽标

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `text` | string | 是 | 徽标文本 |
| `variant` | string | 否 | 样式变体：`"info"` / `"success"` / `"warning"` / `"error"` |

---

### 列表组件

#### List — 列表容器

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `title` | string | 否 | 列表标题 |
| `children` | string[] | 否 | 子组件 ID 列表（ListItem 或 ChecklistItem） |

#### ListItem — 列表项

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `text` | string | 是 | 主文本 |
| `icon` | string | 否 | 图标名称（Ionicons） |
| `secondaryText` | string | 否 | 副文本 |
| `action` | object | 否 | 点击动作（同 Button 的 action 格式） |

#### ChecklistItem — 清单项

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `text` | string | 是 | 清单文本 |
| `checked` | bool | 是 | 是否已完成 |

---

### 交互组件

#### Button — 按钮

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `label` | string | 是 | 按钮文本 |
| `variant` | string | 否 | 样式变体：`"primary"` / `"secondary"` / `"danger"` |
| `icon` | string | 否 | 图标名称（Ionicons） |
| `action` | object | 否 | 点击动作：`{"event": {"name": "事件名", "data": {...}}}` |

#### ButtonGroup — 按钮组

水平排列多个按钮。

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `children` | string[] | 否 | 子组件 ID 列表（Button） |

---

### 媒体组件

#### Image — 图片

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `src` | string | 是 | 图片 URL |
| `alt` | string | 否 | 替代文本 |
| `caption` | string | 否 | 图片说明 |

#### Icon — 图标

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | Ionicons 图标名称 |
| `color` | string | 否 | 图标颜色 |
| `size` | number | 否 | 图标大小 |

---

### 复合组件

复合组件是常见模式的语法糖，内部自动生成子组件。

#### MetricsGrid — 指标网格

自动布局的指标卡片网格，比手动创建多个 MetricCard 更简洁。

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `metrics` | Record<string, string/number> | 是 | 键值对形式的指标数据 |

#### ActivityFeed — 活动流

带时间戳的垂直活动记录。

| 属性 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `items` | array | 是 | 活动条目：`[{"text": "描述", "timestamp": "10:30"}]` |

---

## 组件组合模式

### 父子关系

组件通过 `children: ["child-id-1", "child-id-2"]` 引用子组件。所有组件在 `updateComponents` 中以**扁平数组**传输，渲染器负责根据 children 引用解析出树形结构。

### 示例：看板面板

完整的看板布局，包含状态、进度、指标和活动流：

```json
{"messages": [
  {"createSurface": {"surfaceId": "main", "catalogId": "cteno/v1"}},
  {"updateComponents": {"surfaceId": "main", "components": [
    {"id": "root", "component": "Container", "children": ["status", "main-card"]},
    {"id": "status", "component": "StatusIndicator", "status": "active", "text": "运行中"},
    {"id": "main-card", "component": "Card", "title": "抖音涨粉看板", "children": ["progress", "metrics", "feed"]},
    {"id": "progress", "component": "Progress", "value": 0.52, "label": "总进度 52%"},
    {"id": "metrics", "component": "MetricsGrid", "metrics": {"粉丝数": 5172, "目标": 10000, "昨日播放": 8664}},
    {"id": "feed", "component": "ActivityFeed", "items": [
      {"text": "完成竞品分析", "timestamp": "10:30"},
      {"text": "发布观点视频", "timestamp": "14:15"}
    ]}
  ]}}
]}
```

### 示例：交互操作面板

带按钮的操作面板，用户点击后 Agent 收到事件：

```json
{"messages": [
  {"updateComponents": {"surfaceId": "main", "components": [
    {"id": "actions", "component": "Card", "title": "操作面板", "children": ["btn-group"]},
    {"id": "btn-group", "component": "ButtonGroup", "children": ["btn-run", "btn-pause"]},
    {"id": "btn-run", "component": "Button", "label": "开始实验", "variant": "primary", "icon": "play", "action": {"event": {"name": "start_experiment"}}},
    {"id": "btn-pause", "component": "Button", "label": "暂停", "variant": "secondary", "icon": "pause", "action": {"event": {"name": "pause"}}}
  ]}}
]}
```

### 示例：数据监控面板

指标卡片 + 待办清单组合：

```json
{"messages": [
  {"updateComponents": {"surfaceId": "main", "components": [
    {"id": "monitor", "component": "Card", "title": "数据监控", "children": ["metrics-row", "checklist"]},
    {"id": "metrics-row", "component": "Row", "gap": 12, "children": ["mc-1", "mc-2", "mc-3"]},
    {"id": "mc-1", "component": "MetricCard", "value": "5,172", "label": "粉丝数", "trend": "+58", "trendDirection": "up"},
    {"id": "mc-2", "component": "MetricCard", "value": "8,664", "label": "播放量", "trend": "+1.2k", "trendDirection": "up"},
    {"id": "mc-3", "component": "MetricCard", "value": "3.2%", "label": "互动率", "trend": "-0.1%", "trendDirection": "down"},
    {"id": "checklist", "component": "List", "title": "下一步", "children": ["cl-1", "cl-2", "cl-3"]},
    {"id": "cl-1", "component": "ChecklistItem", "text": "优化封面设计", "checked": true},
    {"id": "cl-2", "component": "ChecklistItem", "text": "测试发布时间", "checked": false},
    {"id": "cl-3", "component": "ChecklistItem", "text": "分析竞品数据", "checked": false}
  ]}}
]}
```

---

## 最佳实践

### 调用模式

1. **首次渲染**：始终将 `createSurface` 和 `updateComponents` 放在同一次 `a2ui_render` 调用的 messages 数组中
2. **增量更新**：后续只发送 `updateComponents`，包含变化的组件即可（ID 匹配则替换，新 ID 则追加）
3. **更新频率**：不要超过每 5 秒一次，确保流畅的用户体验

### 组件 ID 命名

使用语义化 ID，清晰表达组件含义：

| 推荐 | 避免 |
|------|------|
| `"followers-metric"` | `"comp-1"` |
| `"progress-bar"` | `"p1"` |
| `"start-btn"` | `"b"` |

### 组件树结构

- 保持树形结构相对扁平，建议 2-3 层深度
- 使用 Card 分组关联内容
- 优先使用 MetricsGrid 而非手动创建多个 MetricCard
- 使用 Row 水平排列同级组件，Column 垂直排列

### Action 事件命名

事件名应具有描述性，清楚表达用户意图：

| 推荐 | 避免 |
|------|------|
| `"start_experiment"` | `"click1"` |
| `"refresh_data"` | `"action"` |
| `"pause_monitoring"` | `"btn"` |

---

## Action 事件处理

当用户点击带有 `action` 的 Button 时，Agent 会收到一条 `[User Action]` 消息：

```
[User Action] {"surfaceId":"main","componentId":"btn-run","event":{"name":"start_experiment"}}
```

收到此消息后，在下一次回复中根据事件内容执行相应操作。例如：

- 收到 `start_experiment` → 开始执行实验流程
- 收到 `pause` → 暂停当前任务
- 收到 `refresh_data` → 重新采集数据并用 `updateComponents` 刷新界面
