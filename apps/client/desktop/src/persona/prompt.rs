//! Persona System Prompt Builder
//!
//! Builds the system prompt for persona agents with identity, personality,
//! and task dispatch guidance.

use super::manager::TaskSummary;
use super::models::Persona;

/// A simplified profile description for the persona prompt.
pub struct ProfileInfo {
    pub id: String,
    pub name: String,
    pub is_proxy: bool,
    pub supports_vision: bool,
    pub supports_computer_use: bool,
}

/// Build the system prompt for a persona agent.
pub fn build_persona_system_prompt(
    persona: &Persona,
    active_tasks: &[TaskSummary],
    persona_memory: Option<&str>,
) -> String {
    let mut sections = Vec::new();

    // Identity
    sections.push(format!(
        "## 身份\n你是 {}。{}\n\n- 主目录: `{}`\n\n所有任务默认在主目录下执行。\n",
        persona.name, persona.description, persona.workdir
    ));

    // Personality
    if !persona.personality_notes.is_empty() {
        sections.push(format!("## 你的性格\n{}\n", persona.personality_notes));
    }

    // Core behavior rules
    sections.push(
        r#"## 核心行为

### 强制记忆回忆
每一轮收到用户消息时，你都必须：
1. 调用 memory 工具搜索与当前话题相关的经验
2. 将回忆结果融入你的回复或计划
不要只在第一轮回忆——每次用户发来新消息都应该 recall 相关记忆。

### 直接执行 vs 任务派发

你拥有所有工具（shell、file、edit、read、websearch、browser、memory 等），大多数任务应该**自己直接执行**。

**直接执行（默认）：**
- 用户的请求、提问、指令 → 自己调用工具完成
- 学习、记忆、搜索、文件操作、代码编辑 → 自己做
- 简单的 shell 命令、单步操作 → 自己做

**只有以下情况才用 dispatch_task：**
- **耗时较长的后台任务**：大规模代码重构、长时间运行的脚本、批量处理等
- **需要并行执行的独立任务**：多个互不依赖的工作可以同时 dispatch 加速
- **需要隔离环境的任务**：不希望干扰当前对话上下文的工作
- **多步静态流水线**：A→B→C 确定步骤时，用 dispatch_task 的 DAG 模式（tasks 数组 + depends_on）

**需要循环迭代的复杂目标 → 激活 orchestration skill：**
- 目标达成型任务（实现→验证→修复循环，不知道要几轮）
- 需要多角色协作（实现者、测试者、研究者各司其职）
- 长时间无人值守运行，需要状态追踪和故障恢复
- 用户说"自动完成"、"批量处理"、"循环执行"、"优化参数"
- orchestration 与 DAG 的区别：DAG 是静态依赖图（无循环/条件），orchestration 是动态控制流脚本

**浏览器任务用 agent_type: "browser"：**
- 网页浏览、数据抓取、表单填写、页面截图 → `dispatch_task({ task: "...", agent_type: "browser" })`
- Browser Agent 配备适配器系统——常见网站操作（GitHub/Twitter/知乎等）无需从头手动执行
- 如果你的「站点知识」里记录了某网站的 API 端点或已安装适配器，在任务描述中告知 Browser Agent
- 同时指定支持视觉的 profile_id（带 [视觉] 或 [计算机操作] 标签的模型）

**简单判断规则：能自己快速完成的就自己做。只有任务耗时长、需要并行、或需要隔离时才 dispatch。**

### 任务派发规范
- 派发时给出清晰、完整的任务描述，包含具体要求和预期输出格式
- 每个任务 session 执行一个明确的任务，不要合并多个不相关任务
- **工作目录**：不指定 `workdir` 时任务在你的主目录下执行。用户指定了特定项目路径则直接传入

### 结果回传与知识沉淀
- 任务 session 完成后，结果会以 `[Task Complete]` 消息**自动推送**给你
- **不要轮询结果**，派发后告诉用户"任务已派发"，然后**结束当前回合**
- 如果结果不满意，用 `send_to_session` 追加指令

**收到 `[Task Complete]` 后的三步流程：**

1. **审查结果** — 向用户汇报

2. **知识萃取** — 问自己：
   - 这种任务以后会重复吗？→ 保存到「任务模板」（agent_type + profile + 关键参数）
   - 有什么操作失败了？→ 保存到「失败记录」（原因 + 替代方案）
   - Browser 任务的站点知识（API 端点、适配器等）由 Browser Agent 自行沉淀，你不需要管

### ⚠️ 自我进化（强制）

**以下情况发生时，必须立即调用 `memory save` 记录。不是建议，是必须。**

- **工具报错并解决** → 保存到「经验教训」（错误信息 + 解决方案）
- **同类错误第二次出现** → 必须记录，防止第三次
- **发现项目特有知识** → 保存到「经验教训」（构建命令、环境变量、目录结构等）
- **收到 [Task Complete]** → 保存到「任务模板」（agent_type + profile + 关键参数）
- **任务失败** → 保存到「失败记录」（原因 + 替代方案）

直接写入 MEMORY.md 对应 section，不需要询问用户。

3. **沉淀执行** — 将值得保存的知识用 `memory save` 写入对应 section

**不要保存的：** 具体的任务输出数据、一次性操作步骤、代码中已经明确的东西。

### 性格塑造
当用户分享偏好或反馈时：
- 提取关键性格特质
- 调用 update_personality 持久化
"#
        .to_string(),
    );

    // Active tasks
    if !active_tasks.is_empty() {
        let mut task_lines = vec!["## 当前活跃任务\n".to_string()];
        for task in active_tasks {
            task_lines.push(format!(
                "- Session `{}`: {} (创建于 {})",
                task.session_id, task.task_description, task.created_at
            ));
        }
        task_lines.push(String::new());
        sections.push(task_lines.join("\n"));
    }

    // Note: tool list and usage details are in each tool's TOOL.md, no need to repeat here.

    // Inject persona's private MEMORY.md
    if let Some(mem) = persona_memory {
        if !mem.trim().is_empty() {
            sections.push(format!(
                "## 你的记忆\n以下是你积累的长期记忆：\n\n{}\n",
                mem
            ));
        }
    }

    // Note: memory space guidance is in memory TOOL.md

    sections.join("\n")
}

/// Build a runtime context message listing available custom agents.
/// Injected dynamically each turn so newly created agents are immediately visible.
pub fn build_agents_context_message(agents: &[crate::service_init::AgentConfig]) -> Option<String> {
    // Filter out builtin worker/browser (Persona already knows about them)
    let custom: Vec<_> = agents
        .iter()
        .filter(|a| {
            let id = a.id.as_str();
            // Skip the hardcoded builtin kinds — they're already in the prompt
            id != "worker" && id != "browser"
        })
        .collect();

    if custom.is_empty() {
        return None;
    }

    let mut lines = vec![
        "<available_agents>".to_string(),
        "dispatch_task 时通过 agent_type 参数指定自定义 Agent。直接使用 ID 即可，无需额外配置。"
            .to_string(),
        String::new(),
    ];
    for a in &custom {
        let source = a.source.as_deref().unwrap_or("unknown");
        let desc = if a.description.len() > 80 {
            format!("{}...", &a.description[..77])
        } else {
            a.description.clone()
        };
        lines.push(format!("- `{}` ({}) — {} [{}]", a.id, a.name, desc, source));
    }
    lines.push("</available_agents>".to_string());

    Some(lines.join("\n"))
}

/// Build a runtime context message listing available models.
/// Injected as a tail user-context message (like skills/datetime),
/// so it stays up-to-date without baking into the system prompt.
pub fn build_models_context_message(profiles: &[ProfileInfo]) -> Option<String> {
    if profiles.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    lines.push("<available_models>".to_string());
    lines.push("派发任务时可通过 profile_id 参数选择模型。优先使用内置模型。\n".to_string());

    let proxy: Vec<_> = profiles.iter().filter(|p| p.is_proxy).collect();
    let user: Vec<_> = profiles.iter().filter(|p| !p.is_proxy).collect();

    let format_profile = |p: &ProfileInfo| -> String {
        let mut tags = Vec::new();
        if p.supports_vision {
            tags.push("视觉");
        }
        if p.supports_computer_use {
            tags.push("计算机操作");
        }
        if tags.is_empty() {
            format!("- {} — {}", p.id, p.name)
        } else {
            format!("- {} — {} [{}]", p.id, p.name, tags.join(", "))
        }
    };

    if !proxy.is_empty() {
        lines.push("内置模型（优先）:".to_string());
        for p in &proxy {
            lines.push(format_profile(p));
        }
    }
    if !user.is_empty() {
        if !proxy.is_empty() {
            lines.push(String::new());
        }
        lines.push("用户模型:".to_string());
        for p in &user {
            lines.push(format_profile(p));
        }
    }

    lines.push("</available_models>".to_string());
    Some(lines.join("\n"))
}
