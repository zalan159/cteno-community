# Browser Trace 工具测试

## meta
- kind: browser
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 15

## cases

### [pass] 未启动 trace 时 get 的容错
- **message**: "打开 https://example.com，不要 start trace，直接调 browser_trace get"
- **expect**: 返回空结果或提示 "没有活跃的 trace 录制"
- **anti-pattern**: Rust panic；返回 null 无说明
- **severity**: high

### [pass] 录制操作并验证 XPath 和元素信息
> pass: 修复验证通过 — action 执行前快照 xpath/role/name，导航型点击后元素信息完整保留。xpath="/html[1]/…/a[1]"，role="link"，name="Learn more"，timestamp 非零。 (2026-03-29)
- **message**: "打开 https://example.com，browser_trace start，然后点击页面上的 'More information...' 链接，再 browser_trace get"
- **expect**: trace 事件包含 click 动作，element 有 XPath、role(link)、name 信息，URL 为 example.com
- **anti-pattern**: 事件缺少 XPath；role/name 全为 null；timestamp 为 0
- **severity**: high

### [pass] stop 后保存到不存在的目录路径
- **message**: "打开 https://example.com，start trace，点击一个链接，然后 browser_trace stop save_path='/tmp/cteno-test/nonexistent-dir/deep/trace.json'"
- **expect**: 返回错误提示目录不存在，或自动创建目录并保存成功
- **anti-pattern**: 静默丢失 trace 数据；Rust panic
- **severity**: high

### [pass] clear 后 get 应为空但录制仍在继续
- **message**: "打开 https://example.com，start trace，点击链接，clear trace，确认 get 为空，再点击另一个元素，get 应有新事件"
- **expect**: clear 后 get 返回空数组；之后的操作继续被录制，get 返回新事件
- **anti-pattern**: clear 后录制也停止了；clear 没有真正清空
- **severity**: medium

### [pass] 多次 start 覆盖旧 trace
- **message**: "打开 https://example.com，start trace name='first'，点击一个元素，start trace name='second'，get trace"
- **expect**: 第二次 start 替换了第一次的 trace，get 返回空或仅含 second 之后的事件
- **anti-pattern**: 两次 trace 的事件混在一起；第一次 trace 的数据泄露到第二次
- **severity**: medium
