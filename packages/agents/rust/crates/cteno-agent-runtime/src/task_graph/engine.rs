use std::collections::{HashMap, HashSet, VecDeque};

use super::models::*;

const DAG_NODE_INSTRUCTIONS: &str = "\
[DAG 节点执行规则]\n\
你是 runtime task graph 中的一个 SubAgent 节点。请独立完成分配给你的节点任务，并把结果直接返回给父 session。\n\
- 不要为了“回复/打印/echo 一句话”调用 shell、bash、zsh、python 或其他命令工具；这种情况直接用最终文本回答。\n\
- 只有当节点任务明确要求读取、搜索、修改文件、运行命令或访问外部资源时，才调用相应工具。\n\
- 不要再启动新的 DAG 或 subagent 来完成当前节点，除非任务明确要求递归委派。";

pub fn validate_dag(tasks: &[TaskNodeInput]) -> Result<(), String> {
    if tasks.is_empty() {
        return Err("Task graph must contain at least one task".to_string());
    }

    let task_ids: HashSet<&str> = tasks.iter().map(|task| task.id.as_str()).collect();
    if task_ids.len() != tasks.len() {
        return Err("Task graph contains duplicate task IDs".to_string());
    }

    for task in tasks {
        if task.id.trim().is_empty() {
            return Err("Task graph contains an empty task ID".to_string());
        }
        if task.task.trim().is_empty() {
            return Err(format!("Task '{}' has an empty task description", task.id));
        }
        for dep in &task.depends_on {
            if dep == &task.id {
                return Err(format!("Task '{}' depends on itself", task.id));
            }
            if !task_ids.contains(dep.as_str()) {
                return Err(format!(
                    "Task '{}' depends on '{}' which does not exist in the graph",
                    task.id, dep
                ));
            }
        }
    }

    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for task in tasks {
        in_degree.entry(task.id.as_str()).or_insert(0);
        for dep in &task.depends_on {
            adj.entry(dep.as_str()).or_default().push(&task.id);
            *in_degree.entry(task.id.as_str()).or_insert(0) += 1;
        }
    }

    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, &degree)| degree == 0)
        .map(|(&id, _)| id)
        .collect();
    let mut sorted_count = 0usize;

    while let Some(node) = queue.pop_front() {
        sorted_count += 1;
        if let Some(children) = adj.get(node) {
            for &child in children {
                let degree = in_degree
                    .get_mut(child)
                    .expect("child exists in in_degree");
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(child);
                }
            }
        }
    }

    if sorted_count != tasks.len() {
        return Err("Task graph contains circular dependencies".to_string());
    }

    Ok(())
}

pub fn build_task_message(graph: &TaskGraphState, task_input: &TaskNodeInput) -> String {
    if task_input.depends_on.is_empty() {
        return format!("{}\n\n[你的任务]\n{}", DAG_NODE_INSTRUCTIONS, task_input.task);
    }

    let mut upstream_sections = Vec::new();
    for dep_id in &task_input.depends_on {
        if let Some(node) = graph.nodes.iter().find(|node| &node.task_id == dep_id) {
            let result = node
                .result
                .as_deref()
                .or(node.error.as_deref())
                .unwrap_or("(no output)");
            upstream_sections.push(format!("## {}\n{}", dep_id, result));
        }
    }

    format!(
        "{}\n\n[上游任务结果]\n{}\n\n---\n[你的任务]\n{}",
        DAG_NODE_INSTRUCTIONS,
        upstream_sections.join("\n\n"),
        task_input.task
    )
}

pub fn build_group_summary(nodes: &[TaskNodeState]) -> String {
    let has_failed = nodes
        .iter()
        .any(|node| matches!(node.status, TaskNodeStatus::Failed | TaskNodeStatus::Blocked));
    let mut lines = Vec::new();
    lines.push(format!(
        "[Task Group Complete] {}",
        if has_failed { "部分失败" } else { "全部完成" }
    ));
    lines.push(String::new());

    for node in nodes {
        let label = match node.status {
            TaskNodeStatus::Completed => "OK",
            TaskNodeStatus::Failed => "FAIL",
            TaskNodeStatus::Blocked => "BLOCKED",
            TaskNodeStatus::Running => "RUNNING",
            TaskNodeStatus::Pending => "PENDING",
        };
        lines.push(format!(
            "- [{}] {}: {}",
            label, node.task_id, node.task_description
        ));

        if let Some(result) = node.result.as_deref() {
            lines.push(format!("  结果: {}", preview(result, 200)));
        } else if let Some(error) = node.error.as_deref() {
            lines.push(format!("  错误: {}", preview(error, 200)));
        }
    }

    lines.join("\n")
}

fn preview(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, deps: &[&str]) -> TaskNodeInput {
        TaskNodeInput {
            id: id.to_string(),
            task: format!("task {id}"),
            depends_on: deps.iter().map(|dep| dep.to_string()).collect(),
            profile_id: None,
            skill_ids: Vec::new(),
            workdir: None,
            agent_type: None,
        }
    }

    #[test]
    fn validate_rejects_cycles() {
        let tasks = vec![node("a", &["b"]), node("b", &["a"])];
        let err = validate_dag(&tasks).unwrap_err();
        assert!(err.contains("circular"));
    }

    #[test]
    fn validate_rejects_duplicate_ids() {
        let tasks = vec![node("build", &[]), node("build", &[])];
        let err = validate_dag(&tasks).unwrap_err();
        assert!(err.contains("duplicate"));
    }

    #[test]
    fn injects_upstream_results() {
        let graph = TaskGraphState {
            group_id: "g".to_string(),
            parent_session_id: "s".to_string(),
            nodes: vec![TaskNodeState {
                task_id: "a".to_string(),
                task_description: "task a".to_string(),
                depends_on: Vec::new(),
                status: TaskNodeStatus::Completed,
                subagent_id: Some("sub-a".to_string()),
                result: Some("alpha".to_string()),
                error: None,
                profile_id: None,
                skill_ids: Vec::new(),
                workdir: None,
                agent_type: None,
            }],
        };
        let message = build_task_message(&graph, &node("b", &["a"]));
        assert!(message.contains("[DAG 节点执行规则]"));
        assert!(message.contains("[上游任务结果]"));
        assert!(message.contains("## a\nalpha"));
    }

    #[test]
    fn root_node_discourages_shell_for_plain_replies() {
        let graph = TaskGraphState {
            group_id: "g".to_string(),
            parent_session_id: "s".to_string(),
            nodes: Vec::new(),
        };
        let message = build_task_message(&graph, &node("a", &[]));
        assert!(message.contains("[DAG 节点执行规则]"));
        assert!(message.contains("不要为了"));
        assert!(message.contains("shell"));
        assert!(message.contains("[你的任务]\ntask a"));
    }

    #[test]
    fn summary_distinguishes_blocked() {
        let nodes = vec![TaskNodeState {
            task_id: "downstream".to_string(),
            task_description: "blocked task".to_string(),
            depends_on: vec!["root".to_string()],
            status: TaskNodeStatus::Blocked,
            subagent_id: None,
            result: None,
            error: Some("Dependency 'root' failed".to_string()),
            profile_id: None,
            skill_ids: Vec::new(),
            workdir: None,
            agent_type: None,
        }];
        let summary = build_group_summary(&nodes);
        assert!(summary.contains("部分失败"));
        assert!(summary.contains("BLOCKED"));
    }
}
