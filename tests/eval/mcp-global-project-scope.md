# MCP Global + Project Scope

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-mcp-scope-test
- max-turns: 15

## setup
```bash
rm -rf /tmp/cteno-mcp-scope-test
mkdir -p /tmp/cteno-mcp-scope-test/.cteno
```

## cases

### [pending] 新 Cteno session 在无 KV 选择时继承项目 MCP
- **message**: "在这个新目录启动 Cteno 会话，确认 MCP modal/工具面包含项目级 .cteno/mcp_servers.yaml 中 enabled=true 的 server；不要提前写 session.${sessionId}.mcpServerIds。"
- **expect**: session 可见 MCP 列表来自 global + project 合并结果，activeServerIds 默认包含 enabled 项目的 toolNamePrefix。
- **anti-pattern**: activeServerIds 为空；只显示全局 MCP；必须手动重新选择 MCP 才能看到项目 server。
- **severity**: high

### [pending] 项目 MCP 覆盖同名全局 MCP
- **message**: "准备全局和项目两个 mcp_servers.yaml，二者都含 id=cteno-memory 但 command/args 不同；启动项目 session 后检查列表与工具调用使用项目层配置。"
- **expect**: 合并结果中只有一个 cteno-memory，scope 标为 project，command/args 来自项目配置。
- **anti-pattern**: 出现两个同 id server；全局 command 被调用；scope 丢失或标成 global。
- **severity**: high

### [pending] 显式清空 session MCP 不被默认 enabled 覆盖
- **message**: "先通过 MCP modal 将当前 session 的 MCP server 选择保存为空数组，再刷新/恢复 session。"
- **expect**: KV 中显式 [] 保持生效，activeServerIds 仍为空；allServers 仍显示 global+project 可用列表。
- **anti-pattern**: 恢复后因为 enabled=true 自动重新激活；allServers 被清空导致无法重新开启。
- **severity**: medium

### [pending] CtenoSyncer 保留用户项目 MCP 并 upsert host-managed MCP
- **message**: "在 .cteno/mcp_servers.yaml 手写一个 enabled=false 的 user-server，再触发 project reconcile。"
- **expect**: user-server 保留原 enabled=false；cteno-memory 被 upsert 到同一项目文件，包含 --project-dir 指向当前 workdir。
- **anti-pattern**: 文件被重建导致 user-server 丢失；cteno-memory 只写到全局；缺少 --project-dir。
- **severity**: high
