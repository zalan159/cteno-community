# Permission Mode 对接经验

这份文档记录 Cteno 前端与宿主层对接多 agent `permission mode` 的实际经验，目标是避免后续再把它做成“一套统一模式”，或者把 mode 只接到 UI 没有接到底层 runtime control。

## 结论先行

- `permission mode` 必须按 agent 分开显示，不能做成一套跨 vendor 统一菜单。
- 前端可以共用一条“切换 permission mode”的动作链，但菜单选项、标签、图标、默认值都必须按 vendor 决定。
- Claude 的 `auto` / `dontAsk` 不是前端文案问题，而是 runtime control 和持久化都要真正支持。
- 宿主侧 `PermissionHandler` 仍然是四态宿主权限门：`default / acceptEdits / plan / bypassPermissions`。这不是完整的 vendor mode 枚举。
- 对 Claude/Codex/Gemini 这类 vendor-specific mode，session 持久化必须保存 raw mode string，不能只保存宿主四态，否则重连后会丢失真实 mode。

## 当前真实模式面

前端现在按 agent 显示这些 mode：

- Claude:
  - `default`
  - `auto`
  - `acceptEdits`
  - `plan`
  - `dontAsk`
  - `bypassPermissions`
- Codex:
  - `default`
  - `read-only`
  - `safe-yolo`
  - `yolo`
- Gemini:
  - `default`
  - `read-only`
  - `safe-yolo`
  - `yolo`
- Cteno:
  - `default`
  - `acceptEdits`
  - `plan`
  - `bypassPermissions`

前端的 vendor-specific mode 定义集中在：

- [permissionModes.ts](/Users/zal/Cteno2.0/apps/client/app/utils/permissionModes.ts)

UI 消费点在：

- [persona/[id].tsx](/Users/zal/Cteno2.0/apps/client/app/app/(app)/persona/[id].tsx)
- [AgentInput.tsx](/Users/zal/Cteno2.0/apps/client/app/components/AgentInput.tsx)

## 正确链路

### 1. 前端显示层

不要在各个页面自己 hardcode 一套 mode list。

正确做法：

- 前端从 vendor 推出 mode list
- 同时从 vendor 推出 label / icon / keyboard cycle 顺序
- `persona` 页和聊天输入框共用同一套 helper

关键文件：

- [permissionModes.ts](/Users/zal/Cteno2.0/apps/client/app/utils/permissionModes.ts)

### 2. 前端动作层

前端切换 mode 统一走：

- [ops.ts](/Users/zal/Cteno2.0/apps/client/app/sync/ops.ts)
  - `sessionApplyPermissionModeChange()`
  - `sessionSetPermissionMode()`

这层职责是：

- 先看 runtime capability 是否允许热切换
- 调 session RPC `set-permission-mode`
- 若本地 RPC handler 丢了，尝试 `reconnect-session` 后重试
- 成功后把 mode 更新到本地 session store

## 3. 桌面端 RPC 层

本地与远端两条路径都要接：

- 本地 RPC:
  - [local_rpc.rs](/Users/zal/Cteno2.0/apps/client/desktop/src/happy_client/session/local_rpc.rs)
- 远端 Socket RPC:
  - [remote.rs](/Users/zal/Cteno2.0/apps/client/desktop/src/happy_client/session/remote.rs)

它们都做两件事：

- `parse_runtime_permission_mode(mode_str)` 把前端字符串转成 executor 的 `multi_agent_runtime_core::PermissionMode`
- 调 `executor.set_permission_mode(...)`

关键解析入口：

- [permission.rs](/Users/zal/Cteno2.0/apps/client/desktop/src/happy_client/permission.rs)

## 4. Executor 层

这里才是每家 vendor 真正的语义映射点：

- Claude:
  - [agent_executor.rs](/Users/zal/Cteno2.0/packages/multi-agent-runtime/rust/crates/multi-agent-runtime-claude/src/agent_executor.rs)
  - `PermissionMode::Auto -> "auto"`
  - `PermissionMode::DontAsk -> "dontAsk"`
- Codex:
  - [agent_executor.rs](/Users/zal/Cteno2.0/packages/multi-agent-runtime/rust/crates/multi-agent-runtime-codex/src/agent_executor.rs)
  - 本质是 `sandbox + approval_policy`
- Gemini:
  - [agent_executor.rs](/Users/zal/Cteno2.0/packages/multi-agent-runtime/rust/crates/multi-agent-runtime-gemini/src/agent_executor.rs)
  - 本质是 CLI approval preset
- Cteno:
  - [agent_executor.rs](/Users/zal/Cteno2.0/packages/multi-agent-runtime/rust/crates/multi-agent-runtime-cteno/src/agent_executor.rs)

不要把 vendor 语义强压成宿主层四态再传下去，否则 Claude 的 `auto` / `dontAsk` 会丢。

## 5. 持久化层

这是最容易漏的地方。

如果只把 mode 写进宿主 `PermissionHandler`，重连后会丢掉 vendor-specific mode。

正确做法：

- raw mode string 直接持久化到 session context / KV
- 宿主四态只在需要控制本地 permission gate 时才更新

相关文件：

- [session_helpers.rs](/Users/zal/Cteno2.0/apps/client/desktop/src/happy_client/session_helpers.rs)
- [local_rpc.rs](/Users/zal/Cteno2.0/apps/client/desktop/src/happy_client/session/local_rpc.rs)
- [mod.rs](/Users/zal/Cteno2.0/apps/client/desktop/src/happy_client/session/mod.rs)

## 关键坑点

### 1. 不要把 mode 统一成一套

最开始的问题就是把 Claude/Codex/Gemini/Cteno 做成一套共享 union，导致：

- Codex/Gemini 菜单显示成 Claude 风格
- Claude 的 `auto` / `dontAsk` 消失
- session 重连后 mode 语义不一致

### 2. 不要只改前端菜单

如果只把 UI 加上 `auto` / `dontAsk`，但后端 `parse_runtime_permission_mode()` 没接：

- 会直接报 `Unknown mode`
- 或者 UI 看起来能选，实际 vendor runtime 没收到

### 3. 不要只存宿主四态

宿主四态只够驱动本地 `PermissionHandler`。

它不够表达：

- Claude `auto`
- Claude `dontAsk`
- Codex / Gemini 的 sandbox 模式

所以 raw mode string 一定要落 session 持久化。

### 4. Claude `bypassPermissions` 有启动约束

Claude 运行中切到 `bypassPermissions`，要求 session 启动时就带：

- `--dangerously-skip-permissions`

相关实现：

- [agent_executor.rs](/Users/zal/Cteno2.0/packages/multi-agent-runtime/rust/crates/multi-agent-runtime-claude/src/agent_executor.rs)
- [workspace.rs](/Users/zal/Cteno2.0/packages/multi-agent-runtime/rust/crates/multi-agent-runtime-claude/src/workspace.rs)

如果没带这个 flag，热切换机制本身没坏，但 Claude CLI 会返回真实 vendor 错误。

### 5. `server not available` 往往不是真根因

本地模式下经常真正的问题是：

- session RPC handler 没注册
- 本地连接断了
- `Unknown method: <sessionId>:set-permission-mode`

所以动作层要带 reconnect + retry，而不是把所有错误都抹平成 `server unavailable`。

## 新增或修改 mode 时的检查单

1. 先确认这个 mode 是哪家 agent 真支持的，不要凭 UI 猜。
2. 更新 [permissionModes.ts](/Users/zal/Cteno2.0/apps/client/app/utils/permissionModes.ts) 的 vendor mode list。
3. 更新前端类型：
   - [storageTypes.ts](/Users/zal/Cteno2.0/apps/client/app/sync/storageTypes.ts)
   - [typesMessageMeta.ts](/Users/zal/Cteno2.0/apps/client/app/sync/typesMessageMeta.ts)
   - [typesRaw.ts](/Users/zal/Cteno2.0/apps/client/app/sync/typesRaw.ts)
   - [settings.ts](/Users/zal/Cteno2.0/apps/client/app/sync/settings.ts)
4. 更新宿主解析：
   - [permission.rs](/Users/zal/Cteno2.0/apps/client/desktop/src/happy_client/permission.rs)
5. 更新对应 executor 的 vendor 映射。
6. 确认 raw mode string 会被持久化和恢复。
7. 最后做真实 session 验证，不只看 typecheck。
