use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::Mutex;
use uuid::Uuid;

use serde_json::{json, Value};

use crate::hooks;
use crate::subagent::{self, CleanupPolicy, SubAgentNotification, SubAgentStatus};

use super::engine::{build_group_summary, build_task_message, validate_dag};
use super::models::{TaskGraphState, TaskNodeInput, TaskNodeState, TaskNodeStatus};

static GLOBAL_MANAGER: OnceLock<Arc<TaskGraphManager>> = OnceLock::new();
static COMPLETION_TX: OnceLock<UnboundedSender<SubAgentNotification>> = OnceLock::new();

pub fn global() -> Arc<TaskGraphManager> {
    GLOBAL_MANAGER
        .get_or_init(|| Arc::new(TaskGraphManager::new()))
        .clone()
}

pub fn observe_subagent_complete(notification: SubAgentNotification) {
    let tx = COMPLETION_TX.get_or_init(|| {
        let (tx, mut rx) = unbounded_channel::<SubAgentNotification>();
        tokio::spawn(async move {
            while let Some(notification) = rx.recv().await {
                global().on_subagent_complete(&notification).await;
            }
        });
        tx
    });

    if tx.send(notification).is_err() {
        log::warn!("[TaskGraph] completion observer channel closed");
    }
}

pub async fn is_task_graph_subagent(subagent_id: &str) -> bool {
    global()
        .subagent_index
        .lock()
        .await
        .contains_key(subagent_id)
}

#[derive(Debug, Clone)]
pub struct TaskGraphDispatch {
    pub group_id: String,
    pub total_tasks: usize,
    pub started_tasks: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct TaskGraphManager {
    graphs: Mutex<HashMap<String, TaskGraphState>>,
    subagent_index: Mutex<HashMap<String, (String, String)>>,
}

impl TaskGraphManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn dispatch_graph(
        &self,
        parent_session_id: &str,
        tasks: Vec<TaskNodeInput>,
    ) -> Result<TaskGraphDispatch, String> {
        validate_dag(&tasks)?;

        let group_id = format!("tg_{}", Uuid::new_v4());
        let nodes = tasks
            .iter()
            .map(|task| TaskNodeState {
                task_id: task.id.clone(),
                task_description: task.task.clone(),
                depends_on: task.depends_on.clone(),
                status: TaskNodeStatus::Pending,
                subagent_id: None,
                result: None,
                error: None,
                profile_id: task.profile_id.clone(),
                skill_ids: task.skill_ids.clone(),
                workdir: task.workdir.clone(),
                agent_type: task.agent_type.clone(),
            })
            .collect::<Vec<_>>();

        {
            let mut graphs = self.graphs.lock().await;
            graphs.insert(
                group_id.clone(),
                TaskGraphState {
                    group_id: group_id.clone(),
                    parent_session_id: parent_session_id.to_string(),
                    nodes,
                },
            );
        }

        log::info!(
            "[TaskGraph] started group '{}' for parent '{}' with {} nodes",
            group_id,
            parent_session_id,
            tasks.len()
        );
        emit_task_graph_event(
            parent_session_id,
            "task_graph.started",
            json!({
                "groupId": group_id.clone(),
                "totalTasks": tasks.len()
            }),
        )
        .await;

        let roots = tasks
            .iter()
            .filter(|task| task.depends_on.is_empty())
            .cloned()
            .collect::<Vec<_>>();

        let mut started_tasks = Vec::new();
        for root in roots {
            match self.start_node(&group_id, &root).await {
                Ok(subagent_id) => started_tasks.push((root.id.clone(), subagent_id)),
                Err(error) => {
                    log::error!(
                        "[TaskGraph] failed to start root node '{}' in group '{}': {}",
                        root.id,
                        group_id,
                        error
                    );
                    self.mark_node_failed(&group_id, &root.id, error).await;
                }
            }
        }

        self.finalize_if_done(&group_id).await;

        Ok(TaskGraphDispatch {
            group_id,
            total_tasks: tasks.len(),
            started_tasks,
        })
    }

    pub async fn on_subagent_complete(&self, notification: &SubAgentNotification) {
        let (group_id, task_id) = {
            let index = self.subagent_index.lock().await;
            match index.get(&notification.subagent_id) {
                Some(pair) => pair.clone(),
                None => return,
            }
        };

        let (task_message, emit_payload) = {
            let mut graphs = self.graphs.lock().await;
            let Some(graph) = graphs.get_mut(&group_id) else {
                return;
            };
            let Some(node) = graph.nodes.iter_mut().find(|node| node.task_id == task_id) else {
                return;
            };

            match notification.status {
                SubAgentStatus::Completed => {
                    node.status = TaskNodeStatus::Completed;
                    node.result = notification.result.clone();
                    node.error = None;
                    let emit_payload = json!({
                        "groupId": group_id,
                        "taskId": task_id.clone(),
                        "subagentId": notification.subagent_id.clone(),
                        "status": "completed",
                        "result": notification.result.clone(),
                    });
                    (
                        format!(
                            "[Task Complete] {}\n\n{}",
                            task_id,
                            notification.result.as_deref().unwrap_or("")
                        ),
                        emit_payload,
                    )
                }
                _ => {
                    let error = notification.error.clone().unwrap_or_else(|| {
                        format!("SubAgent ended with status {}", notification.status)
                    });
                    node.status = TaskNodeStatus::Failed;
                    node.result = notification.result.clone();
                    node.error = Some(error.clone());
                    block_dependents(graph, &task_id, &error);
                    let emit_payload = json!({
                        "groupId": group_id,
                        "taskId": task_id.clone(),
                        "subagentId": notification.subagent_id.clone(),
                        "status": "failed",
                        "error": error.clone(),
                    });
                    (
                        format!("[Task Failed] {}\n\nError: {}", task_id, error),
                        emit_payload,
                    )
                }
            }
        };

        if let Some(parent_session_id) = self.parent_session_id(&group_id).await {
            let kind = if notification.status == SubAgentStatus::Completed {
                "task_graph.node_completed"
            } else {
                "task_graph.node_failed"
            };
            emit_task_graph_event(&parent_session_id, kind, emit_payload).await;
            subagent::manager::global()
                .send_to_session(&parent_session_id, task_message)
                .await;
        }

        self.start_ready_nodes(&group_id).await;
        self.finalize_if_done(&group_id).await;
    }

    async fn mark_node_failed(&self, group_id: &str, task_id: &str, error: String) {
        {
            let mut graphs = self.graphs.lock().await;
            if let Some(graph) = graphs.get_mut(group_id) {
                if let Some(node) = graph.nodes.iter_mut().find(|node| node.task_id == task_id) {
                    node.status = TaskNodeStatus::Failed;
                    node.error = Some(error.clone());
                }
                block_dependents(graph, task_id, &error);
            }
        }
    }

    async fn start_ready_nodes(&self, group_id: &str) {
        let ready = {
            let graphs = self.graphs.lock().await;
            let Some(graph) = graphs.get(group_id) else {
                return;
            };
            graph
                .nodes
                .iter()
                .filter(|node| {
                    node.status == TaskNodeStatus::Pending
                        && node.depends_on.iter().all(|dep_id| {
                            graph.nodes.iter().any(|dep| {
                                dep.task_id == *dep_id && dep.status == TaskNodeStatus::Completed
                            })
                        })
                })
                .map(node_to_input)
                .collect::<Vec<_>>()
        };

        for node in ready {
            if let Err(error) = self.start_node(group_id, &node).await {
                log::error!(
                    "[TaskGraph] failed to start ready node '{}' in group '{}': {}",
                    node.id,
                    group_id,
                    error
                );
                self.mark_node_failed(group_id, &node.id, error).await;
            }
        }
    }

    async fn start_node(&self, group_id: &str, task: &TaskNodeInput) -> Result<String, String> {
        let graph_snapshot = {
            let graphs = self.graphs.lock().await;
            graphs
                .get(group_id)
                .cloned()
                .ok_or_else(|| format!("Task graph '{}' not found", group_id))?
        };
        let mut task_message = build_task_message(&graph_snapshot, task);
        if let Some(workdir) = task.workdir.as_deref().filter(|value| !value.is_empty()) {
            task_message = format!("{}\n\n[工作目录]\n{}", task_message, workdir);
        }
        let parent_session_id = graph_snapshot.parent_session_id.clone();
        let agent_id = task.agent_type.as_deref().unwrap_or("worker");
        let profile_id = task.profile_id.as_deref();

        let bootstrap = hooks::subagent_bootstrap().ok_or_else(|| {
            "SubagentBootstrapProvider not installed — dispatch_task requires runtime subagent bootstrap"
                .to_string()
        })?;
        let (mut agent_config, exec_ctx) = bootstrap
            .build_subagent_context(agent_id, &parent_session_id, profile_id)
            .await?;

        if !task.skill_ids.is_empty() {
            agent_config.skills = Some(task.skill_ids.clone());
        }

        let subagent_id = subagent::manager::global()
            .spawn(
                parent_session_id,
                agent_id.to_string(),
                task_message,
                Some(task.id.clone()),
                CleanupPolicy::Keep,
                agent_config,
                exec_ctx,
            )
            .await?;

        {
            let mut graphs = self.graphs.lock().await;
            let graph = graphs
                .get_mut(group_id)
                .ok_or_else(|| format!("Task graph '{}' not found after spawn", group_id))?;
            let node = graph
                .nodes
                .iter_mut()
                .find(|node| node.task_id == task.id)
                .ok_or_else(|| format!("Task '{}' not found in graph '{}'", task.id, group_id))?;
            node.status = TaskNodeStatus::Running;
            node.subagent_id = Some(subagent_id.clone());
        }
        self.subagent_index
            .lock()
            .await
            .insert(subagent_id.clone(), (group_id.to_string(), task.id.clone()));

        log::info!(
            "[TaskGraph] node '{}' started in group '{}' as subagent '{}'",
            task.id,
            group_id,
            subagent_id
        );
        emit_task_graph_event(
            graph_snapshot.parent_session_id.as_str(),
            "task_graph.node_started",
            json!({
                "groupId": group_id,
                "taskId": task.id.clone(),
                "subagentId": subagent_id.clone(),
                "dependsOn": task.depends_on.clone(),
            }),
        )
        .await;

        Ok(subagent_id)
    }

    async fn finalize_if_done(&self, group_id: &str) {
        let maybe_done = {
            let graphs = self.graphs.lock().await;
            graphs.get(group_id).and_then(|graph| {
                graph
                    .nodes
                    .iter()
                    .all(|node| node.status.is_terminal())
                    .then(|| {
                        (
                            graph.parent_session_id.clone(),
                            build_group_summary(&graph.nodes),
                        )
                    })
            })
        };

        let Some((parent_session_id, summary)) = maybe_done else {
            return;
        };

        let removed = {
            let mut graphs = self.graphs.lock().await;
            graphs.remove(group_id)
        };
        if let Some(graph) = removed {
            let mut index = self.subagent_index.lock().await;
            for node in graph.nodes {
                if let Some(subagent_id) = node.subagent_id {
                    index.remove(&subagent_id);
                }
            }
        }

        log::info!(
            "[TaskGraph] completed group '{}' for parent '{}'",
            group_id,
            parent_session_id
        );
        // Emit only the typed completion event for the BackgroundRunsModal /
        // status surfaces. Per-node `[Task Complete] X` handoffs already woke
        // the persona ReAct loop; re-sending a group summary here would
        // duplicate every node's text into the persona transcript.
        emit_task_graph_event(
            &parent_session_id,
            "task_graph.completed",
            json!({
                "groupId": group_id,
                "summary": summary,
            }),
        )
        .await;
    }

    async fn parent_session_id(&self, group_id: &str) -> Option<String> {
        let graphs = self.graphs.lock().await;
        graphs
            .get(group_id)
            .map(|graph| graph.parent_session_id.clone())
    }
}

async fn emit_task_graph_event(session_id: &str, event: &str, payload: Value) {
    if let Some(emitter) = hooks::task_graph_event_emitter() {
        emitter
            .emit_task_graph_event(session_id, event, payload)
            .await;
    }
}

fn node_to_input(node: &TaskNodeState) -> TaskNodeInput {
    TaskNodeInput {
        id: node.task_id.clone(),
        task: node.task_description.clone(),
        depends_on: node.depends_on.clone(),
        profile_id: node.profile_id.clone(),
        skill_ids: node.skill_ids.clone(),
        workdir: node.workdir.clone(),
        agent_type: node.agent_type.clone(),
    }
}

fn block_dependents(graph: &mut TaskGraphState, failed_task_id: &str, reason: &str) {
    let blocked_ids = graph
        .nodes
        .iter()
        .filter(|node| {
            node.status == TaskNodeStatus::Pending
                && node.depends_on.iter().any(|dep| dep == failed_task_id)
        })
        .map(|node| node.task_id.clone())
        .collect::<Vec<_>>();

    for task_id in blocked_ids {
        if let Some(node) = graph.nodes.iter_mut().find(|node| node.task_id == task_id) {
            node.status = TaskNodeStatus::Blocked;
            node.error = Some(format!(
                "Dependency '{}' failed; task blocked. Upstream error: {}",
                failed_task_id, reason
            ));
        }
        block_dependents(graph, &task_id, reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_propagates_to_transitive_dependents() {
        let mut graph = TaskGraphState {
            group_id: "g".to_string(),
            parent_session_id: "parent".to_string(),
            nodes: vec![
                TaskNodeState {
                    task_id: "a".to_string(),
                    task_description: "A".to_string(),
                    depends_on: vec![],
                    status: TaskNodeStatus::Failed,
                    subagent_id: None,
                    result: None,
                    error: Some("boom".to_string()),
                    profile_id: None,
                    skill_ids: vec![],
                    workdir: None,
                    agent_type: None,
                },
                TaskNodeState {
                    task_id: "b".to_string(),
                    task_description: "B".to_string(),
                    depends_on: vec!["a".to_string()],
                    status: TaskNodeStatus::Pending,
                    subagent_id: None,
                    result: None,
                    error: None,
                    profile_id: None,
                    skill_ids: vec![],
                    workdir: None,
                    agent_type: None,
                },
                TaskNodeState {
                    task_id: "c".to_string(),
                    task_description: "C".to_string(),
                    depends_on: vec!["b".to_string()],
                    status: TaskNodeStatus::Pending,
                    subagent_id: None,
                    result: None,
                    error: None,
                    profile_id: None,
                    skill_ids: vec![],
                    workdir: None,
                    agent_type: None,
                },
            ],
        };

        block_dependents(&mut graph, "a", "boom");

        assert_eq!(graph.nodes[1].status, TaskNodeStatus::Blocked);
        assert_eq!(graph.nodes[2].status, TaskNodeStatus::Blocked);
    }
}
