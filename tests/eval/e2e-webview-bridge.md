# E2E: Webview Bridge 基础验证

## meta
- kind: e2e
- max-steps: 15

## cases

### [pending] webview eval 基础运算
- **steps**:
  1. `ctenoctl webview eval "return 1+1"` 应返回 2
  2. `ctenoctl webview eval "return 'hello' + ' world'"` 应返回 hello world
- **expect**: eval 能执行 JS 并返回正确结果
- **anti-pattern**: 超时、返回 undefined、报错
- **severity**: high

### [pending] webview eval DOM 访问
- **steps**:
  1. `ctenoctl webview eval "return document.title"` 返回页面标题
  2. `ctenoctl webview eval "return document.querySelector('body') !== null"` 返回 true
  3. `ctenoctl webview eval "return typeof window.__TAURI_INTERNALS__"` 返回 object（确认 Tauri IPC 可用）
- **expect**: 能正常访问 DOM 和 Tauri 环境
- **anti-pattern**: Tauri internals 不存在、DOM 访问失败
- **severity**: high

### [pending] webview eval 异步支持
- **steps**:
  1. `ctenoctl webview eval "return await new Promise(r => setTimeout(() => r('async-ok'), 100))"` 返回 async-ok
  2. `ctenoctl webview eval "return await fetch('/').then(r => r.status)"` 验证 fetch 可用
- **expect**: async/await 正常工作
- **anti-pattern**: Promise 未 resolve、超时
- **severity**: high

### [pending] webview eval 错误处理
- **steps**:
  1. `ctenoctl webview eval "throw new Error('test-error')"` 应返回 error 信息包含 test-error
  2. `ctenoctl webview eval "return nonExistentVariable"` 应返回 ReferenceError
- **expect**: 错误被捕获并返回，不会导致 channel 挂起
- **anti-pattern**: 超时（说明错误没被 catch）、daemon crash
- **severity**: high

### [pending] webview screenshot 截图
- **steps**:
  1. `ctenoctl webview screenshot` 返回 JSON 包含 path/width/height
  2. 检查返回的 path 文件确实存在
- **expect**: 截图文件存在，尺寸合理
- **anti-pattern**: 窗口找不到、截图 0 字节、path 不存在
- **severity**: high
