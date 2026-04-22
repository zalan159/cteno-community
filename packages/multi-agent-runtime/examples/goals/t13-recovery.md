# T13 — Recovery path executor 注入

## Background

当前 `apps/client/desktop/src/happy_client/session/recovery.rs::connect_restored_session`（或等价函数名）仍走 legacy 路径（直接调 `execute_autonomous_agent_with_session`），没通过 `executor.resume_session(native_session_id, hints)` 启动 subprocess。

T11b 已接通 **spawn** 路径（新 session 默认走 executor），但 **resume/recovery** 漏了。daemon 重启或 session 历史恢复时，如果是 Cteno vendor，应通过 `CtenoAgentExecutor::resume_session` 再 spawn 一个 cteno-agent subprocess，而不是 in-process re-execute。

## Scope

1. 修改 `apps/client/desktop/src/happy_client/session/recovery.rs`：
   - 在 `connect_restored_session` 的 Cteno 路径（检测 agent_id 或 vendor == "cteno"）加 `executor.resume_session(native_id, ResumeHints { ... })` 调用
   - native_session_id 从 `AgentSession` SQLite row 读（Wave 2.2b / T2 已加 vendor 字段；native_session_id 字段是否存在需 recon — 如果没有，加一个 TODO 说明持久化缺口）
2. 失败时 fallback 到 legacy（executor 不可用 / resume 报错）
3. Normalizer / SessionConnection 的 executor/session_ref 字段要注入（参照 T11b B2 `try_spawn_executor_session` 的模式）
4. Claude/Codex vendor 暂不处理（它们在 legacy 路径下也没在 local SQLite 存，属 P1 遗留）

## Non-scope

- 不改 AgentSession schema（本轮只利用既有字段）
- 不改 multi-agent-runtime / cteno-agent-stdio
- 不写 git commit，只到 cargo check 通过即可

## Acceptance

- 双形态 `cargo check` 通过（apps/client/desktop + --no-default-features）
- `recovery.rs` 新 executor path 有 opt-in 判断（vendor == "cteno" && EXECUTOR_REGISTRY.is_some()）
- 改动范围控制在 `happy_client/session/recovery.rs` + 可能的 `connection.rs`（如需加字段）+ `executor_session.rs`（可能复用现有 helper）

## Hints for planner

- 看 `apps/client/desktop/src/happy_client/session/spawn.rs::try_spawn_executor_session`（T11b B2 实现）作为参考模板
- `connect_restored_session` 入口在哪里可通过 `git log --grep restore_active_sessions` 查找
- AgentSessionManager::get_session() 返回 AgentSession 含 session_id / agent_id / vendor / messages
