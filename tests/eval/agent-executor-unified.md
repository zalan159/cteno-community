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

### [pending] Claude subagent sidechain 挂回 Task 容器
- **message**: 用 Claude fixture 顺序注入 Task tool-call（callId=toolu_parent）、带 `parent_tool_use_id=toolu_parent` 的 assistant text、sidechain Bash tool-call/result、task_notification；刷新会话消息列表
- **expect**: 前端 reducer 把 sidechain text 和子 tool-call 放进同一个 Task message.children，Task 卡片展开显示 subagent 文本和工具状态，顶层 chat 不重复出现这些子消息
- **anti-pattern**: subagent 消息散落为顶层 agent-text / Task 卡片只显示最终 result / `parent_tool_use_id` 被丢弃导致 children 为空
- **severity**: high

### [pending] Codex subagent result 不散落到顶层
- **message**: 用 Codex fixture 注入 Task/Agent tool-call（callId=codex_task_1）后返回 ToolResult("child agent summary")，再刷新会话消息列表和 Task 卡片
- **expect**: normalizer 生成 parentToolUseId=codex_task_1 的 sidechain message，前端 reducer 挂到 Task message.children，Task 卡片显示 child agent summary，同时原 tool-result 仍闭合父 tool
- **anti-pattern**: child agent summary 只作为顶层 assistant 文本出现 / Task 卡片空白 / tool-result 丢失导致 Task 一直 running
- **severity**: high

### [pending] Gemini subagent result 大小写兼容
- **message**: 用 Gemini fixture 注入小写 `agent` tool-call（callId=gemini_agent_1，input 含 prompt），随后 ToolResult("gemini child done")
- **expect**: `agent` 被识别为 subagent 类工具，ToolResult 生成 sidechain message 并挂回父工具容器，不依赖工具名必须精确等于 `Agent`
- **anti-pattern**: 小写 `agent` 被当普通工具处理 / sidechain parentToolUseId 缺失 / 子结果变成顶层消息
- **severity**: medium

### [pending] Vendor shell 后台任务事件可见
- **message**: 分别用 Codex/Gemini fixture 注入 Bash/execute tool-call（命令为 `sleep 30 && echo ok`），再注入完成或失败 ToolResult
- **expect**: normalizer 在 tool-call start 时写入 BackgroundTaskRecord(status=Running, vendor 保持 codex/gemini)，tool-result 后更新为 Completed/Failed，并通过 BackgroundTaskUpdated host event 推到前端后台任务容器
- **anti-pattern**: 后台任务列表完全无记录 / vendor 被写成 claude / 完成后仍 running / shell output 只在普通 tool-result 中出现而后台容器不更新
- **severity**: high

### [pending] Cteno dispatch_task DAG 事件挂回父工具
- **message**: 用 Cteno fixture 注入 ACP `dispatch_task` tool-call（callId=cteno_dispatch_1），tool-result 返回 `{"group_id":"group-1"}`，随后注入 ACP native_event `task_graph.node_completed` 与 `task_graph.completed`
- **expect**: normalizer 将 group-1 关联回 cteno_dispatch_1，node/completed 事件生成 parentToolUseId=cteno_dispatch_1 的 sidechain message，Dispatch Task 卡片内显示节点结果，后台任务从 Running 变为 Completed/Failed
- **anti-pattern**: task_graph native_event 原样散落为未知消息 / Dispatch Task 卡片没有子结果 / dispatch_task 工具结果一返回后台任务就被提前标记 Completed
- **severity**: high

### [pending] Cteno shell ACP 也进入后台任务容器
- **message**: 用 Cteno fixture 注入 ACP Bash tool-call（callId=cteno_shell_1，input.command=`sleep 30`），再注入 ACP tool-result 成功和失败两组
- **expect**: 即使 Cteno 走 ACP 透明通道而不是 ExecutorEvent::ToolCallStart，normalizer 仍写入 vendor=cteno 的 BackgroundTaskRecord，并在 tool-result 后更新 Completed/Failed
- **anti-pattern**: 只有 Claude/Codex/Gemini shell 有后台记录，Cteno ACP shell 没有 / 完成状态不同步 / callId 丢失导致无法更新同一条记录
- **severity**: high
