# Browser Adapter 工具测试

## meta
- kind: browser
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 12

## cases

### [pass] list 首次运行自动安装默认适配器
- **message**: "调用 browser_adapter list"
- **expect**: 首次运行时自动安装默认适配器（github/repo, twitter/search 等），返回非空列表，显示 name/domain/description
- **anti-pattern**: 返回空列表；安装失败无提示；列表缺少 domain 信息
- **severity**: high

### [pass] run 缺少必填参数
- **message**: "打开 https://github.com，调 browser_adapter run adapter_name='github/repo'，不传 args"
- **expect**: 返回错误提示缺少 required 参数 'repo'，列出需要的参数
- **anti-pattern**: 用空字符串替代必填参数执行脚本；Rust panic；无提示的 JS 错误
- **severity**: high

### [pass] run 不存在的 adapter
- **message**: "调 browser_adapter run adapter_name='nonexistent/tool'"
- **expect**: 返回清晰的 "adapter not found" 错误，建议用 list 查看可用适配器
- **anti-pattern**: Rust panic；返回空结果不说明原因
- **severity**: high

### [pass] create 后 show 验证完整性
- **message**: "用 browser_adapter create 创建一个自定义适配器：{name:'test/echo', domain:'httpbin.org', description:'Echo test', args:[{name:'msg', description:'message', required:true}], script:'return args.msg', read_only:true}，然后 show 它"
- **expect**: create 成功，show 返回完整的适配器定义，args 和 script 与创建时一致
- **anti-pattern**: create 成功但 show 找不到；字段丢失或被截断
- **severity**: high

### [pass] delete 后 run 应报错
- **message**: "先 browser_adapter list 确认有 'test/echo'，然后 delete 它，再 run 它"
- **expect**: delete 成功后 run 返回 adapter not found 错误
- **anti-pattern**: delete 后 run 仍能执行（缓存残留）；delete 报错但文件实际已删
- **severity**: medium

### [pass] create 传入非法 JSON
- **message**: "调 browser_adapter create adapter_json='{这不是合法JSON}'"
- **expect**: 返回 JSON 解析错误提示，不创建文件
- **anti-pattern**: 创建了损坏的 adapter 文件；Rust panic
- **severity**: medium
