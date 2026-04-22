//! Task Graph Engine — validates, executes, and advances DAG task graphs.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

use super::models::*;

/// Delegate trait for the task graph engine.
/// Implementors provide the actual session spawning and completion handling.
pub trait TaskGraphDelegate: Send + Sync {
    /// Spawn a worker session, return session_id.
    fn spawn_worker(
        &self,
        owner_id: &str,
        task_message: &str,
        workdir: Option<&str>,
        profile_id: Option<&str>,
        skill_ids: Option<&[String]>,
        agent_type: Option<&str>,
    ) -> Result<String, String>;

    /// Read last TEXT assistant message from a worker session.
    fn extract_final_output(&self, session_id: &str) -> String;

    /// Called when entire graph completes (all nodes completed or failed).
    fn on_graph_complete(&self, group_id: &str, owner_id: &str, nodes: &[TaskNodeState]);
}

/// Shared task graph execution engine.
pub struct TaskGraphEngine {
    /// In-memory task graph state: group_id -> TaskGraphState.
    task_graphs: Mutex<HashMap<String, TaskGraphState>>,
    /// Reverse lookup: session_id -> (group_id, task_id).
    session_to_graph: Mutex<HashMap<String, (String, String)>>,
}

impl TaskGraphEngine {
    pub fn new() -> Self {
        Self {
            task_graphs: Mutex::new(HashMap::new()),
            session_to_graph: Mutex::new(HashMap::new()),
        }
    }

    /// Dispatch a task graph (DAG of tasks with dependencies).
    ///
    /// Validates the DAG, stores state in memory, and starts all root tasks
    /// (those with no dependencies). Returns the group_id.
    pub fn dispatch_graph(
        &self,
        owner_id: &str,
        tasks: &[TaskNodeInput],
        delegate: &dyn TaskGraphDelegate,
    ) -> Result<String, String> {
        if tasks.is_empty() {
            return Err("Task graph must contain at least one task".into());
        }

        validate_dag(tasks)?;

        let group_id = uuid::Uuid::new_v4().to_string();

        // Build in-memory state
        let graph = TaskGraphState {
            group_id: group_id.clone(),
            owner_id: owner_id.to_string(),
            nodes: tasks
                .iter()
                .map(|t| TaskNodeState {
                    task_id: t.id.clone(),
                    task_description: t.task.clone(),
                    depends_on: t.depends_on.clone(),
                    status: TaskNodeStatus::Pending,
                    session_id: None,
                    result: None,
                    profile_id: t.profile_id.clone(),
                    skill_ids: t.skill_ids.clone(),
                    workdir: t.workdir.clone(),
                    agent_type: t.agent_type.clone(),
                })
                .collect(),
        };

        self.task_graphs
            .lock()
            .unwrap()
            .insert(group_id.clone(), graph);

        // Start root tasks (no dependencies)
        let root_tasks: Vec<_> = tasks.iter().filter(|t| t.depends_on.is_empty()).collect();
        let mut started = Vec::new();

        for t in &root_tasks {
            match self.start_node(owner_id, &group_id, t, delegate) {
                Ok(session_id) => {
                    started.push(format!("{} -> {}", t.id, session_id));
                }
                Err(e) => {
                    log::error!(
                        "[TaskGraph] Failed to start root task '{}' in group {}: {}",
                        t.id,
                        group_id,
                        e
                    );
                    self.set_node_status(&group_id, &t.id, TaskNodeStatus::Failed);
                }
            }
        }

        log::info!(
            "[TaskGraph] Created group {} with {} tasks ({} root started: [{}])",
            group_id,
            tasks.len(),
            root_tasks.len(),
            started.join(", ")
        );

        Ok(group_id)
    }

    /// Called when a session completes. If it belongs to a task graph,
    /// marks the node completed, advances downstream tasks, and finalizes
    /// the graph when all nodes are done.
    ///
    /// Returns `true` if the session was part of a task graph (caller can clean it up).
    pub fn on_session_complete(&self, session_id: &str, delegate: &dyn TaskGraphDelegate) -> bool {
        // Reverse lookup: session_id -> (group_id, task_id)
        let (group_id, task_id) = {
            let lookup = self.session_to_graph.lock().unwrap();
            match lookup.get(session_id) {
                Some(pair) => pair.clone(),
                None => return false, // Not part of a task graph
            }
        };

        // Extract Worker's final output
        let final_output = delegate.extract_final_output(session_id);

        // Mark node completed + collect info for advancement
        let (owner_id, ready_tasks) = {
            let mut graphs = self.task_graphs.lock().unwrap();
            let graph = match graphs.get_mut(&group_id) {
                Some(g) => g,
                None => return true,
            };

            // Mark this node completed
            if let Some(node) = graph.nodes.iter_mut().find(|n| n.task_id == task_id) {
                node.status = TaskNodeStatus::Completed;
                node.result = Some(final_output.clone());
            }

            log::info!(
                "[TaskGraph] Node '{}' completed in group {} (output: {} chars)",
                task_id,
                group_id,
                final_output.len()
            );

            // Find completed task IDs
            let completed_ids: HashSet<&str> = graph
                .nodes
                .iter()
                .filter(|n| n.status == TaskNodeStatus::Completed)
                .map(|n| n.task_id.as_str())
                .collect();

            // Find pending tasks whose deps are now all met
            let ready: Vec<TaskNodeInput> = graph
                .nodes
                .iter()
                .filter(|n| {
                    n.status == TaskNodeStatus::Pending
                        && n.depends_on
                            .iter()
                            .all(|d| completed_ids.contains(d.as_str()))
                })
                .map(|n| TaskNodeInput {
                    id: n.task_id.clone(),
                    task: n.task_description.clone(),
                    depends_on: n.depends_on.clone(),
                    profile_id: n.profile_id.clone(),
                    skill_ids: n.skill_ids.clone(),
                    workdir: n.workdir.clone(),
                    agent_type: n.agent_type.clone(),
                })
                .collect();

            (graph.owner_id.clone(), ready)
        };

        // Start ready downstream tasks (outside the lock)
        for task_input in &ready_tasks {
            match self.start_node(&owner_id, &group_id, task_input, delegate) {
                Ok(sid) => {
                    log::info!(
                        "[TaskGraph] Advanced: started node '{}' -> session {}",
                        task_input.id,
                        sid
                    );
                }
                Err(e) => {
                    log::error!(
                        "[TaskGraph] Failed to start node '{}': {}",
                        task_input.id,
                        e
                    );
                    self.set_node_status(&group_id, &task_input.id, TaskNodeStatus::Failed);
                }
            }
        }

        // Check if entire group is done
        let should_finalize = {
            let graphs = self.task_graphs.lock().unwrap();
            if let Some(graph) = graphs.get(&group_id) {
                graph.nodes.iter().all(|n| {
                    n.status == TaskNodeStatus::Completed || n.status == TaskNodeStatus::Failed
                })
            } else {
                false
            }
        };

        if should_finalize {
            self.finalize_graph(&group_id, &owner_id, delegate);
        }

        true
    }

    /// Start a single task node within a task graph.
    fn start_node(
        &self,
        owner_id: &str,
        group_id: &str,
        task_input: &TaskNodeInput,
        delegate: &dyn TaskGraphDelegate,
    ) -> Result<String, String> {
        let initial_message = self.build_task_message(group_id, task_input);

        let session_id = delegate.spawn_worker(
            owner_id,
            &initial_message,
            task_input.workdir.as_deref(),
            task_input.profile_id.as_deref(),
            if task_input.skill_ids.is_empty() {
                None
            } else {
                Some(&task_input.skill_ids)
            },
            task_input.agent_type.as_deref(),
        )?;

        // Update in-memory state
        {
            let mut graphs = self.task_graphs.lock().unwrap();
            if let Some(graph) = graphs.get_mut(group_id) {
                if let Some(node) = graph.nodes.iter_mut().find(|n| n.task_id == task_input.id) {
                    node.status = TaskNodeStatus::Running;
                    node.session_id = Some(session_id.clone());
                }
            }
        }

        // Register reverse lookup
        self.session_to_graph.lock().unwrap().insert(
            session_id.clone(),
            (group_id.to_string(), task_input.id.clone()),
        );

        log::info!(
            "[TaskGraph] Started node '{}' in group {} -> session {}",
            task_input.id,
            group_id,
            session_id
        );

        Ok(session_id)
    }

    /// Build the initial message for a task, injecting upstream results if any.
    fn build_task_message(&self, group_id: &str, task_input: &TaskNodeInput) -> String {
        if task_input.depends_on.is_empty() {
            return task_input.task.clone();
        }

        let graphs = self.task_graphs.lock().unwrap();
        let mut upstream_sections = Vec::new();

        if let Some(graph) = graphs.get(group_id) {
            for dep_id in &task_input.depends_on {
                if let Some(node) = graph.nodes.iter().find(|n| &n.task_id == dep_id) {
                    let result = node.result.as_deref().unwrap_or("(no output)");
                    upstream_sections.push(format!("## {}\n{}", dep_id, result));
                }
            }
        }

        let mut msg = String::from("[上游任务结果]\n");
        msg.push_str(&upstream_sections.join("\n\n"));
        msg.push_str("\n\n---\n[你的任务]\n");
        msg.push_str(&task_input.task);
        msg
    }

    /// Finalize a completed task graph: call delegate, clean up memory.
    fn finalize_graph(&self, group_id: &str, owner_id: &str, delegate: &dyn TaskGraphDelegate) {
        let (nodes, session_ids) = {
            let mut graphs = self.task_graphs.lock().unwrap();
            let graph = match graphs.remove(group_id) {
                Some(g) => g,
                None => return,
            };

            let session_ids: Vec<String> = graph
                .nodes
                .iter()
                .filter_map(|n| n.session_id.clone())
                .collect();

            let any_failed = graph
                .nodes
                .iter()
                .any(|n| n.status == TaskNodeStatus::Failed);
            let status = if any_failed { "failed" } else { "completed" };
            log::info!(
                "[TaskGraph] Group {} {} ({} tasks)",
                group_id,
                status,
                graph.nodes.len()
            );

            (graph.nodes, session_ids)
        };

        // Clean up reverse lookup
        {
            let mut lookup = self.session_to_graph.lock().unwrap();
            for sid in &session_ids {
                lookup.remove(sid);
            }
        }

        // Notify delegate with all node states
        delegate.on_graph_complete(group_id, owner_id, &nodes);
    }

    /// Helper: set a node's status in the in-memory graph.
    fn set_node_status(&self, group_id: &str, task_id: &str, status: TaskNodeStatus) {
        let mut graphs = self.task_graphs.lock().unwrap();
        if let Some(graph) = graphs.get_mut(group_id) {
            if let Some(node) = graph.nodes.iter_mut().find(|n| n.task_id == task_id) {
                node.status = status;
            }
        }
    }
}

/// Validate that the task inputs form a valid DAG (no cycles, valid references).
pub fn validate_dag(tasks: &[TaskNodeInput]) -> Result<(), String> {
    let task_ids: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();

    if task_ids.len() != tasks.len() {
        return Err("Task graph contains duplicate task IDs".into());
    }

    for t in tasks {
        for dep in &t.depends_on {
            if !task_ids.contains(dep.as_str()) {
                return Err(format!(
                    "Task '{}' depends on '{}' which does not exist in the graph",
                    t.id, dep
                ));
            }
            if dep == &t.id {
                return Err(format!("Task '{}' depends on itself", t.id));
            }
        }
    }

    // Kahn's algorithm: topological sort to detect cycles
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for t in tasks {
        in_degree.entry(t.id.as_str()).or_insert(0);
        for dep in &t.depends_on {
            adj.entry(dep.as_str()).or_default().push(&t.id);
            *in_degree.entry(t.id.as_str()).or_insert(0) += 1;
        }
    }

    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&k, _)| k)
        .collect();
    let mut sorted_count = 0;

    while let Some(node) = queue.pop_front() {
        sorted_count += 1;
        if let Some(children) = adj.get(node) {
            for &child in children {
                let d = in_degree.get_mut(child).unwrap();
                *d -= 1;
                if *d == 0 {
                    queue.push_back(child);
                }
            }
        }
    }

    if sorted_count != tasks.len() {
        return Err("Task graph contains circular dependencies".into());
    }

    Ok(())
}

/// Build a summary message for a completed task group.
pub fn build_group_summary(nodes: &[TaskNodeState], status: &str) -> String {
    let mut lines = Vec::new();
    let status_label = if status == "completed" {
        "全部完成"
    } else {
        "部分失败"
    };
    lines.push(format!("[Task Group Complete] {}", status_label));
    lines.push(String::new());

    for node in nodes {
        let icon = match node.status {
            TaskNodeStatus::Completed => "OK",
            TaskNodeStatus::Failed => "FAIL",
            _ => "...",
        };
        lines.push(format!(
            "- [{}] {}: {}",
            icon, node.task_id, node.task_description
        ));
        if let Some(ref result) = node.result {
            let preview: String = result.chars().take(200).collect();
            if preview.len() < result.len() {
                lines.push(format!("  结果: {}...", preview));
            } else {
                lines.push(format!("  结果: {}", preview));
            }
        }
    }

    lines.join("\n")
}
