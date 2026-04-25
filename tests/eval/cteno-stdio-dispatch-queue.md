# Cteno session dispatch queue

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-stdio-dispatch-queue
- max-turns: 20

## setup
```bash
rm -rf /tmp/cteno-stdio-dispatch-queue
mkdir -p /tmp/cteno-stdio-dispatch-queue
```

## cases

### [pending] 忙碌 turn 期间用户消息排队
- **message**: "连续发送两条消息：第一条要求等待一个较慢工具调用后回复，第二条立即补充要求最终答案必须包含 queued-ok。"
- **expect**: 第二条消息进入 desktop 主 session queue；stdio 不维护独立 pending queue；第一轮结束后由同一个 queue worker 自动处理，最终回复包含 `queued-ok`。
- **anti-pattern**: stdio 自己缓存 pending turns；busy 时拒绝第二条用户消息；需要用户手动重发；第二条消息丢失。
- **severity**: high

### [pending] subagent 完成后 runtime 直接回注 ACP
- **message**: "调用 subagent，让它只回复 `测试成功`。不要用 query_subagent 轮询，等 subagent 完成后由父会话自动总结。"
- **expect**: stdio 注册 runtime SubAgent receiver；subagent 完成通知由 `cteno-agent-runtime/stdio` 直接发 persisted ACP message；desktop 只透明显示/落库，不反查 session、不 enqueue 内部消息；父会话可见 `[SubAgent ... Completed]` 且包含 `测试成功`。
- **anti-pattern**: subagent 完成但父会话无响应；只能手动 query；日志出现 `No registered session`；出现 `session_queue_message` 或 desktop 侧 `enqueue_internal_session_message`。
- **severity**: high

### [pending] DAG node/group completion 都由 runtime 直接回注 ACP
- **message**: "用 DAG 派发 a/b 两个 root 任务分别回复 A_OK/B_OK，c 依赖 a/b 并汇总。不要手动轮询子任务。"
- **expect**: a/b/c 每个节点完成都由 runtime/stdio 发 persisted ACP message 回灌父会话；a/b 完成后 c 自动启动；父会话收到 `[Task Complete]` 和最终 `[Task Group Complete]`；最终 summary 区分 completed/failed/blocked。
- **anti-pattern**: c 不启动；`[Task Group Complete]` 只出现在 runtime 日志不进入父会话；需要用户再发消息才继续；desktop normalizer 代替 runtime 推进 DAG。
- **severity**: high

### [pending] 默认工具面只暴露 dispatch_task
- **message**: "检查 Cteno native agent 的可用工具，然后派发一个 worker 子任务回复 READY。"
- **expect**: agent 使用 `dispatch_task`；默认工具面不包含 `start_subagent`、`query_subagent`、`stop_subagent`；任务完成后通过自动通知继续。
- **anti-pattern**: agent 调用 `start_subagent`；agent 调用 `query_subagent` 轮询；提示词或 tool_search 中仍把 subagent primitive 当成普通可用工具。
- **severity**: high
