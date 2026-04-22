# Local IPC Gateway

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 10

## cases

### [pending] local_rpc fallback when registry empty
- **message**: "在 Tauri 环境下，RPC registry 未注册任何 handler 时调用 local_rpc，应返回错误并降级到 Socket.IO"
- **expect**: 调用不崩溃，自动降级到远程 RPC
- **anti-pattern**: 前端无限等待或白屏
- **severity**: high

### [pending] local_rpc handles concurrent calls
- **message**: "同时发起多个 local_rpc 调用（如 list-skills + list-agents + list-profiles），应全部正确返回"
- **expect**: 所有调用独立完成，结果正确
- **anti-pattern**: 死锁或串行阻塞
- **severity**: high

### [pending] local_rpc socket 迁移到 cteno-host-bridge-localrpc 后路径保持兼容
- **message**: 在新构建的 daemon 下观察 socket 路径，并用旧 ctenoctl 二进制（假设它仍用 `~/.agents/daemon.{env}.sock` 约定）连接
- **expect**: socket 路径未变，旧 client 连接成功
- **anti-pattern**: socket 文件名改变；env_tag 在 bridge crate 与 runtime crate 解析出现差异导致双写两条 socket
- **severity**: high

### [pending] LocalRpcAuthGate 未命中时正确 fallthrough 到 RpcRegistry
- **message**: 调用一个未注册的非 `auth.*` method（如 `nonexistent.method`），观察返回
- **expect**: `AppLocalRpcAuthGate::handle` 返回 `Ok(None)`，registry 给出 "Method not found" 错误，socket 连接保持
- **anti-pattern**: interceptor 吞掉非 auth 方法；socket 被关闭；panic
- **severity**: high

### [pending] auth.trigger-reauth 在 community build 下返回明确错误
- **message**: 用 `cargo build --no-default-features --bin cteno-agentd` 构建 daemon，启动后调 `auth.trigger-reauth`
- **expect**: error 明确告知 "Machine reauth signal is not available in community build" 或同语义，socket 保持
- **anti-pattern**: panic；silent success；socket 断开
- **severity**: medium
