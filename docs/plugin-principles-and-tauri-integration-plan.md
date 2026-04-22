# Plugin 原理与 Tauri 调度器对接计划（Codex / Claude / Gemini）

## 1. 先回答核心问题

### plugin 的原理是什么？是一组 MCP 吗？
结论：**不是只等于 MCP**，但 **MCP 是三家最稳定的共同子集**。

plugin/extension 更像一个“能力打包与分发层”，常见会包含：
- MCP servers（工具能力）
- Skills/Commands/Agents（提示词与工作流能力）
- Hooks/Output styles/LSP/UI 元信息（不同 vendor 的扩展面）

所以“plugin = 一组 MCP”在工程上是个可落地的最小公约数，但不是完整定义。

## 2. 三家各自如何实现 plugin，并接入 agent loop

### 2.1 Codex

实现形态：
- `plugin.json`（`.codex-plugin/plugin.json`）+ 可选 `skills` / `mcp_servers` / `apps` 路径。
- 参考：
  - `/Users/zal/Cteno/tmp/codex/codex-rs/core-plugins/src/manifest.rs:13`
  - `/Users/zal/Cteno/tmp/codex/codex-rs/core-plugins/src/manifest.rs:215`

接入链路（plugin -> tool call）：
- `PluginsManager` 与 `McpManager` 在 `ThreadManager` 初始化时装配。
  - `/Users/zal/Cteno/tmp/codex/codex-rs/core/src/thread_manager.rs:252`
- `Config::to_mcp_config` 把 `loaded_plugins.effective_mcp_servers()` 合并进 `configured_mcp_servers`。
  - `/Users/zal/Cteno/tmp/codex/codex-rs/core/src/config/mod.rs:810`
- turn 构建工具时从 MCP manager 拉取 tools，再构造 `ToolRouter`。
  - `/Users/zal/Cteno/tmp/codex/codex-rs/core/src/session/turn.rs:1191`
- `ToolRouter::build_tool_call` 把命中的 tool 解析为 `ToolPayload::Mcp`。
  - `/Users/zal/Cteno/tmp/codex/codex-rs/core/src/tools/router.rs:173`
- `McpHandler -> handle_mcp_tool_call -> sess.call_tool(...)` 执行。
  - `/Users/zal/Cteno/tmp/codex/codex-rs/core/src/tools/handlers/mcp.rs:21`
  - `/Users/zal/Cteno/tmp/codex/codex-rs/core/src/mcp_tool_call.rs:464`
  - `/Users/zal/Cteno/tmp/codex/codex-rs/core/src/session/mcp.rs:177`

### 2.2 Claude Code

实现形态：
- `.claude-plugin/plugin.json`，扩展面非常大：`mcpServers`、`commands`、`agents`、`skills`、`hooks`、`outputStyles`、`lspServers`、`userConfig`、`channels` 等。
- 参考：
  - `/Users/zal/Downloads/claude-code/src/utils/plugins/schemas.ts:885`
  - `/Users/zal/Downloads/claude-code/src/utils/plugins/schemas.ts:543`
  - `/Users/zal/Downloads/claude-code/src/utils/plugins/pluginLoader.ts:2430`

接入链路（plugin -> tool call）：
- `getClaudeCodeMcpConfigs()` 读取 plugin MCP，做 policy/filter/dedup/merge。
  - `/Users/zal/Downloads/claude-code/src/services/mcp/config.ts:1071`
  - `/Users/zal/Downloads/claude-code/src/services/mcp/config.ts:1172`
- plugin MCP server 经过 `plugin:{plugin}:{server}` scope 注入。
  - `/Users/zal/Downloads/claude-code/src/utils/plugins/mcpPluginIntegration.ts:341`
- MCP 连接更新写入 app state `mcp.tools`。
  - `/Users/zal/Downloads/claude-code/src/services/mcp/useManageMCPConnections.ts:255`
- REPL 把 `mcp.tools` 合并进工具池。
  - `/Users/zal/Downloads/claude-code/src/hooks/useMergedTools.ts:20`
  - `/Users/zal/Downloads/claude-code/src/screens/REPL.tsx:811`
- query loop `runTools(...)`，最终 `tool.call(...)` 执行。
  - `/Users/zal/Downloads/claude-code/src/query.ts:1382`
  - `/Users/zal/Downloads/claude-code/src/services/tools/toolExecution.ts:337`
  - `/Users/zal/Downloads/claude-code/src/services/tools/toolExecution.ts:1207`
- MCP tool wrapper 在 `fetchToolsForClient` 中生成。
  - `/Users/zal/Downloads/claude-code/src/services/mcp/client.ts:1743`
  - `/Users/zal/Downloads/claude-code/src/services/mcp/client.ts:1833`

### 2.3 Gemini CLI

实现形态：
- `gemini-extension.json`（Extension），可带 `mcpServers`、`excludeTools`、`contextFileName`、`themes`、`plan` 等。
- 参考：
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/cli/src/config/extension.ts:24`

接入链路（extension -> tool call）：
- CLI 配置层加载 extension manager，并把 `extensionLoader` 传给 core config。
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/cli/src/config/config.ts:601`
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/cli/src/config/config.ts:978`
- `Config.initialize()` 并行启动 `startConfiguredMcpServers()` 与 `extensionLoader.start(this)`。
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/core/src/config/config.ts:1460`
- extension 启动时调用 MCP manager `startExtension`，逐个发现 MCP server。
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/core/src/utils/extensionLoader.ts:65`
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/core/src/tools/mcp-client-manager.ts:241`
- discover 到 `DiscoveredMCPTool` 并注册进 tool registry。
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/core/src/tools/mcp-client.ts:1272`
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/core/src/tools/mcp-client.ts:1307`
- model 侧 functionDeclarations 从 tool registry 出。
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/core/src/tools/tool-registry.ts:647`
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/core/src/core/client.ts:294`
- scheduler 按 name 查 tool 并执行。
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/core/src/scheduler/scheduler.ts:314`
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/core/src/scheduler/tool-executor.ts:60`

## 3. 三家是否有 CLI/接口可供调度

结论：**都有可调度接口，但成熟度不同。**

- Codex：最强，已有 app-server JSON-RPC 方法可做结构化调度。
  - `marketplace/add`、`marketplace/remove`、`plugin/list`、`plugin/read`、`plugin/install`、`plugin/uninstall`
  - `/Users/zal/Cteno/tmp/codex/codex-rs/app-server-protocol/src/protocol/common.rs:343`

- Claude：有命令入口，结构化程度偏弱。
  - `/plugin`、`/reload-plugins`
  - `/Users/zal/Downloads/claude-code/src/commands/plugin/index.tsx:2`
  - `/Users/zal/Downloads/claude-code/src/commands/reload-plugins/index.ts:7`
  - SDK 可走 control request 触发 reload（注释已说明）
  - `/Users/zal/Downloads/claude-code/src/commands/reload-plugins/index.ts:11`

- Gemini：有 extension 管理命令，适合命令式调度。
  - `/extensions list|install|link|uninstall|restart|update`
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/cli/src/acp/commands/extensions.ts:25`
  - CLI 参数也支持 `--extensions`、`--list-extensions`
  - `/Users/zal/Cteno/tmp/gemini-cli/packages/cli/src/config/config.ts:365`

补充（你们现有 Tauri 现状）：
- `ExecutorRegistry` 已经按 vendor 自动探测并装配三家 CLI（claude/codex/gemini）。
  - `/Users/zal/Cteno2.0/apps/client/desktop/src/executor_registry.rs:159`
  - `/Users/zal/Cteno2.0/apps/client/desktop/src/executor_registry.rs:177`
  - `/Users/zal/Cteno2.0/apps/client/desktop/src/executor_registry.rs:191`
- 三个 runtime adapter 的当前传输形态：
  - Claude：`claude --output-format stream-json --input-format stream-json`
    - `/Users/zal/Cteno2.0/packages/multi-agent-runtime/rust/crates/multi-agent-runtime-claude/src/agent_executor.rs:385`
  - Codex：优先 `codex app-server --listen stdio://`，旧版回退到 `codex exec --experimental-json`
    - `/Users/zal/Cteno2.0/packages/multi-agent-runtime/rust/crates/multi-agent-runtime-codex/src/agent_executor.rs:3`
    - `/Users/zal/Cteno2.0/packages/multi-agent-runtime/rust/crates/multi-agent-runtime-codex/src/agent_executor.rs:364`
  - Gemini：`gemini --acp`（JSON-RPC over ndJSON）
    - `/Users/zal/Cteno2.0/packages/multi-agent-runtime/rust/crates/multi-agent-runtime-gemini/src/agent_executor.rs:1`

## 4. 能否通过目录同步一次安装三家可用？

结论：
- **不能**用同一个 vendor 原生 manifest 直接通吃三家。
- **可以**用“统一 canonical 目录 + vendor 投影同步”实现“一次安装，三家生效”。

你们仓库已经有这套基础设施：
- `cteno-host-agent-sync::reconcile_all(...)`
  - `/Users/zal/Cteno2.0/packages/host/rust/crates/cteno-host-agent-sync/src/lib.rs:40`
- Canonical schema：`McpSpec / PersonaSpec / SkillSpec`
  - `/Users/zal/Cteno2.0/packages/host/rust/crates/cteno-host-agent-sync/src/schemas.rs:25`
- vendor syncers：Claude / Codex / Gemini / Cteno
  - `/Users/zal/Cteno2.0/packages/host/rust/crates/cteno-host-agent-sync/src/lib.rs:31`
- Desktop 启动时已经执行 reconcile
  - `/Users/zal/Cteno2.0/apps/client/desktop/src/service_init.rs:1454`
  - `/Users/zal/Cteno2.0/apps/client/desktop/src/agent_sync_bridge.rs:624`

当前覆盖差异（很关键）：
- Claude/Gemini：MCP + subagents + skills + system prompt symlink。
- Codex：当前 syncer 只做 MCP；subagents/skills/system prompt 为 no-op（Codex 语义不同）。
  - `/Users/zal/Cteno2.0/packages/host/rust/crates/cteno-host-agent-sync/src/vendors/codex.rs:4`
  - `/Users/zal/Cteno2.0/packages/host/rust/crates/cteno-host-agent-sync/src/vendors/codex.rs:120`

## 5. Tauri 调度器对接 plugin 的建议计划

### Phase 0（1-2 天）：统一抽象，不动现网行为

目标：把“plugin 可调度能力”先抽成接口，避免后续把 vendor 特例散落在 scheduler。

- 在 desktop 侧新增 `VendorPluginController` 抽象（仅 host 层，非 session 内）：
  - `install(plugin_ref)`
  - `uninstall(plugin_id)`
  - `enable/disable(plugin_id)`
  - `list()`
  - `reconcile(workdir)`
- 把现有 `agent_sync_bridge` 作为 `reconcile` 的默认实现。
- 能力探测：从 executor registry 暴露 vendor plugin capabilities（是否有 native CLI/RPC）。

### Phase 1（3-5 天）：MVP 可用（推荐先做）

目标：先做“一次安装，多端可见”的最小闭环，以 MCP 为主，不等三家高级能力对齐。

- 安装入口统一写入 Cteno canonical source（例如 `~/.cteno/plugins/<id>`）。
- 安装/启用/禁用后立即调用 `reconcile_project_now(workdir)`。
- 每次会话 spawn 前做轻量 reconcile（带 hash/mtime 防抖，避免每轮 IO）。
- UI 告知 capability：
  - `full`（vendor native）
  - `projected`（通过 sync 投影）
  - `unsupported`

### Phase 2（4-7 天）：接入各家 native 管理接口（增强）

目标：把“投影同步”升级为“投影 + native 管理”双轨，提升一致性与可观测性。

- Codex：优先接 app-server JSON-RPC 插件管理方法。
- Claude：先接 `/reload-plugins` + 文件落盘，必要时再走 SDK control request。
- Gemini：接 `/extensions ...` 命令面（或 ACP command gateway）。
- 调度策略：先写 canonical，再调用 native refresh/install；失败可回滚或标记 `degraded`。

### Phase 3（持续）：策略、回滚与审计

- 为每个 plugin 记录 projection state（每 vendor 的 last applied hash、错误、时间）。
- 当 vendor 失败时不阻塞主会话，降级到“仅 canonical 可用 + 提示修复动作”。
- 在 UI 提供“一键重投影 / 修复”操作。

## 6. 推荐决策

推荐采用：**Canonical-first + Reconcile + Vendor-native 可选增强**。

理由：
- 与你们现有 `cteno-host-agent-sync`、`agent_sync_bridge` 方向一致，改造成本最低。
- 能立刻满足“类似 skill 的目录同步一次安装”诉求。
- 允许三家差异长期存在，不强迫统一 manifest（这是高风险路线）。
