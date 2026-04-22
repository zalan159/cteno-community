# E2E: App Navigation & Basic UI

## meta
- kind: e2e
- max-steps: 30

## setup
```bash
source tests/e2e/browser-helpers.sh
e2e_check_prereqs
```

## cases

### [pending] 首页加载并渲染核心 UI 元素
- **steps**:
  1. `ctenoctl webview eval "return document.title"` 确认页面已加载
  2. `ctenoctl webview screenshot` 截图确认非白屏
  3. `ctenoctl webview eval "return document.querySelector('body').children.length > 0"` 确认 DOM 非空
- **expect**: 页面正常加载，DOM 有内容，截图非白屏
- **anti-pattern**: 白屏、JS 报错、DOM 为空
- **severity**: high

### [pending] 页面路由切换不崩溃
- **steps**:
  1. eval 获取当前页面标识元素
  2. eval 点击导航中的不同入口
  3. 每次切换后 eval 检查页面特征元素 + screenshot
  4. eval 模拟浏览器后退
- **expect**: 每个路由都能正常渲染，前进后退不崩溃
- **anti-pattern**: 路由切换白屏、React error boundary、组件未挂载
- **severity**: high

### [pending] 发送消息的完整业务流程
- **steps**:
  1. eval 进入一个已有的会话
  2. eval 找到消息输入框，输入 "E2E 测试消息"
  3. eval 点击发送按钮（或触发 Enter）
  4. eval + await 等待消息出现在对话列表
  5. screenshot 验证最终显示
- **expect**: 消息成功发送，出现在对话列表中
- **anti-pattern**: 消息卡在 loading、输入框不响应、发送后无反馈
- **severity**: high

### [pending] 滚动加载历史消息不丢失内容
- **steps**:
  1. eval 进入一个有较多历史消息的会话
  2. eval 记录当前可见消息数量
  3. eval 向上滚动多次
  4. 每次滚动后 eval 检查消息数量
  5. eval 滚回底部，确认最新消息仍可见
- **expect**: 滚动流畅，历史消息加载正常
- **anti-pattern**: 滚动卡顿导致白块、消息重复、滚回底部后消息消失
- **severity**: medium
