# RPC Parity 手动测试用例

完成 task-gated 工作流后，使用以下用例在真实 community 桌面端验证。

## 前置条件

- Community 版本已编译通过：`cargo check -p cteno-app --no-default-features`
- Commercial 版本也通过：`cargo check -p cteno-app --features commercial`
- 桌面端已启动（dev 或 release 均可）

---

## Session-scoped RPC 测试

### T1: 创建 Session 并发送消息
1. 在前端点击"新建对话"
2. 选择一个 workdir（如 `~/Desktop`）
3. 发送消息 "你好"
4. **预期**: Session 创建成功，agent 返回回复文本
5. **验证点**: 控制台无 `No handler registered` 或 `Unknown method` 错误

### T2: Permission 审批流程
1. 在 session 中发送需要工具审批的指令（如 "请帮我在桌面创建一个 test.txt 文件"）
2. Agent 请求权限时，点击"允许"
3. **预期**: 权限通过，agent 继续执行
4. **刁难点**: 快速连续点击"允许"→"拒绝"→"允许"，检查是否有竞态

### T3: 中止操作 (Abort)
1. 发送一个耗时指令（如 "请分析一下当前目录下所有文件的内容"）
2. Agent 开始处理后，点击"停止"按钮
3. **预期**: Agent 停止执行，界面回到可输入状态
4. **验证点**: 后续再发消息仍可正常响应

### T4: 切换权限模式
1. 在 session 设置中切换到 "Auto Approve" 模式
2. 发送需要审批的指令
3. **预期**: 工具自动执行，不弹权限确认
4. 切回 "Default" 模式
5. 再次发送类似指令
6. **预期**: 重新弹出权限确认

### T5: 文件浏览器 - readFile
1. 在 session 中打开文件浏览器（如果 UI 有此入口）
2. 点击查看一个文本文件
3. **预期**: 文件内容正确显示
4. **刁难点**: 尝试查看一个二进制文件（如 .png），应返回 base64 而不崩溃

### T6: 文件浏览器 - listDirectory
1. 在文件浏览器中浏览 session workdir
2. **预期**: 目录内容正确列出（文件名、类型、大小）
3. **刁难点**: 浏览一个包含中文文件名的目录

### T7: Session Bash 执行
1. 查看 git status 面板（使用 sessionBash）
2. **预期**: Git 状态正确显示
3. **刁难点**: 在一个非 git 目录创建 session，git 面板应优雅降级而非崩溃

### T8: Kill Session
1. 创建一个 session
2. 发送一条消息让 agent 开始处理
3. 点击"删除/关闭" session
4. **预期**: Session 被关闭，从 session 列表消失
5. **验证点**: 该 session 的 RPC handler 被正确清理（不影响其他 session）

### T9: MCP 服务器配置
1. 在 session 设置中配置 MCP 服务器
2. **预期**: 配置保存成功，无报错
3. **刁难点**: 配置一个不存在的 server ID，应优雅处理

---

## Machine-scoped RPC 测试

### T10: Kill Session (Machine-scoped)
1. 创建多个 session
2. 通过 session 列表的"关闭"按钮关闭其中一个
3. **预期**: 被关闭的 session 消失，其他 session 不受影响

### T11: Persona / Agent 页面
1. 进入 Persona 页面
2. 点击查看某个 agent 的详情
3. **预期**: 页面加载成功（即使内容为空也不应显示 RPC 错误）
4. **验证点**: `get-agent-latest-text` 和 `get-dashboard` 返回 success

### T12: 通知列表
1. 进入通知页面
2. **预期**: 显示空列表或已有通知，不报 RPC 错误
3. 如果有通知，点击标记已读
4. **预期**: 标记成功

---

## 回归测试

### T13: Commercial 模式不受影响
1. 用 commercial 版本启动
2. 重复 T1-T3 基本流程
3. **预期**: 所有行为与之前一致，Socket.IO 路径仍优先

### T14: 多 Session 并发
1. 创建 3 个不同 workdir 的 session
2. 在每个 session 中发送消息
3. **预期**: 每个 session 独立工作，readFile/bash 使用各自的 workdir
4. **刁难点**: 同时在两个 session 中发送消息，检查 RPC 路由是否正确
