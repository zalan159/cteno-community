# Codex connection reuse

Validates the Phase-1 pre-connection refactor for the Codex app-server
transport: one `codex app-server` subprocess should host N threads
(= N sessions) with thread-scoped interrupts, and the legacy
exec-fallback branch must still work for older Codex CLIs that lack
the `app-server` subcommand.

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-codex-reuse
- max-turns: 10

## setup
```bash
mkdir -p /tmp/cteno-codex-reuse
```

## cases

### [pending] Two codex sessions share one app-server subprocess
- **message**: "创建两条 codex session，分别在工作目录 /tmp/cteno-codex-reuse/w1 和 /tmp/cteno-codex-reuse/w2 下，各自发一条简单消息（如 'echo A' 与 'echo B'）。然后在宿主侧枚举 codex app-server 子进程，返回所有 PID 列表与两条 session 的 SessionRef process_handle。"
- **expect**: codex app-server 子进程只有 1 个 PID；两条 session 的 SessionRef 共享同一条底层连接；两条消息都收到 TurnComplete。
- **anti-pattern**: 出现两个 codex app-server PID（说明没走连接复用，退回到了 "每 session 一进程" 的旧模型）；或任何一条 session 收到 `SubprocessExited` / 连接关闭错误。
- **severity**: high

### [pending] Interrupt on thread A leaves thread B streaming
- **message**: "在同一个 codex 连接上启动两条 session A 和 B。让 A 发一个长耗时任务（例如 '从 1 数到 100，每个数字之间暂停'），随后让 B 发一条普通消息（'say hello from B'）。A 的第一个流式事件到达 200ms 后调用 interrupt(A)。报告 A 的 TurnComplete.status、B 的 TurnComplete.status。"
- **expect**: A 的 TurnComplete 带 status=interrupted（或 turn/completed 中 status 为 cancelled/aborted）；B 的 TurnComplete 正常完成（status=completed）；app-server 进程仍然存活（check_connection == Healthy）。
- **anti-pattern**: B 也被连带 interrupt；app-server 崩溃或 EOF；`handle_app_server_notification` 把 A 的 turn/completed 转发到了 B 的事件流。
- **severity**: high

### [pending] App-server crash propagates closure to all live threads
- **message**: "在同一个 codex 连接上启动两条 session A 和 B 并各发一条消息。两条消息流处于 in-flight 时，用 `kill -9 <app-server-pid>` 外部强杀进程。报告每条 session 的事件流最后一个事件的类型、以及 check_connection(handle) 的返回值。"
- **expect**: A 和 B 两条流都以 `Error { recoverable: false }` 或 `Protocol(\"connection closed\")` 错误终止（不 hang）；close_connection 之后再次 open_connection 能成功重连；check_connection 在 kill 之后返回 `Dead { reason: ... }`。
- **anti-pattern**: 只有一条 session 收到错误、另一条无限 hang；check_connection 仍然返回 Healthy；pending JSON-RPC oneshot 没有被 drain（内存泄漏）。
- **severity**: medium

### [pending] Preheat-then-long-idle: stale connection auto-reopens on first message
- **setup addendum**: 启动 daemon → 调用 `preheat_all` 让 registry 缓存 codex app-server handle → 在不发任何消息的前提下等待（真实场景中是 30+ 分钟；测试可通过 `kill -9 <codex-app-server-pid>` 人工模拟 child 死亡）。
- **message**: "在 codex 预热完成并且其 app-server 子进程被外部强杀之后，立即发送一条简单消息 `'echo after-idle'`。报告：(a) 宿主 warn 日志里是否出现 `ExecutorRegistry: cached codex handle is dead` 或 `start_session_on(codex) failed on cached handle ... retrying once`；(b) 最终 SessionRef.id 是否正常返回；(c) 收到的 TurnComplete.status。"
- **expect**: 宿主日志至少出现一条上述 warn（证明自愈分支被走到）；SessionRef 正常返回；TurnComplete.status = completed；`codex app-server` PID 变化成新进程（证明重开而非复用死 handle）。
- **anti-pattern**: 报错 `executor.spawn_session(codex) failed: codex app-server connection is closed; reopen before starting a session`（即用户原始 bug）；自愈连续触发 2 次以上（陷入重试循环）；日志显示用的仍然是旧 PID。
- **severity**: high

### [pending] Auto-reopen retries exactly once, surfaces persistent failure
- **setup addendum**: 把 `CODEX_PATH` 指向一个总是在 `thread/start` 之前退出的 fake codex binary（见 `integration_dead_handle_recovery.rs` 里的 python shim，扩展成 handshake 后退出）。
- **message**: "让 CodexAgentExecutor 用上述 fake codex 尝试创建一条 session 并发消息 `'hi'`。报告整条错误链：registry 观察到 dead handle 的次数、open_connection 的总调用次数、最终用户看到的错误字符串。"
- **expect**: registry 记录"dead handle"至少 1 次；open_connection 被调用 **正好 2 次**（原始 + 重试），不会循环重试 3 次及以上；最终错误冒泡到 `spawn_session` fallback 或以可读文案告知用户"app-server 不可用"。
- **anti-pattern**: 重试次数 > 1；registry 无限循环（CPU 占用上涨）；错误被吞没导致前端 UI 无任何反馈。
- **severity**: medium

### [pending] Exec-fallback path regression: legacy codex binary still usable
- **setup addendum**: 把 `CODEX_PATH` 指向一个 shell 脚本模拟的 codex，它在 `app-server --help` 时返回非零退出码，在 `exec --experimental-json` 模式下正常工作。
- **message**: "让 CodexAgentExecutor 使用上述模拟 codex 二进制，创建一条 session 并发一条消息，报告使用的 transport 类型（SessionTransport::AppServer 还是 SessionTransport::ExecFallback）、以及 SessionRef.id 的前缀。"
- **expect**: transport 是 ExecFallback；SessionRef.id 以 `codex-` 前缀开头（表示合成的 fallback id）；对应的 turn 完成并收到 TurnComplete；`app_server_available()` 返回 false。
- **anti-pattern**: `app_server_available()` 错误地返回 true；走到 AppServer 路径后崩溃；降级失败导致 session 无法创建。
- **severity**: high
