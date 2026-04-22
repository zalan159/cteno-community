# Claude CLI 权限接入经验

接入 `claude` CLI 的 tool-use 权限审批闭环（`control_request can_use_tool` → host UI Modal → `control_response allow/deny` → Claude 恢复 turn）时踩过的坑和对应修复。参考 SDK：`/Users/zal/Cteno/tmp/claude-agent-sdk-python`（完整源码，比 minified TS 更清晰）。

## TL;DR

把 Claude CLI 的权限请求接到自己的 UI 上，**必须同时满足以下条件**（按踩坑顺序分两轮）：

第一轮 — 让 Modal 能弹、点了 Allow 能恢复 turn：

1. spawn 时传 `--permission-prompt-tool stdio`（哨兵值，不是协议名）
2. host 识别 `control_request subtype: "can_use_tool"`，翻译成上层 PermissionRequest 事件
3. UI 拿到后弹审批 Modal，把结果通过 RPC 回传到 host
4. host 用 SDK 规范的 `control_response` 形状回写 CLI stdin，**`updatedInput` 要回传原始 tool input**（不是 `{}`）
5. 所有长时占用 stdout/stdin 的锁必须拆开，避免 reader task 和 respond_to_permission 抢锁死锁

第二轮 — 让按钮高亮正确、"不再询问"生效、运行时切 mode 有效：

6. `complete_permission_request` 把 RPC 响应里的 `decision` / `mode` / `allowTools` / `reason` 全部写进 `agentState.completedRequests[id]`
7. session 允许列表区分 `tools` / `bash_literals` / `bash_prefixes` 三档匹配，`evaluate_pre_approval` 带 `tool_input` 判定
8. `set_permission_mode` / `set_model` / `interrupt` 走 SDK `control_request` 协议，**不要**发 `/permission` / `/cancel` / `/model` 作为用户文本消息

漏掉任何一条都会表现为"Modal 不弹"、"点了 Allow 没反应"、"按钮高亮不对"、"不再询问没用"或"切 mode 不生效"——现象相似但根因各异。

## 1. `stdio` 是官方哨兵值

`--permission-prompt-tool <X>` 看起来像要填一个 MCP 工具名（形如 `mcp__<server>__<tool>`），但 `stdio` 是一个**特殊保留值**，告诉 CLI "把权限请求走 stdio 控制协议，不路由给 MCP"。

- Python SDK `client.py:70`：`configured_options = replace(options, permission_prompt_tool_name="stdio")`
- TS SDK `sdk.mjs`：`if(canUseTool){ cmd.push("--permission-prompt-tool","stdio") }`

两个 SDK 在用户设置了 `canUseTool` 回调时都会**自动**传 `stdio`。我们自己 spawn CLI 也必须传。

**漏传的症状**：CLI 回到内部权限逻辑，写操作默认限制在 cwd 内，表现是"Claude 说 permission denied"或"Stream closed"。

**误传真实 MCP 工具名的症状**：Claude 把权限请求当成对该工具的 `tool_use`，等 ToolResult 永远不来 → 整个 turn 挂 60s 超时。

## 2. `control_response` 形状

回给 Claude 的 `control_response` 必须是**双层嵌套**的，不能展平：

```json
{
  "type": "control_response",
  "response": {
    "subtype": "success",
    "request_id": "<echo 回来的>",
    "response": {
      "behavior": "allow",
      "updatedInput": <原始 tool_input>
    }
  }
}
```

注意外层 `response` 是 envelope（带 `subtype` / `request_id`），内层 `response` 才是 payload（`behavior` / `updatedInput`）。Python SDK `query.py:352-363` 是权威参考。

### `updatedInput` 必须回传原始 tool_input

Claude 把 `control_response.response.updatedInput` 当作**最终执行的 tool 参数**。如果回 `{}`，下游 Bash / Edit 等工具就没参数执行，Claude 等 tool_result 永远不来，turn wedge。

Python SDK `query.py:295-302` 的做法：

```python
response_data = {
    "behavior": "allow",
    "updatedInput": (
        response.updated_input         # 用户修改过的 input（如果支持编辑）
        if response.updated_input is not None
        else original_input            # 原始 input
    ),
}
```

我们的 Rust 实现要把 `can_use_tool` 时收到的 `request.input` 缓存起来，`respond_to_permission` 时取出来塞回 `updatedInput`。

### Deny 的形状

```json
{
  "behavior": "deny",
  "message": "<原因>",
  "interrupt": <可选，true 会中断整个 turn>
}
```

## 3. 死锁陷阱

Claude CLI 会在等 permission response 期间**不发任何 stdout 帧**。此时如果 host 的 stream reader 任务持有 `Mutex<ClaudeSessionProcess>` 读 stdout，同时 `respond_to_permission` 要锁同一个 Mutex 写 stdin，就形成三角死锁：

```
stream task   持 lock → 等 Claude 新帧
Claude        → 等 permission response
respond task  → 等 lock 写 stdin
```

### 修复：锁必须拆开

```rust
struct ClaudeSessionProcess {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,              // 独立锁
    stdout_reader: Option<BufReader<ChildStdout>>,  // 由 stream task take/restore
    pending_permission_inputs: Arc<Mutex<HashMap<String, Value>>>,  // 独立锁
    ...
}
```

- stream task 在 turn 开始时 `take()` stdout_reader 本地持有，turn 结束 put 回。**不再持外层锁**。
- `respond_to_permission` / `set_permission_mode` / `interrupt` 等写 stdin 的操作，**只锁 stdin Arc**，不碰外层。
- `pending_permission_inputs`（can_use_tool → respond_to_permission 间的 input 缓存）同理用自己的 Arc Mutex。

**这个死锁与传输层无关**——本地模式（Tauri IPC）和远程模式（Socket.IO 从 mobile 来）都走同一份 Rust adapter，都会卡。修了所有路径都受益。

## 4. 本地模式的额外问题：broadcast 广播没通道

Claude CLI 的 permission 流程除了 `control_request`，host 还需要**把状态推给前端 UI**：

- ACP `permission-request` 记录消息（tool-call 卡片占位）
- `agentState.requests[permId]` 状态变更（PermissionFooter 渲染 Allow/Deny 按钮的数据源 —— reducer 认这个，**不是**消息本身）

远程模式这两个都通过 Socket.IO `emit("message", …)` / `emit("update-state", …)` 推给 happy-server，移动端订阅。**本地模式没有 happy-server**，socket 是 stub，发出去无人接。

### 统一修复：`LocalEventSink` trait

在 `HappySocket` 层加 trait seam，让每个 broadcast emit 方法 fan-out 到本地 sink（如果装了）：

```rust
pub trait LocalEventSink: Send + Sync + 'static {
    fn on_message(&self, session_id: &str, encrypted_message: &str, local_id: Option<&str>);
    fn on_transient_message(&self, session_id: &str, encrypted_message: &str);
    fn on_state_update(&self, session_id: &str, encrypted_state: Option<&str>, version: u32);
    fn on_metadata_update(&self, session_id: &str, encrypted_metadata: &str, version: u32);
    fn on_session_alive(&self, session_id: &str, ...) { }  // default no-op
}
```

`HappySocket` 的 emit 方法在 remote emit **之前**无条件调 sink（if installed）：

```rust
pub async fn send_message(&self, ...) -> Result<(), String> {
    if let Some(sink) = self.local_sink() {
        sink.on_message(session_id, encrypted_message, local_id.as_deref());
    }
    if let Some(client) = self.remote_client() {
        client.emit("message", payload).await?;
    }
    Ok(())
}
```

desktop 侧实现：
- `on_message` → append 到 SQLite `agent_sessions.messages` + Tauri event `local-session:message-appended`
- `on_state_update` → Tauri event `local-session:state-update` 带 agentState JSON
- 前端 `sync.ts` 监听这些 event → 触发 `fetchMessages` / `applySessions`

**收益**：
- 调用方（`executor_normalizer::send_persisted`、`permission::emit_agent_state`）不再需要 `if self.socket.is_local() { persist... }` 补丁
- 未来新加的 emit 方法自动享受双通道
- trait 定义在 transport crate（不依赖 Tauri），impl 在 desktop crate，分层干净

### 反模式：调用方打补丁

别在每个调用方加：

```rust
socket.send_message(...).await?;
if self.socket.is_local() {
    self.persist_local_session_message(...)?;  // ad-hoc 补丁
}
```

会漏—— `update_session_state` 就是这么漏掉的，导致 Modal 不弹却没人发现是因为 agentState 没推到前端。

## 5. 前端渲染链（本地模式）

PermissionFooter 渲染的数据来源是 **reducer 从 `agentState.requests`** 派生的 `tool.permission` 字段，**不是** ACP permission-request 消息本身。

完整链路：

```
host: executor 发 ExecutorEvent::PermissionRequest
  → normalizer 更新 PermissionHandler.agent_state.requests[id]
  → socket.update_session_state(agentState JSON)
  → [LocalEventSink.on_state_update] Tauri event "local-session:state-update"
  → [frontend sync.ts 监听] applySessions({agentState, agentStateVersion})
  → reducer 发现新 agentState.requests[permId]
  → reducer 填充 tool.permission = { id, status: "pending" }
  → ToolView 渲染 PermissionFooter 按钮
```

## 6. 按钮状态 / 会话级允许 / 运行时切换（第二轮踩坑）

Modal 能弹出、Allow 能生效之后，还有三类"半工作"的后续问题：

### 6.1 `completedRequests` 必须带 decision / mode / allowedTools / reason

`PermissionFooter.tsx` 的按钮高亮靠的不是 pending-状态，而是 completed 后的 `tool.permission` 详情：

```ts
const isApprovedForSession = isApproved &&
    isToolAllowed(toolName, toolInput, permission.allowedTools);
```

`permission.allowedTools` 和 `permission.decision` / `permission.mode` 由 reducer 从 `agentState.completedRequests[id]` 读取。host 端如果 `complete_permission_request` 只回写 `{status, completedAt, tool, arguments, createdAt}`，reducer 拿到的 `tool.permission` 就没有这些字段 → 用户点 "是,不再询问" 但 UI 只高亮 "是"。

修复：把 RPC 响应里的 `decision` / `mode` / `allow_tools` / `reason` 都写进 `completedRequests[id]`。同时写 `allowTools` 和 `allowedTools` 两个键（前端 reducer 和 ACP 对字段名有历史分歧）。

resolver task 要在 `apply_response` 之前 clone 出这些字段——`apply_response` 消费整个 response。

### 6.2 session 允许列表必须区分 literal / prefix

前端 "是,不再询问" 发过来的 `allowTools` 条目有三种：

- `"Bash(rm /Users/zal/Desktop/xxx.png)"` — 精确命令
- `"Bash(npm:*)"` — 前缀通配
- `"Read"` — 裸工具名

host 如果只用一个 `HashSet<String>` 做精确匹配（`contains(tool_name)`），下次 Claude 跑 Bash 带别的 command 就命中不了 → 再次弹 Modal。必须对齐 Happy Coder `permissionHandler.ts:257-281` 的 literal/prefix 拆分：

```rust
struct SessionAllowedTools {
    tools: HashSet<String>,           // "Read" → 全匹配
    bash_literals: HashSet<String>,   // "rm /a" → 只命中同一 command
    bash_prefixes: HashSet<String>,   // "npm" → 前缀匹配
}

fn matches(&self, tool_name: &str, input: &Value) -> bool {
    if self.tools.contains(tool_name) { return true; }
    if tool_name == "Bash" {
        if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
            if self.bash_literals.contains(cmd) { return true; }
            for prefix in &self.bash_prefixes {
                if cmd.starts_with(prefix) { return true; }
            }
        }
    }
    false
}

fn insert(&mut self, entry: &str) {
    if let Some(inner) = entry.strip_prefix("Bash(").and_then(|s| s.strip_suffix(')')) {
        if let Some(prefix) = inner.strip_suffix(":*") {
            self.bash_prefixes.insert(prefix.to_string());
        } else {
            self.bash_literals.insert(inner.to_string());
        }
    } else {
        self.tools.insert(entry.to_string());
    }
}
```

`evaluate_pre_approval` 调用时必须把 `tool_input` 一起传进来（不能只靠 `tool_name`）。

### 6.3 运行时热切换必须走 control_request

用 `"/permission <mode>"` 作为用户文本消息塞给 CLI **不起作用**——在 stream-json 模式下 CLI 把它当普通用户消息回显，不会改变当前 session 的权限模式。

同样：`interrupt` 不是发 `"/cancel"` 文本，`set_model` 不是 `"/model xxx"` 文本。

正确做法是 SDK control 协议（Python SDK `_internal/query.py:611-630`）：

```rust
// set_permission_mode
json!({
    "type": "control_request",
    "request_id": format!("req_set_mode_{}", Uuid::new_v4()),
    "request": { "subtype": "set_permission_mode", "mode": "acceptEdits" }
})

// interrupt
json!({
    "type": "control_request",
    "request_id": ...,
    "request": { "subtype": "interrupt" }
})

// set_model
json!({
    "type": "control_request",
    "request_id": ...,
    "request": { "subtype": "set_model", "model": "claude-opus-4-7" }
})
```

CLI 会回一个 `control_response { subtype: "success", request_id }`。我们的 stream reader 可以继续忽略该响应（或只用来确认生效），但**发送端必须是 control_request**。

**症状对比**：slash-text 路径下切到 `bypassPermissions`，日志里能看到 host-side `Permission Mode set to BypassPermissions`，但下一次 tool call 仍然发 `can_use_tool` 要求审批——因为 CLI 根本没收到切换信号。

## 7. 最小修复清单

1. ✅ spawn 加 `--permission-prompt-tool stdio`（可 plumbing 成 Option<String>，用户给真名优先）
2. ✅ `can_use_tool` control_request → 翻译成 `ExecutorEvent::PermissionRequest`；同时缓存原始 `tool_input`
3. ✅ `hook_callback` / `mcp_message` / 未知 subtype → 回 control_response 避免 CLI 挂住
4. ✅ `respond_to_permission` 用 SDK 双层嵌套 shape，`updatedInput` 回传原始 input
5. ✅ 本地模式：`LocalEventSink` 把 broadcast emit 导到 SQLite + Tauri event
6. ✅ 前端订阅 Tauri event 更新 storage，触发 reducer
7. ✅ 死锁：stdin / pending_permission_inputs / stdout_reader 用独立 Arc<Mutex> / Option
8. ✅ `complete_permission_request` 写全 `decision / mode / allowTools / allowedTools / reason`
9. ✅ session 允许列表拆 `tools` / `bash_literals` / `bash_prefixes`，`evaluate_pre_approval` 带 `tool_input` 判定
10. ✅ `set_permission_mode` / `set_model` / `interrupt` 用 SDK control_request，不用 `/slash` 用户消息

每一条都是必要条件。少做一条就是"看起来差不多但 Modal 不弹 / 点了没反应 / 按钮高亮错 / 切模式不生效"。

## 8. 诊断技巧

- **看 executor 事件序列的 Discriminant**：`Disc(5)` = PermissionRequest；全程没 `Disc(5)` 说明上游根本没发权限请求（大概率是 `--permission-prompt-tool stdio` 没传）。
- **Normalizer 有 `Permission reply approved=true` 但没后续 event** → Claude 收到了 control_response 但没继续 → 大概率 `updatedInput` 空或 shape 错。
- **Normalizer 有 `Permission reply approved=true`、没看到 respond_to_permission 的 eprintln** → 死锁。
- **Host 端 `ACP permission-request sent` 有，前端 Modal 不显示** → agentState 没推到前端（本地模式的 LocalEventSink 问题）。
- **前端 Modal 弹出，但"是,不再询问"按钮高亮只到"是"** → `completedRequests[id]` 漏字段；检查 `complete_permission_request` 是否把 decision/mode/allowTools 回写了。
- **切到 bypassPermissions 下一次还弹审批** → `set_permission_mode` 走的是 `/permission` 用户文本而不是 `control_request`；log 里应该有 host-side mode 变更但 CLI 端不生效。
- **"是,不再询问" 一次 Bash 之后，下次 Bash 别的命令还是弹** → `session_allowed_tools` 没做 literal/prefix 区分，只存了 `Bash(exact-cmd)` 但 `evaluate_pre_approval` 用 `contains(tool_name)` 查。

直接测 Claude CLI 可以快速验证 shape：

```bash
(cat <<'EOF'
{"type":"control_request","request_id":"req_init","request":{"subtype":"initialize","hooks":null}}
{"type":"user","session_id":"","message":{"role":"user","content":"touch /tmp/x"},"parent_tool_use_id":null}
EOF
sleep 10) | claude --output-format stream-json --input-format stream-json --verbose \
  --permission-mode default --permission-prompt-tool stdio --session-id "$(uuidgen)"
```

会看到 `control_request can_use_tool` 打印，然后进程挂住等 control_response。

## 参考

- Python SDK 源码：`/Users/zal/Cteno/tmp/claude-agent-sdk-python/src/claude_agent_sdk/_internal/query.py`（`initialize`、`can_use_tool` 处理全在这）
- Python SDK 端到端示例：`/Users/zal/Cteno/tmp/claude-agent-sdk-python/e2e-tests/test_tool_permissions.py`
- TS SDK 打包产物：`/Users/zal/Cteno/packages/multi-agent-runtime/node_modules/@anthropic-ai/claude-agent-sdk/sdk.mjs`（minified，grep 特定 subtype 字符串）
- Happy Coder 参考实现：`/tmp/happy-reference/packages/happy-cli/src/claude/utils/permissionHandler.ts`（用 SDK canUseTool 回调，纯 Socket.IO 路径）
