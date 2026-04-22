---
id: browser-automation
name: 浏览器页面操作自动化
description: 通过 CDP (Chrome DevTools Protocol) 直接编写和执行网页自动化脚本
when_to_use: 用户需要通过 CDP 编写网页自动化脚本（超出原生 browser 工具能力范围）时使用
version: 3.0.0
author: zal
tags:
  - browser-automation
  - cdp
  - chrome-devtools-protocol
prerequisites:
  - python3
  - websockets (pip install websockets)
  - Google Chrome
---

# 浏览器页面操作自动化

通过 CDP (Chrome DevTools Protocol) 实现网页操作自动化。

## 优先使用原生 Browser 工具

大多数浏览器任务直接用原生工具即可完成，**无需编写脚本**：

| 工具 | 用途 |
|------|------|
| `browser_navigate` | 打开/跳转页面（自动启动 Chrome + 复制 profile 保持登录态） |
| `browser_state` | 获取页面无障碍树（带元素索引，用于 browser_action） |
| `browser_action` | 点击/输入/滚动/上传文件/执行 JS（每步自动返回 DOM 变化） |
| `browser_screenshot` | 截图（CDP 截图，支持 headless 模式） |
| `browser_manage` | 标签页管理 + 关闭浏览器 |

**典型流程**: `browser_navigate` → `browser_state`（如需完整页面结构） → 多次 `browser_action` → `browser_screenshot`（验证）

原生工具的优势：
- **交互式反馈**: 每次 action 自动返回 DOM 变化，无需手动截图验证
- **容错能力**: 看到操作结果后再决定下一步，不怕页面变化
- **无需脚本**: 不用写 Python，不用管 websockets 依赖

## 仅在以下场景使用 CDP 脚本（本 Skill）

- 需要精确时序控制的多步复杂操作
- 自定义事件监听或网络拦截（Fetch.enable）
- 原生工具未覆盖的 CDP 命令
- **原生 tool 与脚本不要混用**（端口冲突）：用脚本时指定 `--port 9322`

---

## CDP 脚本模式

核心思路：**使用 `scripts/cdp.py` 基础模块，编写 Python 脚本完成自动化任务。**

## 目录结构

```
.
├── SKILL.md                          # 本文档
└── scripts/
    ├── cdp.py                        # CDP 基础模块（核心）
    └── douyin_article.py             # 示例：抖音发布文章
```

---

## ⚠️ 编写自动化脚本的核心方法论

**这些经验比任何 API 文档都重要，务必遵循。**

### 1. 先问后做，不要盲目探索

- **先向用户确认目标页面的 URL**。很多 SPA 应用有直达 URL（如 `?type=article`、`/post/new`），直接用 URL 跳转比在页面上找 tab 点击高效 10 倍
- 用户可能已经知道页面结构，先问比自己摸索快得多
- 错误示范：花 5 轮脚本去找"发布文章"入口 → 正确做法：问用户要 URL，或搜索"站点名 + 发布文章 URL"

### 2. 每一步操作后验证结果

不要假设操作成功了，用以下方式验证：

```python
# 方法 1：截图验证（最直观，Agent 有视觉能力时首选）
await browser.screenshot("/tmp/step1.png", sid=sid)

# 方法 2：DOM 验证（最可靠的程序化验证）
value = await browser.evaluate("document.querySelector('input.title')?.value", sid=sid)
assert value == expected, f"Expected '{expected}', got '{value}'"

# 方法 3：URL 变化验证（适用于页面跳转）
new_url = await browser.wait_for_url_change(old_url, sid=sid)

# 方法 4：元素出现/消失验证
await browser.wait_for(".success-toast", sid=sid, timeout=10)
```

### 3. 了解页面技术栈，选择正确的输入方式

不同的前端框架需要不同的输入策略：

| 元素类型 | 方法 | 原因 |
|---------|------|------|
| 普通 `<input>` / `<textarea>` | `type_text()` (React setter) | React 受控组件需要触发 nativeInputValueSetter |
| 富文本编辑器 (TipTap/ProseMirror/Draft.js) | `type_into_contenteditable()` (Input.insertText) | 富文本框监听 beforeinput 事件，不是 value 属性 |
| 非 React 普通 input | 直接设 `.value` + dispatch `input` event | 原生表单不需要 React setter |
| 下拉选择器 | `click()` 打开 → `click_by_text()` 选择 | 自定义 Select 组件不是原生 `<select>` |

**关键教训**：直接设置 `innerHTML` 对 ProseMirror 编辑器无效——它维护自己的内部状态树，只有通过编辑器事件（keyboard/insertText）才能正确同步。

### 4. 用 DOM 探查理解页面结构

写脚本前先用探查方法了解页面：

```python
# 获取所有可见文本元素及位置（了解页面布局）
elements = await browser.get_visible_text_elements(sid=sid)

# 获取所有表单元素（了解可交互区域）
forms = await browser.get_form_elements(sid=sid)

# 搜索特定文本的元素
matches = await browser.evaluate("""
    Array.from(document.querySelectorAll('*')).filter(el =>
        el.textContent.includes('目标文本') && el.children.length === 0
    ).map(el => ({ tag: el.tagName, class: el.className, text: el.textContent.trim() }))
""", sid=sid)
```

### 5. 文件上传的正确姿势

文件上传是 CDP 自动化中最复杂的操作，因为：
- 很多网站的 `<input type="file">` 是**动态创建**的，初始 DOM 中不存在
- JS `.click()` 往往**不能**触发文件对话框（浏览器安全限制），必须用**真实鼠标事件**
- `Page.handleFileChooser` 在某些 Chrome 版本不可用
- WebSocket 单连接不支持并发 `recv()`，click 和事件监听不能并行

**正确流程：**

```python
# 1. 启用文件选择器拦截
# 2. 真实鼠标事件点击触发器（必须是 cursor:pointer 的叶子节点）
# 3. 等待 Page.fileChooserOpened 事件 → 获取 backendNodeId
# 4. DOM.setFileInputFiles 设置文件（比 Page.handleFileChooser 更兼容）
# 5. 关闭拦截

# cdp.py 已封装好:
await browser.upload_file('#upload-trigger', '/path/to/file.jpg', sid=sid)
```

**关键教训：**
- **选择器必须精确到叶子节点**。点击外层 `<div>` 往往无效，要找到内层的 `<span>` 或 `<button>`（看 `cursor: pointer`）
- **CSS class 含 hash 不稳定**，推荐用 JS 按文本定位后设置临时 id：
  ```python
  await browser.evaluate("""
      document.querySelectorAll('span').forEach(s => {
          if (s.textContent.trim() === '点击上传') s.id = '__cdp_upload';
      })
  """, sid=sid)
  await browser.upload_file('#__cdp_upload', file_path, sid=sid)
  ```
- **上传后常有编辑弹窗**（裁剪/预览），需要等待并点击"完成"/"确定"
- WebSocket 并发问题：`upload_file()` 内部把 click 和 recv 安排在单线程中串行处理

### 6. 处理 SPA 的常见坑

- **弹窗/引导遮挡**：很多网站首次访问有引导弹窗，先用 `dismiss_dialogs()` 清除
- **懒加载**：元素可能不在初始 DOM 中，用 `wait_for()` 等待出现
- **页面跳转是异步的**：SPA 导航不会触发 `Page.loadEventFired`，用 `wait_for_url_change()` 或 `wait_for()` 等待目标元素
- **Shadow DOM**：有些组件用 Shadow DOM，常规选择器找不到，需要 `element.shadowRoot.querySelector()`

### 6. 脚本的标准结构

```python
#!/usr/bin/env python3
"""脚本功能一句话说明。"""
import argparse, asyncio, json, sys, os, time

sys.path.insert(0, os.path.join(os.path.dirname(__file__)))
from cdp import CDPBrowser

def log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", file=sys.stderr, flush=True)

async def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=9222)
    parser.add_argument("--dry-run", action="store_true")
    # ... 业务参数
    args = parser.parse_args()

    async with CDPBrowser(port=args.port) as browser:
        sid = await browser.new_page("https://target-url", wait=5)

        # 检查登录状态
        url = await browser.evaluate("location.href", sid=sid)
        if "login" in url:
            log("ERROR: Not logged in")
            return

        # 关闭弹窗
        await browser.dismiss_dialogs(sid=sid)

        # 业务逻辑...（每步后截图验证）

        # stdout 输出 JSON 结果
        print(json.dumps({"status": "done"}, ensure_ascii=False))

asyncio.run(main())
```

**输出协议**：
- **stdout**: JSON 结果（供程序解析）
- **stderr**: 人类可读日志（`[HH:MM:SS] 消息`）

---

## Profile 策略（登录态复用）

**Chrome 不允许 CDP 直接使用正在运行的 Default profile**，因此脚本会自动复制一份到临时目录使用，**不会修改原始 profile**。

复制的关键文件：`Cookies`, `Login Data`, `Web Data`, `Preferences`, `Network`, `Local State`

| 平台 | Chrome 用户数据目录 | 临时 profile 位置 |
|------|---------------------|-------------------|
| macOS | `~/Library/Application Support/Google/Chrome` | `/tmp/cdp_automation_{pid}` |
| Windows | `%LocalAppData%\Google\Chrome\User Data` | `%TEMP%\cdp_automation_{pid}` |
| Linux | `~/.config/google-chrome` | `/tmp/cdp_automation_{pid}` |

**登录态过期时**，用日常 Chrome 正常登录目标网站即可，下次脚本运行会自动复制最新的 Cookies。

---

## CDPBrowser API

### 构造参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `port` | 9222 | CDP 调试端口（多脚本并行时用不同端口） |
| `profile_dir` | 系统 Chrome 目录 | 自定义 Chrome user-data-dir |
| `headless` | False | 无头模式 |
| `window_size` | "1280,900" | 窗口大小 |

### 核心方法

```python
# ── 页面操作 ──
sid = await browser.new_page("https://example.com", wait=3)  # 打开页面，返回 session ID
await browser.screenshot("/tmp/shot.png", sid=sid)            # 截图
cookies = await browser.get_cookies(urls=["..."], sid=sid)    # 获取 Cookies
await browser.scroll_to_bottom(sid=sid)                       # 滚动到底

# ── 元素交互 ──
await browser.click("button.submit", sid=sid)                 # CSS 选择器点击
await browser.click_by_text("发布", tag="button", sid=sid)    # 按文本点击
await browser.type_text("input.title", "Hello", sid=sid)      # 填写 input/textarea（React 兼容）
await browser.type_into_contenteditable(".editor", "正文", sid=sid)  # 填写富文本编辑器
await browser.upload_file("button.upload", "/path/file.jpg", sid=sid)  # 文件上传

# ── 等待 ──
await browser.wait_for(".result", sid=sid, timeout=10)        # 等待元素出现
url = await browser.wait_for_url_change(old_url, sid=sid)     # 等待 URL 变化

# ── DOM 探查 ──
elements = await browser.get_visible_text_elements(sid=sid)   # 所有可见文本元素
forms = await browser.get_form_elements(sid=sid)               # 表单元素（input/button/editor）

# ── 辅助 ──
await browser.dismiss_dialogs(sid=sid)                         # 关闭常见弹窗
value = await browser.evaluate("document.title", sid=sid)      # 执行任意 JS

# ── 底层 CDP ──
result = await browser.send("Page.navigate", {"url": "..."}, sid=sid)
```

### `type_text` vs `type_into_contenteditable`

- **`type_text(selector, text)`** — 用于 `<input>` / `<textarea>`。通过 React nativeInputValueSetter 设值 + 触发 input/change 事件。兼容 React、Vue 等框架的受控组件。
- **`type_into_contenteditable(selector, text)`** — 用于 `contenteditable` 富文本编辑器（TipTap、ProseMirror、Draft.js、Quill 等）。通过 `Input.insertText` CDP 命令输入，模拟真实键盘输入，编辑器能正确同步内部状态。

---

## 示例：抖音发布文章

`scripts/douyin_article.py` 是一个完整的示例，展示了：

1. **直达 URL** — 不去找 tab，直接用 URL 打开文章编辑器
2. **React input 填写** — 标题/摘要用 `type_text()`
3. **富文本编辑器填写** — 正文用 `type_into_contenteditable()`
4. **弹窗处理** — `dismiss_dialogs()` 清除引导
5. **分步截图验证** — 每步操作后截图
6. **dry-run 模式** — 调试时只填不发

```bash
# 试运行（不实际发布）
python3 scripts/douyin_article.py \
  --title "文章标题" \
  --summary "摘要" \
  --body "正文内容" \
  --dry-run --port 9222

# 正式发布
python3 scripts/douyin_article.py \
  --title "我的文章" \
  --body "第一段内容。

第二段内容。" \
  --cover /path/to/cover.jpg
```

**关键 URL**：`https://creator.douyin.com/creator-micro/content/post/article?enter_from=publish_page&media_type=article&type=new`

**页面结构（2026-03 实测）**：
- 标题 input: `input[placeholder*="文章标题"]`
- 摘要 input: `input[placeholder*="摘要"]`
- 正文编辑器: `div.tiptap.ProseMirror` (contenteditable, TipTap)
- 发布按钮: `button` 文本 "发布"

---

## CDP 命令速查

```
连接:       GET http://localhost:{port}/json/version → webSocketDebuggerUrl
多页面:     Target.setAutoAttach + Target.attachedToTarget
执行 JS:    Runtime.evaluate {expression, returnByValue: true}
文本输入:   Input.insertText {text}（富文本编辑器首选）
真实点击:   Input.dispatchMouseEvent {mousePressed + mouseReleased}
文件上传:   Page.setInterceptFileChooserDialog → 真实鼠标点击 →
            Page.fileChooserOpened (backendNodeId) →
            DOM.setFileInputFiles {files, backendNodeId}
            注意: Page.handleFileChooser 部分 Chrome 版本不可用，用 DOM.setFileInputFiles 更稳
导航:       Page.navigate {url}
截图:       Page.captureScreenshot
DOM 查询:   DOM.getDocument + DOM.querySelectorAll
网络拦截:   Fetch.enable + Fetch.requestPaused
Cookie:     Network.getCookies / Network.setCookie
```
