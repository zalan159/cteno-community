use std::collections::{BTreeMap, VecDeque};

use chrono::Utc;
use multi_agent_protocol::{
    ClaimMode, ClaimStatus, CompletionStatus, CoordinatorWorkflowDecision, DispatchStatus,
    MemberStatus, RoleTaskRequest, TaskDispatch, WorkflowEdgeCondition, WorkflowNodeSpec,
    WorkflowNodeType, WorkspaceActivity, WorkspaceActivityKind, WorkspaceClaimResponse,
    WorkspaceClaimWindow, WorkspaceEvent, WorkspaceInstanceParams, WorkspaceMember, WorkspaceMode,
    WorkspaceProfile, WorkspaceSpec, WorkspaceState, WorkspaceStatus, WorkspaceTemplate,
    WorkspaceTurnRequest, WorkspaceVisibility, WorkspaceWorkflowRuntimeState,
    WorkspaceWorkflowVoteResponse, WorkspaceWorkflowVoteWindow,
    build_assignment_from_workflow_node, instantiate_workspace,
};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("unknown role: {0}")]
    UnknownRole(String),
    #[error("unknown dispatch: {0}")]
    UnknownDispatch(Uuid),
    #[error("unknown provider task: {0}")]
    UnknownProviderTask(String),
}

#[derive(Debug, Clone)]
pub struct RuntimeTick {
    pub state: WorkspaceState,
    pub emitted: Vec<WorkspaceEvent>,
}

#[derive(Debug)]
pub struct WorkspaceRuntime {
    spec: WorkspaceSpec,
    state: WorkspaceState,
    pending_dispatch_queue: VecDeque<Uuid>,
    provider_task_to_dispatch: BTreeMap<String, Uuid>,
    history: Vec<WorkspaceEvent>,
}

impl WorkspaceRuntime {
    pub fn new(spec: WorkspaceSpec) -> Self {
        let roles = spec
            .roles
            .iter()
            .cloned()
            .map(|role| (role.id.clone(), role))
            .collect();
        let members = spec
            .roles
            .iter()
            .map(|role| {
                (
                    role.id.clone(),
                    WorkspaceMember {
                        member_id: role.id.clone(),
                        workspace_id: spec.id.clone(),
                        role_id: role.id.clone(),
                        role_name: role.name.clone(),
                        direct: role.direct,
                        session_id: None,
                        status: MemberStatus::Idle,
                        public_state_summary: None,
                        last_activity_at: None,
                    },
                )
            })
            .collect();

        Self {
            state: WorkspaceState {
                workspace_id: spec.id.clone(),
                status: WorkspaceStatus::Idle,
                provider: spec.provider,
                session_id: None,
                started_at: None,
                roles,
                members,
                dispatches: BTreeMap::new(),
                activities: Vec::new(),
                workflow_runtime: WorkspaceWorkflowRuntimeState {
                    mode: WorkspaceMode::GroupChat,
                    active_vote_window: None,
                    active_request_message: None,
                    active_node_id: None,
                    active_stage_id: None,
                },
            },
            spec,
            pending_dispatch_queue: VecDeque::new(),
            provider_task_to_dispatch: BTreeMap::new(),
            history: Vec::new(),
        }
    }

    pub fn from_template(
        template: &WorkspaceTemplate,
        instance: &WorkspaceInstanceParams,
        profile: &WorkspaceProfile,
    ) -> Self {
        Self::new(instantiate_workspace(template, instance, profile))
    }

    pub fn spec(&self) -> &WorkspaceSpec {
        &self.spec
    }

    pub fn snapshot(&self) -> WorkspaceState {
        self.state.clone()
    }

    pub fn history(&self) -> &[WorkspaceEvent] {
        &self.history
    }

    pub fn restore_snapshot(&mut self, state: WorkspaceState, history: Vec<WorkspaceEvent>) {
        self.pending_dispatch_queue = state
            .dispatches
            .values()
            .filter(|dispatch| dispatch.status == DispatchStatus::Queued)
            .map(|dispatch| dispatch.dispatch_id)
            .collect();
        self.provider_task_to_dispatch = state
            .dispatches
            .values()
            .filter_map(|dispatch| {
                dispatch
                    .provider_task_id
                    .as_ref()
                    .map(|task_id| (task_id.clone(), dispatch.dispatch_id))
            })
            .collect();
        self.state = state;
        self.history = history;
    }

    pub fn start(&mut self) -> RuntimeTick {
        self.state.started_at = Some(now());
        self.state.status = WorkspaceStatus::Running;

        let mut events = vec![WorkspaceEvent::WorkspaceStarted {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            spec: self.spec.clone(),
        }];

        for member in self.state.members.values() {
            events.push(WorkspaceEvent::MemberRegistered {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                member: member.clone(),
            });
        }

        self.push_events(events)
    }

    pub fn initialize(
        &mut self,
        session_id: Option<String>,
        available_agents: Vec<String>,
        available_tools: Vec<String>,
        available_commands: Option<Vec<String>>,
    ) -> RuntimeTick {
        if let Some(session_id) = session_id.clone() {
            self.state.session_id = Some(session_id);
        }

        let event = WorkspaceEvent::WorkspaceInitialized {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            session_id,
            available_agents,
            available_tools,
            available_commands,
        };
        self.push_event(event)
    }

    pub fn register_member_session(
        &mut self,
        role_id: &str,
        session_id: impl Into<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        let session_id = session_id.into();
        let member = self
            .state
            .members
            .get_mut(role_id)
            .ok_or_else(|| RuntimeError::UnknownRole(role_id.to_string()))?;
        member.session_id = Some(session_id);
        member.last_activity_at = Some(now());

        let event = WorkspaceEvent::MemberStateChanged {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            member: member.clone(),
        };
        Ok(self.push_event(event))
    }

    pub fn publish_user_message(&mut self, text: impl Into<String>) -> RuntimeTick {
        let text = text.into();
        let visibility = Some(WorkspaceVisibility::Public);
        let events = vec![
            WorkspaceEvent::Message {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                role: "user".to_string(),
                text: text.clone(),
                visibility,
                member_id: None,
                session_id: self.state.session_id.clone(),
                parent_tool_use_id: None,
            },
            WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity: self.make_activity(
                    WorkspaceActivityKind::UserMessage,
                    text,
                    WorkspaceVisibility::Public,
                    None,
                    None,
                    None,
                    None,
                ),
            },
        ];
        self.push_events(events)
    }

    pub fn record_role_message(
        &mut self,
        role_id: &str,
        text: impl Into<String>,
        visibility: WorkspaceVisibility,
        session_id: Option<String>,
        parent_tool_use_id: Option<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        let text = text.into();
        self.ensure_member_exists(role_id)?;
        let mut events = vec![WorkspaceEvent::Message {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            role: role_id.to_string(),
            text: text.clone(),
            visibility: Some(visibility),
            member_id: Some(role_id.to_string()),
            session_id,
            parent_tool_use_id,
        }];

        if visibility != WorkspaceVisibility::Private {
            let kind = if self.spec.coordinator_role_id.as_deref() == Some(role_id) {
                WorkspaceActivityKind::CoordinatorMessage
            } else {
                WorkspaceActivityKind::MemberSummary
            };
            events.push(WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity: self.make_activity(
                    kind,
                    text.clone(),
                    visibility,
                    Some(role_id.to_string()),
                    Some(role_id.to_string()),
                    None,
                    None,
                ),
            });
        }

        self.update_member(role_id, None, Some(text));
        Ok(self.push_events(events))
    }

    pub fn open_claim_window(&mut self, request: WorkspaceTurnRequest) -> RuntimeTick {
        let claim_window = WorkspaceClaimWindow {
            window_id: Uuid::new_v4().to_string(),
            request: request.clone(),
            candidate_role_ids: self.spec.roles.iter().map(|role| role.id.clone()).collect(),
            timeout_ms: self
                .spec
                .claim_policy
                .as_ref()
                .and_then(|policy| policy.claim_timeout_ms),
        };
        let activity = self.make_activity(
            WorkspaceActivityKind::ClaimWindowOpened,
            format!("Claim window opened for: {}", request.message),
            WorkspaceVisibility::Public,
            None,
            None,
            None,
            None,
        );

        self.push_events(vec![
            WorkspaceEvent::ClaimWindowOpened {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                claim_window: claim_window.clone(),
            },
            WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity,
            },
        ])
    }

    pub fn record_claim_response(
        &mut self,
        claim_window: &WorkspaceClaimWindow,
        response: WorkspaceClaimResponse,
    ) -> Result<RuntimeTick, RuntimeError> {
        self.ensure_member_exists(&response.role_id)?;
        let role_id = response.role_id.clone();
        let activity_kind = match response.decision {
            multi_agent_protocol::ClaimDecision::Claim => WorkspaceActivityKind::MemberClaimed,
            multi_agent_protocol::ClaimDecision::Support => WorkspaceActivityKind::MemberSupporting,
            multi_agent_protocol::ClaimDecision::Decline => WorkspaceActivityKind::MemberDeclined,
        };
        let summary = response
            .public_response
            .clone()
            .unwrap_or_else(|| response.rationale.clone());
        let activity = self.make_activity(
            activity_kind,
            summary.clone(),
            WorkspaceVisibility::Public,
            Some(role_id.clone()),
            Some(role_id.clone()),
            None,
            None,
        );

        self.update_member(&role_id, Some(MemberStatus::Waiting), Some(summary.clone()));

        Ok(self.push_events(vec![
            WorkspaceEvent::ClaimResponse {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                claim_window_id: claim_window.window_id.clone(),
                response: response.clone(),
            },
            WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity,
            },
        ]))
    }

    pub fn close_claim_window(
        &mut self,
        claim_window: WorkspaceClaimWindow,
        responses: Vec<WorkspaceClaimResponse>,
        selected_role_ids: Vec<String>,
    ) -> RuntimeTick {
        let summary = if selected_role_ids.is_empty() {
            "Claim window closed with no claimants.".to_string()
        } else {
            format!(
                "Claim window resolved: {}",
                selected_role_ids
                    .iter()
                    .map(|role_id| format!("@{}", role_id))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let activity = self.make_activity(
            WorkspaceActivityKind::ClaimWindowClosed,
            summary,
            WorkspaceVisibility::Public,
            None,
            None,
            None,
            None,
        );

        self.push_events(vec![
            WorkspaceEvent::ClaimWindowClosed {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                claim_window,
                responses,
                selected_role_ids,
            },
            WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity,
            },
        ])
    }

    pub fn open_workflow_vote_window(
        &mut self,
        request: WorkspaceTurnRequest,
        coordinator_decision: CoordinatorWorkflowDecision,
        candidate_role_ids: Vec<String>,
    ) -> RuntimeTick {
        let vote_window = WorkspaceWorkflowVoteWindow {
            vote_id: Uuid::new_v4().to_string(),
            request,
            reason: coordinator_decision
                .workflow_vote_reason
                .clone()
                .unwrap_or_else(|| "Workflow mode proposed for staged execution.".to_string()),
            candidate_role_ids,
            timeout_ms: self
                .spec
                .workflow_vote_policy
                .as_ref()
                .and_then(|policy| policy.timeout_ms),
        };
        self.state.workflow_runtime.mode = WorkspaceMode::WorkflowVote;
        self.state.workflow_runtime.active_vote_window = Some(vote_window.clone());
        let activity = self.make_activity(
            WorkspaceActivityKind::WorkflowVoteOpened,
            vote_window.reason.clone(),
            WorkspaceVisibility::Public,
            self.spec.coordinator_role_id.clone(),
            self.spec.coordinator_role_id.clone(),
            None,
            None,
        );

        self.push_events(vec![
            WorkspaceEvent::WorkflowVoteOpened {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                coordinator_decision: coordinator_decision.clone(),
                vote_window: vote_window.clone(),
            },
            WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity,
            },
        ])
    }

    pub fn record_workflow_vote_response(
        &mut self,
        vote_window: &WorkspaceWorkflowVoteWindow,
        response: WorkspaceWorkflowVoteResponse,
    ) -> Result<RuntimeTick, RuntimeError> {
        self.ensure_member_exists(&response.role_id)?;
        let role_id = response.role_id.clone();
        let activity_kind = match response.decision {
            multi_agent_protocol::WorkflowVoteDecision::Approve => {
                WorkspaceActivityKind::WorkflowVoteApproved
            }
            multi_agent_protocol::WorkflowVoteDecision::Reject => {
                WorkspaceActivityKind::WorkflowVoteRejected
            }
            multi_agent_protocol::WorkflowVoteDecision::Abstain => {
                WorkspaceActivityKind::MemberSummary
            }
        };
        let summary = response
            .public_response
            .clone()
            .unwrap_or_else(|| response.rationale.clone());
        self.update_member(&role_id, Some(MemberStatus::Waiting), Some(summary.clone()));
        let activity = self.make_activity(
            activity_kind,
            summary,
            WorkspaceVisibility::Public,
            Some(role_id.clone()),
            Some(role_id),
            None,
            None,
        );

        Ok(self.push_events(vec![
            WorkspaceEvent::WorkflowVoteResponse {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                vote_id: vote_window.vote_id.clone(),
                response: response.clone(),
            },
            WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity,
            },
        ]))
    }

    pub fn close_workflow_vote_window(
        &mut self,
        vote_window: WorkspaceWorkflowVoteWindow,
        coordinator_decision: CoordinatorWorkflowDecision,
        responses: Vec<WorkspaceWorkflowVoteResponse>,
        approved: bool,
    ) -> RuntimeTick {
        self.state.workflow_runtime.active_vote_window = None;
        self.state.workflow_runtime.mode = if approved {
            WorkspaceMode::WorkflowRunning
        } else {
            WorkspaceMode::GroupChat
        };
        let activity = self.make_activity(
            if approved {
                WorkspaceActivityKind::WorkflowVoteApproved
            } else {
                WorkspaceActivityKind::WorkflowVoteRejected
            },
            if approved {
                "Workflow vote approved.".to_string()
            } else {
                "Workflow vote rejected.".to_string()
            },
            WorkspaceVisibility::Public,
            self.spec.coordinator_role_id.clone(),
            self.spec.coordinator_role_id.clone(),
            None,
            None,
        );

        self.push_events(vec![
            WorkspaceEvent::WorkflowVoteClosed {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                coordinator_decision,
                vote_window,
                responses,
                approved,
            },
            WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity,
            },
        ])
    }

    pub fn start_workflow(
        &mut self,
        coordinator_decision: CoordinatorWorkflowDecision,
        vote_window: Option<WorkspaceWorkflowVoteWindow>,
        request_message: Option<String>,
        node_id: Option<String>,
        stage_id: Option<String>,
    ) -> RuntimeTick {
        self.state.workflow_runtime.mode = WorkspaceMode::WorkflowRunning;
        self.state.workflow_runtime.active_request_message = request_message;
        self.state.workflow_runtime.active_node_id = node_id.clone();
        self.state.workflow_runtime.active_stage_id = stage_id.clone();
        let activity = self.make_activity(
            WorkspaceActivityKind::WorkflowStarted,
            coordinator_decision.response_text.clone(),
            WorkspaceVisibility::Public,
            self.spec.coordinator_role_id.clone(),
            self.spec.coordinator_role_id.clone(),
            None,
            None,
        );

        self.push_events(vec![
            WorkspaceEvent::WorkflowStarted {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                coordinator_decision: coordinator_decision.clone(),
                vote_window,
                node_id: node_id.clone(),
                stage_id: stage_id.clone(),
            },
            WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity,
            },
        ])
    }

    pub fn queue_dispatch(
        &mut self,
        request: RoleTaskRequest,
    ) -> Result<(TaskDispatch, RuntimeTick), RuntimeError> {
        if !self.state.roles.contains_key(&request.role_id) {
            return Err(RuntimeError::UnknownRole(request.role_id));
        }

        let claim_mode = self
            .spec
            .claim_policy
            .as_ref()
            .map(|policy| policy.mode)
            .unwrap_or(ClaimMode::Direct);
        let initial_claim_status = match claim_mode {
            ClaimMode::Direct | ClaimMode::CoordinatorOnly => Some(ClaimStatus::Claimed),
            ClaimMode::Claim => Some(ClaimStatus::Pending),
        };
        let initial_claim_members = if initial_claim_status == Some(ClaimStatus::Claimed) {
            Some(vec![request.role_id.clone()])
        } else {
            None
        };

        let dispatch = TaskDispatch {
            dispatch_id: Uuid::new_v4(),
            workspace_id: self.spec.id.clone(),
            role_id: request.role_id,
            instruction: request.instruction,
            summary: request.summary,
            visibility: request.visibility.or(self.default_visibility()),
            source_role_id: request.source_role_id,
            workflow_node_id: request.workflow_node_id,
            stage_id: request.stage_id,
            status: DispatchStatus::Queued,
            provider_task_id: None,
            tool_use_id: None,
            created_at: now(),
            started_at: None,
            completed_at: None,
            output_file: None,
            last_summary: None,
            result_text: None,
            claimed_by_member_ids: initial_claim_members,
            claim_status: initial_claim_status,
        };

        self.pending_dispatch_queue.push_back(dispatch.dispatch_id);
        self.state
            .dispatches
            .insert(dispatch.dispatch_id, dispatch.clone());

        let mut events = vec![WorkspaceEvent::DispatchQueued {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            dispatch: dispatch.clone(),
        }];

        if dispatch.claim_status == Some(ClaimStatus::Claimed) {
            let member = self
                .state
                .members
                .get(&dispatch.role_id)
                .cloned()
                .ok_or_else(|| RuntimeError::UnknownRole(dispatch.role_id.clone()))?;
            events.push(WorkspaceEvent::DispatchClaimed {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                dispatch: dispatch.clone(),
                member,
                claim_status: ClaimStatus::Claimed,
                note: Some("Assigned by policy".to_string()),
            });
        }

        Ok((dispatch, self.push_events(events)))
    }

    pub fn claim_dispatch(
        &mut self,
        dispatch_id: Uuid,
        role_id: &str,
        claim_status: ClaimStatus,
        note: Option<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        self.ensure_member_exists(role_id)?;
        let dispatch = self
            .state
            .dispatches
            .get_mut(&dispatch_id)
            .ok_or(RuntimeError::UnknownDispatch(dispatch_id))?;

        dispatch.claim_status = Some(claim_status);
        let claims = dispatch.claimed_by_member_ids.get_or_insert_with(Vec::new);
        if !claims.iter().any(|member_id| member_id == role_id) {
            claims.push(role_id.to_string());
        }
        let dispatch_snapshot = dispatch.clone();
        let member = self
            .state
            .members
            .get(role_id)
            .cloned()
            .expect("member exists");

        let mut events = vec![WorkspaceEvent::DispatchClaimed {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            dispatch: dispatch_snapshot.clone(),
            member: member.clone(),
            claim_status,
            note: note.clone(),
        }];

        if claim_status != ClaimStatus::Declined {
            events.push(WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity: self.make_activity(
                    WorkspaceActivityKind::MemberClaimed,
                    note.unwrap_or_else(|| format!("{} claimed the task", member.role_name)),
                    WorkspaceVisibility::Public,
                    Some(role_id.to_string()),
                    Some(role_id.to_string()),
                    Some(dispatch_id),
                    None,
                ),
            });
        }

        Ok(self.push_events(events))
    }

    pub fn start_next_dispatch(
        &mut self,
        provider_task_id: impl Into<String>,
        description: impl Into<String>,
        tool_use_id: Option<String>,
    ) -> Result<(TaskDispatch, RuntimeTick), RuntimeError> {
        let provider_task_id = provider_task_id.into();
        let description = description.into();
        let dispatch_id = self
            .pending_dispatch_queue
            .pop_front()
            .ok_or_else(|| RuntimeError::UnknownProviderTask(provider_task_id.clone()))?;

        let (role_id, dispatch_snapshot) = {
            let dispatch = self
                .state
                .dispatches
                .get_mut(&dispatch_id)
                .ok_or(RuntimeError::UnknownDispatch(dispatch_id))?;

            dispatch.status = DispatchStatus::Started;
            dispatch.provider_task_id = Some(provider_task_id.clone());
            dispatch.tool_use_id = tool_use_id;
            dispatch.started_at = Some(now());
            dispatch.last_summary = Some(description.clone());
            (dispatch.role_id.clone(), dispatch.clone())
        };

        self.provider_task_to_dispatch
            .insert(provider_task_id.clone(), dispatch_id);

        self.update_member(
            &role_id,
            Some(MemberStatus::Active),
            Some(description.clone()),
        );

        let mut events = vec![WorkspaceEvent::DispatchStarted {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            dispatch: dispatch_snapshot.clone(),
            task_id: provider_task_id.clone(),
            description: description.clone(),
        }];

        if self.publish_dispatch_lifecycle() {
            events.push(WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity: self.make_activity(
                    WorkspaceActivityKind::DispatchStarted,
                    description,
                    dispatch_snapshot
                        .visibility
                        .unwrap_or(WorkspaceVisibility::Public),
                    Some(dispatch_snapshot.role_id.clone()),
                    Some(dispatch_snapshot.role_id.clone()),
                    Some(dispatch_snapshot.dispatch_id),
                    Some(provider_task_id),
                ),
            });
        }

        Ok((dispatch_snapshot, self.push_events(events)))
    }

    pub fn progress_dispatch(
        &mut self,
        provider_task_id: &str,
        description: impl Into<String>,
        summary: Option<String>,
        last_tool_name: Option<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        let description = description.into();
        let dispatch_snapshot = {
            let dispatch = self.find_dispatch_mut(provider_task_id)?;
            dispatch.status = DispatchStatus::Running;
            if let Some(summary) = summary.clone() {
                dispatch.last_summary = Some(summary.clone());
            }
            dispatch.clone()
        };
        self.update_member(
            &dispatch_snapshot.role_id,
            Some(MemberStatus::Active),
            summary.clone().or_else(|| Some(description.clone())),
        );

        let mut events = vec![WorkspaceEvent::DispatchProgress {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            dispatch: dispatch_snapshot.clone(),
            task_id: provider_task_id.to_string(),
            description: description.clone(),
            summary: summary.clone(),
            last_tool_name,
        }];

        if self.publish_dispatch_lifecycle() {
            events.push(WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity: self.make_activity(
                    WorkspaceActivityKind::DispatchProgress,
                    summary.unwrap_or(description),
                    dispatch_snapshot
                        .visibility
                        .unwrap_or(WorkspaceVisibility::Public),
                    Some(dispatch_snapshot.role_id.clone()),
                    Some(dispatch_snapshot.role_id.clone()),
                    Some(dispatch_snapshot.dispatch_id),
                    Some(provider_task_id.to_string()),
                ),
            });
        }

        Ok(self.push_events(events))
    }

    pub fn complete_dispatch(
        &mut self,
        provider_task_id: &str,
        status: DispatchStatus,
        output_file: Option<String>,
        summary: impl Into<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        let summary = summary.into();
        let dispatch_snapshot = {
            let dispatch = self.find_dispatch_mut(provider_task_id)?;
            dispatch.status = status;
            dispatch.completed_at = Some(now());
            dispatch.last_summary = Some(summary.clone());
            dispatch.output_file = output_file.clone();
            dispatch.clone()
        };
        self.update_member(
            &dispatch_snapshot.role_id,
            Some(match status {
                DispatchStatus::Completed => MemberStatus::Idle,
                DispatchStatus::Failed => MemberStatus::Blocked,
                DispatchStatus::Stopped => MemberStatus::Waiting,
                DispatchStatus::Queued | DispatchStatus::Started | DispatchStatus::Running => {
                    MemberStatus::Active
                }
            }),
            Some(summary.clone()),
        );

        let mut events = vec![match status {
            DispatchStatus::Completed => WorkspaceEvent::DispatchCompleted {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                dispatch: dispatch_snapshot.clone(),
                task_id: provider_task_id.to_string(),
                output_file: output_file.clone().unwrap_or_default(),
                summary: summary.clone(),
            },
            DispatchStatus::Failed => WorkspaceEvent::DispatchFailed {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                dispatch: dispatch_snapshot.clone(),
                task_id: provider_task_id.to_string(),
                output_file: output_file.clone().unwrap_or_default(),
                summary: summary.clone(),
            },
            DispatchStatus::Stopped => WorkspaceEvent::DispatchStopped {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                dispatch: dispatch_snapshot.clone(),
                task_id: provider_task_id.to_string(),
                output_file: output_file.clone().unwrap_or_default(),
                summary: summary.clone(),
            },
            _ => {
                return Err(RuntimeError::UnknownProviderTask(
                    provider_task_id.to_string(),
                ));
            }
        }];

        if self.publish_dispatch_lifecycle() {
            events.push(WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity: self.make_activity(
                    WorkspaceActivityKind::DispatchCompleted,
                    summary,
                    dispatch_snapshot
                        .visibility
                        .unwrap_or(WorkspaceVisibility::Public),
                    Some(dispatch_snapshot.role_id.clone()),
                    Some(dispatch_snapshot.role_id.clone()),
                    Some(dispatch_snapshot.dispatch_id),
                    Some(provider_task_id.to_string()),
                ),
            });
        }

        Ok(self.push_events(events))
    }

    pub fn attach_result_text(
        &mut self,
        provider_task_id: &str,
        result_text: impl Into<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        let result_text = result_text.into();
        let dispatch = self.find_dispatch_mut(provider_task_id)?;
        dispatch.result_text = Some(result_text.clone());
        let dispatch_snapshot = dispatch.clone();

        let event = WorkspaceEvent::DispatchResult {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            dispatch: dispatch_snapshot,
            task_id: provider_task_id.to_string(),
            result_text,
        };

        Ok(self.push_event(event))
    }

    pub fn advance_workflow_after_dispatch(
        &mut self,
        provider_task_id: &str,
    ) -> Result<(RuntimeTick, Vec<RoleTaskRequest>), RuntimeError> {
        let dispatch = self.find_dispatch_mut(provider_task_id)?.clone();

        if self.state.workflow_runtime.mode != WorkspaceMode::WorkflowRunning {
            return Ok((self.push_events(Vec::new()), Vec::new()));
        }

        let Some(current_node_id) = dispatch.workflow_node_id.clone() else {
            return Ok((self.push_events(Vec::new()), Vec::new()));
        };

        let current_node = {
            let Some(workflow) = self.spec.workflow.as_ref() else {
                return Ok((self.push_events(Vec::new()), Vec::new()));
            };
            let Some(node) = workflow
                .nodes
                .iter()
                .find(|node| node.id == current_node_id)
            else {
                return Ok((self.push_events(Vec::new()), Vec::new()));
            };
            node.clone()
        };

        let mut events = vec![WorkspaceEvent::WorkflowStageCompleted {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            node_id: current_node.id.clone(),
            stage_id: dispatch.stage_id.clone(),
            role_id: Some(dispatch.role_id.clone()),
        }];
        events.push(WorkspaceEvent::ActivityPublished {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            activity: self.make_activity(
                WorkspaceActivityKind::WorkflowStageCompleted,
                format!(
                    "Workflow node \"{}\" completed.",
                    current_node
                        .title
                        .clone()
                        .unwrap_or_else(|| current_node.id.clone())
                ),
                dispatch.visibility.unwrap_or(WorkspaceVisibility::Public),
                Some(dispatch.role_id.clone()),
                Some(dispatch.role_id.clone()),
                Some(dispatch.dispatch_id),
                dispatch.provider_task_id.clone(),
            ),
        });

        if self.is_workflow_terminal_node(&current_node.id, dispatch.status) {
            self.state.workflow_runtime.mode = WorkspaceMode::GroupChat;
            self.state.workflow_runtime.active_node_id = None;
            self.state.workflow_runtime.active_stage_id = None;
            self.state.workflow_runtime.active_request_message = None;
            events.push(WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity: self.make_activity(
                    WorkspaceActivityKind::WorkflowCompleted,
                    "Workflow completed.".to_string(),
                    WorkspaceVisibility::Public,
                    self.spec.coordinator_role_id.clone(),
                    self.spec.coordinator_role_id.clone(),
                    Some(dispatch.dispatch_id),
                    dispatch.provider_task_id.clone(),
                ),
            });
            return Ok((self.push_events(events), Vec::new()));
        }

        let next_node = {
            let Some(workflow) = self.spec.workflow.as_ref() else {
                return Ok((self.push_events(events), Vec::new()));
            };
            let next_condition = self.resolve_workflow_edge_condition(&dispatch, &current_node);
            let next_edge = workflow
                .edges
                .iter()
                .find(|edge| edge.from == current_node.id && edge.when == next_condition)
                .or_else(|| {
                    workflow.edges.iter().find(|edge| {
                        edge.from == current_node.id && edge.when == WorkflowEdgeCondition::Always
                    })
                });
            next_edge.and_then(|edge| {
                workflow
                    .nodes
                    .iter()
                    .find(|node| node.id == edge.to)
                    .cloned()
            })
        };

        let Some(next_node) = next_node else {
            return Ok((self.push_events(events), Vec::new()));
        };

        self.state.workflow_runtime.active_node_id = Some(next_node.id.clone());
        self.state.workflow_runtime.active_stage_id = next_node.stage_id.clone();
        events.push(WorkspaceEvent::WorkflowStageStarted {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            node_id: next_node.id.clone(),
            stage_id: next_node.stage_id.clone(),
            role_id: next_node
                .role_id
                .clone()
                .or(next_node.reviewer_role_id.clone()),
        });
        events.push(WorkspaceEvent::ActivityPublished {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            activity: self.make_activity(
                WorkspaceActivityKind::WorkflowStageStarted,
                format!(
                    "Workflow advanced to \"{}\".",
                    next_node
                        .title
                        .clone()
                        .unwrap_or_else(|| next_node.id.clone())
                ),
                WorkspaceVisibility::Public,
                next_node
                    .role_id
                    .clone()
                    .or(next_node.reviewer_role_id.clone()),
                next_node
                    .role_id
                    .clone()
                    .or(next_node.reviewer_role_id.clone()),
                Some(dispatch.dispatch_id),
                dispatch.provider_task_id.clone(),
            ),
        });

        let followups = self
            .state
            .workflow_runtime
            .active_request_message
            .clone()
            .and_then(|message| {
                build_assignment_from_workflow_node(
                    &self.spec,
                    &WorkspaceTurnRequest {
                        message,
                        visibility: dispatch.visibility,
                        max_assignments: None,
                        prefer_role_id: None,
                    },
                    &next_node,
                )
            })
            .map(|assignment| RoleTaskRequest {
                role_id: assignment.role_id,
                instruction: assignment.instruction,
                summary: assignment.summary,
                visibility: assignment.visibility,
                source_role_id: Some(
                    self.spec
                        .coordinator_role_id
                        .clone()
                        .or(self.spec.default_role_id.clone())
                        .unwrap_or_else(|| "coordinator".to_string()),
                ),
                workflow_node_id: assignment.workflow_node_id,
                stage_id: assignment.stage_id,
            })
            .into_iter()
            .collect::<Vec<_>>();

        Ok((self.push_events(events), followups))
    }

    fn ensure_member_exists(&self, role_id: &str) -> Result<(), RuntimeError> {
        self.state
            .members
            .get(role_id)
            .map(|_| ())
            .ok_or_else(|| RuntimeError::UnknownRole(role_id.to_string()))
    }

    fn default_visibility(&self) -> Option<WorkspaceVisibility> {
        self.spec
            .activity_policy
            .as_ref()
            .and_then(|policy| policy.default_visibility)
    }

    fn publish_dispatch_lifecycle(&self) -> bool {
        self.spec
            .activity_policy
            .as_ref()
            .and_then(|policy| policy.publish_dispatch_lifecycle)
            .unwrap_or(true)
    }

    fn update_member(
        &mut self,
        role_id: &str,
        status: Option<MemberStatus>,
        summary: Option<String>,
    ) {
        if let Some(member) = self.state.members.get_mut(role_id) {
            if let Some(status) = status {
                member.status = status;
            }
            if let Some(summary) = summary {
                member.public_state_summary = Some(summary);
            }
            member.last_activity_at = Some(now());
        }
    }

    fn make_activity(
        &mut self,
        kind: WorkspaceActivityKind,
        text: String,
        visibility: WorkspaceVisibility,
        role_id: Option<String>,
        member_id: Option<String>,
        dispatch_id: Option<Uuid>,
        task_id: Option<String>,
    ) -> WorkspaceActivity {
        let activity = WorkspaceActivity {
            activity_id: Uuid::new_v4(),
            workspace_id: self.spec.id.clone(),
            kind,
            visibility,
            text,
            created_at: now(),
            role_id,
            member_id,
            dispatch_id,
            task_id,
        };
        self.state.activities.push(activity.clone());
        activity
    }

    fn find_dispatch_mut(
        &mut self,
        provider_task_id: &str,
    ) -> Result<&mut TaskDispatch, RuntimeError> {
        let dispatch_id = self
            .provider_task_to_dispatch
            .get(provider_task_id)
            .copied()
            .ok_or_else(|| RuntimeError::UnknownProviderTask(provider_task_id.to_string()))?;

        self.state
            .dispatches
            .get_mut(&dispatch_id)
            .ok_or(RuntimeError::UnknownDispatch(dispatch_id))
    }

    fn push_event(&mut self, event: WorkspaceEvent) -> RuntimeTick {
        self.push_events(vec![event])
    }

    fn push_events(&mut self, events: Vec<WorkspaceEvent>) -> RuntimeTick {
        self.history.extend(events.iter().cloned());
        RuntimeTick {
            state: self.snapshot(),
            emitted: events,
        }
    }

    fn resolve_workflow_edge_condition(
        &self,
        dispatch: &TaskDispatch,
        current_node: &WorkflowNodeSpec,
    ) -> WorkflowEdgeCondition {
        match dispatch.status {
            DispatchStatus::Failed => WorkflowEdgeCondition::Failure,
            DispatchStatus::Stopped => WorkflowEdgeCondition::Timeout,
            DispatchStatus::Completed => {
                let text = dispatch
                    .result_text
                    .clone()
                    .or(dispatch.last_summary.clone())
                    .unwrap_or_default()
                    .to_lowercase();
                match current_node.node_type {
                    WorkflowNodeType::Review => {
                        if contains_any(
                            &text,
                            &["reject", "rejected", "changes requested", "revise"],
                        ) {
                            WorkflowEdgeCondition::Rejected
                        } else {
                            WorkflowEdgeCondition::Approved
                        }
                    }
                    WorkflowNodeType::Evaluate => {
                        if contains_any(&text, &["improved", "better", "win"]) {
                            WorkflowEdgeCondition::Improved
                        } else if contains_any(&text, &["crash", "errored", "error"]) {
                            WorkflowEdgeCondition::Crash
                        } else {
                            WorkflowEdgeCondition::EqualOrWorse
                        }
                    }
                    WorkflowNodeType::Assign if is_test_like_node(current_node) => {
                        if contains_any(
                            &text,
                            &["fail", "failed", "regression", "blocked", "error"],
                        ) {
                            WorkflowEdgeCondition::Fail
                        } else {
                            WorkflowEdgeCondition::Pass
                        }
                    }
                    _ => WorkflowEdgeCondition::Success,
                }
            }
            _ => WorkflowEdgeCondition::Always,
        }
    }

    fn is_workflow_terminal_node(&self, node_id: &str, status: DispatchStatus) -> bool {
        if let Some(completion) = self.spec.completion_policy.as_ref() {
            if completion
                .success_node_ids
                .as_ref()
                .is_some_and(|ids| ids.iter().any(|id| id == node_id))
            {
                return matches!(status, DispatchStatus::Completed);
            }
            if completion
                .failure_node_ids
                .as_ref()
                .is_some_and(|ids| ids.iter().any(|id| id == node_id))
            {
                return matches!(status, DispatchStatus::Failed | DispatchStatus::Stopped);
            }
            if completion.default_status == Some(CompletionStatus::Done)
                && matches!(status, DispatchStatus::Completed)
            {
                return false;
            }
        }

        self.spec
            .workflow
            .as_ref()
            .and_then(|workflow| workflow.nodes.iter().find(|node| node.id == node_id))
            .is_some_and(|node| node.node_type == WorkflowNodeType::Complete)
            && matches!(status, DispatchStatus::Completed)
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn is_test_like_node(node: &WorkflowNodeSpec) -> bool {
    let id = node.id.to_lowercase();
    let title = node.title.clone().unwrap_or_default().to_lowercase();
    id.contains("test")
        || id.contains("validate")
        || title.contains("test")
        || title.contains("validation")
}

#[cfg(test)]
mod tests {
    use multi_agent_protocol::{
        ActivityPolicy, ClaimMode, ClaimPolicy, MultiAgentProvider, RoleAgentSpec, RoleSpec,
        RoleTaskRequest, WorkspaceEvent, WorkspaceInstanceParams, WorkspaceSpec,
        WorkspaceVisibility, create_autoresearch_template, create_codex_workspace_profile,
    };

    use super::*;

    fn sample_spec() -> WorkspaceSpec {
        WorkspaceSpec {
            id: "workspace-1".to_string(),
            name: "Test Workspace".to_string(),
            provider: MultiAgentProvider::Cteno,
            model: "claude-sonnet-4-5".to_string(),
            cwd: Some("/tmp/demo".to_string()),
            orchestrator_prompt: None,
            allowed_tools: None,
            disallowed_tools: None,
            permission_mode: None,
            setting_sources: None,
            default_role_id: Some("coder".to_string()),
            coordinator_role_id: Some("coder".to_string()),
            claim_policy: Some(ClaimPolicy {
                mode: ClaimMode::Claim,
                claim_timeout_ms: Some(1000),
                max_assignees: Some(1),
                allow_supporting_claims: Some(false),
                fallback_role_id: Some("coder".to_string()),
            }),
            activity_policy: Some(ActivityPolicy {
                publish_user_messages: Some(true),
                publish_coordinator_messages: Some(true),
                publish_dispatch_lifecycle: Some(true),
                publish_member_messages: Some(true),
                default_visibility: Some(WorkspaceVisibility::Public),
            }),
            workflow_vote_policy: None,
            workflow: None,
            artifacts: None,
            completion_policy: None,
            roles: vec![RoleSpec {
                id: "coder".to_string(),
                name: "Coder".to_string(),
                description: None,
                direct: Some(true),
                output_root: Some("40-code/".to_string()),
                agent: RoleAgentSpec {
                    description: "Writes code".to_string(),
                    prompt: "Implement the requested changes".to_string(),
                    provider: Some(MultiAgentProvider::Cteno),
                    tools: Some(vec!["Read".to_string(), "Edit".to_string()]),
                    disallowed_tools: None,
                    model: None,
                    skills: None,
                    mcp_servers: None,
                    initial_prompt: None,
                    permission_mode: None,
                },
            }],
        }
    }

    #[test]
    fn start_registers_members_and_broadcasts_user_message() {
        let mut runtime = WorkspaceRuntime::new(sample_spec());
        let tick = runtime.start();
        assert!(
            tick.emitted
                .iter()
                .any(|event| matches!(event, WorkspaceEvent::MemberRegistered { .. }))
        );

        let tick = runtime.publish_user_message("Build group mentions");
        assert!(tick
            .emitted
            .iter()
            .any(|event| matches!(event, WorkspaceEvent::ActivityPublished { activity, .. } if activity.kind == WorkspaceActivityKind::UserMessage)));
    }

    #[test]
    fn dispatch_lifecycle_round_trip() {
        let mut runtime = WorkspaceRuntime::new(sample_spec());
        runtime.start();
        runtime.initialize(
            Some("session-1".to_string()),
            vec!["coder".to_string()],
            vec!["Read".to_string(), "Edit".to_string()],
            None,
        );

        let (dispatch, _) = runtime
            .queue_dispatch(RoleTaskRequest {
                role_id: "coder".to_string(),
                instruction: "Write a feature".to_string(),
                summary: Some("Implement feature".to_string()),
                visibility: Some(WorkspaceVisibility::Public),
                source_role_id: Some("coder".to_string()),
                workflow_node_id: None,
                stage_id: None,
            })
            .expect("dispatch should queue");

        runtime
            .claim_dispatch(
                dispatch.dispatch_id,
                "coder",
                ClaimStatus::Claimed,
                Some("Coder picked this up".to_string()),
            )
            .expect("dispatch should be claimable");
        runtime
            .start_next_dispatch(
                "provider-task-1",
                "Implement feature",
                Some("tool-use-1".to_string()),
            )
            .expect("dispatch should start");
        runtime
            .progress_dispatch(
                "provider-task-1",
                "Editing files",
                Some("Editing src/lib.rs".to_string()),
                Some("Edit".to_string()),
            )
            .expect("dispatch should progress");
        runtime
            .complete_dispatch(
                "provider-task-1",
                DispatchStatus::Completed,
                Some("40-code/output.md".to_string()),
                "Feature implemented",
            )
            .expect("dispatch should complete");
        let tick = runtime
            .attach_result_text("provider-task-1", "Done")
            .expect("dispatch should accept result text");

        let stored = runtime.snapshot().dispatches[&dispatch.dispatch_id].clone();
        assert_eq!(stored.status, DispatchStatus::Completed);
        assert_eq!(stored.provider_task_id.as_deref(), Some("provider-task-1"));
        assert_eq!(stored.tool_use_id.as_deref(), Some("tool-use-1"));
        assert_eq!(stored.result_text.as_deref(), Some("Done"));
        assert_eq!(stored.claim_status, Some(ClaimStatus::Claimed));

        assert!(matches!(
            &tick.emitted[0],
            WorkspaceEvent::DispatchResult { task_id, result_text, .. }
                if task_id == "provider-task-1" && result_text == "Done"
        ));
        assert!(!runtime.snapshot().activities.is_empty());
    }

    #[test]
    fn workflow_progression_advances_to_next_node_after_completion() {
        let template = create_autoresearch_template();
        let profile = create_codex_workspace_profile(Some("gpt-5.4-mini"));
        let instance = WorkspaceInstanceParams {
            id: "workflow-progress".to_string(),
            name: "Workflow Progress".to_string(),
            cwd: Some("/tmp/workflow-progress".to_string()),
        };
        let mut runtime = WorkspaceRuntime::from_template(&template, &instance, &profile);
        runtime.start();
        runtime.initialize(
            None,
            vec!["lead".to_string()],
            vec!["Read".to_string()],
            None,
        );

        runtime.start_workflow(
            multi_agent_protocol::CoordinatorWorkflowDecision {
                kind: multi_agent_protocol::CoordinatorDecisionKind::ProposeWorkflow,
                response_text: "@lead proposes workflow mode.".to_string(),
                target_role_id: None,
                workflow_vote_reason: Some("loop".to_string()),
                rationale: None,
            },
            None,
            Some("Start autoresearch".to_string()),
            Some("frame_hypothesis".to_string()),
            Some("framing".to_string()),
        );

        let (dispatch, _) = runtime
            .queue_dispatch(RoleTaskRequest {
                role_id: "lead".to_string(),
                instruction: "Frame the hypothesis".to_string(),
                summary: Some("frame".to_string()),
                visibility: Some(WorkspaceVisibility::Public),
                source_role_id: Some("lead".to_string()),
                workflow_node_id: Some("frame_hypothesis".to_string()),
                stage_id: Some("framing".to_string()),
            })
            .expect("dispatch should queue");
        runtime
            .start_next_dispatch(dispatch.dispatch_id.to_string(), "frame", None)
            .expect("dispatch should start");
        runtime
            .complete_dispatch(
                &dispatch.dispatch_id.to_string(),
                DispatchStatus::Completed,
                None,
                "Hypothesis drafted",
            )
            .expect("dispatch should complete");
        runtime
            .attach_result_text(&dispatch.dispatch_id.to_string(), "Hypothesis drafted")
            .expect("result should attach");

        let (tick, followups) = runtime
            .advance_workflow_after_dispatch(&dispatch.dispatch_id.to_string())
            .expect("workflow should advance");

        assert!(tick
            .emitted
            .iter()
            .any(|event| matches!(event, WorkspaceEvent::WorkflowStageStarted { node_id, .. } if node_id == "claim_evidence")));
        assert_eq!(
            runtime
                .snapshot()
                .workflow_runtime
                .active_node_id
                .as_deref(),
            Some("claim_evidence")
        );
        assert_eq!(followups.len(), 1);
        assert!(matches!(followups[0].role_id.as_str(), "scout" | "critic"));
        assert_eq!(
            followups[0].workflow_node_id.as_deref(),
            Some("claim_evidence")
        );
    }
}
