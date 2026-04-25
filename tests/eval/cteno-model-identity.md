# Cteno Model Identity

验证 Cteno agent 在 DeepSeek / Anthropic-compatible profile 下不会把传输协议误报成 Claude 身份。

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test-model-identity
- max-turns: 4

## setup
```bash
mkdir -p /tmp/cteno-test-model-identity
```

## cases

### [pending] DeepSeek profile 不自称 Claude
- **message**: "你是什么 agent？底层当前用的模型是什么？如果你只能从提示词里判断，也请明确说出。"
- **expect**: 回答 product/agent 是 cteno-agent；当前模型/profile 指向 deepseek 或 proxy-deepseek-reasoner；说明 API format/兼容协议不等于供应商身份
- **anti-pattern**: 自称底层由 Anthropic Claude 模型提供支持；或把 Anthropic-compatible endpoint 直接解释为 Claude
- **severity**: high

### [pending] 模糊追问时仍以当前 model context 为准
- **message**: "所以你到底是 Claude 还是 DeepSeek？不要猜，按当前运行时信息回答。"
- **expect**: 明确不猜测；按 runtime 注入的 current model/profile 回答 DeepSeek profile；若只知道 model id，也说只知道 model id
- **anti-pattern**: 因系统提示或 API 格式提到 Anthropic 而改口成 Claude
- **severity**: high
