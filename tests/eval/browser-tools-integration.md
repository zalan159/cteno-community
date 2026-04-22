# Browser 新工具集成测试

## meta
- kind: browser
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 20

## cases

### [pass] network + fetch 联动：捕获 fetch 发出的请求
- **message**: "打开 https://httpbin.org，browser_network start_capture，然后 browser_fetch GET https://httpbin.org/headers，再 browser_network get_requests"
- **expect**: network 捕获到 browser_fetch 发出的请求，URL 含 /headers，method=GET，status=200
- **anti-pattern**: fetch 发出的请求未被 network 捕获；两个工具的结果矛盾
- **severity**: high
- **note**: 修复后 agent 正确调用了 browser_network start_capture + browser_fetch GET + browser_network get_requests，network 捕获到 1 条请求：GET https://httpbin.org/headers → 200 (297ms)。browser_prompt.rs 新增工具指引有效 (2026-03-29)

### [pass] trace + extract 联动：录制操作后提取数据
- **message**: "打开 https://news.ycombinator.com，browser_trace start，点击第一条新闻标题，browser_trace stop，然后 browser_extract 提取当前页面 {title: string, url: string}"
- **expect**: trace 记录了 click 事件，extract 提取的是点击后的页面内容（非原始 HN 页面）
- **anti-pattern**: extract 提取的仍是 HN 首页数据；trace 丢失了点击事件
- **severity**: high
- **note**: XPath 重复段 bug 修复验证通过 (2026-03-29)。walk_dom_tree 不再产生 /html[1]/html[1]/body[1]/body[1]/... 这类重复段，trace 事件 xpath 格式正确：/html[1]/body[1]/center[1]/table[1]/... 无重复。trace 录制 1 个 click 事件，extract 返回点击后页面（Submissions from sytse.com | Hacker News）。profile 注意：proxy-deepseek-reasoner 本地 token 无效，实测用 deepseek-direct。

### [pass] adapter run + network 联动：观察适配器的网络行为
- **message**: "打开 https://github.com，browser_network start_capture，browser_adapter run adapter_name='github/repo' args={repo:'anthropics/claude-code'}，然后 browser_network get_requests filter='api.github.com'"
- **expect**: network 捕获到 adapter 脚本内部发出的 API 请求
- **anti-pattern**: adapter 的内部 fetch 调用未被 network 拦截器捕获
- **severity**: medium
- **note**: 修复后通过 (2026-03-29)。`credentials: 'include'` 移除后，adapter 的 fetch 不再触发 CORS 失败。network 成功捕获 2 条 `api.github.com` 请求（1 条 ERROR/首次 start_capture 前遗留，1 条 200）。adapter 返回正确仓库数据（stars: 83867，forks: 7088，language: Shell）。profile 注意：proxy-deepseek-reasoner token 失效，实测用 deepseek-direct。

### [pass] 全链路：导航 → 录制 → 操作 → 提取 → 保存 trace
- **message**: "打开 https://en.wikipedia.org/wiki/Main_Page，开始 trace 录制 name='wiki-flow'，在搜索框输入 'Rust programming'，点击搜索按钮，等页面加载，用 browser_extract 提取 {title: string, first_paragraph: string}，停止 trace 并保存到 /tmp/cteno-test/wiki-trace.json"
- **expect**: 1) trace 包含 type+click 事件 2) extract 返回 Rust 语言页面数据 3) trace 文件成功保存且为合法 JSON
- **anti-pattern**: 任一步骤失败导致后续全部跳过；trace 文件为空
- **severity**: high
- **note**: 修复后通过：trace 保存 6 个事件（click×3 + type×2 + key_press），含 type+click；extract 返回 "Rust (programming language)" 而非搜索结果页；trace 文件 /tmp/cteno-test/wiki-trace.json 合法 JSON。无 CDP 断连。xpath 仍有段路径重复 (walk_dom_tree 的已知 bug，不影响本用例通过标准) (2026-03-29)
