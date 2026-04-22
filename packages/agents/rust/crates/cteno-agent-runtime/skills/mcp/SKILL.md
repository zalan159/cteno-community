---
id: mcp
name: "MCP Skill"
description: "根据用户提供的 MCP 文档（README/配置片段/链接），自动解析、下载安装并注册 MCP Server"
when_to_use: "用户要求安装/接入/注册 MCP，或给出 MCP 文档、mcpServers JSON、MCP 启动命令时使用"
version: "1.0.0"
tags:
  - mcp
  - integration
  - install
  - automation
user_invocable: true
disable_model_invocation: false
argument_hint: "<mcp_doc_path_or_url_or_text> [server_name]"
---

# MCP Skill

把用户提供的 MCP 文档自动落地为可用 MCP Server：**解析文档 -> 安装依赖 -> 注册到本地 MCPRegistry -> 验证可见**。

## 输入

- `$ARGS`: MCP 文档来源，可以是：
  - 本地文件路径（README/Markdown/JSON）
  - URL
  - 一段文本（含 `mcpServers` JSON 或启动命令）

## 执行规则

1. 如果用户已经给了文档路径/URL/文本，直接执行，不要反复追问。
2. 优先使用本 Skill 自带脚本：

```bash
python3 "${SKILL_DIR}/scripts/mcp_setup.py" --input "$ARGS"
```

3. 若文档中有多个 server，优先匹配用户提到的名称；否则使用脚本自动选中的第一个。
4. 如果安装失败：
   - 先输出失败命令与 stderr 摘要
   - 尝试脚本给出的回退路径（例如 npx/uvx 直启）
   - 仍失败再向用户报告缺失前置依赖（Node/Python/uv 等）
5. 注册成功后，必须给出：
   - server id
   - transport（stdio/http_sse）
   - 安装执行结果
   - 注册验证结果（list-mcp-servers 中可见）

## 多 server 文档处理

当文档里有多个 `mcpServers` 条目时，使用：

```bash
python3 "${SKILL_DIR}/scripts/mcp_setup.py" --input "$ARGS" --server "<server_key>"
```

## 仅预览（不执行安装/注册）

```bash
python3 "${SKILL_DIR}/scripts/mcp_setup.py" --input "$ARGS" --dry-run
```

## 结果验收标准

完成时满足以下全部条件：

1. 已解析出结构化 `config`（含 id/name/transport）。
2. 已执行安装步骤，或明确说明为何可跳过安装。
3. `add-mcp-server` RPC 返回成功（或幂等更新成功）。
4. `list-mcp-servers` 中能看到目标 server。
