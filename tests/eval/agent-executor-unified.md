# AgentExecutor Unified — 三家 vendor 跨 executor 契约测试

验证 Cteno / Claude / Codex 三家 AgentExecutor 实现行为对齐，覆盖 capability 不一致、生命周期边界、错误降级。

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-executor-unified
- max-turns: 15

## setup

```bash
mkdir -p /tmp/cteno-executor-unified
# 测试不依赖真实 Claude/Codex CLI（用 stub fixtures），但需要 cteno-agent binary 在 PATH 或 CTENO_AGENT_PATH
```

## cases

### [pending] cross-vendor interrupt race（Claude）
- **message**: 给 ClaudeAgentExecutor session 发 prompt 触发 30s bash sleep，500ms 后 interrupt
- **expect**: ToolResult Err 在 2s 内到，session 存活可继续 send_message
- **anti-pattern**: subprocess kill 过快 / interrupt 让 session 不可用
- **severity**: high

### [pending] Cteno permission-closure denial 不让 session 崩
- **message**: spawn Cteno session in Default mode → 触发 write tool → 收 PermissionRequest → respond_to_permission(Deny)
- **expect**: ToolResult Err("permission denied") 透传给 ReAct，turn 继续
- **anti-pattern**: agent panic / session 不可恢复
- **severity**: high

### [pending] Claude capabilities() 与实际行为一致
- **message**: ClaudeAgentExecutor::capabilities() supports_list_sessions=true → 跑 list_sessions
- **expect**: 5s 内返回 Vec<SessionMeta>（即使 SessionStore 空也返回空 Vec 而非 error）
- **anti-pattern**: 报 Unsupported / 超时 / 返回非空但都是 stale entries
- **severity**: medium

### [pending] Claude native resume miss 不挂死空会话
- **message**: 创建 Claude persona session，profile_id=haiku，持久化一个不存在于 Claude CLI 本地历史中的 native_session_id，messages 保持空数组；打开会话并发送“回复 ok”
- **expect**: 连接恢复 10s 内完成并重新生成 native_session_id，用户消息进入队列，最终收到 assistant 回复
- **anti-pattern**: 日志只停在 `Resuming executor-backed session...` / `No handler registered ...:bash` 循环 / 无用户消息持久化
- **severity**: high

### [pending] Orphan subprocess sweep on daemon restart
- **message**: spawn 2 sessions（Cteno + Codex）→ std::mem::forget executor handles 模拟 daemon crash → 重启 ExecutorRegistry → SubprocessSupervisor::new 触发 sweep
- **expect**: 3s 内两个 child 都被 SIGTERM 杀掉（pgrep cteno-agent / codex 查不到）
- **anti-pattern**: zombie 残留 / pid 文件不清
- **severity**: high

### [pending] Normalizer 收到未知 NativeEvent 不 abort turn
- **message**: 注入合成 ExecutorEvent::NativeEvent { provider: "codex", payload: {"type": "future_event_kind_x"} } 给 ExecutorNormalizer
- **expect**: Ok(()) 返回，无 ACP 消息发出，warn 级别 log
- **anti-pattern**: panic / abort turn / fail send_message stream
- **severity**: medium
