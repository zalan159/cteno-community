# Multi-Vendor Message Type Gap Analysis

> 完整对照 Claude Agent SDK / Codex SDK 的所有消息类型与 Cteno 前端处理能力的差异。

## 方法论

逐条对照三个源头：
1. Claude Agent SDK `sdk.d.ts` 的 25 个 SDKMessage 类型
2. Codex SDK 的 16 个 ThreadItem 类型 + 54 个 ServerNotification 类型
3. Cteno 前端 `typesRaw.ts` / `reducer.ts` / `sync.ts` / `executor_normalizer.rs` 的处理分支

---

## A. Claude SDK — 25 个 SDKMessage 类型

| # | SDKMessage 类型 | Rust Adapter 处理 | Normalizer → ACP | 前端渲染 | 状态 |
|---|---|---|---|---|:---:|
| 1 | `SDKAssistantMessage` (text blocks) | ✅ StreamDelta::Text | text-delta → message | AgentTextBlock | ✅ |
| 2 | `SDKAssistantMessage` (thinking blocks) | ✅ StreamDelta::Thinking | thinking-delta | AgentTextBlock (折叠) | ✅ |
| 3 | `SDKAssistantMessage` (redacted_thinking) | ❌ 忽略 | - | - | **缺** |
| 4 | `SDKAssistantMessage` (tool_use blocks) | ✅ ToolCallStart | tool-call | ToolCallBlock | ✅ |
| 5 | `SDKAssistantMessage` (tool_result blocks) | ✅ ToolResult | tool-result | ToolCallBlock 子消息 | ✅ |
| 6 | `SDKAssistantMessage` (image blocks) | ❌ 忽略 | - | - | **缺** |
| 7 | `SDKAssistantMessage` (web_search_result) | ❌ 忽略 | - | - | **缺** |
| 8 | `SDKAssistantMessage` (web_fetch) | ❌ 忽略 | - | - | **缺** |
| 9 | `SDKAssistantMessage` (code_execution_result) | ❌ 忽略 | - | - | **缺** |
| 10 | `SDKAssistantMessage` (bash_code_execution_*) | ❌ 忽略 | - | - | **缺** |
| 11 | `SDKAssistantMessage` (citations_delta) | ❌ 忽略 | - | - | **缺**(低优) |
| 12 | `SDKAssistantMessage` (document) | ❌ 忽略 | - | - | **缺**(低优) |
| 13 | `SDKAssistantMessage` (mcp_tool_use/result) | ✅ ToolCallStart | tool-call | ToolCallBlock (mcp__前缀) | ✅ |
| 14 | `SDKAssistantMessage` (server_tool_use) | ❌ 忽略 | - | - | **缺**(低优) |
| 15 | `SDKUserMessage` | ✅ 不处理（host 侧发送） | - | UserTextBlock | ✅ |
| 16 | `SDKResultMessage` (success) | ✅ TurnComplete | task_complete | thinking=false | ✅ |
| 17 | `SDKResultMessage` (error_*) | ✅ Error | error ACP | AgentTextBlock | ✅ |
| 18 | `SDKSystemMessage` (init) | ✅ SessionReady | - | - | ✅ |
| 19 | `SDKPartialAssistantMessage` (stream) | ✅ 流式处理 | delta 系列 | streaming 预览 | ✅ |
| 20 | `SDKCompactBoundaryMessage` | ❌ NativeEvent(丢弃) | - | - | **缺** |
| 21 | `SDKStatusMessage` (compacting) | ❌ NativeEvent(丢弃) | - | - | **缺** |
| 22 | `SDKAPIRetryMessage` | ❌ NativeEvent(丢弃) | - | - | **缺** |
| 23 | `SDKLocalCommandOutputMessage` | ❌ NativeEvent(丢弃) | - | - | **缺**(低优) |
| 24 | `SDKHookStartedMessage` | ❌ NativeEvent(丢弃) | - | - | **缺**(低优) |
| 25 | `SDKHookProgressMessage` | ❌ NativeEvent(丢弃) | - | - | **缺**(低优) |
| 26 | `SDKHookResponseMessage` | ❌ NativeEvent(丢弃) | - | - | **缺**(低优) |
| 27 | `SDKToolProgressMessage` | ❌ NativeEvent(丢弃) | - | - | **缺** |
| 28 | `SDKAuthStatusMessage` | ❌ NativeEvent(丢弃) | - | - | **缺**(低优) |
| 29 | `SDKTaskNotificationMessage` | ❌ NativeEvent(丢弃) | - | - | **缺** |
| 30 | `SDKTaskStartedMessage` | ❌ NativeEvent(丢弃) | - | - | **缺** |
| 31 | `SDKTaskProgressMessage` | ❌ NativeEvent(丢弃) | - | - | **缺** |
| 32 | `SDKSessionStateChangedMessage` | ❌ NativeEvent(丢弃) | - | - | **缺** |
| 33 | `SDKFilesPersistedEvent` | ❌ NativeEvent(丢弃) | - | - | **缺**(低优) |
| 34 | `SDKToolUseSummaryMessage` | ❌ NativeEvent(丢弃) | - | - | **缺** |
| 35 | `SDKRateLimitEvent` | ❌ NativeEvent(丢弃) | - | - | **缺** |
| 36 | `SDKElicitationCompleteMessage` | ❌ NativeEvent(丢弃) | - | - | **缺** |
| 37 | `SDKPromptSuggestionMessage` | ❌ NativeEvent(丢弃) | - | - | **缺** |

---

## B. Codex SDK — 16 个 ThreadItem 类型

| # | ThreadItem 类型 | Rust Adapter 映射 | 前端已有 View | 状态 |
|---|---|---|---|:---:|
| 1 | `userMessage` | 不处理（host 侧） | UserTextBlock | ✅ |
| 2 | `agentMessage` | StreamDelta::Text | AgentTextBlock | ✅ |
| 3 | `reasoning` | StreamDelta::Reasoning | ✅(刚修) | ✅ |
| 4 | `commandExecution` | `shell` tool | `Bash`/`CodexBash` view 有 | **名称不匹配** |
| 5 | `fileChange` | `apply_patch` tool | `CodexPatch` view 有 | **名称不匹配** |
| 6 | `mcpToolCall` | `mcp__{s}__{t}` | mcp__ 渲染 | ✅ |
| 7 | `dynamicToolCall` | ❌ 不处理 | 无 | **缺** |
| 8 | `webSearch` | `web_search` tool | `WebSearch` view 有 | **名称不匹配** |
| 9 | `plan` | ❌ 不处理 | `update_plan` view 有 | **缺** |
| 10 | `todo_list` | ❌ NativeEvent(丢弃) | `TodoWrite` view 有 | **缺** |
| 11 | `collabAgentToolCall` | ❌ 不处理 | `Task` view 有 | **缺** |
| 12 | `imageView` | ❌ 不处理 | `screenshot` view 有 | **缺** |
| 13 | `imageGeneration` | ❌ 不处理 | `image_generation` view 有 | **缺** |
| 14 | `enteredReviewMode` | ❌ 不处理 | 无 | **缺**(低优) |
| 15 | `exitedReviewMode` | ❌ 不处理 | 无 | **缺**(低优) |
| 16 | `contextCompaction` | ❌ 不处理 | 无 | **缺**(低优) |
| 17 | `hookPrompt` | ❌ 不处理 | 无 | **缺**(低优) |

---

## C. Codex SDK — 关键 ServerNotification 事件

| # | 事件 | Rust Adapter 处理 | 前端处理 | 状态 |
|---|---|---|---|:---:|
| 1 | `item/agentMessage/delta` | ✅ StreamDelta | streaming 预览 | ✅ |
| 2 | `item/reasoning/textDelta` | ✅ StreamDelta::Reasoning | streaming 预览 | ✅ |
| 3 | `item/reasoning/summaryTextDelta` | ❌ 不处理 | - | **缺** |
| 4 | `item/commandExecution/outputDelta` | ✅ ToolCallInputDelta | tool-call-delta | ✅ |
| 5 | `item/fileChange/outputDelta` | ✅ ToolCallInputDelta | tool-call-delta | ✅ |
| 6 | `item/plan/delta` | ❌ 不处理 | - | **缺** |
| 7 | `item/commandExecution/terminalInteraction` | ❌ 不处理 | - | **缺** |
| 8 | `item/autoApprovalReview/started` | ❌ 不处理 | - | **缺** |
| 9 | `item/autoApprovalReview/completed` | ❌ 不处理 | - | **缺** |
| 10 | `item/mcpToolCall/progress` | ❌ 不处理 | - | **缺** |
| 11 | `turn/diff/updated` | ❌ 不处理 | - | **缺** |
| 12 | `turn/plan/updated` | ❌ 不处理 | - | **缺** |
| 13 | `thread/status/changed` | ❌ 不处理 | - | **缺** |
| 14 | `thread/tokenUsage/updated` | ❌ 不处理 | - | **缺** |
| 15 | `account/rateLimits/updated` | ❌ 不处理 | - | **缺** |
| 16 | `error` (+ willRetry) | ✅ Error | error ACP | ✅ |
| 17 | `hook/started` | ❌ 不处理 | - | **缺**(低优) |
| 18 | `hook/completed` | ❌ 不处理 | - | **缺**(低优) |
| 19 | `model/rerouted` | ❌ 不处理 | - | **缺**(低优) |
| 20 | `thread/realtime/*` (8 个) | ❌ 不处理 | - | **缺**(语音,远期) |

---

## D. Claude 工具名兼容性（已匹配）

Claude CLI 的工具名与前端 knownTools 完全一致：
`Bash`, `Read`, `Write`, `Edit`, `MultiEdit`, `Glob`, `Grep`, `WebSearch`, `WebFetch`, `Task`, `AskUserQuestion`, `Monitor`, `LSP`, `Skill`, `NotebookEdit`, `NotebookRead`, `TodoWrite`

**不需要映射**。

---

## E. 已提交到 tasks.json 但本次发现遗漏的

对照已有 14 个任务，以下是**额外遗漏**（未在任务中覆盖）：

### 遗漏 1: Claude image content blocks
Claude assistant message 可包含 `image` 类型 content block（非 image_generation 工具，而是内联图片）。Adapter 忽略了。

### 遗漏 2: Claude web_fetch content blocks
不同于 WebFetch 工具调用，这是 server-side fetch 结果内联返回。

### 遗漏 3: Codex dynamicToolCall
第三方动态注册的工具调用（非 MCP），Adapter 完全不处理。

### 遗漏 4: Claude SDKToolProgressMessage
工具执行进度（elapsed_time_seconds），可以用来显示长工具的进度条/计时器。

### 遗漏 5: Claude SDKToolUseSummaryMessage
多个工具调用后的摘要总结，用于 compact 后的上下文恢复。

### 遗漏 6: Claude SDKTaskStarted/Progress/Notification
Claude 的 subagent/background task 生命周期事件。前端有 `Task` view 但这些事件被丢弃了。

### 遗漏 7: Codex turn/plan/updated
Codex 的执行计划实时更新（step by step 进度），完全独立于 plan item。

### 遗漏 8: Codex thread/tokenUsage/updated
实时 token 使用量更新，前端有 usage 显示但没接收这个事件。

### 遗漏 9: Claude SDKCompactBoundaryMessage
压缩边界事件，前端 reducer 有 "Compaction completed" 特殊处理（reset usage），但 ACP 路径没传递。

### 遗漏 10: Codex MCP tool call progress
MCP 工具调用中的进度消息，可用于长时间 MCP 调用的 UI 反馈。

---

## F. 优先级汇总（含已有任务 + 新发现遗漏）

### P0 — 已在 tasks.json（3 个）
- [x] Codex 工具名映射
- [x] Codex plan/todo 映射
- [x] Codex file change diff

### P1 — 已在 tasks.json（6 个）
- [x] Codex collab agent → Task
- [x] Codex image 映射
- [x] Claude redacted_thinking
- [x] Rate limit 通知
- [x] Session state 指示器
- [x] Codex Guardian approval

### P1 — 新发现需追加（5 个）
- [ ] Claude image content blocks 内联图片
- [ ] Claude Task 生命周期事件（TaskStarted/Progress/Notification）→ Task view 进度更新
- [ ] Codex dynamicToolCall → 通用工具渲染
- [ ] Claude SDKToolProgressMessage → 工具执行计时器
- [ ] Codex turn/plan/updated → 实时执行计划步骤

### P2 — 已在 tasks.json（5 个）
- [x] Claude PromptSuggestion
- [x] Claude MCP Elicitation
- [x] Codex terminal interaction
- [x] Claude web_search/code_execution blocks
- [x] Codex reasoning summary

### P2 — 新发现需追加（5 个）
- [ ] Claude web_fetch content blocks
- [ ] Claude SDKCompactBoundaryMessage → 前端 compact 通知
- [ ] Claude SDKToolUseSummaryMessage → 工具摘要
- [ ] Codex thread/tokenUsage/updated → 实时 token 统计
- [ ] Codex MCP tool call progress → MCP 工具进度
