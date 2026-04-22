# ctenoctl dispatch --wait 阻塞调度测试

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

### [pass] dispatch --wait 基本阻塞返回
- **message**: 用 `ctenoctl persona list` 获取一个 persona ID，然后执行 `ctenoctl dispatch <persona_id> --wait -m "用 shell 执行 echo hello_from_worker 并返回结果" --timeout 120`。验证返回的 JSON 包含 response 字段且包含 "hello_from_worker"。
- **expect**: CLI 阻塞直到 worker 完成，返回 JSON 包含 `"success": true` 和 `"response"` 字段，response 中包含 worker 的输出
- **anti-pattern**: 立即返回只有 sessionId 没有 response；CLI 挂起不返回；返回 success: false
- **severity**: high

### [pass] dispatch --wait 超时返回错误
- **message**: 获取一个 persona ID，执行 `ctenoctl dispatch <persona_id> --wait --timeout 3 -m "请等待 60 秒后再回复"`。验证 3 秒后返回超时错误而不是无限挂起。
- **expect**: 约 3 秒后返回包含 "timed out" 的错误信息，不会无限阻塞
- **anti-pattern**: CLI 永远不返回；返回 success: true（不应该成功）；panic 崩溃
- **severity**: high

### [pass] dispatch 不带 --wait 立即返回
- **message**: 获取一个 persona ID，执行 `ctenoctl dispatch <persona_id> -m "echo test"`（不带 --wait）。验证立即返回 sessionId。
- **expect**: 立即返回 JSON 包含 `"success": true` 和 `"sessionId"` 字段，没有 response 字段（因为没等待）
- **anti-pattern**: 阻塞等待 worker 完成；返回 response 字段；失败
- **severity**: medium
