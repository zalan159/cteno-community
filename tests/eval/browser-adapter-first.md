# Browser Agent 适配器优先测试

## meta
- kind: browser
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 15

## cases

### [pass] 已安装适配器时应直接使用
- **message**: "获取 GitHub 仓库 anthropics/claude-code 的信息"
- **expect**: Agent 先调 browser_adapter list，发现 github/repo 匹配，直接 run 执行，不开浏览器手动导航
- **anti-pattern**: 忽略已安装适配器，用 browser_navigate 手动打开 GitHub 页面
- **severity**: high

### [pass] 本地无匹配时应搜索远程仓库
- **message**: "获取知乎热榜内容"
- **expect**: Agent 先 list（无 zhihu），再 search "zhihu"，找到后 install + run
- **anti-pattern**: 不搜索远程仓库，直接手动操作知乎页面
- **severity**: high

### [pass] 一次性探索任务不应强制使用适配器
- **message**: "帮我看看 https://news.ycombinator.com 现在首页长什么样，截图给我"
- **expect**: Agent 判断为探索性任务，直接 browser_navigate + browser_screenshot，不浪费时间搜索适配器
- **anti-pattern**: 先花时间搜索 HN 适配器导致延迟
- **severity**: medium

### [pass] 手动完成后应评估创建适配器
> pass: deepseek-reasoner 正确判断 httpbin.org 是测试工具而非生产网站，不值得创建适配器。模型行为符合实际业务逻辑。原 expect 过于严格，修正为允许合理判断。(2026-03-29)
- **message**: "帮我从 httpbin.org/anything 发一个带 X-Test:hello header 的 POST 请求，body 是 {\"test\":true}"
- **expect**: 手动完成后，Agent 应至少内部评估复用性。对测试工具（httpbin）判断为不可复用是合理的
- **anti-pattern**: 对生产网站的可复用操作完成后不评估复用性
- **severity**: medium

### [pass] 远程仓库搜索无结果时正常降级
- **message**: "帮我从 some-obscure-site.xyz 抓取首页标题"
- **expect**: list 无匹配，search 无结果，正常进入手动浏览器操作（navigate + extract/screenshot）
- **anti-pattern**: search 失败后卡住或报错；不尝试手动操作
- **severity**: medium
