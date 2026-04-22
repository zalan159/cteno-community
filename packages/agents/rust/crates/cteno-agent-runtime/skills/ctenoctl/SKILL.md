---
id: ctenoctl
name: "ctenoctl CLI"
description: "通过 ctenoctl 调用本机 daemon RPC，执行 run/tool/persona/session/agent/mcp 等操作"
when_to_use: "用户明确提到 ctenoctl，或任务需要以 CLI 方式操作本地 daemon（尤其是 MCP 注册、连接、验证）时使用"
version: "1.0.0"
tags:
  - ctenoctl
  - cli
  - rpc
  - daemon
  - mcp
user_invocable: true
disable_model_invocation: false
argument_hint: "<goal_or_subcommand>"
---

# ctenoctl Skill

使用 `ctenoctl` 作为统一入口执行本地 daemon 能力，避免直接操作内部实现细节。

## 核心规则

1. 优先使用 `ctenoctl` 子命令，不绕过到私有 socket/RPC 协议细节。
2. 发生连接类错误时，先检查 `ctenoctl [--target agentd|tauri-dev|tauri] status`，再继续执行任务。
3. 需要自动化时，优先输出可复现命令和结构化 JSON 结果。
4. 涉及 MCP 时，走 `ctenoctl mcp ...` 标准链路（安装 -> 注册 -> 列表验证）。
5. 当用户明确指定无头 daemon、桌面 host 或 dev shell 时，优先显式传 `--target`，不要依赖默认 socket 探测。

## 常用命令

### Daemon 与诊断

- `ctenoctl status`
- `ctenoctl --target agentd status`
- `ctenoctl --target tauri-dev status`
- `ctenoctl auth status`
- `ctenoctl auth login`

### Agent 运行

- `ctenoctl run --kind worker --message "<task>"`
- `ctenoctl tool list`
- `ctenoctl tool exec <tool_id> --input '<json>'`
- `ctenoctl persona list`
- `ctenoctl session list`
- `ctenoctl agent list`

### 工作间（multi-agent）

- `ctenoctl workspace templates`
- `ctenoctl workspace bootstrap --template <id> -n "<name>" -w <workdir> [--model <model>]`
- `ctenoctl workspace list`
- `ctenoctl workspace get <persona_id>`
- `ctenoctl workspace state <persona_id>`
- `ctenoctl workspace members <persona_id>`
- `ctenoctl workspace activity <persona_id> [--limit <n>]`
- `ctenoctl workspace events <persona_id> [--limit <n>]`
- `ctenoctl workspace watch <persona_id> [--interval <secs>] [--limit <n>]`
- `ctenoctl workspace send <persona_id> -m "<message>" [--role <role_id>]`
- `ctenoctl workspace delete <persona_id>`

### MCP 管理（推荐）

- `ctenoctl mcp list`
- `ctenoctl mcp add-json --config '<json>'`
- `ctenoctl mcp add-stdio --id <id> --name <name> --command <cmd> [--arg <arg> ...]`
- `ctenoctl mcp add-sse --id <id> --name <name> --url <url> [--header KEY=VALUE ...]`
- `ctenoctl mcp install-stdio --id <id> --name <name> --install '<shell>' --command <cmd> [--arg <arg> ...]`
- `ctenoctl mcp remove <server_id>`
- `ctenoctl mcp enable <server_id>`
- `ctenoctl mcp disable <server_id>`

## 标准流程：安装并接入 MCP（stdio）

1. 准备安装命令和启动命令。
2. 执行：

```bash
ctenoctl mcp install-stdio \
  --id filesystem \
  --name filesystem \
  --install "npm i -g @modelcontextprotocol/server-filesystem" \
  --command npx \
  --arg -y \
  --arg @modelcontextprotocol/server-filesystem \
  --arg /tmp
```

3. 验证：

```bash
ctenoctl mcp list
```

4. 结果中至少确认：
   - `id` 正确
   - `enabled` 为预期
   - `status` 为 `connected`（或给出可执行的错误原因）

## 标准流程：验证 headless workspace

1. 明确目标 shell，例如 `agentd`：

```bash
ctenoctl --target agentd status
ctenoctl --target agentd workspace templates
```

2. 启动工作间：

```bash
ctenoctl --target agentd workspace bootstrap \
  --template coding-studio \
  -n "Feature Squad" \
  -w /tmp/feature-squad \
  --model deepseek-reasoner
```

3. 发送任务并观察状态：

```bash
ctenoctl --target agentd workspace send <persona_id> -m "请写一份 PRD 到 10-prd/group-mentions.md"
ctenoctl --target agentd workspace activity <persona_id> --limit 20
ctenoctl --target agentd workspace events <persona_id> --limit 20
```

## 输出要求

完成任务时应给出：

1. 实际执行的 `ctenoctl` 命令。
2. 关键 JSON 结果摘要（成功/失败、serverId、status、error）。
3. 下一步动作（例如重试命令、启用命令、移除命令）。
