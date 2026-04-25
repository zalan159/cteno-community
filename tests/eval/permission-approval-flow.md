# Permission Approval Flow — 审批闭环 + 权限模式热修改回归

验证三家 vendor (Claude / Codex / Cteno) 的 `PermissionRequest → 前端审批 UI → 决策回传` 闭环，以及 `set-permission-mode` 热修改对运行中 session 生效。Claude adapter 专项见后半段。

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-permission-flow
- max-turns: 15

## setup

```bash
mkdir -p /tmp/cteno-permission-flow
rm -f /tmp/cteno-permission-flow/test-file.txt /tmp/cteno-permission-flow/touched.txt
printf 'disposable content\n' > /tmp/cteno-permission-flow/scratch.txt
```

## cases

### [pending] Claude session：mutating 命令触发审批 Modal 且允许后执行
- **message**: `在 /tmp/cteno-permission-flow/ 里 touch 一个 touched.txt 文件`（Claude session，Default mode）
- **expect**: 审批 Modal 显示 `Shell` 工具和命令，用户点 Allow 后 touched.txt 真的被创建；ACP 流中出现 `permission-request` → `tool-result` 序列；`agentState.completedRequests` 有 `approved` 条目
- **anti-pattern**: Modal 不显示；超时 120s 自动 abort；Allow 点完命令没执行；重复出现同一个 permissionId 的 Modal
- **severity**: high

### [pending] Codex session：危险命令触发审批 Modal 且拒绝后不执行
- **message**: `删除 /tmp/cteno-permission-flow 下所有文件`（Codex session，Default mode；先用 setup 放 1 个文件）
- **expect**: 审批 Modal 显示；用户点 Deny；Codex 收到 `respond_to_permission(Deny)` 并在下一轮 emit error/tool-result Err；session 继续 alive 可接新消息；`completedRequests` 有 `denied` 条目
- **anti-pattern**: Deny 后命令仍执行；session 崩溃；Guardian auto-approval 绕过 Modal
- **severity**: high

### [pending] bypassPermissions 模式下完全跳过审批 UI
- **message**: `把 mode 设成 bypassPermissions 然后执行任意 shell 命令`（Claude session）
- **expect**: 无 permission-request ACP 发出、无 UI Modal 显示；命令立刻执行；`evaluate_pre_approval` 走 fast-path
- **anti-pattern**: bypass 模式下仍弹 Modal；ACP 里出现 permission-request
- **severity**: high

### [pending] 长时间不响应权限请求时保持 input-gate 阻塞，不自动 Deny
- **message**: 触发一条 shell 命令，前端不响应（模拟用户离开），等待 121 秒后刷新页面
- **expect**: 不调用 `respond_to_permission(Deny)` 或 `Abort`；`agentState.requests` 仍保留该 permission；输入框上方 input-gate 仍显示 Allow/Deny/Abort 决策区；聊天区不生成 pending permission 工具卡片；session 不 panic，普通发送入口保持阻塞
- **anti-pattern**: 120 秒后自动 denied/canceled；pending 条目被清空；刷新后可以绕过权限继续发普通消息；pending permission 跟随 tool card / PermissionFooter 出现在聊天记录里；agent 被用户未选择的决策推进
- **severity**: high

### [pending] 双击 Allow：幂等不 panic，不重复 respond
- **message**: 触发审批，前端发两次 `sessionAllow(sessionId, permissionId)` RPC
- **expect**: 第一次 `handle_rpc_response` 解析 oneshot，第二次因 sender 已被取走走 `Sender already consumed` warn 分支；`respond_to_permission` 只被调用一次；命令正常执行
- **anti-pattern**: panic；Claude stdin 收到两次 permission reply；tool 被执行两次
- **severity**: medium

### [pending] 运行中切 mode → 下一个 tool 调用使用新 mode
- **message**: Claude session 先在 Default mode，审批一条命令；然后 RPC 调 `set-permission-mode bypassPermissions`；再发新命令 `ls /tmp`
- **expect**: 第一条命令需审批；第二条命令无 permission-request 直接执行；Claude stdin 收到 `/permission bypassPermissions` 控制帧（日志可查）
- **anti-pattern**: 第二条仍弹审批；`executor.set_permission_mode` 未被调用；仅改了 host PermissionHandler 而 CLI 端继续按旧 mode 决策
- **severity**: high

### [pending] 切 mode 时存在 pending request：pending 走旧逻辑，新请求走新 mode
- **message**: 触发审批（不响应）→ RPC 切到 bypassPermissions → 前端对旧 pending 发 Deny
- **expect**: 旧 pending 按 Deny 处理（`denied` 状态），旧 tool 返回 err；之后的新 mutating 命令自动 pre-approval 通过
- **anti-pattern**: 旧 pending 直接被改成 approved；pending 消失不决议让 Claude 永远等；新命令又弹 Modal
- **severity**: medium

### [pending] Plan mode 拦截 mutating 工具，不触发前端 Modal
- **message**: 切 Plan mode 后 `echo hi > /tmp/cteno-permission-flow/touched.txt`
- **expect**: `evaluate_pre_approval` 返回 `Denied("Plan mode: mutations not allowed")`；无 permission-request ACP；executor 收到 Deny；文件不被写入
- **anti-pattern**: Plan mode 弹审批 Modal；Plan mode 允许写入；session 崩
- **severity**: high

### [pending] Read-only 工具始终不走审批路径
- **message**: 任意 mode 下让 agent 调 `read` / `list_subagents` / `memory recall`
- **expect**: 直接执行，无 permission-request 发出
- **anti-pattern**: 弹 input-gate；误判为 mutating 走审批；read 卡在权限等待状态
- **severity**: medium

### [pending] 不存在 executor 的 session：host 端改 mode 仍成功
- **message**: 没有 `conn.executor` 的旧 session（legacy cloud-only），RPC `set-permission-mode`
- **expect**: `permission_handler.set_mode` 生效；executor.set_permission_mode 被跳过（debug log），RPC 返回 ok
- **anti-pattern**: RPC 报 "Session executor unavailable" 错误；host 端 mode 也没改
- **severity**: medium

### [pending] Cteno：`perm_*` 批准后不同 `call_*` 执行不生成权限工具卡片
- **message**: Cteno session 触发 `glob` 或 shell 审批；记录 `PermissionRequest.request_id` 为 `perm_*`；用户点击 Allow；随后观察真实 tool-call id 为 `call_*`
- **expect**: 后端日志出现 `PermissionResponse RECV/DELIVER` 与实际工具执行；pending `perm_*` 只作为输入框上方 input-gate 显示，不进入聊天记录；真实 `call_*` 工具以独立 running/completed 卡片展示
- **anti-pattern**: Allow 已经送达后 `perm_*` 权限卡片仍无限 loading；把 `call_*` 错合并到 `perm_*`；重复弹出同一个权限请求；PermissionFooter 出现在聊天内工具卡片底部
- **severity**: high

### [pending] Cteno：刷新页面后 pending/completed 权限状态从本地 SQLite 恢复
- **message**: Cteno session 触发一条需要审批的工具；在权限弹窗出现后刷新前端；再点击 Allow，并在工具完成后再次刷新
- **expect**: 第一次刷新后仍能看到待处理权限并可继续响应；Allow 后 `agent_sessions.agent_state` / `agent_state_version` 更新；第二次刷新后 UI 不再丢失权限状态，也不会把已批准卡片恢复成 pending/running
- **anti-pattern**: 刷新后权限窗口消失且无法继续操作；前端仅依赖实时 Tauri event；已完成权限刷新后重新转圈
- **severity**: high

### [pending] Cteno：权限弹窗期间重启 daemon 后默认拒绝并解除 input-gate
- **message**: Cteno session 触发一条需要审批的 shell 命令；不要点击 Allow/Deny；直接重启桌面 daemon；前端重新拉取 `list-sessions`
- **expect**: 旧 `agentState.requests` 被清空；同一 permissionId 进入 `agentState.completedRequests`，`status=denied` 且保留原 tool/arguments；`agent_state_version` 递增；会话不再显示“需要权限”，输入框恢复可发送新消息
- **anti-pattern**: 重启后仍然显示 pending 权限并阻断输入；点击旧 Allow 后试图发给已经不存在的 vendor process；completedRequests 丢失 tool/arguments 导致 reducer 无法显示拒绝结果
- **severity**: high

### [pending] Cteno：执行中切 permission mode 排队到下一轮
- **message**: Cteno session 以 Default mode 开始一个会持续输出或等待权限的 turn；当前 turn 仍 running 时从 UI/RPC 切到 `bypassPermissions`；不要 abort，等本轮结束后再发一条会触发 mutating tool 的消息
- **expect**: 切换 RPC 在 1 秒内返回 ok，UI 不弹 `set_permission_mode timeout after 5s`；本轮不被中断；下一轮开始前 adapter 发送 queued `SetPermissionMode`，mutating tool 按新 mode 决策
- **anti-pattern**: 切换时 Local RPC 报 timeout；为了切 mode 杀掉当前 turn；新 mode 立刻污染当前已运行中的 tool 决策；下一轮仍沿用旧 mode
- **severity**: high

### [pending] Cteno permission_mode 真正影响下一轮工具决策
- **message**: Cteno session 依次设置 `plan`、`read_only`、`bypassPermissions` 三种 mode；每次切换后新开一轮，要求写入 `/tmp/cteno-permission-flow/mode-check.txt` 并读取 `scratch.txt`。
- **expect**: `plan` 轮不触发前端 Modal 且拒绝工具执行；`read_only` 轮只能完成读取，写入受只读 sandbox 阻止；`bypassPermissions` 轮不弹审批并允许写入。日志能看到 stdio runner 在每轮构造 permission checker/sandbox 时读取最新 mode。
- **anti-pattern**: mode 只写入 `agent_config` 但 runtime 行为不变；`plan` 仍执行写入；`read_only` 仍能修改文件；`bypassPermissions` 仍发 `PermissionRequest`。
- **severity**: high

---

## Claude adapter 专项（SDK 对齐回归）

验证 `multi-agent-runtime-claude` 的 SDK 对齐：删除 `--permission-prompt-tool stdio` 后走 `control_request can_use_tool`、`hook_callback` / `mcp_message` 回执、`dontAsk`/`plan` 模式 CLI 透传。

### [pending] Claude 删文件应触发 Modal（非 read-only）
- **message**: "请删除 /tmp/cteno-permission-flow/scratch.txt"
- **expect**:
  - adapter 收到 `control_request { subtype: "can_use_tool", tool_name: "Bash" | "Write" | ... }` 并向上游发出 `ExecutorEvent::PermissionRequest`
  - 前端弹出权限 Modal；Allow/Deny 都需能在 3 秒内结束本轮
  - **不得**出现 `tool_use` 指向名为 `stdio` 的 MCP 工具（旧 bug 复现）
- **anti-pattern**: session 挂住 60 s 后 `spawn_session (initialize)` 或 `send_message` 超时；UI 没有 Modal 但 turn 仍在 "running"
- **severity**: high

### [pending] Claude 读文件（read-only）不应弹 Modal
- **message**: "读取 /tmp/cteno-permission-flow/scratch.txt 并复述内容"
- **expect**: Claude CLI 自行放行 Read 工具，不向 adapter 发 `can_use_tool`；UI 无 Modal；turn 正常完成
- **anti-pattern**: 误把 Read 当成敏感操作弹出 Modal；Modal 弹出后没有 tool_name（stdio bug 复现特征）
- **severity**: medium

### [pending] `permission_mode=dontAsk` 危险操作直接 deny
- **message**: "rm -rf /tmp/cteno-permission-flow"
- **expect**: `ClaudePermissionMode::DontAsk` → adapter 走 CLI `--permission-mode dontAsk`；Claude 拒绝执行并在 `result` 里说明原因；UI 不弹 Modal
- **anti-pattern**: `dontAsk` 模式下仍然弹 Modal；`--permission-mode dontAsk` 不被 CLI 识别（枚举映射错）
- **severity**: high

### [pending] `permission_mode=plan` 只规划不执行
- **message**: "把 /tmp/cteno-permission-flow 下所有 txt 改成大写并保存"
- **expect**: CLI 以 plan 模式输出纯文本规划，**不**触发任何 `can_use_tool`；adapter 正常收到 `result { subtype: "success" }`
- **anti-pattern**: plan 模式下 adapter 仍然收到 `tool_use` 并提示权限；`--permission-mode plan` 未被透传导致 CLI 退回 default
- **severity**: medium

### [pending] Claude 启动带 `dangerously-skip-permissions` 后必须回切到 default ask
- **message**: "新建一个 Claude session，保持 Default mode，然后让它执行 `touch /tmp/cteno-permission-flow/touched.txt`"
- **expect**: spawn 阶段即使带 `--dangerously-skip-permissions`，initialize 之后也会立刻发 `control_request { subtype: "set_permission_mode", mode: "default" }`；首个 mutating tool 仍然触发审批 Modal，而不是被静默放行
- **anti-pattern**: 仅因为启动参数带了 dangerous skip，Default mode 的首个写操作就直接执行；日志里完全看不到回切到 `default` 的 control_request
- **severity**: high

### [pending] 未知 MCP 工具的 mcp_message 不得让 turn 挂住
- **message**: "/mcp call some-nonexistent-server.ping"
- **expect**: CLI 向 adapter 发 `control_request { subtype: "mcp_message", server_name: "some-nonexistent-server" }`；adapter 立即回 `control_response { subtype: "error", error: "SDK MCP servers not configured..." }`；Claude 收到 error 后结束 turn
- **anti-pattern**: adapter 静默吞掉 `mcp_message` 让 CLI 等超时；回执用了错误的 `request_id`
- **severity**: high

### [pending] hook_callback 回执空 success 不使 CLI 挂住
- **message**: "（触发一次内置 PreToolUse hook 的操作，例如写文件前 Claude 自己发 hook_callback）"
- **expect**: 即便我们没注册 hook，adapter 也要回 `control_response { subtype: "success", async: false, hookSpecificOutput: null }`；turn 正常前进
- **anti-pattern**: 静默忽略 hook_callback → CLI 等超时；错误地回 `subtype: "error"` → Claude 把 hook 判为失败、整轮 abort
- **severity**: medium
