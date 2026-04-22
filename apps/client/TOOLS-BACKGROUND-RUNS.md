# Tools 后台任务系统 (Background Runs)

## 概述

这是真正的 Tools 后台执行系统，通过 `RunManager` 管理内存中的后台任务。

## 核心组件

### 1. RunManager (`src/runs.rs`)

**数据结构：**
```rust
pub struct RunRecord {
    pub run_id: String,           // 任务 ID
    pub session_id: String,       // 所属 session
    pub tool_id: String,          // 工具 ID (如 "shell", "image_generation")
    pub command: Option<String>,  // 命令（shell 工具）
    pub workdir: Option<String>,  // 工作目录
    pub status: RunStatus,        // 状态：Running | Exited | Failed | Killed | TimedOut
    pub started_at: i64,          // 启动时间戳
    pub finished_at: Option<i64>, // 完成时间戳
    pub pid: Option<u32>,         // 进程 ID
    pub exit: Option<RunExit>,    // 退出码
    pub error: Option<String>,    // 错误信息
    pub log_path: Option<String>, // 日志文件路径
    pub notify: bool,             // 是否通知用户
    pub hard_timeout_secs: Option<u64>, // 超时时间
}

pub enum RunStatus {
    Running,   // 运行中
    Exited,    // 正常退出
    Failed,    // 失败
    Killed,    // 被杀死
    TimedOut,  // 超时
}
```

**特性：**
- ✅ 内存存储（不持久化，重启后清空）
- ✅ 按 session 分组
- ✅ 支持日志文件（存储在 `{base_dir}/runs/{session_id}/{run_id}.log`）
- ✅ 支持通知队列
- ✅ 自动清理（session 归档时清理）

### 2. HTTP API (`src/extension_server.rs`)

```rust
GET  /runs                          // 列出所有后台任务
     ?session_id=xxx                // 可选：按 session 过滤

GET  /runs/{id}                     // 获取单个任务详情

POST /runs/{id}/stop                // 停止任务

GET  /runs/{id}/logs                // 获取任务日志（tail）
     ?lines=100                     // 可选：行数（默认100）

POST /runs/kill_all                 // 杀死所有任务

POST /runs/kill_by_session/{id}    // 杀死特定 session 的任务

GET  /runs/notifications/{session_id} // 获取通知（弹出式）
```

**返回格式：**
```json
{
  "success": true,
  "data": [
    {
      "run_id": "uuid",
      "session_id": "session-xxx",
      "tool_id": "shell",
      "command": "npm run build",
      "workdir": "/path/to/project",
      "status": "Running",
      "started_at": 1234567890,
      "finished_at": null,
      "pid": 12345,
      "exit": null,
      "error": null,
      "log_path": "/path/to/log",
      "notify": true,
      "hard_timeout_secs": 300
    }
  ],
  "error": null
}
```

### 3. 支持后台执行的 Tools

#### Shell (`src/tool_executors/shell.rs`)
- `supports_background: true`
- 参数：`background: true`
- 用途：长时间运行的命令（build, test, server 等）

#### Image Generation (`src/tool_executors/image_generation.rs`)
- `supports_background: true`
- 用途：AI 图片生成（可能需要几分钟）

#### Upload Artifact (`src/tool_executors/upload_artifact.rs`)
- `supports_background: true`
- 用途：上传大文件

### 4. ToolExecutor Trait

```rust
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    // 同步执行
    async fn execute(&self, input: Value) -> Result<String, String>;

    // 是否支持后台执行
    fn supports_background(&self) -> bool {
        false
    }

    // 后台执行（返回 run_id）
    async fn execute_background(
        &self,
        input: Value,
        session_id: Option<String>,
    ) -> Result<String, String> {
        Err("This tool does not support background execution".to_string())
    }
}
```

## 使用流程

### Agent 调用后台任务

```javascript
// 1. LLM 调用工具时加上 background: true
{
  "tool": "shell",
  "input": {
    "command": "npm run build",
    "background": true
  }
}

// 2. Tool executor 返回 run_id
"Background task started: run_abc123"

// 3. Agent 可以查询状态
GET /runs/run_abc123

// 4. 任务完成后自动通知（如果 notify: true）
// Agent 收到 RunNotification
```

### 前端显示后台任务

```javascript
// 1. 获取所有后台任务
GET /runs?session_id=xxx

// 2. 轮询更新（每5秒）
setInterval(() => {
  fetch('/runs?session_id=xxx')
}, 5000)

// 3. 停止任务
POST /runs/{run_id}/stop

// 4. 查看日志
GET /runs/{run_id}/logs?lines=100
```

## 与弃用的 tasks 表的区别

| 特性 | RunManager (新) | tasks 表 (旧) |
|------|----------------|--------------|
| 存储 | 内存 | SQLite |
| 持久化 | ❌ 重启清空 | ✅ 持久化 |
| 用途 | Tools 后台执行 | 浏览器扩展任务队列 |
| API | `/runs` | `/tasks` |
| 状态 | Running/Exited/Failed/Killed/TimedOut | pending/running/done/failed |
| 日志 | ✅ 文件日志 | ❌ 无日志 |
| 通知 | ✅ 内置通知队列 | ❌ 无通知 |
| 清理 | session 归档时清理 | 手动清理 |

## 前端 UI 设计建议

### 后台任务管理按钮

位置：对话框输入区域，MCP 按钮旁边

```
[Skills] [MCP] [后台任务 (N)] [Git]
                   ↑ 新按钮
```

### 功能需求

1. **任务列表**
   - 显示 Running/Exited/Failed 等状态
   - 显示运行时间
   - 显示工具类型（shell/image_generation）
   - 显示命令/描述

2. **任务操作**
   - 停止任务（POST /runs/{id}/stop）
   - 查看日志（GET /runs/{id}/logs）
   - 手动刷新

3. **自动更新**
   - 每 5 秒轮询 `GET /runs?session_id=xxx`
   - 计算运行中任务数量显示在按钮上

4. **状态指示**
   - Running: 绿色动画
   - Exited: 蓝色
   - Failed: 红色
   - Killed: 灰色
   - TimedOut: 橙色

### Modal 组件设计

```typescript
interface BackgroundRunsModalProps {
  sessionId: string;
  runs: RunRecord[];
  onRefresh: () => void;
  onStop: (runId: string) => void;
  onViewLogs: (runId: string) => void;
  onClose: () => void;
}
```

## 实现清单

- [ ] 添加 API 类型定义到 `ops.ts`
  - `RunRecord` 接口
  - `sessionListRuns()` 函数
  - `sessionStopRun()` 函数
  - `sessionGetRunLogs()` 函数

- [ ] 创建 `BackgroundRunsModal` 组件
  - 显示任务列表
  - 状态颜色和图标
  - 停止按钮
  - 查看日志按钮

- [ ] 在 `AgentInput.tsx` 添加按钮
  - `activeRunCount` prop
  - `onRunsClick` prop
  - 绿色高亮（有运行中任务时）

- [ ] 在 `SessionView.tsx` 集成
  - 轮询 runs API（每5秒）
  - 计算运行中任务数量
  - 打开 Modal

## 注意事项

1. **内存存储**：RunManager 不持久化，重启后所有任务丢失
2. **Session 绑定**：任务必须绑定到 session，session 归档时自动清理
3. **日志文件**：存储在临时目录，可能需要定期清理
4. **并发限制**：目前无并发限制，可能需要限制同时运行的任务数量

## 下一步

1. 实现前端 UI（参考 MCP/Skills 按钮的实现）
2. 考虑是否需要删除弃用的 tasks 表
3. 考虑是否需要添加任务历史记录（持久化已完成的任务）
