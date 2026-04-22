# Orchestration Skill 测试

## meta
- kind: persona
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-orchestration-test
- max-turns: 30

## setup
```bash
rm -rf /tmp/cteno-orchestration-test
mkdir -p /tmp/cteno-orchestration-test
```

## cases

### [pass] 代码实现+验证循环
> fail: orchestrate.sh 生成了但 dispatch 调用全部失败 "Owner 'test' not found"——eval 上下文创建的 eval persona 的 ID 未注入到脚本里，脚本使用占位符 "YOUR_PERSONA_ID"。Persona 回退到自己直接写 calc.py（反模式）。state.json 显示 task-1 stuck after 5 attempts。(2026-04-05)
> fail: Persona ID 注入修复有效（不再有 YOUR_PERSONA_ID 占位符），但 Persona 生成的脚本使用了不存在的命令 `ctenoctl agent run implementer`（正确应为 `ctenoctl dispatch $PERSONA_ID -t implementer`），导致脚本执行后报 0/1 done。Persona 随即回退到 implement.sh/verify.sh 内联写代码的方式（反模式），calc.py 由 shell script heredoc 直接写出，并非通过 Worker dispatch 完成。根因：SKILL.md 第三步调度模板用 `ctenoctl dispatch "$PERSONA_ID"` 但 Persona 把它替换成 `ctenoctl agent run`（不存在的命令）。(2026-04-05)
> pass: 端到端验证通过。SKILL.md 修复（明确禁止 ctenoctl agent run）+ manager.rs 修复（persona kind → AgentKind::Persona + eval-persona-singleton session linking）后，Persona 正确生成 orchestrate_calc.sh：PERSONA_ID="eval-persona-singleton"（无占位符）；AGENT.md 创建在 .cteno/agents/implementer/ 和 .cteno/agents/verifier/；ctenoctl dispatch --wait 调度两个 Worker；implementer 完成 calc.py（递归下降解析器）；verifier 运行 8 类测试全部通过并写入 result.md Status: PASS；state.json 最终 status: done。(2026-04-05)
- **message**: "在 /tmp/cteno-orchestration-test 下创建一个 Python 模块 calc.py，实现一个函数 evaluate(expr: str) -> float，支持加减乘除和括号表达式（如 '(1+2)*3' 返回 9.0）。要求：用编排脚本自动完成，需要有实现和验证两个阶段，不要自己直接写代码。"
- **expect**: 激活 orchestration skill；创建至少两个角色的 AGENT.md（实现者 + 验证者）；生成编排脚本含 state.json 和 implement/test 循环；通过 ctenoctl dispatch 调度 Worker；最终产出可运行的 calc.py 和通过的测试
- **anti-pattern**: Persona 自己直接写 calc.py 代码而不 dispatch；没有验证阶段（只实现不测试）；没有 state.json 或类似状态追踪；没有创建专用 Agent 角色直接用通用 worker
- **severity**: high

### [pass] 简单任务不过度编排
- **message**: "在 /tmp/cteno-orchestration-test 下创建一个 hello.py，内容是 print('hello world')"
- **expect**: 直接执行或单次 dispatch 完成，不激活 orchestration skill，不生成编排脚本
- **anti-pattern**: 为 hello.py 这样的简单任务激活 orchestration skill 或生成编排脚本
- **severity**: medium

### [pass] 静态流水线用 DAG 不用编排脚本
- **message**: "帮我做三步：1) 在 /tmp/cteno-orchestration-test/step1.txt 写入当前日期 2) 读取 step1.txt 内容 3) 把内容写入 step2.txt 并追加 ' - processed'。用任务派发完成，不要自己做。"
- **expect**: 用 dispatch_task DAG 模式（tasks 数组 + depends_on）或顺序 dispatch 单任务；不需要编排脚本因为步骤固定无循环
- **anti-pattern**: 生成完整编排脚本（无循环/条件分支的场景不需要 orchestration）
- **severity**: medium

### [pass] dispatch --skill 参数传递
> fail: CLI → RPC 的参数链路正常（ctenoctl_cli.rs:583 设置 skillIds，manager.rs:2342 读取并传给 dispatch_task）。但 spawn_session_internal（manager.rs:4530）接收 skill_ids 参数后完全忽略，无任何预激活逻辑。Worker 只能在 Available Skills 列表中看到 browser-automation，而非预激活的 <activated_skill> 内容。--skill 参数对 Worker 行为无实际影响。(2026-04-05)
> pass: SessionAgentConfig 新增 pre_activated_skill_ids 字段，spawn_session_internal 在 skill_ids 非空时将其赋值到 agent_config.pre_activated_skill_ids，session.rs 的 runtime_context_messages 在 startup 时注入完整 <activated_skill> 块。日志确认：[Session] Pre-activated skill: browser-automation for session cmnkqtav8004vwhgmivrsw9vg。Worker 收到完整 skill 上下文。(2026-04-05)
- **message**: "用 ctenoctl dispatch 派发一个任务，测试 --skill 参数是否能正常传递。派发时指定 --skill browser-automation，然后检查 Worker 是否收到了该 skill 的激活指令。"
- **expect**: ctenoctl dispatch 命令带 --skill 参数不报错；Worker 在 session 中能看到 browser-automation skill 已激活
- **anti-pattern**: --skill 参数被忽略（之前的 bug：RPC handler 写死 None）
- **severity**: high
