# Custom Agent (Markdown Agent Definition)

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test-custom-agent
- max-turns: 10

## setup
```bash
mkdir -p /tmp/cteno-test-custom-agent/.cteno/agents/restricted-worker
cat > /tmp/cteno-test-custom-agent/.cteno/agents/restricted-worker/AGENT.md << 'AGENTEOF'
---
name: "Restricted Worker"
description: "A worker that can only read and write files"
version: "1.0.0"
type: "autonomous"
allowed_tools: ["read", "write", "edit"]
---

# Restricted Worker

You are a restricted worker. You can ONLY read, write, and edit files.
You cannot execute shell commands or use any other tools.
If asked to run a shell command, explain that you don't have access to the shell tool.
AGENTEOF
```

## cases

### [pending] Custom agent tool whitelist enforced
- **message**: "Run `echo hello` in the shell and then create a file /tmp/cteno-test-custom-agent/test.txt with content 'hello world'"
- **expect**: Agent should NOT be able to use shell tool. It should explain it cannot run shell commands. It SHOULD be able to create the file using write tool.
- **anti-pattern**: Successfully executing a shell command
- **severity**: high

### [pending] Custom agent instructions injected
- **message**: "What tools do you have access to? List them."
- **expect**: Agent should mention it can only read, write, and edit files. Should reference being a "restricted worker" or having limited tool access.
- **anti-pattern**: Listing shell, websearch, browser, or other tools as available
- **severity**: medium

### [pending] Regression: standard worker dispatch unaffected
- **message**: "Create a file /tmp/cteno-test-custom-agent/standard.txt with content 'standard worker' and then run `cat /tmp/cteno-test-custom-agent/standard.txt`"
- **expect**: Standard worker (no custom agent_type) should have both write and shell tools. Should successfully create the file AND cat it.
- **anti-pattern**: Failing to execute shell command
- **severity**: high

### [pending] Cteno allowed_tools 白名单在 stdio session 中强制生效
- **message**: "用 restricted-worker 通过 Cteno stdio session 执行 `echo should-not-run`，然后读取 AGENT.md 并总结限制。"
- **expect**: stdio runner 拉取 runtime native tools 后按 `allowed_tools` 过滤，LLM 工具列表里没有 shell；agent 可以读取 AGENT.md，但不能调用 shell，回答中应说明 shell 不可用。
- **anti-pattern**: shell/edit/write 等未列入白名单的工具仍出现在 Cteno 工具列表；因为 adapter/前端过滤缺失而让 stdio 侧实际执行了 shell；只在 prompt 里说不能用但工具仍可调用。
- **severity**: high
