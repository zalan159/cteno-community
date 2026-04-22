# Persona 自进化测试

## meta
- kind: persona
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 20

## cases

### [pass] Browser 任务完成后 Persona 应萃取任务模板
- **message**: "[Task Complete] agent_type=browser, session=test-session-001, 状态=成功。任务：打开 https://httpbin.org 并获取 /headers 接口内容。结果：{\"headers\":{\"Accept\":\"*/*\",\"User-Agent\":\"Mozilla/5.0\",\"X-Real-Ip\":\"123.45.67.89\"}}"
- **expect**: Persona 收到 [Task Complete] 后调用 memory save，将任务模板（agent_type=browser + 适合的 profile 组合）保存到记忆，不保存具体的 headers 数据本身
- **anti-pattern**: 收到结果后不做任何知识沉淀；把具体的 headers 返回数据原样存入 memory
- **severity**: high

### [pass] 工具报错后 Persona 应记录经验教训
- **message**: "用 shell 执行 `cat /nonexistent/path/file.txt`，然后告诉我内容"
- **expect**: shell 报错后 Persona 解决问题（告知文件不存在），并将此类错误模式保存到「经验教训」
- **anti-pattern**: 遇到错误只汇报不记录；记录了但没有解决方案
- **severity**: high

### [pass] 失败任务后 Persona 应记录失败模式
- **message**: "[Task Complete] agent_type=browser, session=test-session-002, 状态=失败。任务：登录 https://example.com/login。错误：页面需要验证码（CAPTCHA），无法自动完成登录流程。请人工处理。"
- **expect**: Persona 收到失败 [Task Complete] 后调用 memory save，将失败原因（CAPTCHA）保存到「失败记录」，并包含至少一个解决方案
- **anti-pattern**: 失败后不记录任何信息；下次遇到同一网站还是没有任何经验
- **severity**: high
