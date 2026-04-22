use std::collections::BTreeMap;

use chrono::Utc;
use multi_agent_protocol::{
    MemberStatus, TaskDispatch, WorkspaceActivity, WorkspaceMember, WorkspaceSpec,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceShell {
    pub spec: WorkspaceSpec,
    pub members: BTreeMap<String, WorkspaceMember>,
    pub activities: Vec<WorkspaceActivity>,
    pub dispatches: Vec<TaskDispatch>,
}

impl WorkspaceShell {
    pub fn new(spec: WorkspaceSpec) -> Self {
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
            spec,
            members,
            activities: Vec::new(),
            dispatches: Vec::new(),
        }
    }

    pub fn register_member(&mut self, member: WorkspaceMember) -> Option<WorkspaceMember> {
        self.members.insert(member.member_id.clone(), member)
    }

    pub fn record_activity(&mut self, activity: WorkspaceActivity) {
        self.activities.push(activity);
    }

    pub fn record_dispatch(&mut self, dispatch: TaskDispatch) {
        if let Some(existing) = self
            .dispatches
            .iter_mut()
            .find(|existing| existing.dispatch_id == dispatch.dispatch_id)
        {
            *existing = dispatch;
            return;
        }

        self.dispatches.push(dispatch);
    }

    pub fn update_member_status(
        &mut self,
        member_id: &str,
        status: MemberStatus,
    ) -> Option<MemberStatus> {
        self.members.get_mut(member_id).map(|member| {
            let previous = member.status;
            member.status = status;
            member.last_activity_at = Some(now());
            previous
        })
    }

    pub fn snapshot(&self) -> Self {
        self.clone()
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use multi_agent_protocol::{
        DispatchStatus, MultiAgentProvider, PermissionMode, RoleAgentSpec, RoleSpec, TaskDispatch,
        WorkspaceActivity, WorkspaceActivityKind, WorkspaceVisibility,
    };
    use uuid::Uuid;

    use super::*;

    fn sample_spec() -> WorkspaceSpec {
        WorkspaceSpec {
            id: "workspace-1".to_string(),
            name: "Test Workspace".to_string(),
            provider: MultiAgentProvider::Cteno,
            model: "gpt-5.4".to_string(),
            cwd: Some("/tmp/demo".to_string()),
            orchestrator_prompt: None,
            allowed_tools: None,
            disallowed_tools: None,
            permission_mode: Some(PermissionMode::default()),
            setting_sources: None,
            roles: vec![RoleSpec {
                id: "coder".to_string(),
                name: "Coder".to_string(),
                description: None,
                direct: Some(true),
                output_root: Some("40-code/".to_string()),
                agent: RoleAgentSpec {
                    description: "Writes code".to_string(),
                    prompt: "Implement changes".to_string(),
                    tools: None,
                    disallowed_tools: None,
                    model: None,
                    skills: None,
                    mcp_servers: None,
                    initial_prompt: None,
                    permission_mode: None,
                },
            }],
            default_role_id: Some("coder".to_string()),
            coordinator_role_id: Some("coder".to_string()),
            claim_policy: None,
            activity_policy: None,
            workflow_vote_policy: None,
            workflow: None,
            artifacts: None,
            completion_policy: None,
        }
    }

    #[test]
    fn new_initializes_members_from_roles() {
        let shell = WorkspaceShell::new(sample_spec());

        let member = shell.members.get("coder").expect("member should exist");
        assert_eq!(member.workspace_id, shell.spec.id);
        assert_eq!(member.role_name, "Coder");
        assert_eq!(member.status, MemberStatus::Idle);
        assert!(shell.activities.is_empty());
        assert!(shell.dispatches.is_empty());
    }

    #[test]
    fn record_methods_and_snapshot_preserve_state() {
        let mut shell = WorkspaceShell::new(sample_spec());
        let dispatch_id = Uuid::new_v4();

        shell.record_activity(WorkspaceActivity {
            activity_id: Uuid::new_v4(),
            workspace_id: shell.spec.id.clone(),
            kind: WorkspaceActivityKind::SystemNotice,
            visibility: WorkspaceVisibility::Public,
            text: "Workspace ready".to_string(),
            created_at: now(),
            role_id: None,
            member_id: None,
            dispatch_id: None,
            task_id: None,
        });
        shell.record_dispatch(TaskDispatch {
            dispatch_id,
            workspace_id: shell.spec.id.clone(),
            role_id: "coder".to_string(),
            instruction: "Implement feature".to_string(),
            summary: Some("Initial summary".to_string()),
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
        });
        shell.record_dispatch(TaskDispatch {
            dispatch_id,
            workspace_id: shell.spec.id.clone(),
            role_id: "coder".to_string(),
            instruction: "Implement feature".to_string(),
            summary: Some("Updated summary".to_string()),
            visibility: Some(WorkspaceVisibility::Public),
            source_role_id: None,
            workflow_node_id: None,
            stage_id: None,
            status: DispatchStatus::Running,
            provider_task_id: Some("task-1".to_string()),
            tool_use_id: None,
            created_at: now(),
            started_at: Some(now()),
            completed_at: None,
            output_file: None,
            last_summary: Some("In progress".to_string()),
            result_text: None,
            claimed_by_member_ids: None,
            claim_status: None,
        });

        let previous = shell.update_member_status("coder", MemberStatus::Active);
        let snapshot = shell.snapshot();

        assert_eq!(previous, Some(MemberStatus::Idle));
        assert_eq!(snapshot.activities.len(), 1);
        assert_eq!(snapshot.dispatches.len(), 1);
        assert_eq!(snapshot.dispatches[0].status, DispatchStatus::Running);
        assert_eq!(snapshot.members["coder"].status, MemberStatus::Active);
        assert!(snapshot.members["coder"].last_activity_at.is_some());
    }
}
