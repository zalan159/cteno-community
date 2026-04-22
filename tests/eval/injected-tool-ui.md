# Injected Host Tool UI

验证 host runtime 注入的工具调用是否沿统一消息流落到前端，并在现有 tool-call UI 上显示 `Host Tool` 标识。

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-injected-tool-ui
- max-turns: 15

## setup

```bash
rm -rf /tmp/cteno-injected-tool-ui
mkdir -p /tmp/cteno-injected-tool-ui
printf 'host tool eval fixture\n' > /tmp/cteno-injected-tool-ui/context.txt
```

## cases

### [skip: 2026-04-18 QA 仍需 live Cteno UI runtime；当前 daemon/webview 不可用] dispatch_task 注入调用显示为标准 tool-call 且带 Host Tool badge
- **message**: 在支持 host orchestration 的 session 中触发一次 `dispatch_task`，例如“把 /tmp/cteno-injected-tool-ui/context.txt 总结后交给 reviewer 子任务处理”
- **expect**: 对话流里出现标准 tool-call 卡片，tool name 为 `dispatch_task`；卡片使用现有 tool-call 组件样式而不是专用消息样式；标题区显示 `Host Tool` badge；当工具本身没有更具体副标题时，副标题显示 `Triggered by host runtime`
- **anti-pattern**: 只在日志里看到 injected event；前端不落消息；渲染成普通文本而非 tool-call；缺少 `Host Tool` badge
- **severity**: high

### [skip: 2026-04-18 QA 仍需 live Cteno UI runtime；当前 daemon/webview 不可用] host tool 完成后产生 tool-result 并关闭同一调用卡片
- **message**: 等待上一条 `dispatch_task` 执行完成，或触发一条可快速结束的 host-owned `dispatch_task`
- **expect**: 同一调用链后续出现 `tool-result`；结果挂在同一个 call 上并让运行中的 tool-call 卡片结束；结果内容能看到 host tool 的返回摘要或成功信息
- **anti-pattern**: tool-call 一直停在运行中；结果变成独立文本消息；tool-result 丢失 call 关联；同一次 host tool 被渲染成两张无关卡片
- **severity**: high

### [skip: 2026-04-18 QA 仍需 live Cteno UI runtime；当前 daemon/webview 不可用] ask_persona 等其他 host-owned 工具同样带 Host Tool 标识
- **message**: 在 task session 中触发一次 `ask_persona`，例如“让 reviewer persona 只回答这份总结是否完整”
- **expect**: `ask_persona` 沿同一 tool-call UI 渲染；标题区同样显示 `Host Tool` badge；能和普通 agent 自主发起的工具调用区分来源，但不引入第二套展示组件
- **anti-pattern**: 只有 `dispatch_task` 有 badge，`ask_persona` 回退成普通 tool-call；为了 host tool 单独渲染了另一套消息组件
- **severity**: medium

### [skip: 2026-04-18 QA 仍需 live Cteno UI runtime；当前 daemon/webview 不可用] 普通非 host-owned tool-call 不误显示 Host Tool badge
- **message**: 在同一 session 再触发一个普通工具调用，例如读 `/tmp/cteno-injected-tool-ui/context.txt`
- **expect**: 普通工具仍按原样展示，不出现 `Host Tool` badge，也不会平白补上 `Triggered by host runtime`
- **anti-pattern**: badge 判定过宽，普通 Read/Bash/Write 也被标成 host tool；副标题被错误覆盖
- **severity**: medium
