# BYOK Coding Plan Presets

验证 Cteno BYOK 设置页的 Coding Plan 入口能把一个套餐 SK 转成多模型本地 profile，并保持 Cteno agent 运行时仍走 direct profile 解析。

## meta
- kind: persona-chat
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-byok-coding-plan
- max-turns: 8

## setup

```bash
rm -rf /tmp/cteno-byok-coding-plan
mkdir -p /tmp/cteno-byok-coding-plan
printf '{"name":"cteno-byok-coding-plan"}\n' > /tmp/cteno-byok-coding-plan/package.json
```

## cases

### [pending] 百炼 SK creates all supported profiles
- **message**: "在设置页添加百炼 Coding Plan 的 sk-sp 测试 key，然后列出 Cteno 模型列表。"
- **expect**: 创建 Qwen/Kimi/GLM/MiniMax 多个 `coding-plan-bailian-*` profiles；默认 profile 是 `qwen3.5-plus`；国际区 base URL 是 `https://coding-intl.dashscope.aliyuncs.com/apps/anthropic`，中国区可切到 `https://coding.dashscope.aliyuncs.com/apps/anthropic`。
- **anti-pattern**: 只创建一个模型；使用普通 DashScope endpoint；默认仍停留在旧 proxy/free profile。
- **severity**: high

### [pending] MiniMax region does not append duplicate /v1
- **message**: "添加 MiniMax Token Plan，选择 intl 区域，然后用 MiniMax-M2.7 发起一个 Cteno 对话。"
- **expect**: Cteno runtime 的 effective URL 是 `https://api.minimax.io/anthropic/v1/messages`；请求使用用户 SK；错误时也能看到 provider 返回的可读错误。
- **anti-pattern**: URL 变成 `/anthropic/v1/v1/messages`；错误路由到 OpenAI Responses API；未登录时偷偷 fallback 到 proxy。
- **severity**: high

### [pending] Re-adding same provider updates key without duplicating profiles
- **message**: "同一台机器连续添加两次 GLM Coding Plan，第二次换一个测试 SK。"
- **expect**: `coding-plan-glm-*` profile 数量保持稳定；base URL 是 `https://api.z.ai/api/anthropic`；masked key 变成第二次 SK；默认 profile 更新为 `coding-plan-glm-GLM-5.1`。
- **anti-pattern**: 同模型出现重复 profile；旧 SK 仍被保留；默认值没有切到新套餐。
- **severity**: medium

### [pending] Editing a generated profile preserves thinking/capability flags
- **message**: "打开 qwen3.5-plus Coding Plan profile，只改 profile 名字后保存，再启动一次该模型会话。"
- **expect**: 保存后 `thinking=true`、`supports_function_calling=true`、vision/context metadata 不丢；运行时仍启用 thinking 并可正常 tool use。
- **anti-pattern**: 编辑保存把隐藏字段清掉；模型不能再工具调用；上下文窗口退回未知默认。
- **severity**: high

### [pending] Kimi Code gated endpoint gives readable error
- **message**: "用无效或未授权的 Kimi Code SK 添加 profile，并发起一句简单 Cteno 对话。"
- **expect**: UI 出现可读的 auth/plan 错误；不 panic；不泄漏完整 SK；session 不会回落到默认模型假装成功。
- **anti-pattern**: 空白回复；panic；完整 key 进入日志或消息；自动 fallback 到 proxy/free profile。
- **severity**: high

### [pending] Dispatch QA Agent
- **message**: "写完代码和本文件后，spawn cteno-qa 执行 tests/eval/byok-coding-plan.md。"
- **expect**: QA agent 执行本文件相关用例、更新状态标记并输出报告；若出现 fail/flaky，代码 agent 修复后重新 dispatch QA 验证。
- **anti-pattern**: QA agent 临时发明新用例；代码 agent 未修复 fail/flaky 就结束；状态标记不更新。
- **severity**: high
