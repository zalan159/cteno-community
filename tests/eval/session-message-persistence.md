# Session Message Persistence (Local Mode)

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-session-persistence
- max-turns: 8

## 背景

P0 修复：本地模式下 user 消息此前从未写入本地 `agent_sessions.messages`，重启后读回只有 assistant。修复把 `persist_local_user_message` 插在 4 个 `executor.send_message` 入口前。

这些用例全部围绕"重启不丢 user"主题，禁止 happy path — 每个都包含一个刁难点（首轮、连续多轮、中文 unicode、vendor 切换、空文本、并发写、登录后模式 gate）。

## setup

```bash
mkdir -p /tmp/cteno-session-persistence
# 清掉可能残留的历史 DB，避免被旧行污染
rm -f /tmp/cteno-session-persistence/cteno.db
```

## cases

### [pending] 首轮对话后立即关进程 → 重启可见 user
- **message**: "用 ctenoctl 在 local 模式起一个新 claude session，发一句 `ping`，等 assistant 回复完；立即 kill daemon；重启后用 `get_session_messages` RPC 查该 session，断言消息列表第一条 role=user content=`ping`"
- **expect**: 重启后消息列表首条就是 user ping；assistant 紧随其后
- **anti-pattern**: 首条就是 assistant；user 整体缺失；user local_id 丢失导致前端重复展示
- **severity**: high

### [pending] 连续三轮 user / assistant 顺序保留
- **message**: "同一 session 连发三轮，每轮 user 消息内容唯一（`q1` / `q2` / `q3`）；kill daemon；重启后读消息列表，断言 user 三条都在且顺序与发送一致"
- **expect**: 读出的消息序列是 `[user:q1, assistant:..., user:q2, assistant:..., user:q3, assistant:...]`（allowed tool_use/result 事件穿插但 user 三条 role+content 齐全且顺序对）
- **anti-pattern**: user 只剩最后一条；user 顺序错乱；某条 user 被 assistant 覆盖
- **severity**: high

### [pending] 中文 + 多行 + emoji 的 user 文本完整写入
- **message**: "发一条 user 消息内容为 `你好\n世界 🎉 line3`（包含换行/中文/emoji），kill daemon；重启后读回，断言 content 字符串字节序列完全相等"
- **expect**: 读回的 content 与发送时严格相等（包括换行符和 emoji 码位）
- **anti-pattern**: content 被截断；换行被 escape 成 `\\n` 字面量；emoji 被替换成 `?`
- **severity**: medium

### [pending] Vendor 混用：历史是 codex，新 user 走 claude
- **message**: "先用 codex vendor 在 `sess-X` 写一条 assistant 消息（通过 `append_local_session_message` 测试钩或 DB 直写）；再用 claude executor 对 `sess-X` 发 user；kill daemon；重启后读该 session：断言 `vendor=claude`、历史 codex assistant 消息仍在、新 user 紧跟其后"
- **expect**: vendor 列翻转到 claude；messages 数组长度为 2；顺序 [assistant, user]；历史 assistant 的 content 未被修改
- **anti-pattern**: 历史 assistant 被清空；vendor 仍是 codex；新 user 丢失；messages 数组被重建为 `[]`
- **severity**: high

### [pending] 空字符串 user 不引发 sql 异常但写入一条空行
- **message**: "发一条空字符串 user 消息（前端允许空发送的场景，例如只有附件的提交），kill daemon；重启后读回"
- **expect**: messages 数组长度 +1；role=user；content=`""`（不 null、不 absent）；无 sqlite 报错日志
- **anti-pattern**: sqlite bind parameter 报错；content 被替换成 "null" 字符串；write 被静默跳过
- **severity**: medium

### [pending] 并发两次 send_message 的写入不会互相覆盖（race P1 兜底）
- **message**: "同一 session 几乎同时并发发起两个 `executor.send_message`（一个走 agent_rpc_handler，一个走 multi_agent workspace role），用 tokio::join!；等两个 turn 结束；kill daemon；重启后读回，断言两个 user 消息都存在"
- **expect**: 读回的 messages 数组同时包含两个 user prompt（不要求顺序严格，允许 tool_result 穿插）
- **anti-pattern**: 只剩一个 user（全量覆盖式 update_messages 的经典 race）；DB 行被删除；messages JSON 损坏无法反序列化
- **severity**: high

### [pending] 登录后模式（socket.is_local=false）不在本地 DB 重复写 user
- **message**: "伪造 HappySocket::is_local() 返回 false（或用集成测试的 mock socket）；触发 `ExecutorNormalizer::persist_user_message` 一次；断言本地 DB 里该 session 的 messages 数组长度保持为 0（因为登录模式真相源在服务器，不双写本地）"
- **expect**: 本地 messages 长度为 0；函数返回 Ok(())；无报错
- **anti-pattern**: 仍然写了本地 DB（会导致登录模式重启后本地 cache 与服务端拉取结果重复）；函数 panic
- **severity**: medium

### [pending] persist_user_message 返回错误时不吞掉、传给调用方
- **message**: "把 DB 路径设为只读目录（或用 `/dev/null/foo.db` 等必失败路径），触发 user 入口；断言调用方收到 `persist user message failed: ...` 错误，且 `executor.send_message` 没有被调用"
- **expect**: 错误向上传播到 RPC 层；`executor.send_message` 不被触发（日志里看不到 vendor 子进程启动 / 相应 trace）
- **anti-pattern**: persist 失败被 `let _ = ...` 静默；send_message 仍旧执行导致用户只看到 assistant 回复但本地历史丢了 user
- **severity**: high

### [pending] streaming 中 reload 历史不清当前临时文本
- **message**: "本地 cteno session 第 2 轮开始后先收到 `text-delta: line1\nline2\n`，随后因为 tool-call 持久化触发 `reloadSessionMessages`，返回的最近页包含上一轮已完成的 ACP `message` / `task_complete`；继续收到 `text-delta: line3\n`"
- **expect**: reload 后 footer streaming bubble 仍显示 `line1\nline2\n`，继续追加后显示 `line1\nline2\nline3\n`；最终落地 block 与 streaming 内容一致
- **anti-pattern**: 重放上一轮 ACP `message` / `task_complete` 把当前 `streamingText` 清空，只剩 `line3`; 最终 block 正常但流式期间丢前几行
- **severity**: high
