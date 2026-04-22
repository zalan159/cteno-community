# Browser Extract 工具测试

## meta
- kind: browser
- profile: deepseek-direct
- workdir: /tmp/cteno-test
- max-turns: 10

## cases

### [pass] schema 与页面内容不匹配时的降级处理
- **message**: "打开 https://news.ycombinator.com，用 browser_extract 提取数据，schema 要求 {products: [{sku: string, price: number, warehouse: string}]}"
- **expect**: 返回空数组或带说明的降级结果，JSON 仍然符合 schema 结构（即使数据为空）
- **anti-pattern**: 幻觉编造 SKU/价格数据；返回非 JSON；崩溃报错无回复
- **severity**: high

### [flaky] 嵌套深层 schema 的提取能力
> flaky: browser_extract 对 stars 返回 null（GitHub JS 渲染，AX tree 未捕获），agent 通过 browser_state 补充恢复，最终输出正确；topics 始终为 []（GitHub JS 渲染，非工具 bug）；instruction 参数缺失问题在首次运行偶发，第二次运行消失 — 与 fixes 无关的 DeepSeek 参数传递问题 (2026-03-29 re-run)
- **message**: "打开 https://github.com/anthropics/claude-code，用 browser_extract 提取：{repo: {name: string, stars: string, language: string, about: string, topics: [string], latest_commit: {message: string, author: string}}}"
- **expect**: 正确提取仓库名、stars、语言、描述、topics 数组、最新 commit 信息，JSON 结构完整
- **anti-pattern**: 丢失嵌套层级；topics 返回单个字符串而非数组
- **severity**: high

### [skip] 无浏览器 session 时的错误处理
> skip: 测试环境中 browser session 无法隔离——前序测试留存的 Chrome tabs 在同一 Chrome 进程中持续存在，导致"无 session"场景无法复现 (2026-03-29)
- **message**: "不要打开任何网页，直接调用 browser_extract 提取 {title: string}"
- **expect**: Agent 识别需要先导航，主动调用 browser_navigate 或返回清晰错误
- **anti-pattern**: 静默返回空结果假装成功；Rust panic
- **severity**: high

### [pass] 极简 schema 但页面内容密集
- **message**: "打开 https://en.wikipedia.org/wiki/Rust_(programming_language)，用 browser_extract 提取 {summary: string}，要求 summary 不超过 100 字"
- **expect**: 返回简洁摘要，长度合理，内容准确反映页面主题
- **anti-pattern**: 返回整个页面文本；摘要与页面主题无关
- **severity**: medium

### [pass] instruction 模糊歧义时的表现
- **message**: "打开 https://github.com/trending，用 browser_extract，instruction 写 '提取重要的东西'，schema: {items: [{text: string}]}"
- **expect**: Agent 合理解释 "重要的东西"（如 trending repos），返回有意义的数据
- **anti-pattern**: 返回空结果；提取了页面 footer 等无关内容
- **severity**: medium
