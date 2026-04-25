# Workspace 编排简化设计

## 设计原则

1. **去掉 workspace DAG 通用引擎**：workspace 层不做 node/edge/stage/vote 的通用状态机；DAG / subagent / fork-style 子任务属于单个 vendor session 内部能力，由 Cteno / Claude / Codex / Gemini 各自 runtime 或 adapter 管理
2. **每种编排模式写专门代码**：GroupChat、GatedTasks、Autoresearch 各自一个 Rust 模块
3. **收口在共用 UI 和交互**：workspace 壳（创建、角色管理、activity feed）统一
4. **新模板 = 新代码模块**：不是配置一个 JSON 模板，而是写一段 Rust 调度逻辑
5. **workspace 只管顶层 session**：workspace orchestrator 可以把消息路由给某个 role session，但不展开、持久化或推进该 session 内部的 DAG/subagent 节点

---

## 现有代码资产盘点

### 可复用（保留）

| 模块 | 位置 | 说明 |
|------|------|------|
| Workspace 创建 RPC | multi_agent.rs:122-241 | bootstrap-workspace、list/get/delete RPC |
| Provisioner 实现 | multi_agent.rs:1062-1241 | create_workspace_persona、create_role_agent、spawn_role_session |
| Session Messenger | multi_agent.rs:1244-1288 | send_to_session（fire-and-forget）、request_response（同步等待） |
| Live Adapter Registry | multi_agent.rs:547-559 | LIVE_WORKSPACE_ADAPTERS、ensure_live_workspace 恢复 |
| Profile 解析 | multi_agent.rs:562-590 | resolve_workspace_profile_id 按 model 字符串查找 profile |
| 持久化 | adapter.rs persistence | 存储/恢复 workspace 状态到磁盘 |
| Activity Feed 数据模型 | runtime.rs WorkspaceActivity | kind、text、visibility、role_id 等 |
| 前端 RPC 层 | ops.ts workspace 函数 | machineBootstrapWorkspace、machineWorkspaceSendMessage 等 |
| 前端 UI 组件 | persona/[id].tsx | WorkspaceBanner、WorkspaceActivityStrip、WorkspaceChatFeed |

### 需要简化/替换

| 模块 | 问题 | 处理 |
|------|------|------|
| WorkspaceRuntime (1547 行) | workspace 级通用 workflow/claim/vote 状态机，容易被误用成 session 内 DAG 引擎 | 替换为 WorkspaceOrchestrator trait 的具体实现；session 内 DAG 迁移到 vendor runtime |
| CtenoWorkspaceAdapter (adapter.rs) | 包装 WorkspaceRuntime 的胶水层 | 简化为直接调用 orchestrator |
| WorkspaceTemplate.workflow | node/edge/stage 定义 | 改为 orchestrator_type 字符串；不再表达 vendor session 内部 task graph |
| send_workspace_turn (adapter.rs) | 100+ 行通用调度（claim → vote → dispatch） | 拆到各 orchestrator 模块 |
| 前端孤儿组件 | WorkspaceTimeline、WorkflowStatus、DecisionPanel 未挂载 | 按模板类型条件渲染 |

---

## 新架构

### Rust 侧

```
multi-agent-runtime-core/src/
├── lib.rs                    # 导出
├── workspace.rs              # WorkspaceShell: 共用的 persona/roles/sessions/activity 管理
├── orchestrator.rs           # WorkspaceOrchestrator trait 定义
├── orchestration/
│   ├── mod.rs
│   ├── group_chat.rs         # 群聊调度
│   ├── gated_tasks.rs        # 门控任务调度
│   └── autoresearch.rs       # 自主研究调度
└── executor/                 # AgentExecutor trait（不变）
```

### WorkspaceShell（从 runtime.rs 提取共用部分）

```rust
pub struct WorkspaceShell {
    pub spec: WorkspaceSpec,
    pub members: BTreeMap<String, WorkspaceMember>,     // role_id → member
    pub activities: Vec<WorkspaceActivity>,              // 共享活动流
    pub dispatches: Vec<TaskDispatch>,                   // dispatch 历史
}

impl WorkspaceShell {
    pub fn new(spec: WorkspaceSpec) -> Self;
    pub fn register_member(&mut self, role_id: &str, session_id: &str);
    pub fn record_activity(&mut self, activity: WorkspaceActivity);
    pub fn record_dispatch(&mut self, dispatch: TaskDispatch);
    pub fn update_member_status(&mut self, role_id: &str, status: MemberStatus);
    pub fn snapshot(&self) -> ShellSnapshot;             // 给前端的状态快照
}
```

### WorkspaceOrchestrator trait

```rust
#[async_trait]
pub trait WorkspaceOrchestrator: Send + Sync {
    /// 模板类型标识
    fn orchestrator_type(&self) -> &str;  // "group_chat" | "gated_tasks" | "autoresearch"

    /// 用户发消息时的调度逻辑
    async fn handle_user_message(
        &mut self,
        shell: &mut WorkspaceShell,
        messenger: &dyn SessionMessenger,
        message: &str,
        target_role: Option<&str>,
    ) -> Result<OrchestratorResponse, OrchestratorError>;

    /// 某个 role session 完成一轮后的回调
    async fn on_role_completed(
        &mut self,
        shell: &mut WorkspaceShell,
        messenger: &dyn SessionMessenger,
        role_id: &str,
        result: &str,
        success: bool,
    ) -> Result<OrchestratorResponse, OrchestratorError>;

    /// 模板特有的 UI 状态（前端按 orchestrator_type 分发渲染）
    fn template_state(&self) -> serde_json::Value;

    /// 序列化/反序列化（持久化用）
    fn serialize_state(&self) -> serde_json::Value;
    fn restore_state(&mut self, state: serde_json::Value) -> Result<(), OrchestratorError>;
}

pub struct OrchestratorResponse {
    pub activities: Vec<WorkspaceActivity>,  // 新产生的活动
    pub dispatches: Vec<TaskDispatch>,       // 新分派的任务
    pub template_state: serde_json::Value,   // 更新后的模板状态
}
```

### 与 Session 内 DAG / Subagent 的边界

Workspace orchestrator 只编排顶层 role/session，例如把一个用户请求交给 researcher、coder、reviewer，或在 gated task 模式下等待某个顶层 session 的结果。它不能拥有某个 vendor session 内部的 task graph、subagent、fork skill 或 wait/merge 状态。

- Cteno 的 DAG / subagent 能力落在 `cteno-agent-runtime`，通过 `cteno-agent-stdio` 对 host 输出普通事件。
- Claude / Codex / Gemini 的同类能力由对应 adapter 接入原生机制；没有原生能力时返回明确的 unsupported / degraded behavior。
- 旧的 persona `dispatch_task` DAG 是兼容入口，不能作为新 workspace 设计的基础设施继续扩张。

### 三种 Orchestrator 实现

#### GroupChatOrchestrator

```rust
pub struct GroupChatOrchestrator {
    coordinator_role_id: String,
}

// handle_user_message:
//   1. 如果有 target_role（@某人），直接 dispatch 给该 role
//   2. 否则发给 coordinator，coordinator 决定转发给谁
//   3. 转发后等 role 响应，记录到 activity feed

// on_role_completed:
//   1. 记录 role 的回复到 activity feed
//   2. 通知 coordinator 有新回复（可选）

// template_state:
//   { type: "group_chat" }  // 无额外状态
```

#### GatedTasksOrchestrator

```rust
pub struct GatedTasksOrchestrator {
    tasks: Vec<GatedTask>,           // 任务列表
    current_task_index: usize,
    reviewer_role_id: String,
    coder_role_id: String,
    current_phase: GatedPhase,       // Idle | Coding | Reviewing | Committing
}

enum GatedPhase {
    Idle,
    Coding { task_id: String },
    Reviewing { task_id: String, coder_result: String },
    Committing { task_id: String },
}

// handle_user_message:
//   1. 如果是 "开始" / 首条消息 → 读 tasks.json，开始第一个 pending task
//   2. 如果是手动指令（"跳过"/"暂停"/"新增任务"） → 修改任务列表
//   3. 否则转发给当前活跃的 role

// on_role_completed:
//   match current_phase:
//     Coding → 切到 Reviewing，dispatch 给 reviewer
//     Reviewing → 
//       if APPROVED → 切到 Committing，dispatch 给 coder commit
//       if REJECTED → 切回 Coding，带 feedback 重新 dispatch
//     Committing → 标记 task completed，移到下一个 task

// template_state:
//   { type: "gated_tasks", tasks: [...], currentTaskIndex, currentPhase }
```

#### AutoresearchOrchestrator

```rust
pub struct AutoresearchOrchestrator {
    hypotheses: Vec<Hypothesis>,
    experiments: Vec<ExperimentRecord>,
    hypothesis_role_id: String,
    worker_role_id: String,
    gate_role_id: String,            // 评估 agent（或 None 用脚本）
    gate_script: Option<String>,     // 可选的脚本评估命令
    best_metric: Option<f64>,
}

struct Hypothesis {
    id: String,
    description: String,
    confidence: f64,
    parent_id: Option<String>,       // 分裂来源
    status: HypothesisStatus,        // Active | Exploring | Validated | Discarded
    children: Vec<String>,
}

struct ExperimentRecord {
    id: String,
    hypothesis_id: String,
    description: String,
    metric: Option<f64>,
    status: ExperimentStatus,        // Running | Keep | Discard | Crash
}

// handle_user_message:
//   1. 如果首条消息 → 发给 hypothesis agent 提出初始假说
//   2. 如果 "继续" → 让 hypothesis agent 基于当前结果提出下一轮假说
//   3. 如果 "分裂 H1" → 让 hypothesis agent 展开 H1 的子假说

// on_role_completed:
//   if role == worker:
//     1. 解析实验结果（metric）
//     2. 如果有 gate_role → dispatch 给 gate agent 评估
//     3. 如果有 gate_script → 执行脚本评估
//     4. keep/discard 决定
//     5. 更新 hypothesis 置信度
//     6. 通知 hypothesis agent 结果，让它决定下一步
//   if role == hypothesis:
//     1. 解析新假说
//     2. 自动 dispatch worker 去探索
//   if role == gate:
//     1. 解析评估结果（keep/discard + 理由）
//     2. 更新实验记录和假说置信度
//     3. 通知 hypothesis agent

// template_state:
//   { type: "autoresearch", hypotheses: [...], experiments: [...], bestMetric }
```

---

## Desktop 侧改动（multi_agent.rs）

### 替换 CtenoWorkspaceAdapter

```rust
// 旧: CtenoWorkspaceAdapter<P, M> 包装 WorkspaceRuntime
// 新: WorkspaceInstance 包装 WorkspaceShell + dyn WorkspaceOrchestrator

pub struct WorkspaceInstance {
    shell: WorkspaceShell,
    orchestrator: Box<dyn WorkspaceOrchestrator>,
    messenger: CtenoSessionMessenger,
    provisioner: CtenoWorkspaceProvisioner,
    bootstrapped: Option<BootstrappedWorkspace>,
    persistence: Option<LocalWorkspacePersistence>,
}

impl WorkspaceInstance {
    /// 用户发消息
    pub async fn send_message(&mut self, message: &str, role_id: Option<&str>) -> Result<WorkspaceTurnResult> {
        let response = self.orchestrator.handle_user_message(
            &mut self.shell, &self.messenger, message, role_id
        ).await?;
        // 记录 activities、dispatches
        // 持久化
        // 返回结果
    }

    /// role 完成回调
    pub async fn on_role_completed(&mut self, session_id: &str, result: &str, success: bool) -> Result<()> {
        let role_id = self.find_role_by_session(session_id)?;
        let response = self.orchestrator.on_role_completed(
            &mut self.shell, &self.messenger, &role_id, result, success
        ).await?;
        // 记录、持久化
    }
}
```

### 模板注册

```rust
fn create_orchestrator(template_id: &str, spec: &WorkspaceSpec) -> Box<dyn WorkspaceOrchestrator> {
    match template_id {
        "group-chat" => Box::new(GroupChatOrchestrator::new(spec)),
        "gated-tasks" => Box::new(GatedTasksOrchestrator::new(spec)),
        "autoresearch" => Box::new(AutoresearchOrchestrator::new(spec)),
        _ => panic!("unknown template: {}", template_id),
    }
}
```

### 模板定义简化

```rust
// 旧: WorkspaceTemplate 带完整 workflow: { nodes, edges, stages }
// 新: WorkspaceTemplate 只定义角色和元信息

pub struct WorkspaceTemplateSimple {
    pub template_id: String,
    pub template_name: String,
    pub description: String,
    pub orchestrator_type: String,     // "group_chat" | "gated_tasks" | "autoresearch"
    pub roles: Vec<RoleSpec>,
    pub default_role_id: String,
    pub orchestrator_config: serde_json::Value,  // 模板特有配置
}
```

---

## 前端改动

### workspace-send-message RPC 响应增加 template_state

```typescript
// 现有
{ plan, dispatches, events, state }

// 新增
{ plan, dispatches, events, state, templateState: { type: "gated_tasks", tasks: [...] } }
```

### persona/[id].tsx 按 orchestrator_type 条件渲染

```tsx
{effectiveWorkspace && (
    <>
        <WorkspaceBanner ... />
        <WorkspaceActivityStrip ... />
        
        {/* 按模板类型渲染特有面板 */}
        {templateType === 'gated_tasks' && (
            <GatedTasksPanel tasks={templateState.tasks} currentPhase={templateState.currentPhase} />
        )}
        {templateType === 'autoresearch' && (
            <AutoresearchPanel hypotheses={templateState.hypotheses} experiments={templateState.experiments} />
        )}
        
        <WorkspaceChatFeed ... />
    </>
)}
```

### 新前端组件

| 组件 | 模板 | 功能 |
|------|------|------|
| `GatedTasksPanel` | gated-tasks | 任务列表 + 状态 + 审批按钮 |
| `AutoresearchPanel` | autoresearch | 假说树 + 置信度 + 实验记录表 |

群聊不需要额外面板，纯 WorkspaceChatFeed 就够。

---

## 接入 ExecutorRegistry（多 vendor 支持）

在 `spawn_role_session` 中接入：

```rust
// 旧: 全部走 autonomous_agent (Cteno in-process)
// 新: 根据 role.provider 选择 vendor

async fn spawn_role_session(&self, spec, role, agent_id, workspace_persona_id) -> Result<String> {
    let vendor = role.agent.provider.unwrap_or("cteno");
    
    if vendor != "cteno" {
        // 走 ExecutorRegistry
        if let Ok(registry) = crate::local_services::executor_registry() {
            if let Ok(executor) = registry.resolve(vendor) {
                // executor.spawn_session(...)
                // 返回 session_id
            }
        }
    }
    
    // fallback: 走现有 Cteno in-process 路径
    // ...
}
```

同样 `execute_local_workspace_session` 需要判断 vendor，走 executor 路径还是 legacy 路径。

---

## 删除的代码

| 文件 | 删除内容 |
|------|---------|
| runtime.rs | claim_window、vote_window、workflow 相关方法（~800 行） |
| adapter.rs | send_workspace_turn 中的 claim/vote/workflow 逻辑（~400 行） |
| multi_agent.rs | collect_claim_responses、collect_workflow_vote_responses（~100 行） |
| 前端 | WorkspaceWorkflowStatus、WorkspaceDecisionPanel（如果不复用的话） |
