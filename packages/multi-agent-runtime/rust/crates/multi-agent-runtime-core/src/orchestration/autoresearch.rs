use async_trait::async_trait;
use chrono::Utc;
use multi_agent_protocol::{
    DispatchStatus, TaskDispatch, WorkspaceActivity, WorkspaceActivityKind, WorkspaceVisibility,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    OrchestratorError, OrchestratorResponse, SessionMessenger, WorkspaceOrchestrator,
    WorkspaceShell,
};

const ORCHESTRATOR_TYPE: &str = "autoresearch";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisStatus {
    Proposed,
    Testing,
    Kept,
    Discarded,
    Split,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HypothesisNode {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub text: String,
    pub confidence: f64,
    pub status: HypothesisStatus,
    #[serde(default)]
    pub children: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentStatus {
    Running,
    AwaitingGate,
    Keep,
    Discard,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentRecord {
    pub id: String,
    pub hypothesis_id: String,
    pub hypothesis_text: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<f64>,
    pub status: ExperimentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum HypothesisRequestKind {
    Initial,
    Continue,
    Split { parent_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GateDecisionKind {
    Keep,
    Discard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GateDecision {
    kind: GateDecisionKind,
    reason: String,
}

#[derive(Debug, Clone, PartialEq)]
struct HypothesisCandidate {
    text: String,
    confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoresearchOrchestrator {
    pub hypotheses: Vec<HypothesisNode>,
    pub experiments: Vec<ExperimentRecord>,
    pub hypothesis_role_id: String,
    pub worker_role_id: String,
    pub gate_role_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_script: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_metric: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_hypothesis_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_experiment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_gate_experiment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_hypothesis_request: Option<HypothesisRequestKind>,
}

impl AutoresearchOrchestrator {
    pub fn new(
        hypothesis_role_id: impl Into<String>,
        worker_role_id: impl Into<String>,
        gate_role_id: impl Into<String>,
        gate_script: Option<String>,
    ) -> Self {
        Self {
            hypotheses: Vec::new(),
            experiments: Vec::new(),
            hypothesis_role_id: hypothesis_role_id.into(),
            worker_role_id: worker_role_id.into(),
            gate_role_id: gate_role_id.into(),
            gate_script,
            best_metric: None,
            active_hypothesis_id: None,
            active_experiment_id: None,
            pending_gate_experiment_id: None,
            pending_hypothesis_request: None,
        }
    }

    fn create_activity(
        &self,
        shell: &WorkspaceShell,
        kind: WorkspaceActivityKind,
        text: impl Into<String>,
        role_id: Option<&str>,
    ) -> WorkspaceActivity {
        let role_id = role_id.map(ToString::to_string);
        WorkspaceActivity {
            activity_id: Uuid::new_v4(),
            workspace_id: shell.spec.id.clone(),
            kind,
            visibility: WorkspaceVisibility::Public,
            text: text.into(),
            created_at: now(),
            member_id: role_id.clone(),
            role_id,
            dispatch_id: None,
            task_id: None,
        }
    }

    fn create_dispatch(
        &self,
        shell: &WorkspaceShell,
        role_id: &str,
        instruction: impl Into<String>,
        summary: impl Into<String>,
    ) -> Result<TaskDispatch, OrchestratorError> {
        if !shell.members.contains_key(role_id) {
            return Err(OrchestratorError::InvalidState(format!(
                "unknown role: {role_id}"
            )));
        }

        Ok(TaskDispatch {
            dispatch_id: Uuid::new_v4(),
            workspace_id: shell.spec.id.clone(),
            role_id: role_id.to_string(),
            instruction: instruction.into(),
            summary: Some(summary.into()),
            visibility: Some(WorkspaceVisibility::Public),
            source_role_id: None,
            workflow_node_id: None,
            stage_id: None,
            status: DispatchStatus::Queued,
            provider_task_id: None,
            tool_use_id: None,
            created_at: now(),
            started_at: None,
            completed_at: None,
            output_file: None,
            last_summary: None,
            result_text: None,
            claimed_by_member_ids: None,
            claim_status: None,
        })
    }

    fn find_hypothesis_index(&self, hypothesis_id: &str) -> Result<usize, OrchestratorError> {
        self.hypotheses
            .iter()
            .position(|hypothesis| hypothesis.id.eq_ignore_ascii_case(hypothesis_id))
            .ok_or_else(|| {
                OrchestratorError::InvalidState(format!("unknown hypothesis: {hypothesis_id}"))
            })
    }

    fn current_experiment_mut(&mut self) -> Result<&mut ExperimentRecord, OrchestratorError> {
        let experiment_id = self.active_experiment_id.as_deref().ok_or_else(|| {
            OrchestratorError::InvalidState("no active experiment to update".to_string())
        })?;
        self.experiments
            .iter_mut()
            .find(|experiment| experiment.id == experiment_id)
            .ok_or_else(|| {
                OrchestratorError::InvalidState(format!(
                    "missing experiment for active id {experiment_id}"
                ))
            })
    }

    fn next_hypothesis_id(&self) -> String {
        format!("H{}", self.hypotheses.len() + 1)
    }

    fn next_experiment_id(&self) -> String {
        format!("E{}", self.experiments.len() + 1)
    }

    fn is_round_in_flight(&self) -> bool {
        self.pending_hypothesis_request.is_some()
            || self.active_experiment_id.is_some()
            || self.pending_gate_experiment_id.is_some()
    }

    fn dispatch_hypothesis_round(
        &mut self,
        shell: &WorkspaceShell,
        message: &str,
    ) -> Result<TaskDispatch, OrchestratorError> {
        let request = if self.hypotheses.is_empty() {
            HypothesisRequestKind::Initial
        } else if let Some(parent_id) = parse_split_target(message) {
            let parent_index = self.find_hypothesis_index(&parent_id)?;
            let canonical_parent_id = self.hypotheses[parent_index].id.clone();
            HypothesisRequestKind::Split {
                parent_id: canonical_parent_id,
            }
        } else {
            HypothesisRequestKind::Continue
        };

        let instruction = match &request {
            HypothesisRequestKind::Initial => build_initial_hypothesis_instruction(message),
            HypothesisRequestKind::Continue => {
                build_next_hypothesis_instruction(message, &self.template_state())
            }
            HypothesisRequestKind::Split { parent_id } => {
                build_split_instruction(parent_id, message, &self.template_state())
            }
        };
        let summary = match &request {
            HypothesisRequestKind::Initial => "Frame initial hypothesis".to_string(),
            HypothesisRequestKind::Continue => "Propose next autoresearch hypothesis".to_string(),
            HypothesisRequestKind::Split { parent_id } => {
                format!("Expand hypothesis {parent_id}")
            }
        };
        self.pending_hypothesis_request = Some(request);
        self.create_dispatch(shell, &self.hypothesis_role_id, instruction, summary)
    }

    fn register_hypotheses(
        &mut self,
        request: &HypothesisRequestKind,
        result: &str,
    ) -> Result<Vec<String>, OrchestratorError> {
        let candidates = parse_hypothesis_candidates(result);
        if candidates.is_empty() {
            return Err(OrchestratorError::InvalidState(
                "hypothesis agent returned no hypotheses".to_string(),
            ));
        }

        let parent_id = match request {
            HypothesisRequestKind::Initial => None,
            HypothesisRequestKind::Continue => self.active_hypothesis_id.clone(),
            HypothesisRequestKind::Split { parent_id } => Some(parent_id.clone()),
        };

        if let HypothesisRequestKind::Split { parent_id } = request {
            let index = self.find_hypothesis_index(parent_id)?;
            self.hypotheses[index].status = HypothesisStatus::Split;
            self.hypotheses[index].updated_at = now();
        }

        let base_confidence = parent_id
            .as_ref()
            .and_then(|id| {
                self.hypotheses
                    .iter()
                    .find(|hypothesis| hypothesis.id == *id)
            })
            .map(|hypothesis| hypothesis.confidence)
            .unwrap_or(0.5);

        let mut new_ids = Vec::new();
        for candidate in candidates {
            let hypothesis_id = self.next_hypothesis_id();
            let confidence = candidate
                .confidence
                .unwrap_or_else(|| default_confidence(base_confidence, parent_id.is_some()));
            self.hypotheses.push(HypothesisNode {
                id: hypothesis_id.clone(),
                parent_id: parent_id.clone(),
                text: candidate.text,
                confidence: clamp_confidence(confidence),
                status: HypothesisStatus::Proposed,
                children: Vec::new(),
                created_at: now(),
                updated_at: now(),
            });
            if let Some(parent_id) = parent_id.as_ref() {
                let index = self.find_hypothesis_index(parent_id)?;
                self.hypotheses[index].children.push(hypothesis_id.clone());
                self.hypotheses[index].updated_at = now();
            }
            new_ids.push(hypothesis_id);
        }

        if let Some(first_id) = new_ids.first() {
            self.active_hypothesis_id = Some(first_id.clone());
        }

        Ok(new_ids)
    }

    fn dispatch_worker_for_hypothesis(
        &mut self,
        shell: &WorkspaceShell,
        hypothesis_id: &str,
    ) -> Result<TaskDispatch, OrchestratorError> {
        let hypothesis_index = self.find_hypothesis_index(hypothesis_id)?;
        let experiment_id = self.next_experiment_id();
        let best_metric = self.best_metric;
        let (hypothesis_id, hypothesis_text, instruction) = {
            let hypothesis = &mut self.hypotheses[hypothesis_index];
            hypothesis.status = HypothesisStatus::Testing;
            hypothesis.updated_at = now();
            (
                hypothesis.id.clone(),
                hypothesis.text.clone(),
                build_worker_instruction(hypothesis, best_metric),
            )
        };
        let experiment = ExperimentRecord {
            id: experiment_id.clone(),
            hypothesis_id: hypothesis_id.clone(),
            hypothesis_text,
            description: format!("Evaluate {hypothesis_id}"),
            metric: None,
            status: ExperimentStatus::Running,
            worker_result: None,
            gate_reason: None,
            created_at: now(),
            updated_at: now(),
        };
        self.experiments.push(experiment);
        self.active_experiment_id = Some(experiment_id);
        self.create_dispatch(
            shell,
            &self.worker_role_id,
            instruction,
            format!("Run experiment for {hypothesis_id}"),
        )
    }

    fn dispatch_gate_for_active_experiment(
        &mut self,
        shell: &WorkspaceShell,
    ) -> Result<TaskDispatch, OrchestratorError> {
        if self.gate_role_id.trim().is_empty() {
            return Err(OrchestratorError::InvalidState(
                "gate evaluation requested without a gate role".to_string(),
            ));
        }
        if self.active_experiment_id.is_none() {
            return Err(OrchestratorError::InvalidState(
                "gate evaluation requested without an active experiment".to_string(),
            ));
        }

        let (instruction, summary, experiment_id) = {
            let best_metric = self.best_metric;
            let experiment = self.current_experiment_mut()?;
            experiment.status = ExperimentStatus::AwaitingGate;
            experiment.updated_at = now();
            (
                build_gate_instruction(experiment, best_metric),
                format!("Gate {}", experiment.id),
                experiment.id.clone(),
            )
        };
        self.pending_gate_experiment_id = Some(experiment_id);
        self.active_experiment_id = None;
        self.create_dispatch(shell, &self.gate_role_id, instruction, summary)
    }

    fn apply_gate_decision(
        &mut self,
        experiment_id: &str,
        decision: GateDecision,
    ) -> Result<(), OrchestratorError> {
        let experiment_index = self
            .experiments
            .iter()
            .position(|experiment| experiment.id == experiment_id)
            .ok_or_else(|| {
                OrchestratorError::InvalidState(format!("unknown experiment: {experiment_id}"))
            })?;
        let hypothesis_id = self.experiments[experiment_index].hypothesis_id.clone();
        let metric = self.experiments[experiment_index].metric;
        let hypothesis_index = self.find_hypothesis_index(&hypothesis_id)?;

        match decision.kind {
            GateDecisionKind::Keep => {
                self.experiments[experiment_index].status = ExperimentStatus::Keep;
                self.hypotheses[hypothesis_index].status = HypothesisStatus::Kept;
                self.hypotheses[hypothesis_index].confidence =
                    clamp_confidence(self.hypotheses[hypothesis_index].confidence + 0.15);
                if let Some(metric) = metric {
                    self.best_metric = Some(metric);
                }
            }
            GateDecisionKind::Discard => {
                self.experiments[experiment_index].status = ExperimentStatus::Discard;
                self.hypotheses[hypothesis_index].status = HypothesisStatus::Discarded;
                self.hypotheses[hypothesis_index].confidence =
                    clamp_confidence(self.hypotheses[hypothesis_index].confidence - 0.15);
            }
        }

        self.experiments[experiment_index].gate_reason = Some(decision.reason);
        self.experiments[experiment_index].updated_at = now();
        self.hypotheses[hypothesis_index].updated_at = now();
        self.active_hypothesis_id = Some(hypothesis_id);
        Ok(())
    }

    fn serialize_snapshot(&self) -> Value {
        json!({
            "type": ORCHESTRATOR_TYPE,
            "hypotheses": self.hypotheses,
            "experiments": self.experiments,
            "hypothesisRoleId": self.hypothesis_role_id,
            "workerRoleId": self.worker_role_id,
            "gateRoleId": self.gate_role_id,
            "gateScript": self.gate_script,
            "bestMetric": self.best_metric,
            "activeHypothesisId": self.active_hypothesis_id,
            "activeExperimentId": self.active_experiment_id,
            "pendingGateExperimentId": self.pending_gate_experiment_id,
            "pendingHypothesisRequest": self.pending_hypothesis_request,
        })
    }
}

impl Default for AutoresearchOrchestrator {
    fn default() -> Self {
        Self::new("hypothesis", "worker", "gate", None)
    }
}

#[async_trait]
impl WorkspaceOrchestrator for AutoresearchOrchestrator {
    fn orchestrator_type(&self) -> &str {
        ORCHESTRATOR_TYPE
    }

    async fn handle_user_message(
        &mut self,
        shell: &WorkspaceShell,
        _messenger: &dyn SessionMessenger,
        message: &str,
        target_role: Option<&str>,
    ) -> Result<OrchestratorResponse, OrchestratorError> {
        if let Some(role_id) = target_role {
            return Err(OrchestratorError::InvalidState(format!(
                "direct target dispatch is not supported by {ORCHESTRATOR_TYPE}: {role_id}"
            )));
        }

        let mut activities =
            vec![self.create_activity(shell, WorkspaceActivityKind::UserMessage, message, None)];
        let mut dispatches = Vec::new();

        if self.is_round_in_flight() {
            activities.push(self.create_activity(
                shell,
                WorkspaceActivityKind::SystemNotice,
                "Autoresearch already has a round in progress.".to_string(),
                None,
            ));
        } else {
            dispatches.push(self.dispatch_hypothesis_round(shell, message)?);
        }

        Ok(OrchestratorResponse {
            activities,
            dispatches,
            template_state: self.template_state(),
        })
    }

    async fn on_role_completed(
        &mut self,
        shell: &WorkspaceShell,
        _messenger: &dyn SessionMessenger,
        role_id: &str,
        result: &str,
        success: bool,
    ) -> Result<OrchestratorResponse, OrchestratorError> {
        let activity_kind = if success {
            WorkspaceActivityKind::MemberDelivered
        } else {
            WorkspaceActivityKind::MemberBlocked
        };
        let mut activities =
            vec![self.create_activity(shell, activity_kind, result, Some(role_id))];
        let mut dispatches = Vec::new();

        if role_id == self.hypothesis_role_id {
            if !success {
                return Ok(OrchestratorResponse {
                    activities,
                    dispatches,
                    template_state: self.template_state(),
                });
            }

            let request = self.pending_hypothesis_request.take().ok_or_else(|| {
                OrchestratorError::InvalidState(
                    "received hypothesis completion without a pending request".to_string(),
                )
            })?;
            let hypothesis_ids = self.register_hypotheses(&request, result)?;
            let first_id = hypothesis_ids.first().ok_or_else(|| {
                OrchestratorError::InvalidState(
                    "hypothesis registration produced no ids".to_string(),
                )
            })?;
            activities.push(self.create_activity(
                shell,
                WorkspaceActivityKind::SystemNotice,
                format!("Registered hypotheses: {}", hypothesis_ids.join(", ")),
                None,
            ));
            dispatches.push(self.dispatch_worker_for_hypothesis(shell, first_id)?);
        } else if role_id == self.worker_role_id {
            if !success {
                let experiment_id = self.active_experiment_id.take().ok_or_else(|| {
                    OrchestratorError::InvalidState(
                        "worker completed without an active experiment".to_string(),
                    )
                })?;
                self.apply_gate_decision(
                    &experiment_id,
                    GateDecision {
                        kind: GateDecisionKind::Discard,
                        reason: "Worker reported a failure.".to_string(),
                    },
                )?;
                dispatches.push(self.dispatch_hypothesis_round(shell, "continue")?);
            } else if let Some(script) = self.gate_script.clone() {
                let experiment_id = self.active_experiment_id.clone().ok_or_else(|| {
                    OrchestratorError::InvalidState(
                        "worker completed without an active experiment".to_string(),
                    )
                })?;
                let metric = parse_metric(result);
                {
                    let experiment = self.current_experiment_mut()?;
                    experiment.metric = metric;
                    experiment.worker_result = Some(result.to_string());
                    experiment.updated_at = now();
                }
                self.active_experiment_id = None;
                let decision = evaluate_gate_script(&script, metric, self.best_metric);
                let decision_text = match decision.kind {
                    GateDecisionKind::Keep => "keep",
                    GateDecisionKind::Discard => "discard",
                };
                self.apply_gate_decision(&experiment_id, decision)?;
                activities.push(self.create_activity(
                    shell,
                    WorkspaceActivityKind::SystemNotice,
                    format!("Gate script decided to {decision_text} {experiment_id}."),
                    None,
                ));
                dispatches.push(self.dispatch_hypothesis_round(shell, "continue")?);
            } else {
                let metric = parse_metric(result);
                {
                    let experiment = self.current_experiment_mut()?;
                    experiment.metric = metric;
                    experiment.worker_result = Some(result.to_string());
                    experiment.updated_at = now();
                }
                dispatches.push(self.dispatch_gate_for_active_experiment(shell)?);
            }
        } else if role_id == self.gate_role_id {
            if !success {
                let experiment_id = self.pending_gate_experiment_id.take().ok_or_else(|| {
                    OrchestratorError::InvalidState(
                        "gate completed without a pending experiment".to_string(),
                    )
                })?;
                self.apply_gate_decision(
                    &experiment_id,
                    GateDecision {
                        kind: GateDecisionKind::Discard,
                        reason: "Gate agent reported a failure.".to_string(),
                    },
                )?;
                dispatches.push(self.dispatch_hypothesis_round(shell, "continue")?);
            } else {
                let experiment_id = self.pending_gate_experiment_id.take().ok_or_else(|| {
                    OrchestratorError::InvalidState(
                        "gate completed without a pending experiment".to_string(),
                    )
                })?;
                let decision = parse_gate_decision(result);
                let decision_text = match decision.kind {
                    GateDecisionKind::Keep => "keep",
                    GateDecisionKind::Discard => "discard",
                };
                self.apply_gate_decision(&experiment_id, decision)?;
                activities.push(self.create_activity(
                    shell,
                    WorkspaceActivityKind::SystemNotice,
                    format!("Gate agent decided to {decision_text} {experiment_id}."),
                    None,
                ));
                dispatches.push(self.dispatch_hypothesis_round(shell, "continue")?);
            }
        } else {
            activities.push(self.create_activity(
                shell,
                WorkspaceActivityKind::SystemNotice,
                format!("Ignored completion from @{role_id}."),
                None,
            ));
        }

        Ok(OrchestratorResponse {
            activities,
            dispatches,
            template_state: self.template_state(),
        })
    }

    fn template_state(&self) -> Value {
        json!({
            "type": ORCHESTRATOR_TYPE,
            "hypothesisRoleId": self.hypothesis_role_id,
            "workerRoleId": self.worker_role_id,
            "gateRoleId": self.gate_role_id,
            "bestMetric": self.best_metric,
            "hypotheses": self.hypotheses,
            "experiments": self.experiments,
        })
    }

    fn serialize_state(&self) -> Value {
        self.serialize_snapshot()
    }

    fn restore_state(&mut self, state: Value) -> Result<(), OrchestratorError> {
        let object = state.as_object().ok_or_else(|| {
            OrchestratorError::Serialization("autoresearch state must be a JSON object".to_string())
        })?;
        if let Some(orchestrator_type) = object.get("type").and_then(Value::as_str) {
            if orchestrator_type != ORCHESTRATOR_TYPE {
                return Err(OrchestratorError::Serialization(format!(
                    "expected orchestrator type {ORCHESTRATOR_TYPE}, got {orchestrator_type}"
                )));
            }
        }

        self.hypotheses = parse_state_field(object, "hypotheses")?.unwrap_or_default();
        self.experiments = parse_state_field(object, "experiments")?.unwrap_or_default();
        self.hypothesis_role_id = object
            .get("hypothesisRoleId")
            .and_then(Value::as_str)
            .unwrap_or("hypothesis")
            .to_string();
        self.worker_role_id = object
            .get("workerRoleId")
            .and_then(Value::as_str)
            .unwrap_or("worker")
            .to_string();
        self.gate_role_id = object
            .get("gateRoleId")
            .and_then(Value::as_str)
            .unwrap_or("gate")
            .to_string();
        self.gate_script = object
            .get("gateScript")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        self.best_metric = object.get("bestMetric").and_then(Value::as_f64);
        self.active_hypothesis_id = object
            .get("activeHypothesisId")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        self.active_experiment_id = object
            .get("activeExperimentId")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        self.pending_gate_experiment_id = object
            .get("pendingGateExperimentId")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        self.pending_hypothesis_request = parse_state_field(object, "pendingHypothesisRequest")?;
        Ok(())
    }
}

fn build_initial_hypothesis_instruction(message: &str) -> String {
    format!(
        "Propose the first research hypothesis for this request.\n\
Return either strict JSON like {{\"hypotheses\":[{{\"text\":\"...\",\"confidence\":0.55}}]}} \
or a compact bullet list.\n\nUser request:\n{message}"
    )
}

fn build_next_hypothesis_instruction(message: &str, state: &Value) -> String {
    format!(
        "Advance the autoresearch loop. Propose the next hypothesis to test based on the \
current hypothesis tree and experiment log.\n\
Return either strict JSON like {{\"hypotheses\":[{{\"text\":\"...\",\"confidence\":0.62}}]}} \
or a compact bullet list.\n\nUser guidance:\n{message}\n\nCurrent state:\n{}",
        state
    )
}

fn build_split_instruction(parent_id: &str, message: &str, state: &Value) -> String {
    format!(
        "Split hypothesis {parent_id} into a few concrete child hypotheses.\n\
Return either strict JSON like {{\"hypotheses\":[{{\"text\":\"...\"}},{{\"text\":\"...\"}}]}} \
or a compact bullet list.\n\nUser guidance:\n{message}\n\nCurrent state:\n{}",
        state
    )
}

fn build_worker_instruction(hypothesis: &HypothesisNode, best_metric: Option<f64>) -> String {
    let best_metric_note = best_metric
        .map(|metric| format!("Current best metric: {metric}."))
        .unwrap_or_else(|| "No baseline metric is registered yet.".to_string());
    format!(
        "Run the next experiment for hypothesis {}.\n\
Hypothesis: {}\n\
{}\n\
Return the observed metric and a short evidence summary. Prefer a compact format such as \
\"metric: 0.82\" followed by notes.",
        hypothesis.id, hypothesis.text, best_metric_note
    )
}

fn build_gate_instruction(experiment: &ExperimentRecord, best_metric: Option<f64>) -> String {
    let baseline = best_metric
        .map(|metric| metric.to_string())
        .unwrap_or_else(|| "none".to_string());
    format!(
        "Evaluate experiment {} for hypothesis {}.\n\
Hypothesis: {}\n\
Metric: {}\n\
Previous best metric: {baseline}\n\
Worker result:\n{}\n\n\
Respond with KEEP or DISCARD and a short reason. JSON is also acceptable, for example \
{{\"decision\":\"keep\",\"reason\":\"metric improved\"}}.",
        experiment.id,
        experiment.hypothesis_id,
        experiment.hypothesis_text,
        experiment
            .metric
            .map(|metric| metric.to_string())
            .unwrap_or_else(|| "none".to_string()),
        experiment.worker_result.as_deref().unwrap_or_default(),
    )
}

fn parse_hypothesis_candidates(result: &str) -> Vec<HypothesisCandidate> {
    if let Ok(value) = serde_json::from_str::<Value>(result) {
        if let Some(candidates) = parse_hypothesis_candidates_from_value(&value) {
            if !candidates.is_empty() {
                return candidates;
            }
        }
    }

    let mut candidates = Vec::new();
    for line in result.lines() {
        let mut text = line.trim();
        if text.is_empty() {
            continue;
        }
        if let Some(stripped) = text.strip_prefix("- ") {
            text = stripped.trim();
        } else if let Some(stripped) = text.strip_prefix("* ") {
            text = stripped.trim();
        } else if let Some(stripped) = strip_numbered_prefix(text) {
            text = stripped;
        }
        if let Some((prefix, remainder)) = text.split_once(':') {
            let trimmed_prefix = prefix.trim();
            if trimmed_prefix.len() >= 2
                && trimmed_prefix.starts_with('H')
                && trimmed_prefix[1..].chars().all(|ch| ch.is_ascii_digit())
            {
                text = remainder.trim();
            }
        }
        if !text.is_empty() {
            candidates.push(HypothesisCandidate {
                text: text.to_string(),
                confidence: None,
            });
        }
    }

    if candidates.is_empty() && !result.trim().is_empty() {
        candidates.push(HypothesisCandidate {
            text: result.trim().to_string(),
            confidence: None,
        });
    }
    candidates
}

fn parse_hypothesis_candidates_from_value(value: &Value) -> Option<Vec<HypothesisCandidate>> {
    match value {
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(parse_candidate_value)
                .collect::<Vec<_>>(),
        ),
        Value::Object(map) => {
            if let Some(hypotheses) = map.get("hypotheses") {
                return parse_hypothesis_candidates_from_value(hypotheses);
            }
            parse_candidate_value(value).map(|candidate| vec![candidate])
        }
        _ => parse_candidate_value(value).map(|candidate| vec![candidate]),
    }
}

fn parse_candidate_value(value: &Value) -> Option<HypothesisCandidate> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(HypothesisCandidate {
                    text: trimmed.to_string(),
                    confidence: None,
                })
            }
        }
        Value::Object(map) => {
            let text = map
                .get("text")
                .or_else(|| map.get("hypothesis"))
                .or_else(|| map.get("title"))
                .and_then(Value::as_str)?
                .trim()
                .to_string();
            if text.is_empty() {
                None
            } else {
                Some(HypothesisCandidate {
                    text,
                    confidence: map.get("confidence").and_then(Value::as_f64),
                })
            }
        }
        _ => None,
    }
}

fn parse_metric(result: &str) -> Option<f64> {
    if let Ok(value) = serde_json::from_str::<Value>(result) {
        if let Some(metric) = find_metric_in_value(&value) {
            return Some(metric);
        }
    }

    for line in result.lines() {
        let lower = line.to_ascii_lowercase();
        if ["metric", "score", "accuracy", "loss", "value"]
            .iter()
            .any(|keyword| lower.contains(keyword))
        {
            if let Some(metric) = extract_number(line) {
                return Some(metric);
            }
        }
    }

    extract_number(result)
}

fn find_metric_in_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::Array(items) => items.iter().find_map(find_metric_in_value),
        Value::Object(map) => {
            for key in ["metric", "score", "accuracy", "loss", "value"] {
                if let Some(metric) = map.get(key).and_then(find_metric_in_value) {
                    return Some(metric);
                }
            }
            map.values().find_map(find_metric_in_value)
        }
        _ => None,
    }
}

fn parse_gate_decision(result: &str) -> GateDecision {
    if let Ok(value) = serde_json::from_str::<Value>(result) {
        if let Some(decision) = parse_gate_decision_from_value(&value) {
            return decision;
        }
    }

    let lower = result.to_ascii_lowercase();
    if lower.contains("discard")
        || lower.contains("reject")
        || lower.contains("worse")
        || lower.contains("regress")
    {
        GateDecision {
            kind: GateDecisionKind::Discard,
            reason: result.trim().to_string(),
        }
    } else {
        GateDecision {
            kind: GateDecisionKind::Keep,
            reason: result.trim().to_string(),
        }
    }
}

fn parse_gate_decision_from_value(value: &Value) -> Option<GateDecision> {
    let Value::Object(map) = value else {
        return None;
    };
    let decision = map
        .get("decision")
        .or_else(|| map.get("status"))
        .or_else(|| map.get("outcome"))
        .and_then(Value::as_str)?;
    let lower = decision.to_ascii_lowercase();
    let reason = map
        .get("reason")
        .or_else(|| map.get("rationale"))
        .or_else(|| map.get("feedback"))
        .and_then(Value::as_str)
        .unwrap_or(decision)
        .to_string();
    Some(GateDecision {
        kind: if lower.contains("discard") || lower.contains("reject") {
            GateDecisionKind::Discard
        } else {
            GateDecisionKind::Keep
        },
        reason,
    })
}

fn evaluate_gate_script(
    script: &str,
    metric: Option<f64>,
    best_metric: Option<f64>,
) -> GateDecision {
    let Some(metric) = metric else {
        return GateDecision {
            kind: GateDecisionKind::Discard,
            reason: "Gate script could not find a metric to evaluate.".to_string(),
        };
    };

    let lower = script.to_ascii_lowercase();
    let maximize = !lower.contains("minimize")
        && !lower.contains("lower_is_better")
        && !lower.contains("lower-is-better");
    let is_better = match best_metric {
        Some(best) if maximize => metric > best,
        Some(best) if !maximize => metric < best,
        Some(_) | None => true,
    };
    let threshold_ok = threshold_matches(&lower, metric).unwrap_or(true);

    if is_better && threshold_ok {
        GateDecision {
            kind: GateDecisionKind::Keep,
            reason: format!("Gate script kept metric {metric}."),
        }
    } else {
        GateDecision {
            kind: GateDecisionKind::Discard,
            reason: format!("Gate script discarded metric {metric}."),
        }
    }
}

fn threshold_matches(script: &str, metric: f64) -> Option<bool> {
    for operator in [">=", "<=", ">", "<"] {
        if let Some(index) = script.find(operator) {
            let number = extract_number(&script[index + operator.len()..])?;
            return Some(match operator {
                ">=" => metric >= number,
                "<=" => metric <= number,
                ">" => metric > number,
                "<" => metric < number,
                _ => true,
            });
        }
    }
    None
}

fn parse_split_target(message: &str) -> Option<String> {
    let tokens: Vec<&str> = message.split_whitespace().collect();
    let split_index = tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case("split"))?;
    let target = tokens
        .get(split_index + 1)?
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric());
    if target.len() >= 2
        && (target.starts_with('H') || target.starts_with('h'))
        && target[1..].chars().all(|ch| ch.is_ascii_digit())
    {
        Some(format!("H{}", &target[1..]))
    } else {
        None
    }
}

fn extract_number(text: &str) -> Option<f64> {
    let mut buffer = String::new();
    let mut seen_digit = false;
    for ch in text.chars().chain(std::iter::once(' ')) {
        let is_number_char = ch.is_ascii_digit()
            || matches!(ch, '.' | '+' | '-')
            || ((ch == 'e' || ch == 'E') && seen_digit);
        if is_number_char {
            if ch.is_ascii_digit() {
                seen_digit = true;
            }
            buffer.push(ch);
            continue;
        }

        if seen_digit {
            if let Ok(number) = buffer.parse::<f64>() {
                return Some(number);
            }
        }
        buffer.clear();
        seen_digit = false;
    }
    None
}

fn strip_numbered_prefix(text: &str) -> Option<&str> {
    let mut digits = 0;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits += 1;
            continue;
        }
        if ch == '.' && digits > 0 {
            return Some(text[digits + 1..].trim());
        }
        break;
    }
    None
}

fn default_confidence(base_confidence: f64, has_parent: bool) -> f64 {
    if has_parent {
        clamp_confidence(base_confidence * 0.9)
    } else {
        clamp_confidence(base_confidence)
    }
}

fn clamp_confidence(confidence: f64) -> f64 {
    confidence.clamp(0.0, 1.0)
}

fn parse_state_field<T>(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<T>, OrchestratorError>
where
    T: for<'de> Deserialize<'de>,
{
    object
        .get(key)
        .cloned()
        .map(serde_json::from_value::<T>)
        .transpose()
        .map_err(|error| OrchestratorError::Serialization(format!("{key}: {error}")))
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use multi_agent_protocol::{MultiAgentProvider, RoleAgentSpec, RoleSpec, WorkspaceSpec};

    use super::*;

    struct FakeMessenger;

    #[async_trait]
    impl SessionMessenger for FakeMessenger {
        async fn send_to_session(
            &self,
            _session_id: &str,
            _message: &str,
        ) -> Result<(), OrchestratorError> {
            Ok(())
        }

        async fn request_response(
            &self,
            _session_id: &str,
            _message: &str,
            _mode: crate::SessionRequestMode,
        ) -> Result<String, OrchestratorError> {
            Ok(String::new())
        }
    }

    fn sample_shell() -> WorkspaceShell {
        let spec = WorkspaceSpec {
            id: "workspace-1".to_string(),
            name: "Workspace".to_string(),
            provider: MultiAgentProvider::Cteno,
            model: "gpt-5.4".to_string(),
            cwd: None,
            orchestrator_prompt: None,
            allowed_tools: None,
            disallowed_tools: None,
            permission_mode: None,
            setting_sources: None,
            roles: vec![
                RoleSpec {
                    id: "hypothesis".to_string(),
                    name: "Hypothesis".to_string(),
                    description: Some("Frames hypotheses".to_string()),
                    direct: Some(true),
                    output_root: Some("40-code/".to_string()),
                    agent: RoleAgentSpec {
                        description: "Frames hypotheses".to_string(),
                        prompt: "Hypothesize".to_string(),
                        provider: Some(MultiAgentProvider::Cteno),
                        tools: None,
                        disallowed_tools: None,
                        model: None,
                        skills: None,
                        mcp_servers: None,
                        initial_prompt: None,
                        permission_mode: None,
                    },
                },
                RoleSpec {
                    id: "worker".to_string(),
                    name: "Worker".to_string(),
                    description: Some("Runs experiments".to_string()),
                    direct: Some(true),
                    output_root: Some("40-code/".to_string()),
                    agent: RoleAgentSpec {
                        description: "Runs experiments".to_string(),
                        prompt: "Experiment".to_string(),
                        provider: Some(MultiAgentProvider::Cteno),
                        tools: None,
                        disallowed_tools: None,
                        model: None,
                        skills: None,
                        mcp_servers: None,
                        initial_prompt: None,
                        permission_mode: None,
                    },
                },
                RoleSpec {
                    id: "gate".to_string(),
                    name: "Gate".to_string(),
                    description: Some("Evaluates experiment outcomes".to_string()),
                    direct: Some(true),
                    output_root: Some("00-management/".to_string()),
                    agent: RoleAgentSpec {
                        description: "Evaluates experiment outcomes".to_string(),
                        prompt: "Evaluate".to_string(),
                        provider: Some(MultiAgentProvider::Cteno),
                        tools: None,
                        disallowed_tools: None,
                        model: None,
                        skills: None,
                        mcp_servers: None,
                        initial_prompt: None,
                        permission_mode: None,
                    },
                },
            ],
            default_role_id: Some("hypothesis".to_string()),
            coordinator_role_id: Some("hypothesis".to_string()),
            claim_policy: None,
            activity_policy: None,
            workflow_vote_policy: None,
            workflow: None,
            artifacts: None,
            completion_policy: None,
        };

        let mut shell = WorkspaceShell::new(spec);
        shell.members.get_mut("hypothesis").unwrap().session_id =
            Some("session-hypothesis".to_string());
        shell.members.get_mut("worker").unwrap().session_id = Some("session-worker".to_string());
        shell.members.get_mut("gate").unwrap().session_id = Some("session-gate".to_string());
        shell
    }

    #[tokio::test]
    async fn first_message_dispatches_hypothesis_agent() {
        let shell = sample_shell();
        let messenger = FakeMessenger;
        let mut orchestrator = AutoresearchOrchestrator::default();

        let response = orchestrator
            .handle_user_message(&shell, &messenger, "Research mention semantics", None)
            .await
            .expect("initial hypothesis dispatch should succeed");

        assert_eq!(response.dispatches.len(), 1);
        assert_eq!(response.dispatches[0].role_id, "hypothesis");
        assert_eq!(response.template_state["type"], "autoresearch");
        assert!(orchestrator.pending_hypothesis_request.is_some());
    }

    #[tokio::test]
    async fn gate_script_updates_metric_and_loops() {
        let shell = sample_shell();
        let messenger = FakeMessenger;
        let mut orchestrator = AutoresearchOrchestrator::new(
            "hypothesis",
            "worker",
            "gate",
            Some("maximize".to_string()),
        );

        orchestrator
            .handle_user_message(&shell, &messenger, "Research mention semantics", None)
            .await
            .expect("initial request should dispatch");

        let hypothesis_response = orchestrator
            .on_role_completed(
                &shell,
                &messenger,
                "hypothesis",
                r#"{"hypotheses":[{"text":"Mentions propagate through visible channel membership","confidence":0.6}]}"#,
                true,
            )
            .await
            .expect("hypothesis completion should dispatch worker");
        assert_eq!(hypothesis_response.dispatches.len(), 1);
        assert_eq!(hypothesis_response.dispatches[0].role_id, "worker");

        let worker_response = orchestrator
            .on_role_completed(&shell, &messenger, "worker", "metric: 0.82", true)
            .await
            .expect("worker completion should evaluate locally");

        assert_eq!(worker_response.dispatches.len(), 1);
        assert_eq!(worker_response.dispatches[0].role_id, "hypothesis");
        assert_eq!(orchestrator.best_metric, Some(0.82));
        assert_eq!(orchestrator.experiments[0].status, ExperimentStatus::Keep);
        assert_eq!(orchestrator.hypotheses[0].status, HypothesisStatus::Kept);
        assert!(orchestrator.hypotheses[0].confidence > 0.6);
    }

    #[tokio::test]
    async fn worker_completion_can_dispatch_gate_agent() {
        let shell = sample_shell();
        let messenger = FakeMessenger;
        let mut orchestrator = AutoresearchOrchestrator::default();

        orchestrator
            .handle_user_message(&shell, &messenger, "Research mention semantics", None)
            .await
            .expect("initial request should dispatch");
        orchestrator
            .on_role_completed(
                &shell,
                &messenger,
                "hypothesis",
                "H1: Membership gates mentions",
                true,
            )
            .await
            .expect("hypothesis completion should dispatch worker");

        let worker_response = orchestrator
            .on_role_completed(&shell, &messenger, "worker", "metric: 0.51", true)
            .await
            .expect("worker completion should dispatch gate");

        assert_eq!(worker_response.dispatches.len(), 1);
        assert_eq!(worker_response.dispatches[0].role_id, "gate");
        assert_eq!(
            orchestrator.experiments[0].status,
            ExperimentStatus::AwaitingGate
        );

        let gate_response = orchestrator
            .on_role_completed(
                &shell,
                &messenger,
                "gate",
                r#"{"decision":"keep","reason":"improved over baseline"}"#,
                true,
            )
            .await
            .expect("gate completion should advance the loop");

        assert_eq!(gate_response.dispatches.len(), 1);
        assert_eq!(gate_response.dispatches[0].role_id, "hypothesis");
        assert_eq!(orchestrator.experiments[0].status, ExperimentStatus::Keep);
        assert_eq!(orchestrator.best_metric, Some(0.51));
    }

    #[tokio::test]
    async fn split_command_expands_children_under_parent() {
        let shell = sample_shell();
        let messenger = FakeMessenger;
        let mut orchestrator = AutoresearchOrchestrator::default();

        orchestrator
            .handle_user_message(&shell, &messenger, "Research mention semantics", None)
            .await
            .expect("initial request should dispatch");
        orchestrator
            .on_role_completed(
                &shell,
                &messenger,
                "hypothesis",
                "H1: Membership gates mentions",
                true,
            )
            .await
            .expect("hypothesis completion should dispatch worker");

        orchestrator.active_experiment_id = None;
        orchestrator
            .handle_user_message(&shell, &messenger, "split H1 into narrower cases", None)
            .await
            .expect("split request should dispatch");
        let split_response = orchestrator
            .on_role_completed(
                &shell,
                &messenger,
                "hypothesis",
                "- Mention permissions depend on private-channel membership\n- Mention permissions depend on org visibility",
                true,
            )
            .await
            .expect("split completion should register children");

        assert_eq!(split_response.dispatches.len(), 1);
        assert_eq!(split_response.dispatches[0].role_id, "worker");
        assert_eq!(orchestrator.hypotheses[0].children, vec!["H2", "H3"]);
        assert_eq!(orchestrator.hypotheses[1].parent_id.as_deref(), Some("H1"));
        assert_eq!(orchestrator.hypotheses[2].parent_id.as_deref(), Some("H1"));
    }
}
