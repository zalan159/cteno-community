# ctenoctl RPC 客户端测试

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 15

## setup
```bash
mkdir -p /tmp/cteno-test
```

## cases

### [pending] daemon 未运行时 CLI 应提示启动
- **message**: 在 daemon 未运行的环境下执行 `ctenoctl persona list`
- **expect**: 输出包含 "Daemon not running" 的错误提示，建议启动桌面应用
- **anti-pattern**: 崩溃 panic；尝试自己初始化服务；卡住不返回
- **severity**: high

### [pending] CLI 通过 RPC 列出 Persona
- **message**: daemon 运行中，执行 `ctenoctl persona list`
- **expect**: 返回 JSON 格式的 Persona 列表，`success: true`，personas 数组
- **anti-pattern**: 返回空列表（数据不一致）；连接失败；CLI 自己初始化服务
- **severity**: high

### [pending] CLI 通过 RPC 列出工具
- **message**: daemon 运行中，执行 `ctenoctl tool list`
- **expect**: 返回 JSON 格式的工具列表，包含 id/name/description/category
- **anti-pattern**: 空列表；RPC 方法找不到
- **severity**: high

### [pending] CLI 通过 RPC 执行工具
- **message**: daemon 运行中，执行 `ctenoctl tool exec shell --input '{"command":"echo hello"}'`
- **expect**: 通过 daemon RPC 执行 shell 工具，返回包含 "hello" 的输出
- **anti-pattern**: CLI 自己初始化服务执行；超时；工具找不到
- **severity**: high

### [pending] CLI run 命令调用 agent.execute
- **message**: daemon 运行中，执行 `ctenoctl run --kind worker -m "用 shell 执行 echo hello world"`
- **expect**: 通过 RPC 调用 agent.execute，agent 执行完成后返回 JSON 结果
- **anti-pattern**: CLI 自己启动 agent 循环；连接断开后无提示
- **severity**: high

### [pending] status 命令显示 socket 路径
- **message**: 执行 `ctenoctl status`
- **expect**: 输出包含 daemon_socket 路径和 daemon_socket_exists 状态
- **anti-pattern**: 崩溃；缺少 socket 信息
- **severity**: medium

### [pending] daemon 重启后 CLI 自动重连
- **message**: 先 `ctenoctl status`（正常），重启 daemon，再 `ctenoctl persona list`
- **expect**: 第二次请求自动检测到新 daemon 并连接成功（每次请求建新连接）
- **anti-pattern**: 连接旧 socket 失败后不重试；挂死
- **severity**: medium

### [pending] --target agentd 正确切换到 headless identity（P1 回归）
- **message**: `ctenoctl --target agentd status`
- **expect**: `identity.shellKind == "agentd"`，`app_data_dir` 指向 headless 目录（而非 Tauri release），socket 路径带对应 env_tag
- **anti-pattern**: target override 无效仍走 GUI identity；identity 落到 Tauri release 目录；env_tag 误判
- **severity**: high

### [pending] daemon 重启后 completion registry 不泄漏旧 session 绑定
- **message**: 启动 daemon → `register_completion(session_a)` → 不 complete → kill daemon → 重启 daemon → 再 `register_completion(session_a)`
- **expect**: 第二次 register 能正常拿到 oneshot receiver，不被旧 tx 污染；`try_complete_cli_session` 可成功唤醒
- **anti-pattern**: `OnceLock<CompletionMap>` 残留旧 tx 导致新 session 收不到 complete 信号；panic on Duplicate
- **severity**: medium

### [pending] env_tag_from_data_dir 对异常目录名 fallthrough 到 release
- **message**: `app_data_dir` 名为 `Cteno.test`（非 `.dev`/`.preview` 后缀）
- **expect**: `env_tag == ""`（release），socket 路径 `~/.agents/daemon.sock`（无 env 后缀）
- **anti-pattern**: 把 "test" 当成未知环境抛异常；误判为 dev
- **severity**: medium
