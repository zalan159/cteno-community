# Cteno Stdio ACP Transparent Channel

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-stdio-acp
- max-turns: 15

## setup
```bash
mkdir -p /tmp/cteno-stdio-acp
printf 'alpha beta gamma\n' > /tmp/cteno-stdio-acp/notes.txt
```

## cases

### [pending] ACP persisted frame 不被二次翻译
- **message**: "用 Cteno session 读取 notes.txt，然后说明读取结果；过程中需要产生 thinking、tool-call、tool-result。"
- **expect**: 本地 DB 中保留 runtime/stdout 发出的原始 ACP `thinking` / `tool-call` / `tool-result` payload，包含原始 id/name/input/output 字段；adapter 只记录 `NativeEvent(provider=cteno, kind=acp)` 透明通道。
- **anti-pattern**: DB 中只出现 adapter 重建出的 `ToolCallStart` / `ToolResult` 形状；`callId`、`name`、`input` 或 vendor 扩展字段丢失；同一工具调用有两套互相不一致 payload。
- **severity**: high

### [pending] transient text delta 走 ACP 透明通道
- **message**: "连续输出三小段文本，每段前都写一个不同短标题，不要调用工具。"
- **expect**: UI stream callback 收到多个 ACP `text-delta` transient frame；最终 persisted assistant message 完整且只出现一次；`TurnComplete.final_text` 为空或不参与最终拼接。
- **anti-pattern**: adapter 自己累计 `final_text` 导致重复段落；transient 只显示在日志而不进 UI stream；最终消息缺段。
- **severity**: high

### [pending] task_complete 不重复
- **message**: "正常完成一个 Cteno turn；若 stdio 已发 persisted `task_complete`，再等待 TurnComplete。"
- **expect**: 每个 `taskId` 最终只有一条 persisted ACP `task_complete`；前端 loading 状态只结束一次。
- **anti-pattern**: stdio 和 normalizer 各写一条 `task_complete`；刷新后看到两条完成 side-effect；UI completion callback 触发两次。
- **severity**: high

### [pending] host_call_request 不悬挂
- **message**: "触发一个依赖 runtime host hook 的 Cteno 调用，例如调用未安装宿主 handler 的 hook 方法。"
- **expect**: adapter 立即回 `HostCallResponse { ok:false }`，runtime 收到明确错误并结束 turn；日志中能看到 hook/method/request_id 对应关系。
- **anti-pattern**: `HostCallRequest` 只以 NativeEvent 形式 log/drop；runtime 等待到超时；本轮一直 running。
- **severity**: high

### [pending] Cteno 图片附件进入 runtime user_images
- **message**: "向 Cteno session 发送一张图片附件和一段文字，让模型描述图片并忽略旁边的纯文本附件。"
- **expect**: stdio inbound `attachments` 中的图片被转成 runtime `user_images`，模型能基于图片内容回答；非图片附件不会伪装成图片。
- **anti-pattern**: adapter 丢弃 attachments；图片被当成普通文本路径；runtime 没收到 `user_images`。
- **severity**: high

### [pending] session MCP 工具关闭后清理且不污染新 session
- **message**: "打开两个 Cteno stdio session：A 的 workdir 配置一个项目 MCP server，B 不配置；关闭 A 后继续让 B 列出可用工具并尝试调用 A 的 MCP tool。"
- **expect**: A close 时 adapter 发送 `close_session`，stdio 清理 A 的 MCP registry；若没有其他 active session 使用同名 MCP tool，全局 tool registry 中该 tool 被注销；B 不会在工具列表看到 A 的项目 MCP tool。
- **anti-pattern**: A 关闭后 B 仍看到或可调用 A 的 MCP tool；同名 MCP tool 注册被后启动 session 静默覆盖成错误 server；关闭 A 时让仍 active 的同名 MCP tool 从其他 session 消失。
- **severity**: high
