# Cteno Runtime DAG

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-runtime-dag
- max-turns: 15

## setup
```bash
rm -rf /tmp/cteno-runtime-dag
mkdir -p /tmp/cteno-runtime-dag
```

## cases

### [pending] 并行 root + fan-in 汇总
- **message**: "用 DAG 派发三个任务：a 写 a.txt，b 写 b.txt，c 依赖 a/b 合并为 c.txt。不要自己直接写文件。"
- **expect**: a/b 并行启动；c 等待 a/b 完成；c prompt 含两个上游结果；最终 `[Task Group Complete]`
- **anti-pattern**: host `TaskGraphEngine` 推进；c 提前执行；缺失上游结果
- **severity**: high

### [pending] 环依赖拒绝
- **message**: "用 DAG 执行 a depends_on b, b depends_on a。"
- **expect**: runtime 返回 circular dependency 错误，不启动任何 subagent
- **anti-pattern**: 卡住等待；创建部分 worker；host 代为处理
- **severity**: high

### [pending] 上游失败阻塞下游
- **message**: "DAG 中 root 任务故意执行不存在命令，下游依赖它。"
- **expect**: root failed；下游 blocked/failed_due_to_dependency；summary 显示失败
- **anti-pattern**: 下游继续运行；最终显示全部完成
- **severity**: high

### [pending] 重名 task id 拒绝
- **message**: "提交两个 id 都是 build 的 DAG 节点。"
- **expect**: duplicate task IDs 错误
- **anti-pattern**: 后一个覆盖前一个；随机执行一个
- **severity**: medium

### [pending] host 不拥有 DAG 状态
- **message**: "运行一个两层 DAG，并检查日志/事件确认推进来自 cteno-agent-runtime。"
- **expect**: 推进来自 runtime；desktop 只记录 ACP/native events
- **anti-pattern**: 调用 `PersonaManager::dispatch_task_graph`
- **severity**: high

### [pending] subagent 权限继承父 session
- **message**: "用 DAG 派发一个 root 任务，让 subagent 执行需要审批的 shell/edit 操作；保持默认 permission mode，不要 bypass。"
- **expect**: subagent 工具调用通过父 session 发出 `permission_request`；用户批准后节点继续；拒绝后节点 failed 且下游 blocked；sandbox policy 与父 session 一致
- **anti-pattern**: subagent 自动放行；权限请求丢失导致卡住；subagent 使用独立 session/request id 让 host 无法 respond；read_only/plan 模式被绕过
- **severity**: high
