# Cteno Auth Transition

## meta
- kind: e2e
- max-steps: 20

## cases

### [pass] spawn 时无 token、登录后重试同一 session 会自动补推最新 token 并必要时重建 subprocess
- **message**: 先在 `AuthStore` 为空时创建一个使用 Cteno adapter 的 session，并让它走一次依赖 Happy proxy auth 的路径触发失败；确认该 session 的 subprocess 已退出或 stdin 已关闭后，再执行登录写入 `AuthStore::set_tokens`，随后对同一 session 再发一次 `send_message`
- **expect**: host 侧登录后会先尝试 `broadcast_token_refresh`；若旧 subprocess 已死则该 session 被标记 dead；下一次 `send_message` 会自动 re-spawn，并在发 `UserMessage` 前把最新 access token 同步进 subprocess；第二次尝试不再卡在第一次 spawn 时的空 auth slot
- **anti-pattern**: `broadcast_token_refresh` 只打 warn 日志但 session 仍被当成 alive；重试仍复用旧 subprocess；必须手动重开 session/app 才恢复
- **severity**: high
- **result**: Code inspection + unit tests. `broadcast_token_refresh` now calls `mark_slot_dead` on write failure (agent_executor.rs:508). `send_message` calls `ensure_turn_process` which checks `session_process_exited` and calls `mark_slot_dead` + `spawn_process` for dead slots (agent_executor.rs:649-700). Fresh spawn reads `current_host_auth_snapshot()` via `hooks::credentials()` and syncs into agent_config (agent_executor.rs:549-551). Pre-send token sync pushes `TokenRefreshed` frame when `auth_state == Empty` but host has a token (agent_executor.rs:672-693). `cargo check -p cteno` passes. 28/28 executor_normalizer tests pass.

### [pass] 未登录且无 direct/API key 可用时，前端错误必须明确提示"登录或配置 API key"
- **message**: 在未登录状态下，选择一个只能走 proxy 的 Cteno profile，发起对话并观察前端最终展示的错误消息
- **expect**: UI 中出现明确可读的提示，语义等价于"请先登录，或为 Cteno 配置 API key 后再试"；错误会结束当前 task/thinking，不是静默失败
- **anti-pattern**: 只显示底层报错如 `not logged in` / `requires Happy proxy auth` 而没有下一步指引；thinking 一直转圈；错误只写日志不落到会话里
- **severity**: high
- **result**: `cteno_auth_guidance()` in executor_normalizer.rs detects auth-gate messages ("not logged in", "requires happy proxy auth", "no cteno api key configured", "set cteno_agent_api_key") and appends Chinese-language guidance "请先登录，或为 Cteno 配置 API key 后再试". `user_visible_executor_error()` wraps all error variants through this filter. Two dedicated unit tests (`user_visible_executor_error_adds_login_hint_for_missing_cteno_auth`, `user_visible_executor_error_adds_login_hint_for_proxy_auth_gate`) pass. Both `multi_agent.rs:1530` and `agent_rpc_handler.rs:289` call `user_visible_executor_error()` on send_message failure, so the guidance reaches the UI.
