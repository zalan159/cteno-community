# Browser Network 工具测试

## meta
- kind: browser
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 15

## cases

### [pass] 未 start_capture 直接 get_requests
- **message**: "打开 https://news.ycombinator.com，不要 start_capture，直接调用 browser_network get_requests"
- **expect**: 返回空列表或清晰提示需要先 start_capture
- **anti-pattern**: Rust panic；返回旧 session 的残留数据
- **severity**: high

### [pass] 页面导航后 capture 失效的恢复
- **message**: "打开 https://httpbin.org，调 browser_network start_capture，然后导航到 https://news.ycombinator.com，再 get_requests"
- **expect**: Agent 意识到页面导航后拦截器被重置，结果为空或主动重新 start_capture
- **anti-pattern**: 返回导航前的请求数据假装是新页面的；不提示 capture 已失效
- **severity**: high

### [pass] filter 过滤精度
- **message**: "打开 https://github.com/anthropics，先 start_capture，等页面加载完，然后 get_requests filter='api' method_filter='GET'"
- **expect**: 只返回 URL 含 'api' 的 GET 请求，不包含图片/CSS/其他请求
- **anti-pattern**: 返回所有请求无视 filter；filter 大小写敏感导致遗漏
- **severity**: medium

### [pass] stop_capture 后再 start_capture 的状态隔离
- **message**: "打开 https://httpbin.org，start_capture → 触发几个请求 → stop_capture → start_capture → get_requests"
- **expect**: 第二次 start_capture 后 get_requests 返回空（旧数据已清除）
- **anti-pattern**: 返回上一轮 capture 的残留数据
- **severity**: medium

### [pass] max_requests 边界
- **message**: "打开一个请求密集的页面（如 https://twitter.com），start_capture，等 5 秒，get_requests max_requests=3"
- **expect**: 最多返回 3 条请求记录
- **anti-pattern**: 忽略 max_requests 返回全部；返回数量超过 3
- **severity**: low
