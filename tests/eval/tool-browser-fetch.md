# Browser Fetch 工具测试

## meta
- kind: browser
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 10

## cases

### [pass] 无浏览器 session 时调用 fetch
- **message**: "不要打开浏览器，直接用 browser_fetch 请求 https://httpbin.org/get"
- **expect**: Agent 识别需要先有浏览器 session，主动 browser_navigate 或返回清晰错误提示
- **anti-pattern**: Rust panic；静默返回空结果
- **severity**: high

### [pass] POST 请求 body 格式错误
- **message**: "打开 https://httpbin.org，用 browser_fetch POST https://httpbin.org/post，body 写成非法 JSON：{name: 缺少引号}"
- **expect**: 返回错误提示 body 格式无效，或按原样发送并返回服务端响应
- **anti-pattern**: 崩溃无回复；吞掉错误假装成功
- **severity**: high

### [pass] 404 响应的处理
- **message**: "打开 https://httpbin.org，用 browser_fetch GET https://httpbin.org/status/404"
- **expect**: 正常返回 status:404 和 statusText，不把 HTTP 错误码当工具执行失败
- **anti-pattern**: 把 404 当作工具报错；不返回 status code
- **severity**: high

### [pass] 需要认证的 API 带 cookie 透传
- **message**: "先打开 https://github.com 并确认已登录状态，然后用 browser_fetch GET https://api.github.com/user"
- **expect**: 如果浏览器已登录 GitHub，fetch 应携带 cookie 返回用户信息；未登录则返回 401 并正确展示
- **anti-pattern**: 忽略浏览器 cookie 导致始终 401；编造用户数据
- **severity**: medium
- **last-run**: 2026-03-29 (profile: deepseek-direct)

### [pass] 不支持的 HTTP method
- **message**: "打开 https://httpbin.org，用 browser_fetch 发送 method=TRACE 到 https://httpbin.org/get"
- **expect**: 返回错误提示 method 不支持（TRACE 不在 enum 中），或降级为 GET 并说明
- **anti-pattern**: 静默忽略 method 参数；Rust panic
- **severity**: medium
