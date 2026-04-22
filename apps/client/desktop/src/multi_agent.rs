use async_trait::async_trait;
use multi_agent_protocol::{
    direct_workspace_turn_plan, plan_workspace_turn, TaskDispatch, WorkspaceActivity,
    WorkspaceEvent, WorkspaceInstanceParams, WorkspaceMember, WorkspaceMode, WorkspaceProfile,
    WorkspaceSpec, WorkspaceState, WorkspaceStatus, WorkspaceTemplate, WorkspaceTurnAssignment,
    WorkspaceTurnPlan, WorkspaceTurnRequest, WorkspaceVisibility, WorkspaceWorkflowRuntimeState,
    WorkspaceWorkflowVoteResponse, WorkspaceWorkflowVoteWindow,
};
use multi_agent_runtime_core::{
    AgentExecutor, AutoresearchOrchestrator, GatedTasksOrchestrator, GroupChatOrchestrator,
    ModelSpec, OrchestratorError, PermissionMode as ExecutorPermissionMode, SessionRef,
    SpawnSessionSpec, UserMessage, WorkspaceOrchestrator, WorkspaceShell,
};
use multi_agent_runtime_cteno::{
    AdapterError, BootstrappedWorkspace, CtenoWorkspaceAdapter, SessionMessenger,
    SessionRequestMode, WorkspaceProvisioner,
};
use multi_agent_runtime_local::{
    LocalWorkspacePersistence, PersistedProviderBinding, PersistedProviderState,
    ProviderConversationKind,
};

use crate::executor_normalizer::user_visible_executor_error;
use crate::happy_client::permission::PermissionMode;
use crate::persona::models::{Persona, PersonaSessionLink, PersonaSessionType, WorkspaceBinding};
use crate::service_init::AgentConfig;
use serde::Deserialize;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;

const WORKSPACE_AGENT_PREFIX: &str = "group";

type WorkspaceInstanceRegistry = Arc<Mutex<HashMap<String, WorkspaceInstance>>>;
type LocalWorkspaceAbortRegistry = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;
type LiveWorkspaceExecutorRegistry = Arc<Mutex<HashMap<String, LiveWorkspaceExecutorSession>>>;

static LIVE_WORKSPACE_INSTANCES: OnceLock<WorkspaceInstanceRegistry> = OnceLock::new();
static LOCAL_WORKSPACE_ABORT_FLAGS: OnceLock<LocalWorkspaceAbortRegistry> = OnceLock::new();
static LIVE_WORKSPACE_EXECUTOR_SESSIONS: OnceLock<LiveWorkspaceExecutorRegistry> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CtenoWorkspaceMetadata {
    workspace_persona_id: String,
    workspace_session_id: String,
    roles: Vec<multi_agent_runtime_cteno::ProvisionedRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    orchestrator_state: Option<Value>,
}

struct WorkspaceInstance {
    shell: WorkspaceShell,
    state: WorkspaceState,
    history: Vec<WorkspaceEvent>,
    bootstrapped: BootstrappedWorkspace,
    orchestrator: Box<dyn WorkspaceOrchestrator>,
    messenger: CtenoSessionMessenger,
    provisioner: CtenoWorkspaceProvisioner,
    persistence: Option<LocalWorkspacePersistence>,
}

#[derive(Clone)]
struct LiveWorkspaceExecutorSession {
    executor: Arc<dyn AgentExecutor>,
    session_ref: SessionRef,
}

impl WorkspaceInstance {
    fn from_parts(
        template_id: &str,
        spec: WorkspaceSpec,
        mut state: WorkspaceState,
        history: Vec<WorkspaceEvent>,
        bootstrapped: BootstrappedWorkspace,
        orchestrator_state: Option<Value>,
    ) -> Result<Self, String> {
        let mut shell = workspace_shell_from_state(&spec, &state);
        for role in &bootstrapped.roles {
            if let Some(member) = shell.members.get_mut(&role.role_id) {
                member.session_id = Some(role.session_id.clone());
            }
            if let Some(member) = state.members.get_mut(&role.role_id) {
                member.session_id = Some(role.session_id.clone());
            }
        }

        if state.session_id.is_none() {
            state.session_id = Some(bootstrapped.workspace_session_id.clone());
        }
        if state.started_at.is_none() {
            state.started_at = Some(chrono::Utc::now().to_rfc3339());
        }

        let mut orchestrator = create_orchestrator(template_id);
        if let Some(orchestrator_state) = orchestrator_state {
            orchestrator
                .restore_state(orchestrator_state)
                .map_err(|error| error.to_string())?;
        }

        Ok(Self {
            shell,
            state,
            history,
            bootstrapped,
            orchestrator,
            messenger: CtenoSessionMessenger,
            provisioner: CtenoWorkspaceProvisioner,
            persistence: LocalWorkspacePersistence::from_spec(&spec).ok(),
        })
    }

    fn template_state(&self) -> Value {
        self.orchestrator.template_state()
    }

    fn snapshot(&self) -> WorkspaceState {
        self.state.clone()
    }

    fn history(&self) -> &[WorkspaceEvent] {
        &self.history
    }

    fn has_role_session(&self, session_id: &str) -> bool {
        self.shell
            .members
            .values()
            .any(|member| member.session_id.as_deref() == Some(session_id))
    }

    fn role_id_for_session(&self, session_id: &str) -> Option<String> {
        self.shell.members.iter().find_map(|(role_id, member)| {
            (member.session_id.as_deref() == Some(session_id)).then(|| role_id.clone())
        })
    }

    async fn handle_user_message(
        &mut self,
        message: &str,
        role_id: Option<&str>,
    ) -> Result<WorkspaceTurnResponse, String> {
        let request = WorkspaceTurnRequest {
            message: message.to_string(),
            visibility: Some(WorkspaceVisibility::Public),
            max_assignments: role_id.is_none().then_some(1),
            prefer_role_id: role_id.map(ToString::to_string),
        };
        let response = self
            .orchestrator
            .handle_user_message(&self.shell, &self.messenger, message, role_id)
            .await
            .map_err(|error| error.to_string())?;

        let plan = workspace_turn_plan_from_response(
            &self.shell.spec,
            &request,
            role_id,
            &response.activities,
            &response.dispatches,
        );
        let mut events = self.record_activities(response.activities);
        let (dispatches, mut dispatch_events) = self.queue_dispatches(response.dispatches).await?;
        events.append(&mut dispatch_events);
        self.history.extend(events.clone());
        self.state.status = if dispatches.is_empty() {
            WorkspaceStatus::Idle
        } else {
            WorkspaceStatus::Running
        };
        self.sync_state_from_shell();
        self.persist()?;

        let session_id = dispatches
            .first()
            .and_then(|dispatch| {
                self.shell
                    .members
                    .get(&dispatch.role_id)
                    .and_then(|member| member.session_id.clone())
            })
            .unwrap_or_else(|| self.bootstrapped.workspace_session_id.clone());
        let primary_role_id = dispatches.first().map(|dispatch| dispatch.role_id.clone());

        Ok(WorkspaceTurnResponse {
            plan,
            workflow_vote_window: None,
            workflow_vote_responses: Vec::new(),
            dispatches: dispatches.clone(),
            session_id,
            role_id: primary_role_id,
            dispatch: dispatches.first().cloned(),
            events,
            state: self.snapshot(),
            template_state: response.template_state,
        })
    }

    async fn on_role_completed(
        &mut self,
        role_id: &str,
        response_text: &str,
        success: bool,
    ) -> Result<(), String> {
        let response = self
            .orchestrator
            .on_role_completed(
                &self.shell,
                &self.messenger,
                role_id,
                response_text,
                success,
            )
            .await
            .map_err(|error| error.to_string())?;

        let mut events = self.update_member_after_completion(role_id, response_text, success);
        let mut activity_events = self.record_activities(response.activities);
        let (_, mut dispatch_events) = self.queue_dispatches(response.dispatches).await?;
        events.append(&mut activity_events);
        events.append(&mut dispatch_events);
        self.history.extend(events);
        self.state.status = self
            .shell
            .dispatches
            .iter()
            .any(|dispatch| {
                !matches!(
                    dispatch.status,
                    multi_agent_protocol::DispatchStatus::Completed
                        | multi_agent_protocol::DispatchStatus::Failed
                        | multi_agent_protocol::DispatchStatus::Stopped
                )
            })
            .then_some(WorkspaceStatus::Running)
            .unwrap_or(WorkspaceStatus::Idle);
        self.sync_state_from_shell();
        self.persist()?;
        Ok(())
    }

    async fn delete_workspace(&mut self) -> Result<(), String> {
        self.provisioner
            .cleanup_workspace(&self.shell.spec, &self.bootstrapped)
            .await
            .map_err(|error| error.to_string())?;
        self.bootstrapped.roles.clear();
        self.shell.members.values_mut().for_each(|member| {
            member.session_id = None;
        });
        if let Some(persistence) = self.persistence.as_ref() {
            persistence
                .delete_workspace()
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn update_member_after_completion(
        &mut self,
        role_id: &str,
        response_text: &str,
        success: bool,
    ) -> Vec<WorkspaceEvent> {
        let status = if success {
            multi_agent_protocol::MemberStatus::Idle
        } else {
            multi_agent_protocol::MemberStatus::Blocked
        };
        let summary = summarize_workspace_message(response_text);

        if let Some(member) = self.shell.members.get_mut(role_id) {
            member.status = status;
            member.public_state_summary = Some(summary.clone());
            member.last_activity_at = Some(chrono::Utc::now().to_rfc3339());
        }
        if let Some(member) = self.state.members.get_mut(role_id) {
            member.status = status;
            member.public_state_summary = Some(summary);
            member.last_activity_at = Some(chrono::Utc::now().to_rfc3339());
        }

        self.shell
            .members
            .get(role_id)
            .cloned()
            .map(|member| {
                vec![WorkspaceEvent::MemberStateChanged {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    workspace_id: self.state.workspace_id.clone(),
                    member,
                }]
            })
            .unwrap_or_default()
    }

    fn record_activities(&mut self, activities: Vec<WorkspaceActivity>) -> Vec<WorkspaceEvent> {
        let mut events = Vec::with_capacity(activities.len());
        for activity in activities {
            self.shell.record_activity(activity.clone());
            self.state.activities.push(activity.clone());
            events.push(WorkspaceEvent::ActivityPublished {
                timestamp: chrono::Utc::now().to_rfc3339(),
                workspace_id: self.state.workspace_id.clone(),
                activity,
            });
        }
        events
    }

    async fn queue_dispatches(
        &mut self,
        dispatches: Vec<TaskDispatch>,
    ) -> Result<(Vec<TaskDispatch>, Vec<WorkspaceEvent>), String> {
        let mut events = Vec::new();
        for dispatch in &dispatches {
            let member = {
                let member = self
                    .shell
                    .members
                    .get_mut(&dispatch.role_id)
                    .ok_or_else(|| format!("unknown role '{}'", dispatch.role_id))?;
                member.status = multi_agent_protocol::MemberStatus::Active;
                member.public_state_summary = dispatch.summary.clone();
                member.last_activity_at = Some(chrono::Utc::now().to_rfc3339());
                member.clone()
            };
            let session_id = member.session_id.clone().ok_or_else(|| {
                format!(
                    "missing provisioned session for role '{}'",
                    dispatch.role_id
                )
            })?;

            if let Some(state_member) = self.state.members.get_mut(&dispatch.role_id) {
                *state_member = member.clone();
            }

            self.shell.record_dispatch(dispatch.clone());
            self.state
                .dispatches
                .insert(dispatch.dispatch_id, dispatch.clone());
            events.push(WorkspaceEvent::MemberStateChanged {
                timestamp: chrono::Utc::now().to_rfc3339(),
                workspace_id: self.state.workspace_id.clone(),
                member: member.clone(),
            });
            events.push(WorkspaceEvent::DispatchQueued {
                timestamp: chrono::Utc::now().to_rfc3339(),
                workspace_id: self.state.workspace_id.clone(),
                dispatch: dispatch.clone(),
            });
            self.messenger
                .send_to_session(&session_id, &dispatch.instruction)
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok((dispatches, events))
    }

    fn sync_state_from_shell(&mut self) {
        self.state.roles = self
            .shell
            .spec
            .roles
            .iter()
            .cloned()
            .map(|role| (role.id.clone(), role))
            .collect();
        self.state.members = self.shell.members.clone();
        self.state.dispatches = self
            .shell
            .dispatches
            .iter()
            .cloned()
            .map(|dispatch| (dispatch.dispatch_id, dispatch))
            .collect();
        self.state.activities = self.shell.activities.clone();
    }

    fn build_provider_state(&self) -> PersistedProviderState {
        PersistedProviderState {
            workspace_id: self.state.workspace_id.clone(),
            provider: multi_agent_protocol::MultiAgentProvider::Cteno,
            root_conversation_id: Some(self.bootstrapped.workspace_session_id.clone()),
            member_bindings: self
                .shell
                .members
                .iter()
                .filter_map(|(role_id, member)| {
                    member.session_id.as_ref().map(|session_id| {
                        (
                            role_id.clone(),
                            PersistedProviderBinding {
                                role_id: role_id.clone(),
                                provider_conversation_id: session_id.clone(),
                                kind: ProviderConversationKind::Session,
                                updated_at: chrono::Utc::now().to_rfc3339(),
                            },
                        )
                    })
                })
                .collect(),
            metadata: serde_json::to_value(CtenoWorkspaceMetadata {
                workspace_persona_id: self.bootstrapped.workspace_persona_id.clone(),
                workspace_session_id: self.bootstrapped.workspace_session_id.clone(),
                roles: self.bootstrapped.roles.clone(),
                orchestrator_state: Some(self.orchestrator.serialize_state()),
            })
            .ok(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    fn persist(&self) -> Result<(), String> {
        if let Some(persistence) = self.persistence.as_ref() {
            persistence
                .persist_runtime(&self.state, &self.history, &self.build_provider_state())
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
struct LocalWorkspaceExecutionOptions {
    disable_tools: bool,
    profile_or_model_override: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMemberSummary {
    pub role_id: Option<String>,
    pub session_id: String,
    pub agent_id: Option<String>,
    pub task_description: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSummary {
    pub binding: WorkspaceBinding,
    pub persona: Persona,
    pub members: Vec<WorkspaceMemberSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<WorkspaceRuntimeSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRuntimeSummary {
    pub state: WorkspaceState,
    pub recent_activities: Vec<WorkspaceActivity>,
    pub recent_events: Vec<WorkspaceEvent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTemplateSummary {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub coordinator_role_id: Option<String>,
    pub default_role_id: Option<String>,
    pub role_count: usize,
    pub workflow_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTurnResponse {
    pub plan: WorkspaceTurnPlan,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_vote_window: Option<WorkspaceWorkflowVoteWindow>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub workflow_vote_responses: Vec<WorkspaceWorkflowVoteResponse>,
    pub dispatches: Vec<TaskDispatch>,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<TaskDispatch>,
    pub events: Vec<WorkspaceEvent>,
    pub state: WorkspaceState,
    pub template_state: Value,
}

pub fn workspace_template_catalog() -> Vec<WorkspaceTemplateSummary> {
    workspace_templates()
        .into_iter()
        .map(|template| WorkspaceTemplateSummary {
            id: template.template_id.clone(),
            name: template.template_name.clone(),
            provider: "agnostic".to_string(),
            coordinator_role_id: template.coordinator_role_id.clone(),
            default_role_id: template.default_role_id.clone(),
            role_count: template.roles.len(),
            workflow_mode: template
                .workflow
                .as_ref()
                .map(|workflow| format!("{:?}", workflow.mode).to_lowercase()),
        })
        .collect()
}

pub fn workspace_template_by_id(template_id: &str) -> Option<WorkspaceTemplate> {
    workspace_templates()
        .into_iter()
        .find(|template| template.template_id == template_id)
}

pub async fn register_local_workspace_rpc_handlers(
    registry: Arc<cteno_host_rpc_core::RpcRegistry>,
    machine_id: &str,
) {
    crate::usage_monitor::register_rpc(registry.clone(), machine_id).await;

    let rpc_method_bootstrap_workspace = format!("{}:bootstrap-workspace", machine_id);
    let rpc_method_list_agent_workspace_templates =
        format!("{}:list-agent-workspace-templates", machine_id);
    let rpc_method_list_agent_workspaces = format!("{}:list-agent-workspaces", machine_id);
    let rpc_method_get_agent_workspace = format!("{}:get-agent-workspace", machine_id);
    let rpc_method_delete_agent_workspace = format!("{}:delete-agent-workspace", machine_id);
    let rpc_method_workspace_send = format!("{}:workspace-send-message", machine_id);

    registry
        .register_persistent(
            &rpc_method_list_agent_workspace_templates,
            move |_params: Value| async move {
                Ok(json!({
                    "success": true,
                    "templates": crate::multi_agent::workspace_template_catalog(),
                }))
            },
        )
        .await;

    registry
        .register_persistent(
            &rpc_method_bootstrap_workspace,
            move |params: Value| async move {
                let template_id = params
                    .get("templateId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if template_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing templateId" }));
                }

                let Some(mut template) = crate::multi_agent::workspace_template_by_id(&template_id)
                else {
                    return Ok(json!({
                        "success": false,
                        "error": format!("Unsupported workspace template '{}'", template_id)
                    }));
                };

                if let Some(overrides) = params
                    .get("roleVendorOverrides")
                    .and_then(|v| v.as_object())
                {
                    for role in template.roles.iter_mut() {
                        let Some(vendor_val) = overrides.get(&role.id).and_then(|v| v.as_str())
                        else { continue };
                        match vendor_val {
                            "cteno" => {
                                role.agent.provider =
                                    Some(multi_agent_protocol::MultiAgentProvider::Cteno);
                            }
                            "claude" => {
                                role.agent.provider =
                                    Some(multi_agent_protocol::MultiAgentProvider::ClaudeAgentSdk);
                            }
                            "codex" => {
                                role.agent.provider =
                                    Some(multi_agent_protocol::MultiAgentProvider::CodexSdk);
                            }
                            other => {
                                log::warn!(
                                    "[bootstrap-workspace] ignoring unsupported vendor override '{}' for role '{}' (template '{}')",
                                    other,
                                    role.id,
                                    template_id
                                );
                            }
                        }
                    }
                }

                let cwd = params
                    .get("workdir")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        std::env::current_dir()
                            .ok()
                            .map(|p| p.display().to_string())
                    });
                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| template.template_name.clone());
                let workspace_id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let model = params
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("deepseek-chat");

                let instance = multi_agent_protocol::WorkspaceInstanceParams {
                    id: workspace_id.clone(),
                    name: name.clone(),
                    cwd,
                };
                let profile = multi_agent_protocol::WorkspaceProfile {
                    provider: multi_agent_protocol::MultiAgentProvider::Cteno,
                    model: model.to_string(),
                    permission_mode: Some(multi_agent_protocol::PermissionMode::AcceptEdits),
                    role_edit_permission_mode: Some(
                        multi_agent_protocol::PermissionMode::AcceptEdits,
                    ),
                    setting_sources: Some(vec![multi_agent_protocol::SettingSource::Project]),
                    allowed_tools: None,
                    disallowed_tools: None,
                };

                match crate::multi_agent::bootstrap_template_workspace(
                    &template, &instance, &profile,
                )
                .await
                {
                    Ok((bootstrapped, events)) => {
                        reconcile_project_skills_for_workspace(instance.cwd.as_deref()).await;
                        let roles_json: Vec<Value> = bootstrapped
                            .roles
                            .iter()
                            .map(|role| {
                                json!({
                                    "roleId": role.role_id,
                                    "agentId": role.agent_id,
                                    "sessionId": role.session_id,
                                })
                            })
                            .collect();
                        Ok(json!({
                            "success": true,
                            "workspace": {
                                "id": workspace_id,
                                "name": name,
                                "templateId": template.template_id,
                                "personaId": bootstrapped.workspace_persona_id,
                                "sessionId": bootstrapped.workspace_session_id,
                                "roles": roles_json,
                            },
                            "events": events,
                        }))
                    }
                    Err(error) => Ok(json!({ "success": false, "error": error.to_string() })),
                }
            },
        )
        .await;

    registry
        .register_persistent(
            &rpc_method_list_agent_workspaces,
            move |_params: Value| async move {
                match crate::multi_agent::list_workspace_summaries_live().await {
                    Ok(workspaces) => Ok(json!({ "success": true, "workspaces": workspaces })),
                    Err(error) => Ok(json!({ "success": false, "error": error })),
                }
            },
        )
        .await;

    registry
        .register_persistent(
            &rpc_method_get_agent_workspace,
            move |params: Value| async move {
                let persona_id = params
                    .get("personaId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if persona_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing personaId" }));
                }
                match crate::multi_agent::get_workspace_summary_live(&persona_id).await {
                    Ok(Some(workspace)) => Ok(json!({ "success": true, "workspace": workspace })),
                    Ok(None) => Ok(json!({ "success": false, "error": "Workspace not found" })),
                    Err(error) => Ok(json!({ "success": false, "error": error })),
                }
            },
        )
        .await;

    registry
        .register_persistent(
            &rpc_method_delete_agent_workspace,
            move |params: Value| async move {
                let persona_id = params
                    .get("personaId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if persona_id.is_empty() {
                    return Ok(json!({ "success": false, "error": "Missing personaId" }));
                }
                match crate::multi_agent::delete_workspace(&persona_id).await {
                    Ok(()) => Ok(json!({ "success": true })),
                    Err(error) => Ok(json!({ "success": false, "error": error })),
                }
            },
        )
        .await;

    registry
        .register_persistent(
            &rpc_method_workspace_send,
            move |params: Value| async move {
                let persona_id = params
                    .get("personaId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let message = params
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let role_id = params
                    .get("roleId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                if persona_id.is_empty() || message.is_empty() {
                    return Ok(
                        json!({ "success": false, "error": "Missing personaId or message" }),
                    );
                }
                match crate::multi_agent::send_workspace_message(
                    &persona_id,
                    role_id.as_deref(),
                    &message,
                )
                .await
                {
                    Ok(turn) => Ok(json!({
                        "success": true,
                        "plan": turn.plan,
                        "workflowVoteWindow": turn.workflow_vote_window,
                        "workflowVoteResponses": turn.workflow_vote_responses,
                        "dispatches": turn.dispatches,
                        "sessionId": turn.session_id,
                        "personaId": persona_id,
                        "roleId": turn.role_id,
                        "dispatch": turn.dispatch,
                        "events": turn.events,
                        "state": turn.state,
                        "templateState": turn.template_state,
                    })),
                    Err(error) => Ok(json!({ "success": false, "error": error })),
                }
            },
        )
        .await;
}

async fn reconcile_project_skills_for_workspace(cwd: Option<&str>) {
    let Some(raw) = cwd.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    let expanded = shellexpand::tilde(raw).to_string();
    let workdir = PathBuf::from(expanded);
    if !workdir.is_absolute() {
        return;
    }
    crate::agent_sync_bridge::reconcile_project_now(&workdir).await;
}

#[derive(Clone, Default)]
pub struct CtenoWorkspaceProvisioner;

#[derive(Clone, Default)]
pub struct CtenoSessionMessenger;

pub async fn bootstrap_template_workspace(
    template: &WorkspaceTemplate,
    instance: &WorkspaceInstanceParams,
    profile: &WorkspaceProfile,
) -> Result<(BootstrappedWorkspace, Vec<WorkspaceEvent>), AdapterError> {
    let mut adapter = CtenoWorkspaceAdapter::from_template(
        template,
        instance,
        profile,
        CtenoWorkspaceProvisioner,
        CtenoSessionMessenger,
    );
    let events = adapter.bootstrap().await?;
    let bootstrapped = adapter.bootstrapped().cloned().ok_or_else(|| {
        AdapterError::Provisioning("workspace bootstrap produced no metadata".to_string())
    })?;
    persist_workspace_binding(
        adapter.runtime().spec(),
        &template.template_id,
        &bootstrapped,
    )?;
    let live_instance = WorkspaceInstance::from_parts(
        &template.template_id,
        adapter.runtime().spec().clone(),
        adapter.snapshot(),
        adapter.history().to_vec(),
        bootstrapped.clone(),
        None,
    )
    .map_err(AdapterError::Provisioning)?;
    register_live_workspace(bootstrapped.workspace_persona_id.clone(), live_instance).await;
    Ok((bootstrapped, events))
}

pub async fn bootstrap_workspace(
    spec: WorkspaceSpec,
) -> Result<(BootstrappedWorkspace, Vec<WorkspaceEvent>), AdapterError> {
    let mut adapter =
        CtenoWorkspaceAdapter::new(spec, CtenoWorkspaceProvisioner, CtenoSessionMessenger);
    let events = adapter.bootstrap().await?;
    let bootstrapped = adapter.bootstrapped().cloned().ok_or_else(|| {
        AdapterError::Provisioning("workspace bootstrap produced no metadata".to_string())
    })?;
    persist_workspace_binding(adapter.runtime().spec(), "custom", &bootstrapped)?;
    let live_instance = WorkspaceInstance::from_parts(
        "custom",
        adapter.runtime().spec().clone(),
        adapter.snapshot(),
        adapter.history().to_vec(),
        bootstrapped.clone(),
        None,
    )
    .map_err(AdapterError::Provisioning)?;
    register_live_workspace(bootstrapped.workspace_persona_id.clone(), live_instance).await;
    Ok((bootstrapped, events))
}

pub async fn send_workspace_message(
    persona_id: &str,
    role_id: Option<&str>,
    message: &str,
) -> Result<WorkspaceTurnResponse, String> {
    ensure_live_workspace(persona_id).await?;

    let registry = workspace_instance_registry();
    let mut instances = registry.lock().await;
    let instance = instances.get_mut(persona_id).ok_or_else(|| {
        format!(
            "Workspace persona '{}' is not active in the runtime registry.",
            persona_id
        )
    })?;

    instance.handle_user_message(message, role_id).await
}

pub async fn delete_workspace(persona_id: &str) -> Result<(), String> {
    let pm = crate::local_services::persona_manager()?;
    let store = pm.store();
    let binding = store
        .get_workspace_binding(persona_id)?
        .ok_or_else(|| format!("Workspace binding for persona '{}' not found", persona_id))?;

    let removed = {
        let registry = workspace_instance_registry();
        registry.lock().await.remove(persona_id)
    };

    if let Some(mut instance) = removed {
        instance
            .delete_workspace()
            .await
            .map_err(|e| e.to_string())?;
    } else if let Ok(mut adapter) = CtenoWorkspaceAdapter::restore_from_local(
        &binding.workdir,
        &binding.workspace_id,
        CtenoWorkspaceProvisioner,
        CtenoSessionMessenger,
    ) {
        adapter
            .delete_workspace()
            .await
            .map_err(|e| e.to_string())?;
    }

    store
        .delete_workspace_binding(persona_id)
        .map_err(|e| format!("Failed to delete workspace binding: {}", e))?;
    store
        .delete_persona(persona_id)
        .map_err(|e| format!("Failed to delete workspace persona: {}", e))?;

    Ok(())
}

pub fn get_workspace_summary(persona_id: &str) -> Result<Option<WorkspaceSummary>, String> {
    let pm = crate::local_services::persona_manager()?;
    let store = pm.store();
    let binding = match store.get_workspace_binding(persona_id)? {
        Some(binding) => binding,
        None => return Ok(None),
    };
    let persona = store
        .get_persona(persona_id)?
        .ok_or_else(|| format!("Workspace persona '{}' not found", persona_id))?;
    let members = store
        .list_member_sessions(persona_id)?
        .into_iter()
        .map(member_to_summary)
        .collect();
    let runtime = live_runtime_summary(persona_id);

    Ok(Some(WorkspaceSummary {
        binding,
        persona,
        members,
        runtime,
    }))
}

pub async fn get_workspace_summary_live(
    persona_id: &str,
) -> Result<Option<WorkspaceSummary>, String> {
    let _ = ensure_live_workspace(persona_id).await;
    get_workspace_summary(persona_id)
}

pub fn list_workspace_summaries() -> Result<Vec<WorkspaceSummary>, String> {
    let pm = crate::local_services::persona_manager()?;
    let store = pm.store();
    let mut workspaces = Vec::new();

    for binding in store.list_workspace_bindings()? {
        let Some(persona) = store.get_persona(&binding.persona_id)? else {
            log::warn!(
                "[MultiAgent] Skipping workspace {} because persona {} is missing",
                binding.workspace_id,
                binding.persona_id
            );
            continue;
        };
        let members = store
            .list_member_sessions(&binding.persona_id)?
            .into_iter()
            .map(member_to_summary)
            .collect();
        let runtime = live_runtime_summary(&binding.persona_id);
        workspaces.push(WorkspaceSummary {
            binding,
            persona,
            members,
            runtime,
        });
    }

    Ok(workspaces)
}

pub async fn list_workspace_summaries_live() -> Result<Vec<WorkspaceSummary>, String> {
    let pm = crate::local_services::persona_manager()?;
    let store = pm.store();
    let bindings = store.list_workspace_bindings()?;
    for binding in bindings {
        let _ = ensure_live_workspace(&binding.persona_id).await;
    }
    list_workspace_summaries()
}

fn workspace_templates() -> Vec<WorkspaceTemplate> {
    vec![
        multi_agent_protocol::create_coding_studio_template(),
        multi_agent_protocol::create_opc_solo_company_template(),
        multi_agent_protocol::create_autoresearch_template(),
        multi_agent_protocol::create_edict_governance_template(),
        multi_agent_protocol::create_task_gate_coding_manual_template(),
    ]
}

fn create_orchestrator(template_id: &str) -> Box<dyn WorkspaceOrchestrator> {
    match template_id {
        "autoresearch" => Box::new(AutoresearchOrchestrator::new(
            "lead",
            "experimenter",
            "critic",
            None,
        )),
        "task-gate-coding" | "task-gate-coding-manual" => {
            Box::new(GatedTasksOrchestrator::new("reviewer", "coder"))
        }
        _ => Box::new(GroupChatOrchestrator::new(String::new())),
    }
}

fn workspace_shell_from_state(spec: &WorkspaceSpec, state: &WorkspaceState) -> WorkspaceShell {
    let mut shell = WorkspaceShell::new(spec.clone());
    shell.members = state.members.clone();
    shell.activities = state.activities.clone();
    shell.dispatches = state.dispatches.values().cloned().collect();
    shell
}

fn workspace_turn_plan_from_response(
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
    target_role: Option<&str>,
    activities: &[WorkspaceActivity],
    dispatches: &[TaskDispatch],
) -> WorkspaceTurnPlan {
    if let Some(role_id) = target_role {
        return direct_workspace_turn_plan(spec, request, role_id);
    }

    if !dispatches.is_empty() {
        let response_text = activities
            .iter()
            .rev()
            .find(|activity| {
                activity.kind != multi_agent_protocol::WorkspaceActivityKind::UserMessage
            })
            .map(|activity| activity.text.clone())
            .unwrap_or_else(|| format!("@{} will take this next.", dispatches[0].role_id));
        return WorkspaceTurnPlan {
            coordinator_role_id: spec
                .coordinator_role_id
                .clone()
                .or_else(|| spec.default_role_id.clone())
                .unwrap_or_else(|| "coordinator".to_string()),
            response_text,
            assignments: dispatches
                .iter()
                .map(|dispatch| WorkspaceTurnAssignment {
                    role_id: dispatch.role_id.clone(),
                    instruction: dispatch.instruction.clone(),
                    summary: dispatch.summary.clone(),
                    visibility: dispatch.visibility,
                    workflow_node_id: dispatch.workflow_node_id.clone(),
                    stage_id: dispatch.stage_id.clone(),
                })
                .collect(),
            rationale: Some("Planned by the workspace orchestrator.".to_string()),
        };
    }

    let mut plan = plan_workspace_turn(spec, request);
    plan.assignments.clear();
    if let Some(activity) = activities
        .iter()
        .rev()
        .find(|activity| activity.kind != multi_agent_protocol::WorkspaceActivityKind::UserMessage)
    {
        plan.response_text = activity.text.clone();
    }
    plan
}

fn workspace_instance_registry() -> &'static WorkspaceInstanceRegistry {
    LIVE_WORKSPACE_INSTANCES.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

fn workspace_abort_registry() -> &'static LocalWorkspaceAbortRegistry {
    LOCAL_WORKSPACE_ABORT_FLAGS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

async fn register_live_workspace(persona_id: String, instance: WorkspaceInstance) {
    workspace_instance_registry()
        .lock()
        .await
        .insert(persona_id, instance);
}

async fn resolve_workspace_profile_id(requested_model: &str) -> String {
    let Ok(runtime_ctx) = crate::local_services::agent_runtime_context() else {
        return requested_model.to_string();
    };
    let profile_store = runtime_ctx.profile_store.read().await;
    let proxy_profiles = runtime_ctx.proxy_profiles.read().await;
    if profile_store
        .get_profile_or_proxy(requested_model, &proxy_profiles)
        .is_some()
    {
        return requested_model.to_string();
    }
    if let Some(profile_id) = profile_store
        .profiles
        .iter()
        .find(|profile| profile.chat.model == requested_model)
        .map(|profile| profile.id.clone())
    {
        return profile_id;
    }
    if let Some(profile_id) = proxy_profiles
        .iter()
        .find(|profile| profile.chat.model == requested_model)
        .map(|profile| profile.id.clone())
    {
        return profile_id;
    }
    profile_store.default_profile_id.clone()
}

fn session_context_field(session_id: &str, key: &str) -> Option<Value> {
    let runtime_ctx = crate::local_services::agent_runtime_context().ok()?;
    let manager = crate::agent_session::AgentSessionManager::new(runtime_ctx.db_path.clone());
    manager
        .get_session(session_id)
        .ok()
        .flatten()
        .and_then(|session| session.context_data)
        .and_then(|context| context.get(key).cloned())
}

fn upsert_local_agent_session_context(
    session_id: &str,
    agent_id: &str,
    workdir: &str,
    profile_id: Option<&str>,
    owner_session_id: Option<&str>,
    vendor: Option<&str>,
    permission_mode: Option<multi_agent_protocol::PermissionMode>,
) -> Result<(), String> {
    let runtime_ctx = crate::local_services::agent_runtime_context()?;
    let manager = crate::agent_session::AgentSessionManager::new(runtime_ctx.db_path.clone());
    let existing = manager.get_session(session_id)?;

    if existing.is_none() {
        match vendor {
            Some(vendor) => manager
                .create_session_with_id_and_vendor(session_id, agent_id, None, None, vendor)?,
            None => manager.create_session_with_id(session_id, agent_id, None, None)?,
        };
    } else if let Some(vendor) = vendor {
        if existing.as_ref().map(|session| session.vendor.as_str()) != Some(vendor) {
            manager.set_vendor(session_id, vendor)?;
        }
    }

    let mut context = existing
        .and_then(|session| session.context_data)
        .unwrap_or_else(|| json!({}));
    let context_obj = context
        .as_object_mut()
        .ok_or_else(|| "Session context_data is not an object".to_string())?;
    context_obj.insert("workdir".to_string(), Value::String(workdir.to_string()));
    context_obj.insert("agent_id".to_string(), Value::String(agent_id.to_string()));
    if let Some(profile_id) = profile_id {
        context_obj.insert(
            "profile_id".to_string(),
            Value::String(profile_id.to_string()),
        );
    }
    if let Some(permission_mode) = permission_mode {
        context_obj.insert(
            "permission_mode".to_string(),
            Value::String(permission_mode_name(permission_mode).to_string()),
        );
    }
    manager.update_context_data(session_id, &context)?;
    manager.update_owner_session_id(session_id, owner_session_id)?;
    Ok(())
}

fn workspace_executor_session_registry() -> LiveWorkspaceExecutorRegistry {
    LIVE_WORKSPACE_EXECUTOR_SESSIONS
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

async fn workspace_executor_session(session_id: &str) -> Option<LiveWorkspaceExecutorSession> {
    workspace_executor_session_registry()
        .lock()
        .await
        .get(session_id)
        .cloned()
}

async fn store_workspace_executor_session(
    session_id: &str,
    live_session: LiveWorkspaceExecutorSession,
) {
    workspace_executor_session_registry()
        .lock()
        .await
        .insert(session_id.to_string(), live_session);
}

async fn remove_workspace_executor_session(
    session_id: &str,
) -> Option<LiveWorkspaceExecutorSession> {
    workspace_executor_session_registry()
        .lock()
        .await
        .remove(session_id)
}

fn session_vendor(session_id: &str) -> Option<String> {
    let runtime_ctx = crate::local_services::agent_runtime_context().ok()?;
    let manager = crate::agent_session::AgentSessionManager::new(runtime_ctx.db_path.clone());
    manager
        .get_session(session_id)
        .ok()
        .flatten()
        .map(|session| session.vendor)
}

fn session_workdir(session_id: &str) -> String {
    session_context_field(session_id, "workdir")
        .and_then(|value| value.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "~".to_string())
}

fn session_executor_permission_mode(session_id: &str) -> ExecutorPermissionMode {
    match session_context_field(session_id, "permission_mode").and_then(|value| {
        value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    }) {
        Some(mode) if mode == "acceptEdits" => ExecutorPermissionMode::AcceptEdits,
        Some(mode) if mode == "plan" => ExecutorPermissionMode::Plan,
        Some(mode) if mode == "bypassPermissions" => ExecutorPermissionMode::BypassPermissions,
        _ => ExecutorPermissionMode::Default,
    }
}

fn executor_vendor_name(provider: multi_agent_protocol::MultiAgentProvider) -> &'static str {
    match provider {
        multi_agent_protocol::MultiAgentProvider::ClaudeAgentSdk => "claude",
        multi_agent_protocol::MultiAgentProvider::CodexSdk => "codex",
        multi_agent_protocol::MultiAgentProvider::Cteno => "cteno",
    }
}

fn requested_role_executor_vendor(
    spec: &WorkspaceSpec,
    role: &multi_agent_protocol::RoleSpec,
) -> &'static str {
    executor_vendor_name(role.agent.provider.unwrap_or(spec.provider))
}

fn executor_model_provider(vendor: &str) -> &'static str {
    match vendor {
        "claude" => "anthropic",
        "codex" => "openai",
        _ => "cteno",
    }
}

fn build_workspace_executor_spawn_spec(
    session_id: &str,
    vendor: &str,
    system_prompt: String,
    model: String,
    temperature: f32,
) -> SpawnSessionSpec {
    let mut agent_config = json!({});
    // Cteno sessions inherit the host's unified accessToken via the adapter's
    // `extract_auth_from_agent_config`. Other vendors carry their own CLI auth
    // and the block is simply unused.
    crate::executor_session::merge_auth_into(&mut agent_config);
    SpawnSessionSpec {
        workdir: PathBuf::from(shellexpand::tilde(&session_workdir(session_id)).to_string()),
        system_prompt: Some(system_prompt),
        model: Some(ModelSpec {
            provider: executor_model_provider(vendor).to_string(),
            model_id: model,
            reasoning_effort: None,
            temperature: Some(temperature),
        }),
        permission_mode: session_executor_permission_mode(session_id),
        allowed_tools: None,
        additional_directories: Vec::new(),
        env: BTreeMap::new(),
        agent_config,
        resume_hint: None,
    }
}

async fn spawn_workspace_executor_session(
    session_id: &str,
    vendor: &str,
    spawn_spec: SpawnSessionSpec,
) -> Result<LiveWorkspaceExecutorSession, String> {
    if let Some(existing) = workspace_executor_session(session_id).await {
        return Ok(existing);
    }

    let runtime_ctx = crate::local_services::agent_runtime_context()?;
    let registry = crate::local_services::executor_registry()?;
    let executor = registry.resolve(vendor)?;
    let vendor_key: Option<&'static str> = match vendor {
        "cteno" => Some("cteno"),
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "gemini" => Some("gemini"),
        _ => None,
    };
    let session_ref = if let Some(vendor_key) = vendor_key {
        // Use the registry's auto-reopen-on-closed helper so a cached
        // subprocess that died during a long idle window is transparently
        // redialed once before we fall back to `spawn_session`.
        match registry
            .start_session_with_autoreopen(vendor_key, spawn_spec.clone())
            .await
        {
            Ok(session) => Ok(session),
            Err(err) => {
                log::warn!(
                    "multi_agent spawn: start_session_with_autoreopen({vendor}) failed: {err} — falling back to spawn_session"
                );
                executor.spawn_session(spawn_spec).await
            }
        }
    } else {
        executor.spawn_session(spawn_spec).await
    }
    .map_err(|error| {
        format!(
            "executor.spawn_session({vendor}) failed: {}",
            user_visible_executor_error(&error)
        )
    })?;
    crate::happy_client::session_helpers::upsert_agent_session_native_session_id(
        &runtime_ctx.db_path,
        session_id,
        session_ref.vendor,
        session_ref.id.as_str(),
    )?;

    let live_session = LiveWorkspaceExecutorSession {
        executor,
        session_ref,
    };
    store_workspace_executor_session(session_id, live_session.clone()).await;
    Ok(live_session)
}

fn is_workspace_custom_agent(kind: &crate::agent_kind::AgentKind) -> bool {
    matches!(
        kind,
        crate::agent_kind::AgentKind::Custom(agent_id)
            if agent_id.starts_with(&format!("{WORKSPACE_AGENT_PREFIX}-"))
    )
}

async fn local_role_execution_input(
    session_id: &str,
    options: &LocalWorkspaceExecutionOptions,
) -> Result<
    (
        AgentConfig,
        String,
        String,
        String,
        String,
        f32,
        u32,
        bool,
        crate::llm_profile::ApiFormat,
        bool,
        bool,
        bool,
        Vec<crate::llm::Tool>,
        Vec<AgentConfig>,
        Vec<String>,
    ),
    String,
> {
    let runtime_ctx = crate::local_services::agent_runtime_context()?;
    let resolution = crate::agent_kind::resolve_agent_kind(session_id);
    let fallback_workdir = session_context_field(session_id, "workdir")
        .and_then(|value| value.as_str().map(|s| s.to_string()));
    let workdir = resolution
        .workdir()
        .map(|value| value.to_string())
        .or(fallback_workdir)
        .unwrap_or_else(|| "~".to_string());
    let requested_profile_id = options
        .profile_or_model_override
        .clone()
        .or_else(|| {
            session_context_field(session_id, "profile_id")
                .and_then(|value| value.as_str().map(|s| s.to_string()))
        })
        .unwrap_or_else(|| "default".to_string());
    let profile_id = resolve_workspace_profile_id(&requested_profile_id).await;

    let proxy_profiles = runtime_ctx.proxy_profiles.read().await;
    let profile_store = runtime_ctx.profile_store.read().await;
    let profile = profile_store
        .get_profile_or_proxy(&profile_id, &proxy_profiles)
        .unwrap_or_else(|| profile_store.get_default().clone());
    drop(profile_store);
    drop(proxy_profiles);

    let direct_api_key = if profile.chat.api_key.is_empty() {
        runtime_ctx.global_api_key.read().await.clone()
    } else {
        profile.chat.api_key.clone()
    };
    let use_proxy = crate::llm_profile::is_proxy_profile(&profile.id) || direct_api_key.is_empty();
    let (api_key, base_url) = if use_proxy {
        let (auth_token, _, _, _) =
            crate::auth_store_boot::load_persisted_machine_auth(&runtime_ctx.data_dir)?
                .ok_or_else(|| "proxy profiles require logged-in Happy Server auth".to_string())?;
        (auth_token, crate::resolved_happy_server_url())
    } else {
        (direct_api_key, profile.chat.base_url.clone())
    };

    let workspace_skills_dir = Some(
        PathBuf::from(shellexpand::tilde(&workdir).to_string())
            .join(".cteno")
            .join("skills"),
    );
    let enabled_skills = crate::service_init::load_all_skills(
        &runtime_ctx.builtin_skills_dir,
        &runtime_ctx.user_skills_dir,
        workspace_skills_dir.as_deref(),
    );
    let mut runtime_context_messages = Vec::new();
    if let Some(skill_index) = crate::service_init::build_skill_index_message(
        &enabled_skills,
        profile.chat.context_window_tokens.unwrap_or(128_000),
    ) {
        runtime_context_messages.push(skill_index);
    }

    let workspace_agents_dir = Some(
        PathBuf::from(shellexpand::tilde(&workdir).to_string())
            .join(".cteno")
            .join("agents"),
    );
    let all_agents = crate::service_init::load_all_agents(
        &runtime_ctx.builtin_agents_dir,
        &runtime_ctx.user_agents_dir,
        workspace_agents_dir.as_deref(),
    );
    let agent_config = all_agents
        .iter()
        .find(|agent| match &resolution.kind {
            crate::agent_kind::AgentKind::Custom(agent_id) => agent.id == *agent_id,
            _ => agent.id == "worker",
        })
        .cloned()
        .unwrap_or_default();

    let is_workspace_custom_agent = is_workspace_custom_agent(&resolution.kind);

    let mut native_tools = Vec::new();
    if !options.disable_tools {
        let (fetched_tools, deferred_summaries) =
            crate::autonomous_agent::fetch_native_tools_split().await;
        native_tools = fetched_tools;
        native_tools.extend(crate::autonomous_agent::build_agent_tools(&all_agents));
        crate::agent_kind::apply_tool_filter(&mut native_tools, &resolution);
        if !is_workspace_custom_agent {
            if let Some(deferred_ctx) =
                crate::autonomous_agent::build_deferred_tools_context(&deferred_summaries)
            {
                runtime_context_messages.push(deferred_ctx);
            }
        }
    }

    let base_prompt =
        crate::system_prompt::build_system_prompt(&crate::system_prompt::PromptOptions {
            include_tool_style: profile.supports_function_calling && !options.disable_tools,
            ..Default::default()
        });
    let (effective_system_prompt, _persona_id, _persona_workdir) =
        crate::agent_kind::build_agent_prompt(&resolution, &base_prompt);
    runtime_context_messages.insert(
        0,
        crate::system_prompt::build_runtime_datetime_context(&effective_system_prompt),
    );
    runtime_context_messages.push(build_local_model_identity_context(
        &profile.chat.model,
        profile.supports_vision,
        profile.supports_computer_use,
    ));

    Ok((
        agent_config,
        effective_system_prompt,
        api_key,
        base_url,
        profile.chat.model,
        profile.chat.temperature,
        profile.chat.max_tokens,
        use_proxy,
        profile.api_format,
        profile.supports_vision,
        profile.thinking,
        profile.supports_function_calling && !options.disable_tools,
        native_tools,
        all_agents,
        runtime_context_messages,
    ))
}

async fn execute_local_workspace_session(
    session_id: String,
    message: String,
) -> Result<String, String> {
    execute_local_workspace_session_with_options(
        session_id,
        message,
        LocalWorkspaceExecutionOptions::default(),
    )
    .await
}

async fn execute_local_workspace_session_with_options(
    session_id: String,
    message: String,
    options: LocalWorkspaceExecutionOptions,
) -> Result<String, String> {
    let (
        _agent_config,
        effective_system_prompt,
        _api_key,
        _base_url,
        model,
        temperature,
        _max_tokens,
        _use_proxy,
        _api_format,
        _supports_vision,
        _enable_thinking,
        _supports_function_calling,
        _tools,
        _all_agents,
        runtime_context_messages,
    ) = local_role_execution_input(&session_id, &options).await?;

    let session_vendor = session_vendor(&session_id).unwrap_or_else(|| "cteno".to_string());
    let live_session = match workspace_executor_session(&session_id).await {
        Some(existing) => existing,
        None => {
            let spawn_spec = build_workspace_executor_spawn_spec(
                &session_id,
                &session_vendor,
                effective_system_prompt.clone(),
                model.clone(),
                temperature,
            );
            spawn_workspace_executor_session(&session_id, &session_vendor, spawn_spec).await?
        }
    };

    let mut prompt = message.clone();
    if !runtime_context_messages.is_empty() {
        prompt = format!("{}\n\n{}", runtime_context_messages.join("\n\n"), prompt);
    }

    // `agent_runtime_context()` is the source of the db path used by the
    // event loop below; fetch it once so we can persist the user turn
    // before the vendor consumes it (the workspace role path runs without
    // an ExecutorNormalizer, so we go through the module-level helper).
    let runtime_ctx = crate::local_services::agent_runtime_context()?;
    crate::executor_normalizer::persist_local_user_message(
        &runtime_ctx.db_path,
        &session_id,
        live_session.session_ref.vendor,
        &prompt,
        None,
    )
    .map_err(|error| format!("persist user message failed: {error}"))?;

    let mut stream = live_session
        .executor
        .send_message(
            &live_session.session_ref,
            UserMessage {
                content: prompt,
                attachments: Vec::new(),
                parent_tool_use_id: None,
                injected_tools: Vec::new(),
            },
        )
        .await
        .map_err(|error| format!("executor.send_message({}) failed: {error}", session_vendor))?;
    let mut streamed_text = String::new();
    let mut final_text = None;
    let mut last_error = None;

    use futures_util::StreamExt;
    while let Some(event) = stream.next().await {
        let event = event.map_err(|error| format!("executor stream error: {error}"))?;
        match event {
            multi_agent_runtime_core::ExecutorEvent::SessionReady { native_session_id } => {
                crate::happy_client::session_helpers::upsert_agent_session_native_session_id(
                    &runtime_ctx.db_path,
                    &session_id,
                    live_session.session_ref.vendor,
                    native_session_id.as_str(),
                )?;
            }
            multi_agent_runtime_core::ExecutorEvent::StreamDelta { kind, content } => {
                if kind == multi_agent_runtime_core::DeltaKind::Text {
                    streamed_text.push_str(&content);
                }
            }
            multi_agent_runtime_core::ExecutorEvent::TurnComplete {
                final_text: turn_text,
                ..
            } => {
                final_text =
                    turn_text.or_else(|| (!streamed_text.is_empty()).then_some(streamed_text));
                break;
            }
            multi_agent_runtime_core::ExecutorEvent::Error {
                message,
                recoverable,
            } => {
                if recoverable {
                    last_error = Some(message);
                } else {
                    return Err(message);
                }
            }
            _ => {}
        }
    }

    if let Some(text) = final_text {
        Ok(text)
    } else if let Some(error) = last_error {
        Err(error)
    } else {
        Err(format!(
            "executor '{}' produced no completion",
            session_vendor
        ))
    }
}

async fn ensure_live_workspace(persona_id: &str) -> Result<(), String> {
    {
        let registry = workspace_instance_registry();
        let instances = registry.lock().await;
        if instances.contains_key(persona_id) {
            return Ok(());
        }
    }

    let instance = restore_live_workspace(persona_id).await?;
    register_live_workspace(persona_id.to_string(), instance).await;
    Ok(())
}

async fn restore_live_workspace(persona_id: &str) -> Result<WorkspaceInstance, String> {
    let pm = crate::local_services::persona_manager()?;
    let store = pm.store();
    let binding = store
        .get_workspace_binding(persona_id)?
        .ok_or_else(|| format!("Workspace binding for persona '{}' not found", persona_id))?;

    let persistence =
        LocalWorkspacePersistence::from_workspace(&binding.workdir, &binding.workspace_id);
    if let (Ok(spec), Ok(state), Ok(history), Ok(provider_state)) = (
        persistence.load_workspace_spec(),
        persistence.load_workspace_state(),
        persistence.load_events(),
        persistence.load_provider_state(),
    ) {
        if let Some(metadata) = provider_state
            .metadata
            .and_then(|value| serde_json::from_value::<CtenoWorkspaceMetadata>(value).ok())
        {
            return WorkspaceInstance::from_parts(
                &binding.template_id,
                spec,
                state,
                history,
                BootstrappedWorkspace {
                    workspace_persona_id: metadata.workspace_persona_id,
                    workspace_session_id: metadata.workspace_session_id,
                    roles: metadata.roles,
                },
                metadata.orchestrator_state,
            );
        }
    }

    if let Ok(adapter) = CtenoWorkspaceAdapter::restore_from_local(
        &binding.workdir,
        &binding.workspace_id,
        CtenoWorkspaceProvisioner,
        CtenoSessionMessenger,
    ) {
        let bootstrapped = adapter
            .bootstrapped()
            .cloned()
            .ok_or_else(|| "restored workspace is missing bootstrapped metadata".to_string())?;
        return WorkspaceInstance::from_parts(
            &binding.template_id,
            adapter.runtime().spec().clone(),
            adapter.snapshot(),
            adapter.history().to_vec(),
            bootstrapped,
            None,
        );
    }

    let persona = store
        .get_persona(persona_id)?
        .ok_or_else(|| format!("Workspace persona '{}' not found", persona_id))?;
    let member_links = store.list_member_sessions(persona_id)?;

    let template = workspace_template_by_id(&binding.template_id)
        .ok_or_else(|| format!("Unsupported workspace template '{}'", binding.template_id))?;
    let instance = WorkspaceInstanceParams {
        id: binding.workspace_id.clone(),
        name: persona.name.clone(),
        cwd: Some(binding.workdir.clone()),
    };
    let profile = WorkspaceProfile {
        provider: multi_agent_protocol::MultiAgentProvider::Cteno,
        model: binding.model.clone(),
        permission_mode: Some(multi_agent_protocol::PermissionMode::AcceptEdits),
        role_edit_permission_mode: Some(multi_agent_protocol::PermissionMode::AcceptEdits),
        setting_sources: Some(vec![multi_agent_protocol::SettingSource::Project]),
        allowed_tools: None,
        disallowed_tools: None,
    };

    let mut adapter = CtenoWorkspaceAdapter::from_template(
        &template,
        &instance,
        &profile,
        CtenoWorkspaceProvisioner,
        CtenoSessionMessenger,
    );

    let bootstrapped = BootstrappedWorkspace {
        workspace_persona_id: persona.id.clone(),
        workspace_session_id: persona.chat_session_id.clone(),
        roles: member_links
            .into_iter()
            .filter_map(|member| {
                let role_id = member.label?;
                Some(multi_agent_runtime_cteno::ProvisionedRole {
                    agent_id: member.agent_type.unwrap_or_else(|| {
                        build_workspace_agent_id(&binding.workspace_id, &role_id)
                    }),
                    role_id,
                    session_id: member.session_id,
                })
            })
            .collect(),
    };

    adapter
        .restore_existing(bootstrapped.clone())
        .map_err(|e| e.to_string())?;

    let state = adapter.snapshot();
    let history = adapter.history().to_vec();
    WorkspaceInstance::from_parts(
        &binding.template_id,
        adapter.runtime().spec().clone(),
        state,
        history,
        bootstrapped,
        None,
    )
}

pub async fn record_workspace_member_response(
    session_id: &str,
    response_text: &str,
    success: bool,
) {
    let registry = workspace_instance_registry();
    let mut instances = registry.lock().await;
    for instance in instances.values_mut() {
        if !instance.has_role_session(session_id) {
            continue;
        }
        let Some(role_id) = instance.role_id_for_session(session_id) else {
            break;
        };
        if let Err(error) = instance
            .on_role_completed(&role_id, response_text, success)
            .await
        {
            log::warn!(
                "[MultiAgent] Failed to process member response for session {}: {}",
                session_id,
                error
            );
        }
        break;
    }
}

fn live_runtime_summary(persona_id: &str) -> Option<WorkspaceRuntimeSummary> {
    let registry = workspace_instance_registry();
    let instances = registry.try_lock().ok()?;
    let instance = instances.get(persona_id)?;
    let state = instance.snapshot();
    let recent_events = instance
        .history()
        .iter()
        .rev()
        .take(24)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    let recent_activities = state
        .activities
        .iter()
        .rev()
        .take(24)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    Some(WorkspaceRuntimeSummary {
        state,
        recent_activities,
        recent_events,
    })
}

#[async_trait]
impl WorkspaceProvisioner for CtenoWorkspaceProvisioner {
    async fn prepare_workspace_layout(&self, spec: &WorkspaceSpec) -> Result<(), AdapterError> {
        let workdir = spec.cwd.as_deref().unwrap_or("~");
        let workspace_root = PathBuf::from(shellexpand::tilde(workdir).to_string());

        std::fs::create_dir_all(&workspace_root).map_err(|e| {
            AdapterError::Provisioning(format!("Failed to create workspace root: {}", e))
        })?;

        for role in &spec.roles {
            if let Some(output_root) = &role.output_root {
                let path = workspace_root.join(output_root);
                std::fs::create_dir_all(&path).map_err(|e| {
                    AdapterError::Provisioning(format!(
                        "Failed to create role output directory {}: {}",
                        path.display(),
                        e
                    ))
                })?;
            }
        }

        if let Some(artifacts) = &spec.artifacts {
            for artifact in artifacts {
                let path = workspace_root.join(&artifact.path);
                std::fs::create_dir_all(&path).map_err(|e| {
                    AdapterError::Provisioning(format!(
                        "Failed to create artifact directory {}: {}",
                        path.display(),
                        e
                    ))
                })?;
            }
        }

        Ok(())
    }

    async fn create_workspace_persona(
        &self,
        spec: &WorkspaceSpec,
    ) -> Result<(String, String), AdapterError> {
        let pm = crate::local_services::persona_manager().map_err(|e| {
            AdapterError::Provisioning(format!("persona manager unavailable: {}", e))
        })?;
        let runtime_ctx = crate::local_services::agent_runtime_context().map_err(|e| {
            AdapterError::Provisioning(format!("agent runtime context unavailable: {}", e))
        })?;

        let workdir = spec.cwd.as_deref().unwrap_or("~");
        let effective_profile_id = resolve_workspace_profile_id(&spec.model).await;
        let persona = pm
            .create_persona(
                &spec.name,
                spec.orchestrator_prompt
                    .as_deref()
                    .unwrap_or("Multi-agent workspace orchestrator"),
                &spec.model,
                None,
                Some(&effective_profile_id),
                Some("cteno"),
                Some(workdir),
            )
            .map_err(AdapterError::Provisioning)?;

        let session_id = uuid::Uuid::new_v4().to_string();
        upsert_local_agent_session_context(
            &session_id,
            "persona",
            workdir,
            Some(&effective_profile_id),
            None,
            Some("cteno"),
            spec.permission_mode,
        )
        .map_err(AdapterError::Provisioning)?;

        pm.store()
            .update_chat_session_id(&persona.id, &session_id)
            .map_err(AdapterError::Provisioning)?;

        cteno_host_bridge_localrpc::set_session_kind_label(
            session_id.clone(),
            crate::agent_kind::agent_kind_label(&crate::agent_kind::AgentKind::Persona),
        )
        .await;

        Ok((persona.id, session_id))
    }

    async fn create_role_agent(
        &self,
        spec: &WorkspaceSpec,
        role: &multi_agent_protocol::RoleSpec,
    ) -> Result<String, AdapterError> {
        let workdir = spec.cwd.as_deref().unwrap_or("~");
        let agent_id = build_workspace_agent_id(&spec.id, &role.id);
        let agent_dir = workspace_agent_dir(workdir, &agent_id);

        let spec = workspace_role_agent_file_spec(spec, role);
        crate::custom_agent_fs::write_custom_agent_dir(&agent_dir, &spec)
            .map_err(AdapterError::Provisioning)?;

        Ok(agent_id)
    }

    async fn spawn_role_session(
        &self,
        spec: &WorkspaceSpec,
        role: &multi_agent_protocol::RoleSpec,
        agent_id: &str,
        workspace_persona_id: &str,
    ) -> Result<String, AdapterError> {
        let pm = crate::local_services::persona_manager().map_err(|e| {
            AdapterError::Provisioning(format!("persona manager unavailable: {}", e))
        })?;

        let workspace_persona = pm
            .store()
            .get_persona(workspace_persona_id)
            .map_err(AdapterError::Provisioning)?
            .ok_or_else(|| {
                AdapterError::Provisioning(format!(
                    "workspace persona {} not found",
                    workspace_persona_id
                ))
            })?;

        let effective_profile_id = resolve_workspace_profile_id(&spec.model).await;
        let requested_vendor = requested_role_executor_vendor(spec, role);
        let session_id = uuid::Uuid::new_v4().to_string();
        upsert_local_agent_session_context(
            &session_id,
            agent_id,
            spec.cwd.as_deref().unwrap_or(&workspace_persona.workdir),
            Some(&effective_profile_id),
            Some(&workspace_persona.chat_session_id),
            Some(requested_vendor),
            role.agent.permission_mode.or(spec.permission_mode),
        )
        .map_err(AdapterError::Provisioning)?;

        if requested_vendor != "cteno" {
            let (
                _agent_config,
                effective_system_prompt,
                _api_key,
                _base_url,
                model,
                temperature,
                _max_tokens,
                _use_proxy,
                _api_format,
                _supports_vision,
                _enable_thinking,
                _supports_function_calling,
                _tools,
                _all_agents,
                _runtime_context_messages,
            ) = local_role_execution_input(&session_id, &LocalWorkspaceExecutionOptions::default())
                .await
                .map_err(AdapterError::Provisioning)?;
            let spawn_spec = build_workspace_executor_spawn_spec(
                &session_id,
                requested_vendor,
                effective_system_prompt,
                model,
                temperature,
            );
            if let Err(error) =
                spawn_workspace_executor_session(&session_id, requested_vendor, spawn_spec).await
            {
                log::warn!(
                    "[Workspace {} role {}] Executor vendor '{}' unavailable ({}), falling back to cteno",
                    spec.id,
                    role.id,
                    requested_vendor,
                    error
                );
                let runtime_ctx = crate::local_services::agent_runtime_context().map_err(|e| {
                    AdapterError::Provisioning(format!("agent runtime context unavailable: {}", e))
                })?;
                let manager =
                    crate::agent_session::AgentSessionManager::new(runtime_ctx.db_path.clone());
                manager
                    .set_vendor(&session_id, "cteno")
                    .map_err(AdapterError::Provisioning)?;
            }
        }

        cteno_host_bridge_localrpc::set_session_kind_label(
            session_id.clone(),
            crate::agent_kind::agent_kind_label(&crate::agent_kind::AgentKind::Custom(
                agent_id.to_string(),
            )),
        )
        .await;

        let link = PersonaSessionLink {
            persona_id: workspace_persona_id.to_string(),
            session_id: session_id.clone(),
            session_type: PersonaSessionType::Member,
            task_description: Some(format!("{} role member", role.name)),
            agent_type: Some(agent_id.to_string()),
            owner_kind: "persona".to_string(),
            label: Some(role.id.clone()),
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        pm.store()
            .link_session(&link)
            .map_err(AdapterError::Provisioning)?;

        Ok(session_id)
    }

    async fn cleanup_workspace(
        &self,
        spec: &WorkspaceSpec,
        bootstrapped: &BootstrappedWorkspace,
    ) -> Result<(), AdapterError> {
        let pm = crate::local_services::persona_manager().map_err(|e| {
            AdapterError::Provisioning(format!("persona manager unavailable: {}", e))
        })?;
        let store = pm.store();
        let member_links = store
            .list_member_sessions(&bootstrapped.workspace_persona_id)
            .map_err(AdapterError::Provisioning)?;

        kill_workspace_sessions(&bootstrapped.workspace_session_id, &member_links).await;
        delete_workspace_agent_dirs(spec)?;
        Ok(())
    }
}

#[async_trait]
impl SessionMessenger for CtenoSessionMessenger {
    async fn send_to_session(
        &self,
        session_id: &str,
        message: &str,
    ) -> Result<(), OrchestratorError> {
        let session_id = session_id.to_string();
        let message = message.to_string();
        tokio::spawn(async move {
            match execute_local_workspace_session(session_id.clone(), message).await {
                Ok(response_text) => {
                    record_workspace_member_response(&session_id, &response_text, true).await;
                }
                Err(error) => {
                    record_workspace_member_response(&session_id, &error, false).await;
                }
            }
        });
        Ok(())
    }

    async fn request_response(
        &self,
        session_id: &str,
        message: &str,
        mode: SessionRequestMode,
    ) -> Result<String, OrchestratorError> {
        let options = match mode {
            SessionRequestMode::Work => LocalWorkspaceExecutionOptions::default(),
            SessionRequestMode::CoordinatorDecision => LocalWorkspaceExecutionOptions {
                disable_tools: true,
                profile_or_model_override: Some("deepseek-chat".to_string()),
            },
            SessionRequestMode::Claim | SessionRequestMode::WorkflowVote => {
                LocalWorkspaceExecutionOptions {
                    disable_tools: true,
                    profile_or_model_override: Some("deepseek-chat".to_string()),
                }
            }
        };
        execute_local_workspace_session_with_options(
            session_id.to_string(),
            message.to_string(),
            options,
        )
        .await
        .map_err(OrchestratorError::Messaging)
    }
}

fn build_workspace_agent_id(workspace_id: &str, role_id: &str) -> String {
    format!(
        "{}-{}-{}",
        WORKSPACE_AGENT_PREFIX,
        slugify(workspace_id),
        slugify(role_id)
    )
}

fn workspace_agent_dir(workdir: &str, agent_id: &str) -> std::path::PathBuf {
    let expanded = shellexpand::tilde(workdir).to_string();
    std::path::PathBuf::from(expanded)
        .join(".cteno")
        .join("agents")
        .join(agent_id)
}

fn workspace_role_agent_file_spec(
    spec: &WorkspaceSpec,
    role: &multi_agent_protocol::RoleSpec,
) -> crate::custom_agent_fs::CustomAgentFileSpec {
    let mut instructions = String::new();
    instructions.push_str(&format!("# {}\n\n", role.name));
    instructions.push_str(&role.agent.prompt);
    instructions.push_str("\n\n");
    instructions.push_str(&format!(
        "You are the `{}` role inside the workspace `{}`.\n",
        role.id, spec.name
    ));
    if let Some(ref output_root) = role.output_root {
        instructions.push_str(&format!(
            "Prefer writing your deliverables under `{}` unless the task requires a different path.\n",
            output_root
        ));
    }
    if let Some(ref initial_prompt) = role.agent.initial_prompt {
        instructions.push_str("\nDefault collaboration guidance:\n");
        instructions.push_str(initial_prompt);
        instructions.push('\n');
    }

    crate::custom_agent_fs::CustomAgentFileSpec {
        name: role.name.clone(),
        description: role.agent.description.clone(),
        instructions,
        model: Some(
            role.agent
                .model
                .clone()
                .unwrap_or_else(|| spec.model.clone()),
        ),
        tools: role.agent.tools.clone().map(|tools| {
            tools
                .into_iter()
                .map(|tool| map_runtime_tool_to_cteno_tool(&tool))
                .collect()
        }),
        skills: role.agent.skills.clone(),
        allowed_tools: None,
        excluded_tools: role.agent.disallowed_tools.clone().map(|tools| {
            tools
                .into_iter()
                .map(|tool| map_runtime_tool_to_cteno_tool(&tool))
                .collect()
        }),
        permission_mode: role
            .agent
            .permission_mode
            .map(permission_mode_name)
            .map(|value| value.to_string()),
    }
}

fn map_runtime_tool_to_cteno_tool(tool: &str) -> String {
    match tool.trim() {
        "Read" | "read" => "read".to_string(),
        "Write" | "write" => "write".to_string(),
        "Edit" | "edit" => "edit".to_string(),
        "Glob" | "glob" => "glob".to_string(),
        "Grep" | "grep" => "grep".to_string(),
        "Shell" | "shell" => "shell".to_string(),
        "WebSearch" | "websearch" | "web_search" => "websearch".to_string(),
        "WebFetch" | "Fetch" | "fetch" | "web_fetch" => "fetch".to_string(),
        "ComputerUse" | "computer_use" => "computer_use".to_string(),
        "BrowserNavigate" | "browser_navigate" => "browser_navigate".to_string(),
        "BrowserAction" | "browser_action" => "browser_action".to_string(),
        "BrowserManage" | "browser_manage" => "browser_manage".to_string(),
        "BrowserNetwork" | "browser_network" => "browser_network".to_string(),
        "BrowserCDP" | "browser_cdp" => "browser_cdp".to_string(),
        "Wait" | "wait" => "wait".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

fn persist_workspace_binding(
    spec: &WorkspaceSpec,
    template_id: &str,
    bootstrapped: &BootstrappedWorkspace,
) -> Result<(), AdapterError> {
    let pm = crate::local_services::persona_manager()
        .map_err(|e| AdapterError::Provisioning(format!("persona manager unavailable: {}", e)))?;
    let now = chrono::Utc::now().to_rfc3339();
    let binding = WorkspaceBinding {
        persona_id: bootstrapped.workspace_persona_id.clone(),
        workspace_id: spec.id.clone(),
        template_id: template_id.to_string(),
        provider: provider_name(spec.provider).to_string(),
        default_role_id: spec.default_role_id.clone(),
        model: spec.model.clone(),
        workdir: spec.cwd.clone().unwrap_or_else(|| "~".to_string()),
        created_at: now.clone(),
        updated_at: now,
    };
    pm.store()
        .upsert_workspace_binding(&binding)
        .map_err(AdapterError::Provisioning)
}

fn provider_name(provider: multi_agent_protocol::MultiAgentProvider) -> &'static str {
    match provider {
        multi_agent_protocol::MultiAgentProvider::ClaudeAgentSdk => "claude-agent-sdk",
        multi_agent_protocol::MultiAgentProvider::CodexSdk => "codex-sdk",
        multi_agent_protocol::MultiAgentProvider::Cteno => "cteno",
    }
}

async fn kill_workspace_sessions(chat_session_id: &str, member_links: &[PersonaSessionLink]) {
    let Ok(runtime_ctx) = crate::local_services::agent_runtime_context() else {
        return;
    };
    let manager = crate::agent_session::AgentSessionManager::new(runtime_ctx.db_path.clone());

    let mut session_ids = Vec::with_capacity(member_links.len() + 1);
    session_ids.push(chat_session_id.to_string());
    session_ids.extend(member_links.iter().map(|member| member.session_id.clone()));

    for session_id in session_ids {
        if let Some(flag) = workspace_abort_registry().lock().await.remove(&session_id) {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(live_session) = remove_workspace_executor_session(&session_id).await {
            if let Err(error) = live_session
                .executor
                .close_session(&live_session.session_ref)
                .await
            {
                log::warn!(
                    "[Workspace session {}] executor.close_session({}) failed: {}",
                    session_id,
                    live_session.session_ref.vendor,
                    error
                );
            }
        }
        let _ = manager.close_session(&session_id);
        cteno_host_bridge_localrpc::remove_session_kind_label(&session_id).await;
    }
}

fn delete_workspace_agent_dirs(spec: &WorkspaceSpec) -> Result<(), AdapterError> {
    let workdir = spec.cwd.as_deref().unwrap_or("~");
    for role in &spec.roles {
        let agent_id = build_workspace_agent_id(&spec.id, &role.id);
        let agent_dir = workspace_agent_dir(workdir, &agent_id);
        crate::custom_agent_fs::delete_custom_agent_dir(&agent_dir)
            .map_err(AdapterError::Provisioning)?;
    }
    Ok(())
}

fn member_to_summary(member: PersonaSessionLink) -> WorkspaceMemberSummary {
    WorkspaceMemberSummary {
        role_id: member.label,
        session_id: member.session_id,
        agent_id: member.agent_type,
        task_description: member.task_description,
        created_at: member.created_at,
    }
}

fn build_local_model_identity_context(
    model: &str,
    supports_vision: bool,
    supports_computer_use: bool,
) -> String {
    format!(
        "<system-reminder>\nModel: {}\nVision support: {}\nComputer-use support: {}\n</system-reminder>",
        model,
        if supports_vision { "yes" } else { "no" },
        if supports_computer_use { "yes" } else { "no" }
    )
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

fn permission_mode_name(mode: multi_agent_protocol::PermissionMode) -> &'static str {
    match mode {
        multi_agent_protocol::PermissionMode::Default => "default",
        multi_agent_protocol::PermissionMode::AcceptEdits => "acceptEdits",
        multi_agent_protocol::PermissionMode::Plan => "plan",
        // Cteno does not currently expose a distinct `dontAsk` mode. Map it
        // to the closest existing behavior so workspace role agents still boot.
        multi_agent_protocol::PermissionMode::DontAsk => "bypassPermissions",
        multi_agent_protocol::PermissionMode::BypassPermissions => "bypassPermissions",
    }
}

fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_dash = false;
    for ch in input.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };
        if normalized == '-' {
            if !prev_dash && !out.is_empty() {
                out.push('-');
            }
            prev_dash = true;
        } else {
            out.push(normalized);
            prev_dash = false;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use multi_agent_protocol::{
        PermissionMode as RuntimePermissionMode, RoleAgentSpec, RoleSpec, WorkspaceSpec,
    };

    use super::*;

    #[test]
    fn workspace_agent_id_is_stable_and_safe() {
        assert_eq!(
            build_workspace_agent_id("Coding Studio 1", "PRD"),
            "group-coding-studio-1-prd"
        );
    }

    #[test]
    fn workspace_role_agent_file_spec_includes_frontmatter_and_guidance() {
        let spec = WorkspaceSpec {
            id: "coding-studio".to_string(),
            name: "Coding Studio".to_string(),
            provider: multi_agent_protocol::MultiAgentProvider::Cteno,
            model: "gpt-5".to_string(),
            cwd: Some("/tmp/demo".to_string()),
            orchestrator_prompt: None,
            claim_policy: None,
            activity_policy: None,
            workflow_vote_policy: None,
            workflow: None,
            artifacts: None,
            completion_policy: None,
            allowed_tools: None,
            disallowed_tools: None,
            permission_mode: None,
            setting_sources: None,
            roles: vec![],
            default_role_id: Some("prd".to_string()),
            coordinator_role_id: None,
        };
        let role = RoleSpec {
            id: "prd".to_string(),
            name: "PRD".to_string(),
            description: None,
            direct: None,
            output_root: Some("10-prd/".to_string()),
            agent: RoleAgentSpec {
                description: "Writes PRDs".to_string(),
                prompt: "Produce concrete product specs.".to_string(),
                provider: None,
                tools: Some(vec!["Read".to_string(), "Write".to_string()]),
                disallowed_tools: None,
                model: None,
                skills: Some(vec!["flow".to_string()]),
                mcp_servers: None,
                initial_prompt: Some("Coordinate with PM before finalizing scope.".to_string()),
                permission_mode: Some(RuntimePermissionMode::AcceptEdits),
            },
        };

        let rendered = crate::custom_agent_fs::render_custom_agent_markdown(
            &workspace_role_agent_file_spec(&spec, &role),
        );
        assert!(rendered.contains("tools: [\"Read\", \"Write\"]"));
        assert!(rendered.contains("skills: [\"flow\"]"));
        assert!(rendered.contains("permission_mode: \"acceptEdits\""));
        assert!(rendered.contains("Prefer writing your deliverables under `10-prd/`"));
    }

    #[test]
    fn detects_workspace_custom_agents_by_group_prefix() {
        assert!(is_workspace_custom_agent(
            &crate::agent_kind::AgentKind::Custom("group-demo-prd".to_string())
        ));
        assert!(!is_workspace_custom_agent(
            &crate::agent_kind::AgentKind::Custom("custom-researcher".to_string())
        ));
        assert!(!is_workspace_custom_agent(
            &crate::agent_kind::AgentKind::Worker
        ));
    }
}
