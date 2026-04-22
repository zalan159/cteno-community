# AgentExecutor Hotswap Matrix — runtime model / permission / restart-resume

验证 Claude / Codex / Gemini / Cteno 四家 executor 在运行中切 model、切 permission mode、以及需要 restart/resume 时的语义是否清晰一致。

本文件只覆盖 session 已启动后的 runtime control。这里的 hotswap 仅指同一 vendor executor 内的 model / permission / restart-resume 语义，不验证 vendor selector、跨 vendor 切换入口，或 injected host tool 的展示。

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-agent-executor-hotswap
- max-turns: 18

## setup

```bash
mkdir -p /tmp/cteno-agent-executor-hotswap
rm -f /tmp/cteno-agent-executor-hotswap/touched.txt
printf 'seed context\n' > /tmp/cteno-agent-executor-hotswap/context.txt
# 如使用 stub CLI/fixture，确保四家 executor 的测试入口都指向可重复夹具。
```

## matrix

| vendor | `set_model` | `set_permission_mode` | restart / resume 预期 |
| --- | --- | --- | --- |
| Claude | 运行中直接 `Applied` | 运行中直接生效 | 不需要 restart；同一 session 继续处理后续 turn |
| Codex | 返回 `Applied`，executor 内部原地重建 transport | 运行中可切；空闲时立即生效，忙时下一轮生效 | caller 不需要显式 `close_session` / `resume_session`；同一 session/thread 上下文延续 |
| Gemini | 返回 `RestartRequired`，并为下次启动暂存新 model | `Unsupported`，spawn-time only | caller 负责 cold restart；`supports_resume=false`，不能走 resume 快路 |
| Cteno | `Unsupported` | `Unsupported`，spawn-time only | `resume_session` 只用于 reconnect/recovery，不用于 runtime hotswap |

## out-of-scope

- vendor selector / discovery 的 installed / logged-in / ready / refresh 状态
- 跨 vendor 切换入口、selector 下拉项或 auth CTA
- host / injected tool 的 tool-call 或 tool-result 展示
- 权限审批 Modal 文案、双击 Allow、120 秒 timeout 等纯前端/审批闭环细节

## cases

### [skip: 2026-04-18 QA 未拿到可用 Claude CLI live session；仅离线核对 control_request set_model 路径] Claude：运行中切 model 立即生效，不触发 restart
- **message**: spawn Claude session with supported model A，先发一轮消息确认会话存活；随后调用 `set_model(model B)`，再发第二轮消息
- **expect**: `set_model` 返回 `ModelChangeOutcome::Applied`；不需要 `close_session` / `resume_session`；第二轮继续走原 session，历史不重放，日志或 native event 可见新 model 已被 CLI 接收
- **anti-pattern**: 返回 `RestartRequired`；热切换后 session 卡死；第二轮丢上下文或重复上一条 user message
- **severity**: high

### [skip: 2026-04-18 QA 未拿到可用 Claude CLI live session；仅离线核对 control_request set_permission_mode 路径] Claude：运行中切 permission mode 后，下一次 tool 决策使用新 mode
- **message**: Claude session 先以 `Default` mode 跑一条需要审批的 mutating 命令；然后调用 `set_permission_mode(bypassPermissions)`；再发第二条 mutating 命令
- **expect**: 第一条按旧 mode 走审批；第二条不再发 `permission-request`，直接执行；CLI stdin 可观察到 `control_request { subtype: "set_permission_mode" }`
- **anti-pattern**: mode 改了但下一条仍按旧逻辑弹审批；只改 host 侧状态、Claude CLI 实际没收到控制帧
- **severity**: high

### [pass: 2026-04-19 离线单测 `agent_executor::tests::set_model_and_permission_mode_restart_and_resume_with_persisted_config`] Codex：切 model 由 executor 原地重建 transport，同一 session/thread 上下文延续
- **message**: spawn Codex session with model A，完成一轮对话；调用 `set_model(model B)`；随后继续在同一 session 发新消息
- **expect**: `set_model` 返回 `ModelChangeOutcome::Applied`；caller 不需要显式 `close_session` / `resume_session`；executor 在内部用既有 thread id 重建 subprocess，后续 turn 使用新 model，workdir/history 保留且不重复回放
- **anti-pattern**: `set_model` 成功但后续仍跑旧 model；内部重建后 thread id 丢失；历史重放两次；对外暴露成必须 caller 自己 restart
- **severity**: high

### [pass: 2026-04-19 离线单测 `agent_executor::tests::set_model_and_permission_mode_restart_and_resume_with_persisted_config`] Codex：permission mode 在同一对话内可热切换，但若当前 turn 正在运行则顺延到下一轮
- **message**: spawn Codex session in `Default` mode，调用 `set_permission_mode(bypassPermissions)`；随后继续发送会触发审批的命令
- **expect**: 调用本身返回成功；若 session 空闲，executor 立即原地重建 transport 并在下一条请求中带上新 `approvalPolicy` / `sandbox`；若当前 turn 正在运行，则不打断当前 turn，而是在它结束后的下一轮按新 mode 生效
- **anti-pattern**: host/UI 把 Codex 标成 `Unsupported`；RPC 返回 ok 但后续请求仍使用旧 approval policy；切 mode 必须依赖 caller 显式 `close_session` / `resume_session`
- **severity**: high

### [pass: 2026-04-18 QA 通过离线单测 `agent_executor::tests::set_model_updates_env_and_requires_restart`] Gemini：切 model 只为下次启动暂存配置，并要求 cold restart
- **message**: spawn Gemini session with model A；调用 `set_model(model B)`；在不重启的情况下再发一条消息，然后关闭并重新 spawn 一次
- **expect**: `set_model` 返回 `RestartRequired`，并把新 model 暂存到下一次启动配置；当前 subprocess 不应假装已热切；由于 `supports_resume=false`，host 走 fresh spawn 而不是 `resume_session`
- **anti-pattern**: 当前进程立刻切到新 model 但能力表仍宣称静态；host 误走 `resume_session`；重启后仍读旧 `GEMINI_MODEL`
- **severity**: high

### [skip: 2026-04-18 QA 未拿到可用 Gemini live restart harness；仅离线核对 Unsupported 分支] Gemini：permission mode 仍是 spawn-time only，不提供 resume 快路
- **message**: spawn Gemini session in `Default` mode，调用 `set_permission_mode(plan)` 或 `set_permission_mode(bypassPermissions)`
- **expect**: 返回 `Unsupported`；host 若要应用新 mode，只能结束旧 session 后以新 mode 重建；验收时不得把“Gemini 可 resume”写入期望
- **anti-pattern**: 把 permission 变更记成已生效；文档或日志暗示 Gemini 支持 resume-based hotswap
- **severity**: medium

### [skip: 2026-04-18 QA 无可连接 agentd/session；仅离线核对 Unsupported 分支] Cteno：runtime control 明确不支持 live model / permission hotswap
- **message**: spawn Cteno session；依次调用 `set_model(...)` 与 `set_permission_mode(...)`；随后发送一条普通消息和一条需要权限决策的消息
- **expect**: `set_model` 返回 `ModelChangeOutcome::Unsupported`，`set_permission_mode` 返回 `Unsupported`；当前 session 行为保持不变；permission mode 只来自 spawn 时写入的 `agent_config.permission_mode`
- **anti-pattern**: API 返回成功但实际未切换；调用后 session 崩溃；文档把 Cteno 误写成支持 runtime model/control
- **severity**: high

### [skip: 2026-04-18 QA 无可连接 agentd/session，无法执行 reconnect/recovery 验证] Cteno：`resume_session` 只用于恢复既有 native session，不承担 hotswap
- **message**: 先用 Cteno session 完成一轮对话并记录 `native_session_id`；模拟 host reconnect，调用 `resume_session(native_session_id, hints)`；恢复后再让 agent 复述上一轮上下文
- **expect**: 会话可按既有 `native_session_id` / vendor cursor 恢复，workdir 和历史上下文延续；该路径只用于 reconnect/recovery，不应和 `set_model` / `set_permission_mode` 绑成“重启后自动热切换”
- **anti-pattern**: resume 生成全新 native session；恢复路径偷偷带上新的 model 或 permission 并改变既有语义；历史消息丢失
- **severity**: medium
