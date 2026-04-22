# Browser 工具体系测试

## meta
- kind: browser
- profile: proxy-kimi-k2.5
- workdir: /tmp/cteno-test
- max-turns: 25

## cases

### [pass] JS alert 对话框不阻塞操作
- **message**: "用 browser_navigate 打开 about:blank，然后用 browser_action evaluate 执行 `setTimeout(() => alert('test alert'), 500); 'scheduled'`，等 2 秒后再 browser_action screenshot 截图"
- **expect**: alert 被自动处理，screenshot 正常返回不超时
- **anti-pattern**: Runtime.evaluate 超时；CDP 连接断开
- **severity**: high

### [pass] confirm() 对话框自动 accept 返回 true
- **message**: "用 browser_navigate 打开 about:blank，执行 JS：`window.__result = 'pending'; setTimeout(() => { window.__result = confirm('Proceed?') ? 'yes' : 'no'; }, 300); 'ok'`，等 1 秒后用 evaluate 读取 `window.__result`"
- **expect**: `window.__result` 为 `'yes'`
- **anti-pattern**: `window.__result` 仍为 `'pending'`
- **severity**: high

### [pending] browser_cdp 能发送 DOM.setFileInputFiles 上传文件
- **message**: "用 browser_navigate 打开 about:blank，用 browser_action evaluate 注入一个 file input：`document.body.innerHTML='<input type=file id=f>'; 'done'`，然后用 shell 创建一个测试文件 `echo test > /tmp/cdp-test.txt`，再用 browser_cdp 发送 DOM.getDocument，然后 DOM.querySelectorAll selector='input[type=file]' 拿到 nodeId，最后 DOM.setFileInputFiles 注入文件"
- **expect**: DOM.setFileInputFiles 成功返回（无 error），文件被设置到 input 上
- **anti-pattern**: CDP 命令返回错误；Agent 不知道怎么用 browser_cdp
- **severity**: high

### [pending] browser_network CDP 模式捕获所有请求
- **message**: "用 browser_navigate 打开 https://httpbin.org，browser_network start_capture，然后用 browser_action evaluate 执行 `fetch('/get').then(r=>r.status)`，最后 browser_network get_requests"
- **expect**: 捕获到 GET /get 请求，status 200
- **anti-pattern**: 请求列表为空（CDP Network.enable 没有生效）
- **severity**: high

### [pass] 下载行为不弹对话框阻塞操作
- **message**: "打开一个 PDF 文件链接（如 https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf），确认操作不被阻塞，然后 browser_action screenshot"
- **expect**: screenshot 正常返回
- **anti-pattern**: 操作被下载对话框阻塞
- **severity**: medium

### [pending] 探索方法论：Agent 探索新站点使用 CDP + network
- **message**: "探索 https://news.ycombinator.com 这个站点，了解它有哪些可以 API 化的功能，分析后给出总结"
- **expect**: Agent 使用 browser_network 捕获请求，分析 API 模式，给出总结
- **anti-pattern**: 不开 network 就直接看页面；只截图不分析 API
- **severity**: medium
