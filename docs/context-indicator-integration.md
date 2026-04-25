# Context 指示器对接经验

这份文档记录聊天输入框里的 `context: xx/yy` 指示器应该怎么接，以及之前踩过的几条错误链路。

这里说的指示器是 session 聊天框里的 context 使用提示，不是 session 列表里的 compact/compress 视图。

## 结论先行

- `context: xx/yy` 的主链必须跟随持久化 usage 事件更新，不能依赖 heartbeat。
- `used` 的来源是 session 持久化回来的 usage/context usage。
- `total` 应该走 agent 默认窗口或 vendor 自己的明确上报，不能继续依赖旧的 `compressionThreshold`。
- heartbeat 可以保留给在线状态，但不要再承担 context 指示器的数据语义。

## 当前正确语义

### `used`

优先级：

1. `session.contextTokens`
2. `session.latestUsage?.contextSize`
3. `0`

前端实现：

- [sessionUtils.ts](/Users/zal/Cteno2.0/apps/client/app/utils/sessionUtils.ts)

### `total`

当前前端 fallback：

- Codex / Cteno: `256K`
- Claude / Gemini: `1.0M`

这条规则也在：

- [sessionUtils.ts](/Users/zal/Cteno2.0/apps/client/app/utils/sessionUtils.ts)

如果某家 agent 后续提供更权威的窗口上报，可以覆盖掉 fallback；但不要再回退到旧的 `compressionThreshold` 展示链。

## 正确数据链

### 1. Executor 产生 usage

不同 agent 的 usage 来源不同：

- Codex:
  - app-server `thread/tokenUsage/updated`
  - adapter 必须把 `tokenUsage.last.totalTokens + tokenUsage.modelContextWindow` 转成 `NativeEvent(kind=context_usage)`
  - 以及 turn 完成时的 usage；`UsageUpdate/token_count` 只适合作为 fallback，不包含真实 context window
- Claude:
  - SDK `get_context_usage()`
- Gemini:
  - ACP `session/prompt` response 的 `_meta.quota.token_count`
  - adapter 同步生成 `NativeEvent(kind=context_usage)`；若响应将来直接带 context window，优先使用响应值，否则按 Gemini CLI 官方 `tokenLimit(model)` 规则从 `_meta.quota.model_usage[].model` 得出窗口
- Cteno:
  - stdio 在 `TurnComplete` 前发 `ContextUsage`，adapter 转成 `NativeEvent(kind=context_usage)`
  - `total_tokens` 是最后一次 LLM 请求的上下文占用，`max_tokens` 是 profile/model 的实际 context window

### 2. Normalizer 持久化成 ACP usage 消息

统一持久化入口：

- [executor_normalizer.rs](/Users/zal/Cteno2.0/apps/client/desktop/src/executor_normalizer.rs)

关键消息类型：

- `token_count`
- `context_usage`

### 3. 前端 sync 把 side-effect 写回 session

前端不能只把这些 ACP 消息当作“不渲染的消息”跳过。

必须在同步层把它们应用到 session：

- [sync.ts](/Users/zal/Cteno2.0/apps/client/app/sync/sync.ts)
  - `applyContextUsageToSession(...)`
  - 本地消息回放也要应用 side-effect

### 4. 存储层保留 `contextTokens`

存储层要保证：

- 显式 `session.contextTokens` 优先于 `latestUsage.contextSize`
- 刷新 / 重载后能从持久化恢复

关键文件：

- [storage.ts](/Users/zal/Cteno2.0/apps/client/app/sync/storage.ts)

### 5. UI 层只消费 session 状态

UI 只读 `useSessionStatus()` 结果，不直接碰 heartbeat 或 message payload。

关键文件：

- [sessionUtils.ts](/Users/zal/Cteno2.0/apps/client/app/utils/sessionUtils.ts)
- [AgentInput.tsx](/Users/zal/Cteno2.0/apps/client/app/components/AgentInput.tsx)
- [PersonaChatInput.tsx](/Users/zal/Cteno2.0/apps/client/app/components/PersonaChatInput.tsx)

## 已经踩过的坑

### 1. 不要依赖 heartbeat

本地模式一开始曾尝试从 heartbeat / `session-alive` 刷 `contextTokens` 和 `compressionThreshold`。

这条路的问题是：

- turn 很快结束时，heartbeat 可能根本赶不上
- 本地 sink / 本地回放路径很容易漏转发
- UI 会变成“碰运气更新”

现在这条链已经被降级为非主链，不应该再作为 context 指示器的数据源。

### 2. 不要继续走旧的 `compressionThreshold`

之前出现过：

- UI 先显示 `256K`
- 随后又被旧链路改成 `102K`

这是旧 `compressionThreshold` 逻辑在覆盖 agent 默认窗口。

后果是：

- 指示器看起来像“跳动更新”
- 实际是在走错误分母

所以现在 `context: xx/yy` 的 `yy` 不再跟旧 `compressionThreshold` 绑定。

### 3. 不要只修 socket 实时链，不修本地回放链

远端 socket 路径可能已经把 `token_count` side-effect 应用到 session 了，但本地 `fetchMessages` / 本地持久化回放如果没做同样处理：

- 打开历史 session 仍然不显示
- 重载页面后又消失

这个坑最后就是靠在 [sync.ts](/Users/zal/Cteno2.0/apps/client/app/sync/sync.ts) 里给本地回放补 side-effect 才彻底收住。

### 4. Claude 的 context used 不是旧 host threshold

Claude 这条线不能再幻想从旧 host `compressionThreshold` 推出来。

正确来源是：

- Claude SDK `get_context_usage()`

它会被 normalizer 持久化成 `context_usage`，然后前端再写回 `session.contextTokens`。

### 5. Codex 的 usage 已回来，不代表 UI 就自动更新

Codex 曾经出现过：

- app-server 已发 `thread/tokenUsage/updated`
- normalizer 日志里也看到了 `UsageUpdate`
- 但最终 UI 还是没有 `context: xx/yy`

根因不是 Codex 没数据，而是：

- `thread/tokenUsage/updated` 只转成了 `token_count`，没有同步转成携带 `modelContextWindow` 的 `context_usage`
- 或本地回放把 `token_count` 当成“无需处理”的普通消息跳过了

## 现在的判断标准

如果 `context: xx/yy` 没显示，排查顺序应该是：

1. 该 agent 的 executor 有没有真实产生 usage / context usage
2. normalizer 有没有持久化成 `token_count` 或 `context_usage`
3. 前端 `sync.ts` 有没有把这类 ACP 消息应用到 session
4. `storage.ts` 有没有保住 `session.contextTokens`
5. `sessionUtils.ts` 有没有拿到 `session.contextWindowTokens`；没有真实窗口时不要用 vendor 猜值硬显示

不要先去看 heartbeat。

## 新 agent 接 context 指示器时的检查单

1. 先确认该 agent 的 usage 真实事件在哪里产生。
2. 在 normalizer 层把它转成统一的持久化 ACP usage 消息。
3. 确认本地实时路径和本地历史回放路径都会应用 side-effect。
4. 确认 `session.contextTokens` 会被存储层保留。
5. UI 分母优先用该 agent 的真实窗口；没有就用明确 fallback，不要复用旧 compression 链。
6. 用真实 session 验证一次：
   - 新发一条消息时会更新
   - 刷新页面 / 重开 session 后仍然显示
