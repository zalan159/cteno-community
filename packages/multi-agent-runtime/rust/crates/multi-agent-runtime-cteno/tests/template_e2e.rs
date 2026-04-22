use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use multi_agent_protocol::{
    DispatchStatus, WorkspaceEvent, WorkspaceInstanceParams, WorkspaceTemplate,
    create_autoresearch_template, create_claude_workspace_profile, create_codex_workspace_profile,
    create_coding_studio_template, create_opc_solo_company_template,
};
use multi_agent_runtime_cteno::{
    AdapterError, CtenoWorkspaceAdapter, OrchestratorError, SessionMessenger, SessionRequestMode,
    WorkspaceProvisioner,
};

#[derive(Clone, Default)]
struct FakeProvisioner {
    calls: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl WorkspaceProvisioner for FakeProvisioner {
    async fn prepare_workspace_layout(
        &self,
        spec: &multi_agent_protocol::WorkspaceSpec,
    ) -> Result<(), AdapterError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("prepare:{}", spec.id));
        Ok(())
    }

    async fn create_workspace_persona(
        &self,
        spec: &multi_agent_protocol::WorkspaceSpec,
    ) -> Result<(String, String), AdapterError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("persona:{}", spec.id));
        Ok(("persona-1".to_string(), "session-main".to_string()))
    }

    async fn create_role_agent(
        &self,
        _spec: &multi_agent_protocol::WorkspaceSpec,
        role: &multi_agent_protocol::RoleSpec,
    ) -> Result<String, AdapterError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("agent:{}", role.id));
        Ok(format!("agent-{}", role.id))
    }

    async fn spawn_role_session(
        &self,
        _spec: &multi_agent_protocol::WorkspaceSpec,
        role: &multi_agent_protocol::RoleSpec,
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
        spec: &multi_agent_protocol::WorkspaceSpec,
        _bootstrapped: &multi_agent_runtime_cteno::BootstrappedWorkspace,
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
        let role_id = session_id.strip_prefix("session-").unwrap_or(session_id);
        let response = match mode {
            SessionRequestMode::Work => format!("handled by {}", role_id),
            SessionRequestMode::Claim => {
                if matches!(role_id, "prd" | "finance" | "scout" | "lead") {
                    format!(
                        r#"{{"decision":"claim","confidence":0.92,"rationale":"{} is the best owner"}} "#,
                        role_id
                    )
                } else {
                    r#"{"decision":"decline","confidence":0.35,"rationale":"not the best fit"}"#
                        .to_string()
                }
            }
            SessionRequestMode::WorkflowVote => {
                r#"{"decision":"approve","confidence":0.88,"rationale":"workflow is appropriate"}"#
                    .to_string()
            }
            SessionRequestMode::CoordinatorDecision => {
                if role_id == "pm" {
                    r#"{"decision":"delegate","summary":"delegate to the strongest claimant","role_ids":["prd"],"needs_workflow_vote":false}"#.to_string()
                } else if role_id == "ceo" {
                    r#"{"decision":"delegate","summary":"delegate to finance","role_ids":["finance"],"needs_workflow_vote":false}"#.to_string()
                } else {
                    r#"{"decision":"propose_workflow","summary":"start the research workflow","needs_workflow_vote":true}"#.to_string()
                }
            }
        };
        Ok(response.trim().to_string())
    }
}

#[tokio::test]
async fn coding_template_e2e_dispatch_lifecycle() {
    run_template_dispatch_flow(
        create_coding_studio_template(),
        create_claude_workspace_profile(None),
        "prd",
        "Create a PRD for group mentions",
        Some(6),
    )
    .await;
}

#[tokio::test]
async fn opc_template_e2e_dispatch_lifecycle() {
    run_template_dispatch_flow(
        create_opc_solo_company_template(),
        create_claude_workspace_profile(None),
        "finance",
        "Prepare a monthly close checklist",
        Some(5),
    )
    .await;
}

#[tokio::test]
async fn autoresearch_template_e2e_dispatch_lifecycle() {
    run_template_dispatch_flow(
        create_autoresearch_template(),
        create_codex_workspace_profile(None),
        "scout",
        "Research how @mentions work in collaboration tools",
        Some(4),
    )
    .await;
}

async fn run_template_dispatch_flow(
    template: WorkspaceTemplate,
    profile: multi_agent_protocol::WorkspaceProfile,
    role_id: &str,
    instruction: &str,
    expected_role_count: Option<usize>,
) {
    let provisioner = FakeProvisioner::default();
    let messenger = FakeMessenger::default();
    let sent = messenger.sent.clone();

    let instance = WorkspaceInstanceParams {
        id: format!("{}-workspace", template.template_id),
        name: format!("{} Workspace", template.template_name),
        cwd: Some("/tmp/template-e2e".to_string()),
    };

    let mut adapter = CtenoWorkspaceAdapter::from_template(
        &template,
        &instance,
        &profile,
        provisioner,
        messenger,
    );

    let bootstrap_events = adapter.bootstrap().await.expect("bootstrap should succeed");
    assert!(
        bootstrap_events
            .iter()
            .any(|event| matches!(event, WorkspaceEvent::WorkspaceStarted { .. }))
    );
    assert!(
        bootstrap_events
            .iter()
            .any(|event| matches!(event, WorkspaceEvent::WorkspaceInitialized { .. }))
    );

    if let Some(expected_role_count) = expected_role_count {
        assert_eq!(
            adapter.bootstrapped().unwrap().roles.len(),
            expected_role_count,
            "expected bootstrapped role count to match template",
        );
    }

    let (dispatch, queued_events) = adapter
        .assign_role_task(multi_agent_protocol::RoleTaskRequest {
            role_id: role_id.to_string(),
            instruction: instruction.to_string(),
            summary: Some(format!("summary for {}", role_id)),
            visibility: None,
            source_role_id: None,
            workflow_node_id: None,
            stage_id: None,
        })
        .await
        .expect("assign role task should succeed");

    assert_eq!(dispatch.role_id, role_id);
    assert!(!queued_events.is_empty());
    assert!(matches!(
        queued_events.first(),
        Some(WorkspaceEvent::DispatchQueued { .. })
    ));
    let expect_claimed_event = matches!(
        adapter
            .runtime()
            .spec()
            .claim_policy
            .as_ref()
            .map(|policy| policy.mode),
        Some(
            multi_agent_protocol::ClaimMode::Direct
                | multi_agent_protocol::ClaimMode::CoordinatorOnly
        )
    );
    assert_eq!(
        queued_events
            .iter()
            .any(|event| matches!(event, WorkspaceEvent::DispatchClaimed { .. })),
        expect_claimed_event
    );

    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, format!("session-{}", role_id));
    assert!(sent[0].1.contains("Current task for you:"));
    assert!(sent[0].1.contains(instruction));
    drop(sent);

    let start_events = adapter
        .start_provider_task(
            &format!("provider-task-{}", role_id),
            "provider task started",
            Some(format!("tool-{}", role_id)),
        )
        .expect("start provider task should succeed");
    assert!(matches!(
        start_events.first(),
        Some(WorkspaceEvent::DispatchStarted { .. })
    ));

    let progress_events = adapter
        .progress_provider_task(
            &format!("provider-task-{}", role_id),
            "provider task progress",
            Some("halfway there".to_string()),
            Some("Bash".to_string()),
        )
        .expect("progress provider task should succeed");
    assert!(matches!(
        progress_events.first(),
        Some(WorkspaceEvent::DispatchProgress { .. })
    ));

    let complete_events = adapter
        .complete_provider_task(
            &format!("provider-task-{}", role_id),
            DispatchStatus::Completed,
            Some(format!("{}/result.md", role_id)),
            "provider task completed",
            Some(format!("final result for {}", role_id)),
        )
        .await
        .expect("complete provider task should succeed");

    assert_eq!(complete_events.len(), 3);
    assert!(matches!(
        complete_events.first(),
        Some(WorkspaceEvent::DispatchCompleted { .. })
    ));
    assert!(
        complete_events
            .iter()
            .any(|event| matches!(event, WorkspaceEvent::ActivityPublished { .. }))
    );
    assert!(
        complete_events
            .iter()
            .any(|event| matches!(event, WorkspaceEvent::DispatchResult { .. }))
    );

    let snapshot = adapter.runtime().snapshot();
    let stored_dispatch = snapshot
        .dispatches
        .get(&dispatch.dispatch_id)
        .expect("dispatch should exist in runtime snapshot");
    let expected_provider_task_id = format!("provider-task-{}", role_id);
    let expected_result_text = format!("final result for {}", role_id);
    assert_eq!(stored_dispatch.status, DispatchStatus::Completed);
    assert_eq!(
        stored_dispatch.provider_task_id.as_deref(),
        Some(expected_provider_task_id.as_str())
    );
    assert_eq!(
        stored_dispatch.result_text.as_deref(),
        Some(expected_result_text.as_str())
    );
}
