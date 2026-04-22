use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::Utc;
use futures_util::future::join_all;
use multi_agent_protocol::{
    ClaimDecision, ClaimStatus, CoordinatorDecisionKind, CoordinatorWorkflowDecision,
    DispatchStatus, RoleSpec, RoleTaskRequest, TaskDispatch, WorkspaceClaimResponse,
    WorkspaceEvent, WorkspaceInstanceParams, WorkspaceProfile, WorkspaceSpec, WorkspaceState,
    WorkspaceTemplate, WorkspaceTurnPlan, WorkspaceTurnRequest, WorkspaceVisibility,
    WorkspaceWorkflowVoteResponse, WorkspaceWorkflowVoteWindow, build_coordinator_decision_prompt,
    build_plan_from_claim_responses, build_workflow_entry_plan, build_workflow_vote_prompt,
    build_workspace_claim_prompt, direct_workspace_turn_plan, instantiate_workspace,
    parse_coordinator_decision, parse_workflow_vote_response, parse_workspace_claim_response,
    resolve_claim_candidate_role_ids, resolve_workflow_vote_candidate_role_ids,
    should_approve_workflow_vote, should_propose_workflow_heuristically,
};
use multi_agent_runtime_core::{
    OrchestratorError, RuntimeError, RuntimeTick, SessionMessenger, SessionRequestMode,
    WorkspaceRuntime,
};
use multi_agent_runtime_local::{
    LocalPersistenceError, LocalWorkspacePersistence, PersistedProviderBinding,
    PersistedProviderState, ProviderConversationKind,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::{Duration, timeout};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProvisionedRole {
    pub role_id: String,
    pub agent_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BootstrappedWorkspace {
    pub workspace_persona_id: String,
    pub workspace_session_id: String,
    pub roles: Vec<ProvisionedRole>,
}

#[async_trait]
pub trait WorkspaceProvisioner: Send + Sync {
    async fn prepare_workspace_layout(&self, spec: &WorkspaceSpec) -> Result<(), AdapterError>;
    async fn create_workspace_persona(
        &self,
        spec: &WorkspaceSpec,
    ) -> Result<(String, String), AdapterError>;
    async fn create_role_agent(
        &self,
        spec: &WorkspaceSpec,
        role: &RoleSpec,
    ) -> Result<String, AdapterError>;
    async fn spawn_role_session(
        &self,
        spec: &WorkspaceSpec,
        role: &RoleSpec,
        agent_id: &str,
        workspace_persona_id: &str,
    ) -> Result<String, AdapterError>;
    async fn cleanup_workspace(
        &self,
        spec: &WorkspaceSpec,
        bootstrapped: &BootstrappedWorkspace,
    ) -> Result<(), AdapterError>;
}

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("orchestrator error: {0}")]
    Orchestrator(#[from] OrchestratorError),
    #[error("provisioning error: {0}")]
    Provisioning(String),
    #[error("messaging error: {0}")]
    Messaging(String),
    #[error("missing provisioned role session for role {0}")]
    MissingRoleSession(String),
    #[error("local persistence error: {0}")]
    LocalPersistence(#[from] LocalPersistenceError),
    #[error("provider metadata error: {0}")]
    Metadata(String),
}

pub struct CtenoWorkspaceAdapter<P, M> {
    runtime: WorkspaceRuntime,
    provisioner: P,
    messenger: M,
    bootstrapped: Option<BootstrappedWorkspace>,
    role_sessions: BTreeMap<String, String>,
    public_context_cursors: BTreeMap<String, String>,
    persistence: Option<LocalWorkspacePersistence>,
    restored_from_persistence: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CtenoProviderMetadata {
    workspace_persona_id: String,
    workspace_session_id: String,
    roles: Vec<ProvisionedRole>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    public_context_cursors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceTurnResult {
    pub request: WorkspaceTurnRequest,
    pub plan: WorkspaceTurnPlan,
    pub workflow_vote_window: Option<WorkspaceWorkflowVoteWindow>,
    pub workflow_vote_responses: Vec<WorkspaceWorkflowVoteResponse>,
    pub role_id: Option<String>,
    pub session_id: String,
    pub dispatch: Option<TaskDispatch>,
    pub dispatches: Vec<TaskDispatch>,
    pub events: Vec<WorkspaceEvent>,
    pub state: WorkspaceState,
}

impl<P, M> CtenoWorkspaceAdapter<P, M>
where
    P: WorkspaceProvisioner,
    M: SessionMessenger,
{
    const RECENT_PUBLIC_ACTIVITY_LIMIT: usize = 25;
    const RECENT_PUBLIC_TEXT_LIMIT: usize = 240;
    const DEFAULT_CLAIM_TIMEOUT_MS: u64 = 15_000;
    const DEFAULT_VOTE_TIMEOUT_MS: u64 = 15_000;
    const DEFAULT_COORDINATOR_TIMEOUT_MS: u64 = 15_000;

    pub fn new(spec: WorkspaceSpec, provisioner: P, messenger: M) -> Self {
        let persistence = LocalWorkspacePersistence::from_spec(&spec).ok();
        Self {
            runtime: WorkspaceRuntime::new(spec),
            provisioner,
            messenger,
            bootstrapped: None,
            role_sessions: BTreeMap::new(),
            public_context_cursors: BTreeMap::new(),
            persistence,
            restored_from_persistence: false,
        }
    }

    pub fn from_template(
        template: &WorkspaceTemplate,
        instance: &WorkspaceInstanceParams,
        profile: &WorkspaceProfile,
        provisioner: P,
        messenger: M,
    ) -> Self {
        Self::new(
            instantiate_workspace(template, instance, profile),
            provisioner,
            messenger,
        )
    }

    pub fn restore_from_local(
        cwd: impl AsRef<std::path::Path>,
        workspace_id: &str,
        provisioner: P,
        messenger: M,
    ) -> Result<Self, AdapterError> {
        let persistence = LocalWorkspacePersistence::from_workspace(cwd, workspace_id);
        let spec = persistence.load_workspace_spec()?;
        let state = persistence.load_workspace_state()?;
        let history = persistence.load_events()?;
        let provider_state = persistence.load_provider_state()?;

        let mut adapter = Self::new(spec, provisioner, messenger);
        adapter.runtime.restore_snapshot(state, history);
        adapter.role_sessions = provider_state
            .member_bindings
            .iter()
            .map(|(role_id, binding)| (role_id.clone(), binding.provider_conversation_id.clone()))
            .collect();
        if let Some(metadata) = provider_state.metadata {
            let decoded: CtenoProviderMetadata = serde_json::from_value(metadata)
                .map_err(|error| AdapterError::Metadata(error.to_string()))?;
            adapter.bootstrapped = Some(BootstrappedWorkspace {
                workspace_persona_id: decoded.workspace_persona_id,
                workspace_session_id: decoded.workspace_session_id,
                roles: decoded.roles,
            });
            adapter.public_context_cursors = decoded.public_context_cursors;
        }
        adapter.restored_from_persistence = true;
        Ok(adapter)
    }

    pub fn runtime(&self) -> &WorkspaceRuntime {
        &self.runtime
    }

    pub fn bootstrapped(&self) -> Option<&BootstrappedWorkspace> {
        self.bootstrapped.as_ref()
    }

    pub fn snapshot(&self) -> WorkspaceState {
        self.runtime.snapshot()
    }

    pub fn history(&self) -> &[WorkspaceEvent] {
        self.runtime.history()
    }

    pub fn persistence_root(&self) -> Option<&std::path::Path> {
        self.persistence.as_ref().map(|p| p.root())
    }

    pub fn has_role_session(&self, session_id: &str) -> bool {
        self.role_sessions.values().any(|value| value == session_id)
    }

    pub async fn bootstrap(&mut self) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        let spec = self.runtime.spec().clone();
        let mut emitted = Vec::new();

        if !self.restored_from_persistence {
            if let Some(persistence) = self.persistence.as_ref() {
                persistence.ensure_workspace_initialized(&spec)?;
            }
        }

        self.provisioner.prepare_workspace_layout(&spec).await?;

        emitted.extend(self.runtime.start().emitted);

        let (workspace_persona_id, workspace_session_id) =
            self.provisioner.create_workspace_persona(&spec).await?;

        let mut roles = Vec::new();
        for role in &spec.roles {
            let agent_id = self.provisioner.create_role_agent(&spec, role).await?;
            let session_id = self
                .provisioner
                .spawn_role_session(&spec, role, &agent_id, &workspace_persona_id)
                .await?;
            self.role_sessions
                .insert(role.id.clone(), session_id.clone());
            emitted.extend(
                self.runtime
                    .register_member_session(&role.id, session_id.clone())?
                    .emitted,
            );
            roles.push(ProvisionedRole {
                role_id: role.id.clone(),
                agent_id,
                session_id,
            });
        }

        self.bootstrapped = Some(BootstrappedWorkspace {
            workspace_persona_id,
            workspace_session_id: workspace_session_id.clone(),
            roles,
        });

        emitted.extend(
            self.runtime
                .initialize(
                    Some(workspace_session_id),
                    spec.roles.iter().map(|role| role.id.clone()).collect(),
                    spec.allowed_tools.clone().unwrap_or_default(),
                    None,
                )
                .emitted,
        );

        self.persist_runtime(&emitted)?;

        Ok(emitted)
    }

    pub fn restore_existing(
        &mut self,
        bootstrapped: BootstrappedWorkspace,
    ) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        let spec = self.runtime.spec().clone();
        let mut emitted = self.runtime.start().emitted;

        for role in &bootstrapped.roles {
            self.role_sessions
                .insert(role.role_id.clone(), role.session_id.clone());
            emitted.extend(
                self.runtime
                    .register_member_session(&role.role_id, role.session_id.clone())?
                    .emitted,
            );
        }

        emitted.extend(
            self.runtime
                .initialize(
                    Some(bootstrapped.workspace_session_id.clone()),
                    spec.roles.iter().map(|role| role.id.clone()).collect(),
                    spec.allowed_tools.clone().unwrap_or_default(),
                    None,
                )
                .emitted,
        );

        self.bootstrapped = Some(bootstrapped);
        self.persist_runtime(&emitted)?;
        Ok(emitted)
    }

    pub async fn assign_role_task(
        &mut self,
        request: RoleTaskRequest,
    ) -> Result<(multi_agent_protocol::TaskDispatch, Vec<WorkspaceEvent>), AdapterError> {
        let role_id = request.role_id.clone();
        let role_session_id = self
            .role_sessions
            .get(&role_id)
            .cloned()
            .ok_or_else(|| AdapterError::MissingRoleSession(role_id.clone()))?;

        let (dispatch, queued_tick) = self.runtime.queue_dispatch(request)?;
        let contextual_instruction = self.build_role_dispatch_message(
            &dispatch.role_id,
            &dispatch.instruction,
            dispatch.workflow_node_id.as_deref(),
        );
        self.messenger
            .send_to_session(&role_session_id, &contextual_instruction)
            .await?;
        self.advance_public_context_cursor(&dispatch.role_id);
        self.persist_runtime(&queued_tick.emitted)?;

        Ok((dispatch, queued_tick.emitted))
    }

    async fn request_coordinator_decision(
        &mut self,
        request: &WorkspaceTurnRequest,
        claim_responses: Option<&[WorkspaceClaimResponse]>,
    ) -> Result<CoordinatorWorkflowDecision, AdapterError> {
        let coordinator_role_id = self
            .runtime
            .spec()
            .coordinator_role_id
            .clone()
            .or_else(|| self.runtime.spec().default_role_id.clone())
            .ok_or_else(|| {
                AdapterError::Messaging(
                    "workspace has no coordinator role for coordinator decision".to_string(),
                )
            })?;
        let coordinator_session_id = self
            .role_sessions
            .get(&coordinator_role_id)
            .cloned()
            .ok_or_else(|| AdapterError::MissingRoleSession(coordinator_role_id.clone()))?;
        let prompt = self.with_public_context_delta(
            &coordinator_role_id,
            &build_coordinator_decision_prompt(self.runtime.spec(), request, claim_responses),
            "New public workspace context since your last sync",
        );
        let raw = timeout(
            Duration::from_millis(Self::DEFAULT_COORDINATOR_TIMEOUT_MS),
            self.messenger.request_response(
                &coordinator_session_id,
                &prompt,
                SessionRequestMode::CoordinatorDecision,
            ),
        )
        .await
        .map_err(|_| {
            AdapterError::Messaging(format!(
                "coordinator decision timed out after {} ms",
                Self::DEFAULT_COORDINATOR_TIMEOUT_MS
            ))
        })??;
        self.advance_public_context_cursor(&coordinator_role_id);
        Ok(parse_coordinator_decision(
            &raw,
            self.runtime.spec(),
            request,
        ))
    }

    async fn collect_claim_responses(
        &mut self,
        request: &WorkspaceTurnRequest,
    ) -> Vec<WorkspaceClaimResponse> {
        let candidate_role_ids = resolve_claim_candidate_role_ids(self.runtime.spec(), request);
        let timeout_ms = self
            .runtime
            .spec()
            .claim_policy
            .as_ref()
            .and_then(|policy| policy.claim_timeout_ms)
            .unwrap_or(Self::DEFAULT_CLAIM_TIMEOUT_MS);
        let mut pending = Vec::new();
        for role_id in candidate_role_ids {
            let Some(role) = self
                .runtime
                .spec()
                .roles
                .iter()
                .find(|role| role.id == role_id)
                .cloned()
            else {
                continue;
            };
            let Some(session_id) = self.role_sessions.get(&role.id).cloned() else {
                continue;
            };
            let prompt = self.with_public_context_delta(
                &role.id,
                &build_workspace_claim_prompt(self.runtime.spec(), &role, request),
                "New public workspace context since your last sync",
            );
            pending.push((role, session_id, prompt));
        }

        let messenger = &self.messenger;
        let results = join_all(pending.iter().map(|(role, session_id, prompt)| async move {
            (
                role.clone(),
                timeout(
                    Duration::from_millis(timeout_ms),
                    messenger.request_response(session_id, prompt, SessionRequestMode::Claim),
                )
                .await,
            )
        }))
        .await;

        let mut responses = Vec::new();
        for (role, result) in results {
            let response = match result {
                Ok(Ok(raw)) => {
                    self.advance_public_context_cursor(&role.id);
                    parse_workspace_claim_response(&raw, &role, request)
                }
                Ok(Err(error)) => WorkspaceClaimResponse {
                    role_id: role.id.clone(),
                    decision: ClaimDecision::Decline,
                    confidence: 0.1,
                    rationale: format!("@{} could not complete claim check: {}", role.id, error),
                    public_response: Some(format!("@{} is unavailable for this turn.", role.id)),
                    proposed_instruction: None,
                },
                Err(_) => WorkspaceClaimResponse {
                    role_id: role.id.clone(),
                    decision: ClaimDecision::Decline,
                    confidence: 0.1,
                    rationale: format!(
                        "@{} claim check timed out after {} ms",
                        role.id, timeout_ms
                    ),
                    public_response: Some(format!("@{} did not respond in time.", role.id)),
                    proposed_instruction: None,
                },
            };
            responses.push(response);
        }
        responses
    }

    async fn collect_workflow_vote_responses(
        &mut self,
        request: &WorkspaceTurnRequest,
        coordinator_decision: &CoordinatorWorkflowDecision,
        candidate_role_ids: &[String],
    ) -> Vec<WorkspaceWorkflowVoteResponse> {
        let timeout_ms = self
            .runtime
            .spec()
            .workflow_vote_policy
            .as_ref()
            .and_then(|policy| policy.timeout_ms)
            .unwrap_or(Self::DEFAULT_VOTE_TIMEOUT_MS);
        let mut pending = Vec::new();
        for role_id in candidate_role_ids {
            let Some(role) = self
                .runtime
                .spec()
                .roles
                .iter()
                .find(|role| role.id == *role_id)
                .cloned()
            else {
                continue;
            };
            let Some(session_id) = self.role_sessions.get(&role.id).cloned() else {
                continue;
            };
            let prompt = self.with_public_context_delta(
                &role.id,
                &build_workflow_vote_prompt(
                    self.runtime.spec(),
                    &role,
                    request,
                    coordinator_decision,
                ),
                "New public workspace context since your last sync",
            );
            pending.push((role, session_id, prompt));
        }

        let messenger = &self.messenger;
        let results = join_all(pending.iter().map(|(role, session_id, prompt)| async move {
            (
                role.clone(),
                timeout(
                    Duration::from_millis(timeout_ms),
                    messenger.request_response(
                        session_id,
                        prompt,
                        SessionRequestMode::WorkflowVote,
                    ),
                )
                .await,
            )
        }))
        .await;

        let mut responses = Vec::new();
        for (role, result) in results {
            let response = match result {
                Ok(Ok(raw)) => {
                    self.advance_public_context_cursor(&role.id);
                    parse_workflow_vote_response(
                        &raw,
                        &role,
                        self.runtime.spec(),
                        request,
                        coordinator_decision,
                    )
                }
                Ok(Err(error)) => WorkspaceWorkflowVoteResponse {
                    role_id: role.id.clone(),
                    decision: multi_agent_protocol::WorkflowVoteDecision::Abstain,
                    confidence: 0.1,
                    rationale: format!("@{} could not complete workflow vote: {}", role.id, error),
                    public_response: Some(format!("@{} abstained.", role.id)),
                },
                Err(_) => WorkspaceWorkflowVoteResponse {
                    role_id: role.id.clone(),
                    decision: multi_agent_protocol::WorkflowVoteDecision::Abstain,
                    confidence: 0.1,
                    rationale: format!(
                        "@{} workflow vote timed out after {} ms",
                        role.id, timeout_ms
                    ),
                    public_response: Some(format!("@{} did not vote in time.", role.id)),
                },
            };
            responses.push(response);
        }
        responses
    }

    pub async fn send_workspace_turn(
        &mut self,
        message: &str,
        role_id: Option<&str>,
    ) -> Result<WorkspaceTurnResult, AdapterError> {
        let request = WorkspaceTurnRequest {
            message: message.to_string(),
            visibility: Some(WorkspaceVisibility::Public),
            max_assignments: role_id
                .is_none()
                .then(|| infer_group_reply_max_assignments(self.runtime.spec(), message))
                .flatten(),
            prefer_role_id: role_id.map(ToString::to_string),
        };
        let mut events = self.runtime.publish_user_message(message).emitted;
        let coordinator_role_id = self
            .runtime
            .spec()
            .coordinator_role_id
            .clone()
            .or_else(|| self.runtime.spec().default_role_id.clone())
            .unwrap_or_else(|| "coordinator".to_string());
        let mut workflow_vote_window = None;
        let mut workflow_vote_responses = Vec::new();
        let plan = if let Some(role_id) = role_id {
            let coordinator_decision = CoordinatorWorkflowDecision {
                kind: CoordinatorDecisionKind::Delegate,
                response_text: format!("@{} will take this next.", role_id),
                target_role_id: Some(role_id.to_string()),
                workflow_vote_reason: None,
                rationale: Some("Direct role targeting bypassed coordinator routing.".to_string()),
            };
            events.extend(
                self.runtime
                    .record_role_message(
                        &coordinator_role_id,
                        coordinator_decision.response_text.clone(),
                        WorkspaceVisibility::Public,
                        None,
                        None,
                    )?
                    .emitted,
            );
            direct_workspace_turn_plan(self.runtime.spec(), &request, role_id)
        } else {
            let claim_tick = self.runtime.open_claim_window(request.clone());
            let claim_window = claim_tick
                .emitted
                .iter()
                .find_map(|event| match event {
                    WorkspaceEvent::ClaimWindowOpened { claim_window, .. } => {
                        Some(claim_window.clone())
                    }
                    _ => None,
                })
                .ok_or_else(|| {
                    AdapterError::Messaging("claim window did not emit an opened event".to_string())
                })?;
            events.extend(claim_tick.emitted);

            let claim_responses = self.collect_claim_responses(&request).await;
            for response in &claim_responses {
                events.extend(
                    self.runtime
                        .record_claim_response(&claim_window, response.clone())?
                        .emitted,
                );
            }

            let claim_signal_count = claim_responses
                .iter()
                .filter(|response| response.decision != ClaimDecision::Decline)
                .count();
            let claim_plan = if claim_signal_count == 0 {
                WorkspaceTurnPlan {
                    coordinator_role_id: coordinator_role_id.clone(),
                    response_text: String::new(),
                    assignments: Vec::new(),
                    rationale: Some(
                        "No members claimed or supported the request during group-chat mode."
                            .to_string(),
                    ),
                }
            } else {
                build_plan_from_claim_responses(self.runtime.spec(), &request, &claim_responses)
            };
            let selected_role_ids = claim_plan
                .assignments
                .iter()
                .map(|assignment| assignment.role_id.clone())
                .collect::<Vec<_>>();
            events.extend(
                self.runtime
                    .close_claim_window(
                        claim_window,
                        claim_responses.clone(),
                        selected_role_ids.clone(),
                    )
                    .emitted,
            );

            let should_check_workflow =
                should_propose_workflow_heuristically(self.runtime.spec(), &request.message);
            let needs_no_claim_fallback = claim_plan.assignments.is_empty();

            let coordinator_decision = if should_check_workflow || needs_no_claim_fallback {
                Some(
                    self.request_coordinator_decision(&request, Some(&claim_responses))
                        .await
                        .unwrap_or_else(|_| CoordinatorWorkflowDecision {
                            kind: CoordinatorDecisionKind::Respond,
                            response_text: format!(
                                "@{} did not receive any claims and will wait for clarification.",
                                coordinator_role_id
                            ),
                            target_role_id: None,
                            workflow_vote_reason: None,
                            rationale: Some(
                                "Coordinator fallback responded because no group-chat claim succeeded."
                                    .to_string(),
                            ),
                        }),
                )
            } else {
                None
            };

            if let Some(coordinator_decision) = coordinator_decision {
                if coordinator_decision.kind == CoordinatorDecisionKind::ProposeWorkflow {
                    if !coordinator_decision.response_text.trim().is_empty() {
                        events.extend(
                            self.runtime
                                .record_role_message(
                                    &coordinator_role_id,
                                    coordinator_decision.response_text.clone(),
                                    WorkspaceVisibility::Public,
                                    None,
                                    None,
                                )?
                                .emitted,
                        );
                    }
                    let candidate_role_ids =
                        resolve_workflow_vote_candidate_role_ids(self.runtime.spec());
                    let vote_tick = self.runtime.open_workflow_vote_window(
                        request.clone(),
                        coordinator_decision.clone(),
                        candidate_role_ids.clone(),
                    );
                    events.extend(vote_tick.emitted);
                    let vote_window = vote_tick.state.workflow_runtime.active_vote_window.clone();
                    workflow_vote_window = vote_window.clone();
                    workflow_vote_responses = self
                        .collect_workflow_vote_responses(
                            &request,
                            &coordinator_decision,
                            &candidate_role_ids,
                        )
                        .await;
                    if let Some(vote_window) = vote_window.as_ref() {
                        for response in &workflow_vote_responses {
                            events.extend(
                                self.runtime
                                    .record_workflow_vote_response(vote_window, response.clone())?
                                    .emitted,
                            );
                        }
                    }
                    let approved =
                        should_approve_workflow_vote(self.runtime.spec(), &workflow_vote_responses);
                    if let Some(vote_window) = vote_window.clone() {
                        events.extend(
                            self.runtime
                                .close_workflow_vote_window(
                                    vote_window.clone(),
                                    coordinator_decision.clone(),
                                    workflow_vote_responses.clone(),
                                    approved,
                                )
                                .emitted,
                        );
                    }
                    if approved {
                        let workflow_plan =
                            build_workflow_entry_plan(self.runtime.spec(), &request);
                        let first_assignment = workflow_plan.assignments.first();
                        events.extend(
                            self.runtime
                                .start_workflow(
                                    coordinator_decision.clone(),
                                    workflow_vote_window.clone(),
                                    Some(request.message.clone()),
                                    first_assignment
                                        .and_then(|assignment| assignment.workflow_node_id.clone()),
                                    first_assignment
                                        .and_then(|assignment| assignment.stage_id.clone()),
                                )
                                .emitted,
                        );
                        workflow_plan
                    } else if !claim_plan.assignments.is_empty() {
                        claim_plan
                    } else {
                        WorkspaceTurnPlan {
                            coordinator_role_id: coordinator_role_id.clone(),
                            response_text: coordinator_decision.response_text.clone(),
                            assignments: Vec::new(),
                            rationale: Some(
                                "Workflow vote rejected; staying in group chat mode.".to_string(),
                            ),
                        }
                    }
                } else if claim_plan.assignments.is_empty() {
                    if !coordinator_decision.response_text.trim().is_empty() {
                        events.extend(
                            self.runtime
                                .record_role_message(
                                    &coordinator_role_id,
                                    coordinator_decision.response_text.clone(),
                                    WorkspaceVisibility::Public,
                                    None,
                                    None,
                                )?
                                .emitted,
                        );
                    }

                    match coordinator_decision.kind {
                        CoordinatorDecisionKind::Respond => WorkspaceTurnPlan {
                            coordinator_role_id: coordinator_role_id.clone(),
                            response_text: coordinator_decision.response_text.clone(),
                            assignments: Vec::new(),
                            rationale: coordinator_decision.rationale.clone(),
                        },
                        CoordinatorDecisionKind::Delegate => {
                            if let Some(target_role_id) =
                                coordinator_decision.target_role_id.clone()
                            {
                                direct_workspace_turn_plan(
                                    self.runtime.spec(),
                                    &request,
                                    &target_role_id,
                                )
                            } else {
                                WorkspaceTurnPlan {
                                    coordinator_role_id: coordinator_role_id.clone(),
                                    response_text: coordinator_decision.response_text.clone(),
                                    assignments: Vec::new(),
                                    rationale: coordinator_decision.rationale.clone(),
                                }
                            }
                        }
                        CoordinatorDecisionKind::ProposeWorkflow => unreachable!(),
                    }
                } else {
                    claim_plan
                }
            } else {
                claim_plan
            }
        };

        let Some(primary_assignment) = plan.assignments.first().cloned() else {
            let session_id = self
                .bootstrapped
                .as_ref()
                .map(|bootstrapped| bootstrapped.workspace_session_id.clone())
                .ok_or_else(|| {
                    AdapterError::Messaging(
                        "workspace has no coordinator role or workspace session".to_string(),
                    )
                })?;
            self.messenger.send_to_session(&session_id, message).await?;
            let result = WorkspaceTurnResult {
                request,
                plan,
                workflow_vote_window,
                workflow_vote_responses,
                role_id: None,
                session_id,
                dispatch: None,
                dispatches: Vec::new(),
                events,
                state: self.runtime.snapshot(),
            };
            self.persist_runtime(&result.events)?;
            return Ok(result);
        };

        let role_session_id = self
            .role_sessions
            .get(&primary_assignment.role_id)
            .cloned()
            .ok_or_else(|| AdapterError::MissingRoleSession(primary_assignment.role_id.clone()))?;

        let mut dispatches = Vec::new();
        for assignment in &plan.assignments {
            let mut run_dispatches = self
                .dispatch_assignment_chain(
                    RoleTaskRequest {
                        role_id: assignment.role_id.clone(),
                        instruction: assignment.instruction.clone(),
                        summary: assignment
                            .summary
                            .clone()
                            .or_else(|| Some(summarize_workspace_message(&assignment.instruction))),
                        visibility: assignment.visibility.or(Some(WorkspaceVisibility::Public)),
                        source_role_id: Some(plan.coordinator_role_id.clone()),
                        workflow_node_id: assignment.workflow_node_id.clone(),
                        stage_id: assignment.stage_id.clone(),
                    },
                    if role_id.is_some() {
                        Some("Directly addressed by user".to_string())
                    } else {
                        Some("Claimed by coordinator routing".to_string())
                    },
                )
                .await?;
            events.append(&mut run_dispatches.1);
            dispatches.append(&mut run_dispatches.0);
        }

        let result = WorkspaceTurnResult {
            request,
            plan,
            workflow_vote_window,
            workflow_vote_responses,
            role_id: Some(primary_assignment.role_id),
            session_id: role_session_id,
            dispatch: dispatches.first().cloned(),
            dispatches,
            events,
            state: self.runtime.snapshot(),
        };
        self.persist_runtime(&result.events)?;
        Ok(result)
    }

    async fn dispatch_assignment_chain(
        &mut self,
        request: RoleTaskRequest,
        claim_note: Option<String>,
    ) -> Result<(Vec<TaskDispatch>, Vec<WorkspaceEvent>), AdapterError> {
        let dispatch = self.dispatch_assignment_once(request, claim_note).await?;
        Ok((vec![dispatch.0], dispatch.1))
    }

    async fn dispatch_assignment_once(
        &mut self,
        request: RoleTaskRequest,
        claim_note: Option<String>,
    ) -> Result<(TaskDispatch, Vec<WorkspaceEvent>), AdapterError> {
        let role_session_id = self
            .role_sessions
            .get(&request.role_id)
            .cloned()
            .ok_or_else(|| AdapterError::MissingRoleSession(request.role_id.clone()))?;

        let (dispatch, queued_tick) = self.runtime.queue_dispatch(request)?;
        let mut emitted = queued_tick.emitted;

        let should_claim = self
            .runtime
            .snapshot()
            .dispatches
            .get(&dispatch.dispatch_id)
            .and_then(|stored| stored.claim_status)
            != Some(ClaimStatus::Claimed);
        if should_claim {
            emitted.extend(
                self.runtime
                    .claim_dispatch(
                        dispatch.dispatch_id,
                        &dispatch.role_id,
                        ClaimStatus::Claimed,
                        claim_note,
                    )?
                    .emitted,
            );
        }

        let contextual_instruction = self.build_role_dispatch_message(
            &dispatch.role_id,
            &dispatch.instruction,
            dispatch.workflow_node_id.as_deref(),
        );
        self.messenger
            .send_to_session(&role_session_id, &contextual_instruction)
            .await?;
        self.advance_public_context_cursor(&dispatch.role_id);

        let provider_task_id = format!("cteno:{}:{}", role_session_id, dispatch.dispatch_id);
        let (_, started_tick) = self.runtime.start_next_dispatch(
            provider_task_id,
            dispatch
                .summary
                .clone()
                .unwrap_or_else(|| summarize_workspace_message(&dispatch.instruction)),
            None,
        )?;
        emitted.extend(started_tick.emitted);

        let final_dispatch = self
            .runtime
            .snapshot()
            .dispatches
            .get(&dispatch.dispatch_id)
            .cloned()
            .expect("dispatch should exist after start_next_dispatch");

        Ok((final_dispatch, emitted))
    }

    fn build_role_dispatch_message(
        &self,
        role_id: &str,
        instruction: &str,
        workflow_node_id: Option<&str>,
    ) -> String {
        let mut sections = Vec::new();
        sections.push(
            "You are replying inside a shared workspace group chat. Use the recent public context below to understand what the team has already said.".to_string(),
        );
        if let Some(node_id) = workflow_node_id {
            sections.push(format!("Current workflow node: {}", node_id));
        }
        let public_context = self.build_public_context_delta(role_id);
        if !public_context.is_empty() {
            sections.push(format!(
                "New public workspace context since your last sync:\n{}",
                public_context
            ));
        }
        sections.push(format!("Current task for you:\n{}", instruction.trim()));
        sections.join("\n\n")
    }

    fn with_public_context_delta(&self, role_id: &str, prompt: &str, heading: &str) -> String {
        let public_context = self.build_public_context_delta(role_id);
        if public_context.is_empty() {
            prompt.to_string()
        } else {
            format!("{}\n\n{}:\n{}", prompt.trim_end(), heading, public_context)
        }
    }

    fn build_public_context_delta(&self, role_id: &str) -> String {
        let snapshot = self.runtime.snapshot();
        let mut lines = Vec::new();

        if let Some(mode_line) = self.describe_workflow_runtime(&snapshot.workflow_runtime) {
            lines.push(format!("[workspace] {}", mode_line));
        }

        let public_activities = snapshot
            .activities
            .iter()
            .filter(|activity| activity.visibility == WorkspaceVisibility::Public)
            .collect::<Vec<_>>();
        let start_index = self
            .public_context_cursors
            .get(role_id)
            .and_then(|cursor| {
                public_activities
                    .iter()
                    .position(|activity| activity.activity_id.to_string() == *cursor)
                    .map(|index| index + 1)
            })
            .unwrap_or_else(|| {
                public_activities
                    .len()
                    .saturating_sub(Self::RECENT_PUBLIC_ACTIVITY_LIMIT)
            });

        for activity in public_activities.into_iter().skip(start_index) {
            if activity.role_id.as_deref() == Some(role_id)
                || activity.member_id.as_deref() == Some(role_id)
            {
                continue;
            }
            let actor = activity
                .role_id
                .as_deref()
                .or(activity.member_id.as_deref())
                .unwrap_or("workspace");
            let text = truncate_context_text(&activity.text, Self::RECENT_PUBLIC_TEXT_LIMIT);
            lines.push(format!(
                "[{}][{}][{}] {}",
                activity.activity_id,
                actor,
                activity_kind_label(activity.kind),
                text
            ));
        }

        lines.join("\n")
    }

    fn advance_public_context_cursor(&mut self, role_id: &str) {
        if let Some(activity_id) = self
            .runtime
            .snapshot()
            .activities
            .iter()
            .rev()
            .find(|activity| activity.visibility == WorkspaceVisibility::Public)
            .map(|activity| activity.activity_id.to_string())
        {
            self.public_context_cursors
                .insert(role_id.to_string(), activity_id);
        }
    }

    fn describe_workflow_runtime(
        &self,
        runtime: &multi_agent_protocol::WorkspaceWorkflowRuntimeState,
    ) -> Option<String> {
        match runtime.mode {
            multi_agent_protocol::WorkspaceMode::GroupChat => None,
            multi_agent_protocol::WorkspaceMode::WorkflowVote => Some(format!(
                "workflow vote is active{}",
                runtime
                    .active_vote_window
                    .as_ref()
                    .map(|window| format!(" for reason: {}", window.reason))
                    .unwrap_or_default()
            )),
            multi_agent_protocol::WorkspaceMode::WorkflowRunning => Some(format!(
                "workflow is running{}{}",
                runtime
                    .active_node_id
                    .as_ref()
                    .map(|node| format!(" at node {}", node))
                    .unwrap_or_default(),
                runtime
                    .active_stage_id
                    .as_ref()
                    .map(|stage| format!(" (stage {})", stage))
                    .unwrap_or_default()
            )),
        }
    }

    pub fn start_provider_task(
        &mut self,
        provider_task_id: &str,
        description: &str,
        tool_use_id: Option<String>,
    ) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        let events = self
            .runtime
            .start_next_dispatch(
                provider_task_id.to_string(),
                description.to_string(),
                tool_use_id,
            )?
            .1
            .emitted;
        self.persist_runtime(&events)?;
        Ok(events)
    }

    pub fn progress_provider_task(
        &mut self,
        provider_task_id: &str,
        description: &str,
        summary: Option<String>,
        last_tool_name: Option<String>,
    ) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        let events = self
            .runtime
            .progress_dispatch(
                provider_task_id,
                description.to_string(),
                summary,
                last_tool_name,
            )?
            .emitted;
        self.persist_runtime(&events)?;
        Ok(events)
    }

    pub async fn complete_provider_task(
        &mut self,
        provider_task_id: &str,
        status: DispatchStatus,
        output_file: Option<String>,
        summary: &str,
        result_text: Option<String>,
    ) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        let mut emitted = self
            .runtime
            .complete_dispatch(provider_task_id, status, output_file, summary.to_string())?
            .emitted;

        if let Some(result_text) = result_text {
            emitted.extend(
                self.runtime
                    .attach_result_text(provider_task_id, result_text)?
                    .emitted,
            );
        }

        let (advance_tick, followups) = self
            .runtime
            .advance_workflow_after_dispatch(provider_task_id)?;
        emitted.extend(advance_tick.emitted);

        for followup in followups {
            let (_, mut followup_events) = self
                .dispatch_assignment_chain(
                    followup,
                    Some("Claimed by workflow progression".to_string()),
                )
                .await?;
            emitted.append(&mut followup_events);
        }

        self.persist_runtime(&emitted)?;
        Ok(emitted)
    }

    pub fn record_message(&mut self, role: &str, text: &str) -> RuntimeTick {
        self.runtime
            .record_role_message(role, text, WorkspaceVisibility::Public, None, None)
            .unwrap_or_else(|_| RuntimeTick {
                state: self.runtime.snapshot(),
                emitted: vec![WorkspaceEvent::Error {
                    timestamp: Utc::now().to_rfc3339(),
                    workspace_id: self.runtime.spec().id.clone(),
                    error: format!("failed to record message for role {}", role),
                }],
            })
    }

    pub async fn ingest_member_response(
        &mut self,
        session_id: &str,
        response_text: &str,
        success: bool,
    ) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        let Some(role_id) = self
            .role_sessions
            .iter()
            .find_map(|(role_id, value)| (value == session_id).then_some(role_id.clone()))
        else {
            return Ok(Vec::new());
        };

        let active_dispatch = self
            .runtime
            .snapshot()
            .dispatches
            .values()
            .filter(|dispatch| {
                dispatch.role_id == role_id
                    && matches!(
                        dispatch.status,
                        DispatchStatus::Queued | DispatchStatus::Started | DispatchStatus::Running
                    )
            })
            .filter(|dispatch| {
                dispatch
                    .provider_task_id
                    .as_deref()
                    .map(|task_id| task_id.starts_with(&format!("cteno:{}:", session_id)))
                    .unwrap_or(false)
            })
            .max_by(|left, right| left.created_at.cmp(&right.created_at))
            .cloned();

        let summary = summarize_workspace_message(response_text);
        if let Some(dispatch) = active_dispatch {
            let provider_task_id = dispatch
                .provider_task_id
                .unwrap_or_else(|| format!("cteno:{}:{}", session_id, dispatch.dispatch_id));
            return self
                .complete_provider_task(
                    &provider_task_id,
                    if success {
                        DispatchStatus::Completed
                    } else {
                        DispatchStatus::Failed
                    },
                    dispatch.output_file,
                    &summary,
                    Some(response_text.to_string()),
                )
                .await;
        }

        let emitted = self.record_message(&role_id, &summary).emitted;
        self.persist_runtime(&emitted)?;
        Ok(emitted)
    }

    pub async fn delete_workspace(&mut self) -> Result<(), AdapterError> {
        if let Some(bootstrapped) = self.bootstrapped.as_ref() {
            self.provisioner
                .cleanup_workspace(self.runtime.spec(), bootstrapped)
                .await?;
        }
        self.role_sessions.clear();
        self.bootstrapped = None;
        if let Some(persistence) = self.persistence.as_ref() {
            persistence.delete_workspace()?;
        }
        Ok(())
    }

    fn build_provider_state(&self) -> PersistedProviderState {
        PersistedProviderState {
            workspace_id: self.runtime.spec().id.clone(),
            provider: multi_agent_protocol::MultiAgentProvider::Cteno,
            root_conversation_id: self
                .bootstrapped
                .as_ref()
                .map(|bootstrapped| bootstrapped.workspace_session_id.clone()),
            member_bindings: self
                .role_sessions
                .iter()
                .map(|(role_id, session_id)| {
                    (
                        role_id.clone(),
                        PersistedProviderBinding {
                            role_id: role_id.clone(),
                            provider_conversation_id: session_id.clone(),
                            kind: ProviderConversationKind::Session,
                            updated_at: Utc::now().to_rfc3339(),
                        },
                    )
                })
                .collect(),
            metadata: self.bootstrapped.as_ref().and_then(|bootstrapped| {
                serde_json::to_value(CtenoProviderMetadata {
                    workspace_persona_id: bootstrapped.workspace_persona_id.clone(),
                    workspace_session_id: bootstrapped.workspace_session_id.clone(),
                    roles: bootstrapped.roles.clone(),
                    public_context_cursors: self.public_context_cursors.clone(),
                })
                .ok()
            }),
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    fn persist_runtime(&self, events: &[WorkspaceEvent]) -> Result<(), AdapterError> {
        if let Some(persistence) = self.persistence.as_ref() {
            persistence.persist_runtime(
                &self.runtime.snapshot(),
                events,
                &self.build_provider_state(),
            )?;
        }
        Ok(())
    }
}

fn summarize_workspace_message(message: &str) -> String {
    const MAX_LEN: usize = 120;
    let trimmed = message.trim().replace('\n', " ");
    if trimmed.chars().count() <= MAX_LEN {
        return trimmed;
    }
    let mut out = String::with_capacity(MAX_LEN + 1);
    for ch in trimmed.chars().take(MAX_LEN.saturating_sub(1)) {
        out.push(ch);
    }
    out.push('…');
    out
}

fn truncate_context_text(message: &str, max_len: usize) -> String {
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_len {
        return normalized;
    }
    let mut out = String::with_capacity(max_len + 1);
    for ch in normalized.chars().take(max_len.saturating_sub(1)) {
        out.push(ch);
    }
    out.push('…');
    out
}

fn activity_kind_label(kind: multi_agent_protocol::WorkspaceActivityKind) -> &'static str {
    use multi_agent_protocol::WorkspaceActivityKind as Kind;

    match kind {
        Kind::UserMessage => "user_message",
        Kind::CoordinatorMessage => "coordinator_message",
        Kind::ClaimWindowOpened => "claim_window_opened",
        Kind::ClaimWindowClosed => "claim_window_closed",
        Kind::MemberClaimed => "member_claimed",
        Kind::MemberSupporting => "member_supporting",
        Kind::MemberDeclined => "member_declined",
        Kind::MemberProgress => "member_progress",
        Kind::MemberBlocked => "member_blocked",
        Kind::MemberDelivered => "member_delivered",
        Kind::MemberSummary => "member_summary",
        Kind::WorkflowVoteOpened => "workflow_vote_opened",
        Kind::WorkflowVoteApproved => "workflow_vote_approved",
        Kind::WorkflowVoteRejected => "workflow_vote_rejected",
        Kind::WorkflowStarted => "workflow_started",
        Kind::WorkflowStageStarted => "workflow_stage_started",
        Kind::WorkflowStageCompleted => "workflow_stage_completed",
        Kind::WorkflowCompleted => "workflow_completed",
        Kind::DispatchStarted => "dispatch_started",
        Kind::DispatchProgress => "dispatch_progress",
        Kind::DispatchCompleted => "dispatch_completed",
        Kind::SystemNotice => "system_notice",
    }
}

fn infer_group_reply_max_assignments(spec: &WorkspaceSpec, message: &str) -> Option<u8> {
    let lowered = message.to_lowercase();
    let multi_reply_signals = [
        "各自",
        "分别",
        "每个人",
        "都说一下",
        "都报一下",
        "同步一下",
        "报一下进展",
        "分别汇报",
        "进展",
        "status update",
        "each of you",
        "everyone",
        "report back",
    ];
    if multi_reply_signals
        .iter()
        .any(|signal| lowered.contains(signal))
    {
        let max_roles = spec.roles.len().clamp(1, u8::MAX as usize) as u8;
        Some(max_roles)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use multi_agent_protocol::{
        MultiAgentProvider, RoleAgentSpec, RoleSpec, RoleTaskRequest, WorkspaceInstanceParams,
        WorkspaceSpec, create_autoresearch_template, create_claude_workspace_profile,
        create_codex_workspace_profile, create_coding_studio_template,
    };

    use super::*;

    #[derive(Clone, Default)]
    struct FakeProvisioner {
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl WorkspaceProvisioner for FakeProvisioner {
        async fn prepare_workspace_layout(&self, spec: &WorkspaceSpec) -> Result<(), AdapterError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("prepare:{}", spec.id));
            Ok(())
        }

        async fn create_workspace_persona(
            &self,
            spec: &WorkspaceSpec,
        ) -> Result<(String, String), AdapterError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("persona:{}", spec.id));
            Ok(("persona-1".to_string(), "session-main".to_string()))
        }

        async fn create_role_agent(
            &self,
            _spec: &WorkspaceSpec,
            role: &RoleSpec,
        ) -> Result<String, AdapterError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("agent:{}", role.id));
            Ok(format!("agent-{}", role.id))
        }

        async fn spawn_role_session(
            &self,
            _spec: &WorkspaceSpec,
            role: &RoleSpec,
            _agent_id: &str,
            _workspace_persona_id: &str,
        ) -> Result<String, AdapterError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("session:{}", role.id));
            Ok(format!("session-{}", role.id))
        }

        async fn cleanup_workspace(
            &self,
            spec: &WorkspaceSpec,
            _bootstrapped: &BootstrappedWorkspace,
        ) -> Result<(), AdapterError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("cleanup:{}", spec.id));
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct FakeMessenger {
        sent: Arc<Mutex<Vec<(String, String)>>>,
        requests: Arc<Mutex<Vec<(String, SessionRequestMode)>>>,
    }

    #[async_trait]
    impl SessionMessenger for FakeMessenger {
        async fn send_to_session(
            &self,
            session_id: &str,
            message: &str,
        ) -> Result<(), OrchestratorError> {
            self.sent
                .lock()
                .unwrap()
                .push((session_id.to_string(), message.to_string()));
            Ok(())
        }

        async fn request_response(
            &self,
            session_id: &str,
            _message: &str,
            mode: SessionRequestMode,
        ) -> Result<String, OrchestratorError> {
            self.requests
                .lock()
                .unwrap()
                .push((session_id.to_string(), mode));
            let response = match mode {
                SessionRequestMode::Claim => {
                    if session_id.ends_with("coder") {
                        r#"{"decision":"claim","confidence":0.9,"rationale":"coder can own this","publicResponse":"@coder can take this.","proposedInstruction":"Implement the requested change"}"#
                    } else {
                        r#"{"decision":"decline","confidence":0.2,"rationale":"not my lane","publicResponse":"","proposedInstruction":""}"#
                    }
                }
                SessionRequestMode::CoordinatorDecision => {
                    r#"{"kind":"delegate","responseText":"@coder will take this next.","targetRoleId":"coder","workflowVoteReason":"","rationale":"default fake coordinator routing"}"#
                }
                SessionRequestMode::WorkflowVote => {
                    r#"{"decision":"approve","confidence":0.8,"rationale":"workflow is fine","publicResponse":"@coder approves entering workflow mode."}"#
                }
                SessionRequestMode::Work => r#"{"response":"ok"}"#,
            };
            Ok(response.to_string())
        }
    }

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_workspace_dir(label: &str) -> String {
        std::env::temp_dir()
            .join(format!(
                "multi-agent-runtime-cteno-{label}-{}-{}-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                std::process::id(),
                TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed)
            ))
            .to_string_lossy()
            .to_string()
    }

    fn sample_spec() -> WorkspaceSpec {
        WorkspaceSpec {
            id: "workspace-1".to_string(),
            name: "Cteno Workspace".to_string(),
            provider: MultiAgentProvider::Cteno,
            model: "claude-sonnet-4-5".to_string(),
            cwd: Some(temp_workspace_dir("sample")),
            orchestrator_prompt: None,
            allowed_tools: Some(vec!["Read".to_string(), "Edit".to_string()]),
            disallowed_tools: None,
            permission_mode: None,
            setting_sources: None,
            default_role_id: Some("coder".to_string()),
            coordinator_role_id: Some("coder".to_string()),
            claim_policy: None,
            activity_policy: None,
            workflow_vote_policy: None,
            workflow: None,
            artifacts: None,
            completion_policy: None,
            roles: vec![RoleSpec {
                id: "coder".to_string(),
                name: "Coder".to_string(),
                description: Some("Implements changes".to_string()),
                direct: Some(true),
                output_root: Some("40-code/".to_string()),
                agent: RoleAgentSpec {
                    description: "Writes code".to_string(),
                    prompt: "Implement the requested change".to_string(),
                    provider: None,
                    tools: Some(vec!["Read".to_string(), "Edit".to_string()]),
                    disallowed_tools: None,
                    model: None,
                    skills: Some(vec!["flow".to_string()]),
                    mcp_servers: None,
                    initial_prompt: None,
                    permission_mode: None,
                },
            }],
        }
    }

    #[tokio::test]
    async fn bootstrap_and_assign_role_task() {
        let provisioner = FakeProvisioner::default();
        let messenger = FakeMessenger::default();
        let sent = messenger.sent.clone();

        let mut adapter = CtenoWorkspaceAdapter::new(sample_spec(), provisioner, messenger);
        let bootstrap_events = adapter.bootstrap().await.expect("bootstrap should succeed");
        assert!(bootstrap_events.len() >= 2);
        assert!(adapter.bootstrapped().is_some());

        let (dispatch, queued_events) = adapter
            .assign_role_task(RoleTaskRequest {
                role_id: "coder".to_string(),
                instruction: "Implement group mentions".to_string(),
                summary: Some("Mention MVP".to_string()),
                visibility: None,
                source_role_id: None,
                workflow_node_id: None,
                stage_id: None,
            })
            .await
            .expect("assign role task should succeed");

        assert_eq!(dispatch.role_id, "coder");
        assert_eq!(queued_events.len(), 2);
        assert!(matches!(
            queued_events.first(),
            Some(WorkspaceEvent::DispatchQueued { .. })
        ));

        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "session-coder");
        assert!(sent[0].1.contains("Current task for you:"));
        assert!(sent[0].1.contains("Implement group mentions"));
    }

    #[tokio::test]
    async fn bootstrap_from_template_constructor() {
        let provisioner = FakeProvisioner::default();
        let messenger = FakeMessenger::default();

        let template = create_coding_studio_template();
        let instance = WorkspaceInstanceParams {
            id: "template-workspace".to_string(),
            name: "Template Workspace".to_string(),
            cwd: Some("/tmp/template".to_string()),
        };
        let profile = create_claude_workspace_profile(None);

        let mut adapter = CtenoWorkspaceAdapter::from_template(
            &template,
            &instance,
            &profile,
            provisioner,
            messenger,
        );
        let events = adapter.bootstrap().await.expect("bootstrap should succeed");

        assert!(events.len() >= 2);
        assert_eq!(
            adapter.runtime().spec().default_role_id.as_deref(),
            Some("pm")
        );
        assert_eq!(
            adapter.runtime().spec().provider,
            MultiAgentProvider::ClaudeAgentSdk
        );
        assert_eq!(adapter.bootstrapped().unwrap().roles.len(), 6);
    }

    #[tokio::test]
    async fn workspace_turn_publishes_user_message_and_routes_to_default_role() {
        let provisioner = FakeProvisioner::default();
        let messenger = FakeMessenger::default();
        let sent = messenger.sent.clone();
        let requests = messenger.requests.clone();

        let mut adapter = CtenoWorkspaceAdapter::new(sample_spec(), provisioner, messenger);
        adapter.bootstrap().await.expect("bootstrap should succeed");

        let result = adapter
            .send_workspace_turn("Please implement group mentions", None)
            .await
            .expect("workspace turn should succeed");

        assert_eq!(result.request.message, "Please implement group mentions");
        assert_eq!(result.role_id.as_deref(), Some("coder"));
        assert_eq!(result.session_id, "session-coder");
        assert!(result.dispatch.is_some());
        assert_eq!(result.plan.assignments.len(), 1);
        assert_eq!(result.plan.assignments[0].role_id, "coder");
        assert!(result.events.iter().any(|event| matches!(
            event,
            WorkspaceEvent::ActivityPublished { activity, .. }
                if activity.kind == multi_agent_protocol::WorkspaceActivityKind::UserMessage
        )));
        assert!(
            result
                .events
                .iter()
                .any(|event| matches!(event, WorkspaceEvent::DispatchClaimed { .. }))
        );
        assert!(
            result
                .events
                .iter()
                .any(|event| matches!(event, WorkspaceEvent::DispatchStarted { .. }))
        );

        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "session-coder");
        assert!(
            sent[0]
                .1
                .contains("New public workspace context since your last sync:")
        );
        assert!(sent[0].1.contains("Implement the requested change"));
        drop(sent);

        let requests = requests.lock().unwrap();
        assert!(
            requests
                .iter()
                .any(|(session_id, mode)| session_id == "session-coder"
                    && *mode == SessionRequestMode::Claim)
        );
        assert!(
            !requests
                .iter()
                .any(|(_, mode)| *mode == SessionRequestMode::CoordinatorDecision)
        );
    }

    #[derive(Clone, Default)]
    struct ConflictMessenger {
        sent: Arc<Mutex<Vec<(String, String)>>>,
        requests: Arc<Mutex<Vec<(String, SessionRequestMode)>>>,
    }

    #[async_trait]
    impl SessionMessenger for ConflictMessenger {
        async fn send_to_session(
            &self,
            session_id: &str,
            message: &str,
        ) -> Result<(), OrchestratorError> {
            self.sent
                .lock()
                .unwrap()
                .push((session_id.to_string(), message.to_string()));
            Ok(())
        }

        async fn request_response(
            &self,
            session_id: &str,
            _message: &str,
            mode: SessionRequestMode,
        ) -> Result<String, OrchestratorError> {
            self.requests
                .lock()
                .unwrap()
                .push((session_id.to_string(), mode));
            let response = match mode {
                SessionRequestMode::Claim => {
                    if session_id.ends_with("prd") || session_id.ends_with("architect") {
                        format!(
                            r#"{{"decision":"claim","confidence":0.86,"rationale":"{} can own this","publicResponse":"@{} can take this."}}"#,
                            session_id, session_id
                        )
                    } else {
                        r#"{"decision":"decline","confidence":0.25,"rationale":"not my lane"}"#.to_string()
                    }
                }
                SessionRequestMode::CoordinatorDecision => {
                    r#"{"kind":"delegate","responseText":"@architect should own this one.","targetRoleId":"architect","workflowVoteReason":"","rationale":"conflicting claims resolved in favor of architect"}"#.to_string()
                }
                SessionRequestMode::WorkflowVote => {
                    r#"{"decision":"approve","confidence":0.8,"rationale":"workflow is fine"}"#.to_string()
                }
                SessionRequestMode::Work => r#"{"response":"ok"}"#.to_string(),
            };
            Ok(response)
        }
    }

    #[tokio::test]
    async fn workspace_turn_allows_multiple_claimers_to_reply_in_group_chat() {
        let provisioner = FakeProvisioner::default();
        let messenger = ConflictMessenger::default();
        let sent = messenger.sent.clone();
        let requests = messenger.requests.clone();

        let template = create_coding_studio_template();
        let instance = WorkspaceInstanceParams {
            id: "workspace-turn-conflict".to_string(),
            name: "Workspace Turn Conflict".to_string(),
            cwd: Some("/tmp/template".to_string()),
        };
        let profile = create_claude_workspace_profile(None);
        let mut adapter = CtenoWorkspaceAdapter::from_template(
            &template,
            &instance,
            &profile,
            provisioner,
            messenger,
        );
        adapter.bootstrap().await.expect("bootstrap should succeed");

        let result = adapter
            .send_workspace_turn("请你们各自报一下 group mentions 相关工作的进展", None)
            .await
            .expect("workspace turn should succeed");

        assert_eq!(result.role_id.as_deref(), Some("architect"));
        assert_eq!(result.plan.assignments.len(), 2);
        assert_eq!(result.plan.assignments[0].role_id, "architect");
        assert_eq!(result.plan.assignments[1].role_id, "prd");

        let requests = requests.lock().unwrap();
        assert!(
            requests
                .iter()
                .filter(|(_, mode)| *mode == SessionRequestMode::Claim)
                .count()
                >= 2
        );
        assert!(
            !requests
                .iter()
                .any(|(_, mode)| *mode == SessionRequestMode::CoordinatorDecision)
        );
        drop(requests);

        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0, "session-architect");
        assert!(sent[0].1.contains("Current task for you:"));
        assert_eq!(sent[1].0, "session-prd");
        assert!(sent[1].1.contains("Current task for you:"));
    }

    #[derive(Clone, Default)]
    struct WorkflowMessenger {
        sent: Arc<Mutex<Vec<(String, String)>>>,
        requests: Arc<Mutex<Vec<(String, SessionRequestMode)>>>,
    }

    #[async_trait]
    impl SessionMessenger for WorkflowMessenger {
        async fn send_to_session(
            &self,
            session_id: &str,
            message: &str,
        ) -> Result<(), OrchestratorError> {
            self.sent
                .lock()
                .unwrap()
                .push((session_id.to_string(), message.to_string()));
            Ok(())
        }

        async fn request_response(
            &self,
            session_id: &str,
            _message: &str,
            mode: SessionRequestMode,
        ) -> Result<String, OrchestratorError> {
            self.requests
                .lock()
                .unwrap()
                .push((session_id.to_string(), mode));
            let response = match mode {
                SessionRequestMode::Claim => {
                    r#"{"decision":"decline","confidence":0.3,"rationale":"this looks bigger than a direct reply"}"#.to_string()
                }
                SessionRequestMode::CoordinatorDecision => {
                    r#"{"kind":"propose_workflow","responseText":"@lead proposes entering workflow mode.","workflowVoteReason":"this needs a staged research loop","rationale":"staged workflow is a better fit"}"#.to_string()
                }
                SessionRequestMode::WorkflowVote => {
                    format!(
                        r#"{{"decision":"approve","confidence":0.9,"rationale":"{} agrees workflow is appropriate","publicResponse":"@{} approves workflow mode."}}"#,
                        session_id, session_id
                    )
                }
                SessionRequestMode::Work => r#"{"response":"ok"}"#.to_string(),
            };
            Ok(response)
        }
    }

    #[tokio::test]
    async fn workspace_turn_starts_workflow_after_vote_approval() {
        let provisioner = FakeProvisioner::default();
        let messenger = WorkflowMessenger::default();
        let sent = messenger.sent.clone();
        let requests = messenger.requests.clone();

        let template = create_autoresearch_template();
        let instance = WorkspaceInstanceParams {
            id: "workspace-turn-workflow".to_string(),
            name: "Workspace Turn Workflow".to_string(),
            cwd: Some("/tmp/template".to_string()),
        };
        let profile = create_codex_workspace_profile(None);
        let mut adapter = CtenoWorkspaceAdapter::from_template(
            &template,
            &instance,
            &profile,
            provisioner,
            messenger,
        );
        adapter.bootstrap().await.expect("bootstrap should succeed");

        let result = adapter
            .send_workspace_turn(
                "Research how teams talk about group mentions and write a sourced brief",
                None,
            )
            .await
            .expect("workspace turn should succeed");

        assert_eq!(result.role_id.as_deref(), Some("lead"));
        assert_eq!(result.plan.assignments.len(), 1);
        assert_eq!(result.plan.assignments[0].role_id, "lead");
        assert!(result.workflow_vote_window.is_some());
        assert!(!result.workflow_vote_responses.is_empty());
        assert!(
            result
                .events
                .iter()
                .any(|event| matches!(event, WorkspaceEvent::WorkflowVoteOpened { .. }))
        );
        assert!(result.events.iter().any(|event| matches!(
            event,
            WorkspaceEvent::WorkflowVoteClosed { approved: true, .. }
        )));
        assert!(
            result
                .events
                .iter()
                .any(|event| matches!(event, WorkspaceEvent::WorkflowStarted { .. }))
        );

        let requests = requests.lock().unwrap();
        assert!(
            requests
                .iter()
                .any(|(session_id, mode)| session_id == "session-lead"
                    && *mode == SessionRequestMode::CoordinatorDecision)
        );
        assert!(
            requests
                .iter()
                .any(|(_, mode)| *mode == SessionRequestMode::WorkflowVote)
        );
        drop(requests);

        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "session-lead");
        assert!(sent[0].1.contains("Current task for you:"));
    }

    #[tokio::test]
    async fn workspace_turn_honors_direct_role_targeting() {
        let provisioner = FakeProvisioner::default();
        let messenger = FakeMessenger::default();
        let sent = messenger.sent.clone();

        let template = create_coding_studio_template();
        let instance = WorkspaceInstanceParams {
            id: "workspace-turn-template".to_string(),
            name: "Workspace Turn Template".to_string(),
            cwd: Some("/tmp/template".to_string()),
        };
        let profile = create_claude_workspace_profile(None);
        let mut adapter = CtenoWorkspaceAdapter::from_template(
            &template,
            &instance,
            &profile,
            provisioner,
            messenger,
        );
        adapter.bootstrap().await.expect("bootstrap should succeed");

        let result = adapter
            .send_workspace_turn("Write the PRD for group mentions", Some("prd"))
            .await
            .expect("workspace turn should succeed");

        assert_eq!(result.plan.assignments.len(), 1);
        assert_eq!(result.plan.assignments[0].role_id, "prd");
        assert_eq!(result.role_id.as_deref(), Some("prd"));
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "session-prd");
        assert!(sent[0].1.contains("Current task for you:"));
        assert!(sent[0].1.contains("Write the PRD for group mentions"));
    }

    #[tokio::test]
    async fn restores_and_deletes_local_workspace() {
        let spec = sample_spec();
        let cwd = spec.cwd.clone().unwrap();
        let provisioner = FakeProvisioner::default();
        let messenger = FakeMessenger::default();
        let mut adapter =
            CtenoWorkspaceAdapter::new(spec.clone(), provisioner.clone(), messenger.clone());
        adapter.bootstrap().await.expect("bootstrap should succeed");

        let persisted_root = adapter
            .persistence_root()
            .expect("persistence root should exist")
            .to_path_buf();
        assert!(persisted_root.exists());

        let restored =
            CtenoWorkspaceAdapter::restore_from_local(&cwd, &spec.id, provisioner, messenger)
                .expect("restore should succeed");
        assert!(restored.bootstrapped().is_some());
        assert!(restored.has_role_session("session-coder"));

        let mut restored = restored;
        restored
            .delete_workspace()
            .await
            .expect("delete should succeed");
        assert!(!persisted_root.exists());

        let _ = fs::remove_dir_all(cwd);
    }
}
