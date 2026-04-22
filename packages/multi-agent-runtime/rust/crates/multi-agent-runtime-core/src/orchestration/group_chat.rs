use async_trait::async_trait;
use chrono::Utc;
use multi_agent_protocol::{
    CoordinatorDecisionKind, DispatchStatus, TaskDispatch, WorkspaceActivity,
    WorkspaceActivityKind, WorkspaceTurnRequest, WorkspaceVisibility,
    build_coordinator_decision_prompt, direct_workspace_turn_plan, parse_coordinator_decision,
    resolve_coordinator_role_id,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    OrchestratorError, OrchestratorResponse, SessionMessenger, SessionRequestMode,
    WorkspaceOrchestrator, WorkspaceShell,
};

const ORCHESTRATOR_TYPE: &str = "group_chat";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupChatOrchestrator {
    coordinator_role_id: String,
}

impl GroupChatOrchestrator {
    pub fn new(coordinator_role_id: impl Into<String>) -> Self {
        Self {
            coordinator_role_id: coordinator_role_id.into(),
        }
    }

    fn coordinator_role_id(&self, shell: &WorkspaceShell) -> String {
        if self.coordinator_role_id.trim().is_empty() {
            resolve_coordinator_role_id(&shell.spec)
        } else {
            self.coordinator_role_id.clone()
        }
    }

    fn workspace_turn_request(message: &str, target_role: Option<&str>) -> WorkspaceTurnRequest {
        WorkspaceTurnRequest {
            message: message.to_string(),
            visibility: Some(WorkspaceVisibility::Public),
            max_assignments: Some(1),
            prefer_role_id: target_role.map(ToString::to_string),
        }
    }

    fn create_dispatch(
        &self,
        shell: &WorkspaceShell,
        role_id: &str,
        message: &str,
        summary: Option<String>,
    ) -> Result<TaskDispatch, OrchestratorError> {
        if !shell.members.contains_key(role_id) {
            return Err(OrchestratorError::InvalidState(format!(
                "unknown role: {role_id}"
            )));
        }

        let request = Self::workspace_turn_request(message, Some(role_id));
        let assignment = direct_workspace_turn_plan(&shell.spec, &request, role_id)
            .assignments
            .into_iter()
            .next()
            .ok_or_else(|| {
                OrchestratorError::InvalidState(format!(
                    "no assignment generated for role: {role_id}"
                ))
            })?;
        Ok(TaskDispatch {
            dispatch_id: Uuid::new_v4(),
            workspace_id: shell.spec.id.clone(),
            role_id: role_id.to_string(),
            instruction: assignment.instruction,
            summary: summary.or(assignment.summary),
            visibility: assignment.visibility,
            source_role_id: None,
            workflow_node_id: assignment.workflow_node_id,
            stage_id: assignment.stage_id,
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

    fn create_activity(
        &self,
        shell: &WorkspaceShell,
        kind: WorkspaceActivityKind,
        text: impl Into<String>,
        role_id: Option<&str>,
        dispatch_id: Option<Uuid>,
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
            dispatch_id,
            task_id: None,
        }
    }

    fn session_id_for_role<'a>(
        &self,
        shell: &'a WorkspaceShell,
        role_id: &str,
    ) -> Result<&'a str, OrchestratorError> {
        shell
            .members
            .get(role_id)
            .ok_or_else(|| OrchestratorError::InvalidState(format!("unknown role: {role_id}")))?
            .session_id
            .as_deref()
            .ok_or_else(|| {
                OrchestratorError::InvalidState(format!(
                    "role {role_id} is missing an active session"
                ))
            })
    }
}

impl Default for GroupChatOrchestrator {
    fn default() -> Self {
        Self::new(String::new())
    }
}

#[async_trait]
impl WorkspaceOrchestrator for GroupChatOrchestrator {
    fn orchestrator_type(&self) -> &str {
        ORCHESTRATOR_TYPE
    }

    async fn handle_user_message(
        &mut self,
        shell: &WorkspaceShell,
        messenger: &dyn SessionMessenger,
        message: &str,
        target_role: Option<&str>,
    ) -> Result<OrchestratorResponse, OrchestratorError> {
        let mut activities = vec![self.create_activity(
            shell,
            WorkspaceActivityKind::UserMessage,
            message,
            None,
            None,
        )];
        let mut dispatches = Vec::new();

        if let Some(role_id) = target_role {
            let dispatch = self.create_dispatch(
                shell,
                role_id,
                message,
                Some(format!("Direct request to @{role_id}")),
            )?;
            dispatches.push(dispatch);
            return Ok(OrchestratorResponse {
                activities,
                dispatches,
                template_state: self.template_state(),
            });
        }

        let coordinator_role_id = self.coordinator_role_id(shell);
        let coordinator_session_id = self.session_id_for_role(shell, &coordinator_role_id)?;
        let request = Self::workspace_turn_request(message, None);
        let prompt = build_coordinator_decision_prompt(&shell.spec, &request, None);
        let raw_response = messenger
            .request_response(
                coordinator_session_id,
                &prompt,
                SessionRequestMode::CoordinatorDecision,
            )
            .await?;
        let decision = parse_coordinator_decision(&raw_response, &shell.spec, &request);

        activities.push(self.create_activity(
            shell,
            WorkspaceActivityKind::CoordinatorMessage,
            decision.response_text.clone(),
            Some(&coordinator_role_id),
            None,
        ));

        if decision.kind == CoordinatorDecisionKind::Delegate {
            if let Some(role_id) = decision.target_role_id.as_deref() {
                let dispatch =
                    self.create_dispatch(shell, role_id, message, Some(decision.response_text))?;
                dispatches.push(dispatch);
            }
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
        messenger: &dyn SessionMessenger,
        role_id: &str,
        result: &str,
        success: bool,
    ) -> Result<OrchestratorResponse, OrchestratorError> {
        let activity_kind = if success {
            WorkspaceActivityKind::MemberDelivered
        } else {
            WorkspaceActivityKind::MemberBlocked
        };
        let activity = self.create_activity(shell, activity_kind, result, Some(role_id), None);

        let coordinator_role_id = self.coordinator_role_id(shell);
        if role_id != coordinator_role_id {
            if let Some(coordinator_session_id) = shell
                .members
                .get(&coordinator_role_id)
                .and_then(|member| member.session_id.as_deref())
            {
                let status = if success {
                    "completed"
                } else {
                    "reported a problem with"
                };
                let note =
                    format!("@{role_id} {status} the delegated work.\n\nLatest reply:\n{result}");
                messenger
                    .send_to_session(coordinator_session_id, &note)
                    .await?;
            }
        }

        Ok(OrchestratorResponse {
            activities: vec![activity],
            dispatches: Vec::new(),
            template_state: self.template_state(),
        })
    }

    fn template_state(&self) -> Value {
        json!({ "type": ORCHESTRATOR_TYPE })
    }

    fn serialize_state(&self) -> Value {
        json!({
            "type": ORCHESTRATOR_TYPE,
            "coordinatorRoleId": self.coordinator_role_id,
        })
    }

    fn restore_state(&mut self, state: Value) -> Result<(), OrchestratorError> {
        let Some(object) = state.as_object() else {
            return Err(OrchestratorError::Serialization(
                "group chat state must be a JSON object".to_string(),
            ));
        };

        if let Some(orchestrator_type) = object.get("type").and_then(Value::as_str) {
            if orchestrator_type != ORCHESTRATOR_TYPE {
                return Err(OrchestratorError::Serialization(format!(
                    "expected orchestrator type {ORCHESTRATOR_TYPE}, got {orchestrator_type}"
                )));
            }
        }

        self.coordinator_role_id = object
            .get("coordinatorRoleId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(())
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use multi_agent_protocol::{
        MultiAgentProvider, RoleAgentSpec, RoleSpec, WorkspaceSpec, WorkspaceVisibility,
    };

    use super::*;

    #[derive(Clone, Default)]
    struct FakeMessenger {
        requests: Arc<Mutex<Vec<(String, SessionRequestMode, String)>>>,
        sent: Arc<Mutex<Vec<(String, String)>>>,
        coordinator_response: Arc<Mutex<String>>,
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
                .expect("sent mutex poisoned")
                .push((session_id.to_string(), message.to_string()));
            Ok(())
        }

        async fn request_response(
            &self,
            session_id: &str,
            message: &str,
            mode: SessionRequestMode,
        ) -> Result<String, OrchestratorError> {
            self.requests
                .lock()
                .expect("requests mutex poisoned")
                .push((session_id.to_string(), mode, message.to_string()));
            Ok(self
                .coordinator_response
                .lock()
                .expect("coordinator response mutex poisoned")
                .clone())
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
                    id: "lead".to_string(),
                    name: "Lead".to_string(),
                    description: Some("Coordinates the room".to_string()),
                    direct: Some(true),
                    output_root: Some("40-code/".to_string()),
                    agent: RoleAgentSpec {
                        description: "Coordinates work".to_string(),
                        prompt: "Coordinate".to_string(),
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
                    id: "coder".to_string(),
                    name: "Coder".to_string(),
                    description: Some("Implements code".to_string()),
                    direct: Some(true),
                    output_root: Some("40-code/".to_string()),
                    agent: RoleAgentSpec {
                        description: "Writes code".to_string(),
                        prompt: "Implement".to_string(),
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
            default_role_id: Some("lead".to_string()),
            coordinator_role_id: Some("lead".to_string()),
            claim_policy: None,
            activity_policy: None,
            workflow_vote_policy: None,
            workflow: None,
            artifacts: None,
            completion_policy: None,
        };
        let mut shell = WorkspaceShell::new(spec);
        shell.members.get_mut("lead").unwrap().session_id = Some("session-lead".to_string());
        shell.members.get_mut("coder").unwrap().session_id = Some("session-coder".to_string());
        shell
    }

    #[tokio::test]
    async fn direct_target_dispatch_bypasses_coordinator() {
        let shell = sample_shell();
        let messenger = FakeMessenger::default();
        let mut orchestrator = GroupChatOrchestrator::new("lead");

        let response = orchestrator
            .handle_user_message(&shell, &messenger, "Implement mentions", Some("coder"))
            .await
            .expect("direct dispatch should succeed");

        assert_eq!(response.dispatches.len(), 1);
        assert_eq!(response.dispatches[0].role_id, "coder");
        assert_eq!(response.dispatches[0].status, DispatchStatus::Queued);
        assert_eq!(
            response.dispatches[0].visibility,
            Some(WorkspaceVisibility::Public)
        );
        assert_eq!(response.activities.len(), 1);
        assert_eq!(
            response.activities[0].kind,
            WorkspaceActivityKind::UserMessage
        );
        assert!(
            messenger
                .requests
                .lock()
                .expect("requests mutex poisoned")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn coordinator_reply_delegates_to_selected_role() {
        let shell = sample_shell();
        let messenger = FakeMessenger {
            coordinator_response: Arc::new(Mutex::new(
                r#"{"kind":"delegate","responseText":"@coder will take this next.","targetRoleId":"coder","workflowVoteReason":"","rationale":"coder owns the implementation"}"#.to_string(),
            )),
            ..FakeMessenger::default()
        };
        let mut orchestrator = GroupChatOrchestrator::new("lead");

        let response = orchestrator
            .handle_user_message(&shell, &messenger, "Implement mentions", None)
            .await
            .expect("coordinator dispatch should succeed");

        assert_eq!(response.dispatches.len(), 1);
        assert_eq!(response.dispatches[0].role_id, "coder");
        assert_eq!(response.activities.len(), 2);
        assert_eq!(
            response.activities[1].kind,
            WorkspaceActivityKind::CoordinatorMessage
        );
        assert_eq!(response.activities[1].role_id.as_deref(), Some("lead"));
        let requests = messenger.requests.lock().expect("requests mutex poisoned");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "session-lead");
        assert_eq!(requests[0].1, SessionRequestMode::CoordinatorDecision);
        assert!(requests[0].2.contains("Return strict JSON only."));
    }

    #[tokio::test]
    async fn role_completion_records_activity_and_notifies_coordinator() {
        let shell = sample_shell();
        let messenger = FakeMessenger::default();
        let mut orchestrator = GroupChatOrchestrator::new("lead");

        let response = orchestrator
            .on_role_completed(&shell, &messenger, "coder", "Finished the patch", true)
            .await
            .expect("completion handling should succeed");

        assert_eq!(response.dispatches.len(), 0);
        assert_eq!(response.activities.len(), 1);
        assert_eq!(
            response.activities[0].kind,
            WorkspaceActivityKind::MemberDelivered
        );
        assert_eq!(response.activities[0].role_id.as_deref(), Some("coder"));

        let sent = messenger.sent.lock().expect("sent mutex poisoned");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "session-lead");
        assert!(sent[0].1.contains("@coder completed the delegated work."));
        assert!(sent[0].1.contains("Finished the patch"));
    }
}
