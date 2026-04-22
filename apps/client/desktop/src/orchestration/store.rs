//! In-memory store for orchestration flows.
//!
//! Follows the same `RwLock<HashMap>` pattern as `A2uiStore`.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::RwLock;

use super::models::{FlowEdge, FlowEdgeType, FlowNode, FlowNodeStatus, OrchestrationFlow};

/// In-memory store for orchestration flows.
pub struct OrchestrationStore {
    /// flow_id -> OrchestrationFlow
    flows: RwLock<HashMap<String, OrchestrationFlow>>,
    /// persona_id -> flow_id (latest flow per persona)
    persona_to_flow: RwLock<HashMap<String, String>>,
    /// session_id -> (flow_id, node_id) reverse index
    session_to_node: RwLock<HashMap<String, (String, String)>>,
}

impl OrchestrationStore {
    pub fn new() -> Self {
        Self {
            flows: RwLock::new(HashMap::new()),
            persona_to_flow: RwLock::new(HashMap::new()),
            session_to_node: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new orchestration flow from JSON nodes and edges.
    /// Returns the flow ID.
    pub fn create_flow(
        &self,
        persona_id: &str,
        session_id: &str,
        title: &str,
        nodes_json: Value,
        edges_json: Value,
    ) -> String {
        let flow_id = uuid::Uuid::new_v4().to_string();

        let nodes: Vec<FlowNode> = if let Some(arr) = nodes_json.as_array() {
            arr.iter()
                .filter_map(|v| {
                    let id = v.get("id")?.as_str()?.to_string();
                    let label = v
                        .get("label")
                        .and_then(|l| l.as_str())
                        .unwrap_or(&id)
                        .to_string();
                    let agent_type = v
                        .get("agentType")
                        .and_then(|a| a.as_str())
                        .map(String::from);
                    let max_iterations = v
                        .get("maxIterations")
                        .and_then(|m| m.as_u64())
                        .map(|n| n as u32);
                    Some(FlowNode {
                        id,
                        label,
                        agent_type,
                        status: FlowNodeStatus::Pending,
                        session_id: None,
                        iteration: if max_iterations.is_some() {
                            Some(0)
                        } else {
                            None
                        },
                        max_iterations,
                    })
                })
                .collect()
        } else {
            Vec::new()
        };

        let edges: Vec<FlowEdge> = if let Some(arr) = edges_json.as_array() {
            arr.iter()
                .filter_map(|v| {
                    let from = v.get("from")?.as_str()?.to_string();
                    let to = v.get("to")?.as_str()?.to_string();
                    let condition = v
                        .get("condition")
                        .and_then(|c| c.as_str())
                        .map(String::from);
                    let edge_type_str = v
                        .get("edgeType")
                        .and_then(|e| e.as_str())
                        .unwrap_or("normal");
                    let edge_type = match edge_type_str {
                        "retry" => FlowEdgeType::Retry,
                        "conditional" => FlowEdgeType::Conditional,
                        _ => FlowEdgeType::Normal,
                    };
                    Some(FlowEdge {
                        from,
                        to,
                        condition,
                        edge_type,
                    })
                })
                .collect()
        } else {
            Vec::new()
        };

        let flow = OrchestrationFlow {
            id: flow_id.clone(),
            persona_id: persona_id.to_string(),
            session_id: session_id.to_string(),
            title: title.to_string(),
            nodes,
            edges,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        {
            let mut flows = self.flows.write().unwrap();
            flows.insert(flow_id.clone(), flow);
        }
        {
            let mut p2f = self.persona_to_flow.write().unwrap();
            p2f.insert(persona_id.to_string(), flow_id.clone());
        }

        log::info!(
            "[Orchestration] Created flow '{}' for persona {}",
            flow_id,
            persona_id
        );
        flow_id
    }

    /// Get a flow by its ID.
    pub fn get_flow(&self, flow_id: &str) -> Option<OrchestrationFlow> {
        let flows = self.flows.read().unwrap();
        flows.get(flow_id).cloned()
    }

    /// Get the latest flow for a persona.
    pub fn get_flow_by_persona(&self, persona_id: &str) -> Option<OrchestrationFlow> {
        let p2f = self.persona_to_flow.read().unwrap();
        let flow_id = p2f.get(persona_id)?;
        let flows = self.flows.read().unwrap();
        flows.get(flow_id).cloned()
    }

    /// Delete a flow.
    pub fn delete_flow(&self, flow_id: &str) -> bool {
        let mut flows = self.flows.write().unwrap();
        if let Some(flow) = flows.remove(flow_id) {
            // Clean up persona -> flow mapping
            let mut p2f = self.persona_to_flow.write().unwrap();
            if p2f.get(&flow.persona_id).map(|id| id.as_str()) == Some(flow_id) {
                p2f.remove(&flow.persona_id);
            }
            // Clean up session -> node mappings
            let mut s2n = self.session_to_node.write().unwrap();
            s2n.retain(|_, (fid, _)| fid != flow_id);
            true
        } else {
            false
        }
    }

    /// Link a session to a flow node by matching label.
    /// Called from PersonaManager::dispatch_task when --label is set.
    pub fn link_session_to_node_by_label(&self, persona_id: &str, label: &str, session_id: &str) {
        let p2f = self.persona_to_flow.read().unwrap();
        let flow_id = match p2f.get(persona_id) {
            Some(fid) => fid.clone(),
            None => return,
        };
        drop(p2f);

        let mut flows = self.flows.write().unwrap();
        if let Some(flow) = flows.get_mut(&flow_id) {
            if let Some(node) = flow.nodes.iter_mut().find(|n| n.id == label) {
                node.status = FlowNodeStatus::Running;
                node.session_id = Some(session_id.to_string());
                // Increment iteration for loop nodes
                if node.iteration.is_some() {
                    node.iteration = Some(node.iteration.unwrap_or(0) + 1);
                }
                log::info!(
                    "[Orchestration] Linked session {} to node '{}' in flow {}",
                    session_id,
                    label,
                    flow_id
                );
            }
        }

        // Update reverse index
        let mut s2n = self.session_to_node.write().unwrap();
        s2n.insert(session_id.to_string(), (flow_id, label.to_string()));
    }

    /// Update node status when a session completes.
    /// Called from PersonaManager::notify_task_result.
    pub fn on_session_complete(&self, session_id: &str) {
        let s2n = self.session_to_node.read().unwrap();
        let (flow_id, node_id) = match s2n.get(session_id) {
            Some(pair) => pair.clone(),
            None => return,
        };
        drop(s2n);

        let mut flows = self.flows.write().unwrap();
        if let Some(flow) = flows.get_mut(&flow_id) {
            if let Some(node) = flow.nodes.iter_mut().find(|n| n.id == node_id) {
                // Mark as completed (the actual success/failure could be inferred
                // from the session's final output, but for now we use Completed)
                node.status = FlowNodeStatus::Completed;
                log::info!(
                    "[Orchestration] Node '{}' completed in flow {}",
                    node_id,
                    flow_id
                );
            }
        }
    }

    /// Update a specific node's status (e.g. from an RPC call).
    pub fn update_node_status(
        &self,
        flow_id: &str,
        node_id: &str,
        status: FlowNodeStatus,
        session_id: Option<&str>,
    ) -> bool {
        let mut flows = self.flows.write().unwrap();
        if let Some(flow) = flows.get_mut(flow_id) {
            if let Some(node) = flow.nodes.iter_mut().find(|n| n.id == node_id) {
                node.status = status;
                if let Some(sid) = session_id {
                    node.session_id = Some(sid.to_string());
                }
                return true;
            }
        }
        false
    }
}

impl Default for OrchestrationStore {
    fn default() -> Self {
        Self::new()
    }
}
