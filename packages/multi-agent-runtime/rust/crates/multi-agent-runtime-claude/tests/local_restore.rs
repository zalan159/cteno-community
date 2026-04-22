use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use multi_agent_protocol::{
    WorkspaceActivity, WorkspaceActivityKind, WorkspaceInstanceParams, WorkspaceMode,
    WorkspaceState, WorkspaceStatus, WorkspaceVisibility, WorkspaceWorkflowRuntimeState,
    create_claude_workspace_profile, create_coding_studio_template, instantiate_workspace,
};
use multi_agent_runtime_claude::{ClaudePermissionMode, ClaudeWorkspace, ClaudeWorkspaceOptions};
use multi_agent_runtime_local::{LocalWorkspacePersistence, PersistedProviderState};

fn temp_workspace_dir(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "multi-agent-runtime-claude-{label}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ))
}

#[test]
fn restores_and_deletes_local_workspace() {
    let cwd = temp_workspace_dir("restore");
    let template = create_coding_studio_template();
    let instance = WorkspaceInstanceParams {
        id: "claude-local-restore".to_string(),
        name: "Claude Local Restore".to_string(),
        cwd: Some(cwd.to_string_lossy().to_string()),
    };
    let profile = create_claude_workspace_profile(None);
    let spec = instantiate_workspace(&template, &instance, &profile);
    let persistence = LocalWorkspacePersistence::from_spec(&spec).unwrap();
    persistence.initialize_workspace(&spec).unwrap();

    let state = WorkspaceState {
        workspace_id: spec.id.clone(),
        status: WorkspaceStatus::Running,
        provider: spec.provider,
        session_id: Some("claude-root-session".to_string()),
        started_at: Some(chrono::Utc::now().to_rfc3339()),
        roles: spec
            .roles
            .iter()
            .cloned()
            .map(|role| (role.id.clone(), role))
            .collect(),
        members: spec
            .roles
            .iter()
            .map(|role| {
                (
                    role.id.clone(),
                    multi_agent_protocol::WorkspaceMember {
                        member_id: role.id.clone(),
                        workspace_id: spec.id.clone(),
                        role_id: role.id.clone(),
                        role_name: role.name.clone(),
                        direct: role.direct,
                        session_id: None,
                        status: multi_agent_protocol::MemberStatus::Idle,
                        public_state_summary: None,
                        last_activity_at: None,
                    },
                )
            })
            .collect(),
        dispatches: Default::default(),
        activities: vec![WorkspaceActivity {
            activity_id: uuid::Uuid::new_v4(),
            workspace_id: spec.id.clone(),
            kind: WorkspaceActivityKind::UserMessage,
            visibility: WorkspaceVisibility::Public,
            text: "hello".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            role_id: None,
            member_id: None,
            dispatch_id: None,
            task_id: None,
        }],
        workflow_runtime: WorkspaceWorkflowRuntimeState {
            mode: WorkspaceMode::GroupChat,
            active_vote_window: None,
            active_request_message: None,
            active_node_id: None,
            active_stage_id: None,
        },
    };
    persistence
        .persist_runtime(
            &state,
            &[],
            &PersistedProviderState {
                workspace_id: spec.id.clone(),
                provider: spec.provider,
                root_conversation_id: Some("claude-root-session".to_string()),
                member_bindings: BTreeMap::new(),
                metadata: None,
                updated_at: chrono::Utc::now().to_rfc3339(),
            },
        )
        .unwrap();

    let mut workspace = ClaudeWorkspace::restore_from_local(
        &cwd,
        &spec.id,
        ClaudeWorkspaceOptions {
            claude_path: "claude".into(),
            permission_mode: ClaudePermissionMode::BypassPermissions,
            working_directory: Some(cwd.clone()),
            additional_directories: Vec::new(),
            turn_timeout: std::time::Duration::from_secs(30),
            max_workflow_followups: 0,
            ..ClaudeWorkspaceOptions::default()
        },
    )
    .unwrap();

    assert_eq!(
        workspace.runtime().snapshot().session_id.as_deref(),
        Some("claude-root-session")
    );
    assert!(workspace.persistence_root().unwrap().exists());

    workspace.delete_workspace().unwrap();
    assert!(!cwd.join(".multi-agent-runtime").join(&spec.id).exists());

    let _ = fs::remove_dir_all(cwd);
}
