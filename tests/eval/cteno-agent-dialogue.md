# Cteno Agent Dialogue Regression

覆盖 Cteno persona 对话链路的回归场景，重点验证未登录、vendor 路由、spawn/interrupt、profile 透传和错误冒泡。禁止 happy path；每条用例至少包含一个跨状态、配置错误或并发竞态的刁难点。

## meta
- kind: persona-chat
- profile: proxy-deepseek-reasoner
- profile-unauth: default / user-byok
- workdir: /tmp/cteno-test
- max-turns: 6

## setup

```bash
mkdir -p /tmp/cteno-test
printf '{"name":"cteno-dialogue-eval"}\n' > /tmp/cteno-test/package.json
```

## cases

### [skip: requires live daemon] Cteno persona 未登录首次对话收到可读响应（不是空气）
- **message**: `你好，介绍一下你自己`
- **setup**: 未登录态启动 Cteno persona；不注入任何 Happy access token，也不设置任何 Cteno API key；保持默认 proxy profile，复现历史 bug 的无鉴权首轮对话。
- **expect**: 至少收到一条 assistant `StreamDelta` 文本响应，或收到可读的“需要登录/配置 API key”错误消息；无论哪条路径，都必须有可见文本落到会话里。
- **anti-pattern**: 前端一直停在 thinking；`final_text` 为空字符串且没有 error；subprocess 静默退出导致空白会话。
- **刁难点**: 没登录态 + 没设任何环境 API key + 默认 proxy profile，命中最容易“空气响应”的组合。
- **severity**: high

### [pass] Cteno persona dispatch_task 必须走 Cteno vendor 不是 Claude
- **message**: `帮我 dispatch 一个子任务：读取当前目录 package.json`
- **setup**: 当前 persona 配置 `agent=cteno`，并保证 `/tmp/cteno-test/package.json` 存在；观察 parent session 触发 `dispatch_task` 后创建的 child session 元数据和工具来源。
- **expect**: 子 session 的 `vendor` 字段为 `cteno`；工具调用来自 `cteno-agent` 的 ReAct 循环，而不是 Claude adapter。
- **anti-pattern**: 子 session `vendor=claude`；dispatch 回调绕开 persona.agent；tool 调用链落到 Claude CLI。
- **刁难点**: persona.agent 已明确是 `cteno`，但历史回调可能仍硬编码 `claude`。
- **severity**: high

### [skip: requires live daemon] spawn 时未登录、对话中登录的过渡态
- **message**: `现在请调用一次 proxy profile 的 LLM 回复一句话`
- **setup**: 先在未登录状态下 spawn Cteno persona 并等待 subprocess `Ready`；随后 mock `auth_store.set_tokens` 推送一个有效 `access_token`，再继续当前对话，不重新建 session。
- **expect**: `send_message` 不再报 `auth_token not available`；subprocess 收到 `TokenRefreshed`；后续请求能继续成功返回一句可读文本。
- **anti-pattern**: 仍报旧的 auth 缺失错误；token 已写入但 subprocess 没刷新；子进程在过渡态中直接死掉。
- **刁难点**: 跨登录边界保持同一条 stream/session，不允许靠重开会话掩盖问题。
- **severity**: medium

### [pass] subprocess 启动失败（binary 路径错）友好报错
- **message**: `创建一个新的 Cteno persona 会话并回复 hi`
- **setup**: 将 `CTENO_AGENT_PATH` 设为不存在的路径，再走一次 Cteno persona spawn；模拟 CI/打包漏带 `cteno-agent` binary。
- **expect**: `spawn_session` 返回明确 `Err`；前端展示语义等价于 `会话启动失败：cteno-agent binary not found at ...` 的错误气泡。
- **anti-pattern**: panic；前端静默无提示；只留下空对话或永远 loading。
- **刁难点**: 配置错误发生在启动前，要求 adapter 把底层 spawn 失败转成用户可读错误。
- **severity**: medium

### [skip: requires live daemon] Abort 正在进行的 Cteno turn
- **message**: `从 1 数到 100，每个数字单独一行`
- **setup**: 发出消息后等待第一个 `StreamDelta` 到达，立刻调用 `executor.interrupt`；记录 abort 发生时 turn 仍在流式输出中的竞态。
- **expect**: subprocess `abort_flag` 被置位；turn 尽快结束并返回 `TurnComplete` 或可恢复的 `Error`；中断后 session 仍可继续使用。
- **anti-pattern**: subprocess 继续完整输出到 100；interrupt 无响应；中断直接把整个 session 弄坏。
- **刁难点**: interrupt 与流式输出抢时序，必须验证真正中断当前 turn 而不是只改 UI 状态。
- **severity**: medium

### [skip: requires live daemon] 切换 vendor 热 spawn：Claude session 存在时新建 Cteno session 不互相污染
- **message**: `先让 Claude 回一句，再新建一个 Cteno 会话也回一句`
- **setup**: 先创建一个 Claude persona 完成一轮对话；保持该 session 存活，再并行或紧接着创建一个 Cteno persona 并完成一轮；检查 registry、session id 和工具路由。
- **expect**: 两个 subprocess 独立存在；`executor_registry.resolve` 路由正确；各自事件只落回对应 vendor/session。
- **anti-pattern**: session id 交叉；tool call 路由到错误 vendor；新建 Cteno session 时污染已有 Claude session 状态。
- **刁难点**: 多 vendor 并发 session，共享同一个宿主 registry，最容易出现串线。
- **severity**: medium

### [pass] profile_id 透传：Cteno session 使用指定的 user-* profile 而非 fallback 到 DEFAULT_PROXY_PROFILE
- **message**: `你用的是哪个模型？`
- **setup**: 在 profile store 中写入 `id='user-deepseek-byok'` 的 direct profile，并保证对应 API key 已配置；以该 profile 创建 Cteno session，观察传给 subprocess 的 `Init/agent_config`。
- **expect**: `agent_config.profile_id='user-deepseek-byok'` 被透传进入 `Init`；subprocess 按该 profile 走 BYOK/direct 链路，而不是默认 proxy fallback。
- **anti-pattern**: 落入 `DEFAULT_PROXY_PROFILE` 分支；`profile_id` 在 host 到 subprocess 之间丢失；实际使用的模型与指定 profile 不一致。
- **刁难点**: 验证 profile 透传，不允许未登录或默认值逻辑把显式 user profile 吞掉。
- **severity**: high

### [skip: requires live daemon] stderr 有 panic 行时 UI 能看到
- **message**: `触发一个会 panic 的测试工具并报告结果`
- **setup**: 在某个 `cteno-agent` tool executor 中故意注入 panic，或通过 mock/stub 让 subprocess stderr 打出 panic 行；观察 adapter 到 normalizer 再到 UI 的错误链路。
- **expect**: Cteno adapter 捕获 stderr 并发出 `ExecutorEvent::Error`；normalizer 持久化成 `{type: "error", message}` 会话消息；UI 能直接看到错误。
- **anti-pattern**: UI 静默；错误只进 log 不进会话；stderr panic 行被吞掉后 turn 卡住。
- **刁难点**: 错误走 stderr 而不是标准协议事件，要求 adapter 主动兜底。
- **severity**: medium
