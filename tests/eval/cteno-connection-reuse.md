# Cteno connection-reuse (Phase 1)

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 4

## context

Phase 1 of the vendor pre-connection refactor switched the Cteno adapter
(`multi-agent-runtime-cteno`) from one-subprocess-per-session to
one-subprocess-per-connection, multiplexing multiple sessions onto the
same `cteno-agent` child process via `session_id` routing. These cases
verify the observable host-side effects.

See:
- `docs/cteno-p1-protocol-findings.md` — the protocol contract validated
  by manual CLI probing
- `packages/multi-agent-runtime/rust/crates/multi-agent-runtime-cteno/tests/integration_connection_reuse.rs`
  — the Rust integration tests the QA agent mirrors at the end-to-end
  level

## cases

### [pending] Two concurrent Cteno sessions on one host don't cross-talk
- **message**: "打开两个新的 Cteno session（sid=A, sid=B），分别发一条互相没关联的消息：A 让它 echo 字符串 'alpha'；B 让它 echo 字符串 'bravo'。观察两个 session 的回复。"
- **expect**: A 只返回带 'alpha' 的内容、B 只返回带 'bravo' 的内容；两个 session 的 TurnComplete 各自独立到达；没有互相串流（B 的回复里出现 alpha 或反之算失败）。
- **anti-pattern**: 两个 session 的 delta 交替混入同一个 session 的流；A 的 TurnComplete 之后 B 也立刻随同完成（证明它们在同一个 session 里）；B 收到了 "already in progress" 的 session busy 错误（说明两个 session 撞到同一个 SessionHandle 上）。
- **severity**: high

### [pending] 杀掉 subprocess 的过程中两个 session 都能可恢复地报错
- **message**: "在 A 和 B 两个 session 都跑起来后，从另一个 shell 里 `kill -9` 掉 cteno-agent 进程。两个 session 的当前回合应该都收到一个 fatal error（recoverable=false），而不是有一个继续等 Ready 等到超时。"
- **expect**: A 和 B 都能在 2 秒内拿到一个 Error 事件；错误消息里包含 'cteno-agent' 字样或 'subprocess' / 'connection' / 'exited' 这类线索；两个 session 的 Error 事件 timestamps 相差不超过 500ms（证明广播发生，不是各自独立地超时）。
- **anti-pattern**: 其中一个 session 收到 error，另一个继续 hang 到 turn_timeout（600s）；host 把 error 当成了 recoverable=true 并尝试自动重启；subprocess supervisor 没有清理 pid。
- **severity**: high

### [pending] supports_multi_session_per_process 能通过 list_available_vendors RPC 暴露为 true
- **message**: "从前端（或者直接调 RPC）调用 list_available_vendors 或 get_vendor_capabilities，检查 cteno 这一行的 supports_multi_session_per_process 字段。"
- **expect**: 返回的 cteno capability 里 `supports_multi_session_per_process == true`；其它 capability 字段（例如 supports_runtime_set_model, supports_permission_closure）保持原值不变。
- **anti-pattern**: 返回 false（说明 capability flip 没生效到 RPC 层）；返回 undefined / 字段缺失（说明 RPC schema 没 pick up 新字段）；其它 capability 被误改。
- **severity**: medium

### [pending] close_session 一个 session 不影响另一个共享 connection 的 session
- **message**: "打开两个 session A、B 挂在同一个 connection 上（通过 AgentExecutor::start_session_on），A 先发一条消息跑完一个回合；紧接着 close_session A；B 再发一条消息。"
- **expect**: A close 后 B 仍然能正常收到 Ready / Delta / TurnComplete，没有 connection dead 错误；subprocess 没有退出（pid 仍然存在）；只有 A 的 event_rx 被 drop。
- **anti-pattern**: close_session A 把整个 subprocess 杀了导致 B 的回合立刻 error；B 的消息收到 "session not found" 但实际上 B 没被 close；connection check 返回 Dead。
- **severity**: high

### [pending] Cteno turn timeout 显示可重试错误并释放同一 session
- **message**: "把 Cteno adapter 的 turn_timeout 临时设成 1 秒，用会触发长时间等待的 Cteno 回合复现 timeout；看到错误后，立刻在同一个 session 发送第二条短消息 'echo retry-ok'。"
- **expect**: 第一轮在前端瞬态提示区出现 `cteno-agent response timed out after 1s`，并带有可重试提示；刷新/重载消息列表后这条 timeout 不作为聊天记录出现；同一轮随后写入 task_complete，界面停止 thinking；第二轮不会收到 `a turn is already in progress for this session`，并能正常完成。
- **anti-pattern**: 超时只出现在 daemon stderr 或 Last stderr 泄漏一整段 tool schema；timeout 被持久化成 agent 聊天气泡；界面一直转圈没有 task_complete；第二轮被 busy 错误拒绝；同一个 timeout 产生两条重复错误气泡。
- **severity**: high

### [pending] 未知 session_id 的 outbound 帧不会 panic 或打断其他 session
- **message**: "向一个 Cteno connection 上注入一条伪造的 outbound 帧，session_id 指向一个从未 register 过的 UUID。连着跑几个合法 session 的请求，观察 demuxer 是否稳定。"
- **expect**: 伪造帧被 silently drop（stderr 只打 warn），合法 session 的 delta / toolcall 流不受干扰；没有 panic / 没有 connection 级别的 Error 事件漏出去。
- **anti-pattern**: demuxer task 退出导致整个 connection 连锁死亡；某个合法 session 收到了那条伪造 payload；panic 被 tokio runtime 吞掉但 subprocess 被 kill。
- **severity**: medium

### [pending] Token refresh 广播同时覆盖 legacy 和 connection-backed session
- **message**: "开 session C（通过 spawn_session 走新 connection 路径），再 resume 一个 legacy session D（走旧 one-child-per-session 路径）。然后触发一次 `AuthStore::set_tokens` → `broadcast_token_refresh`，确认 C 和 D 都收到 token rotation。"
- **expect**: C 侧 cteno-agent 进程的 stderr 里有 "access token rotated" 日志；D 侧同样；后续 C 和 D 发起任何需要 auth 的 cloud 调用不再返回 401。
- **anti-pattern**: 只有其中一路看到 token rotation（说明 broadcast 漏了某个 backing 类型）；某一路 refresh 失败把 session 标记 dead 但另一路被跳过；广播到 connection 时写入 stdin 阻塞导致其他 session 的流被卡住。
- **severity**: medium
