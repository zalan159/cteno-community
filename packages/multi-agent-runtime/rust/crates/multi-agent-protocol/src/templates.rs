use serde::{Deserialize, Serialize};

use crate::{
    ActivityPolicy, ClaimMode, ClaimPolicy, CompletionPolicy, CompletionStatus, MultiAgentProvider,
    PermissionMode, RoleAgentSpec, RoleSpec, SettingSource, WorkflowArtifactKind,
    WorkflowArtifactSpec, WorkflowEdgeCondition, WorkflowEdgeSpec, WorkflowMode, WorkflowNodeSpec,
    WorkflowNodeType, WorkflowSpec, WorkflowStageSpec, WorkflowVotePolicy, WorkspaceSpec,
    WorkspaceVisibility,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    Read,
    Write,
    Edit,
    Glob,
    Grep,
    Shell,
    WebFetch,
    WebSearch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TemplateRoleAgentSpec {
    pub description: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<MultiAgentProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<AgentCapability>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_edit_access: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TemplateRoleSpec {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_root: Option<String>,
    pub agent: TemplateRoleAgentSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTemplate {
    pub template_id: String,
    pub template_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_policy: Option<ClaimPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_policy: Option<ActivityPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_vote_policy: Option<WorkflowVotePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<WorkflowArtifactSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_policy: Option<CompletionPolicy>,
    pub roles: Vec<TemplateRoleSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInstanceParams {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProfile {
    pub provider: MultiAgentProvider,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_edit_permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<Vec<SettingSource>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
}

pub fn create_claude_workspace_profile(model: Option<&str>) -> WorkspaceProfile {
    WorkspaceProfile {
        provider: MultiAgentProvider::ClaudeAgentSdk,
        model: model.unwrap_or("claude-sonnet-4-5").to_string(),
        permission_mode: Some(PermissionMode::AcceptEdits),
        role_edit_permission_mode: Some(PermissionMode::AcceptEdits),
        setting_sources: Some(vec![SettingSource::Project]),
        allowed_tools: None,
        disallowed_tools: None,
    }
}

pub fn create_codex_workspace_profile(model: Option<&str>) -> WorkspaceProfile {
    WorkspaceProfile {
        provider: MultiAgentProvider::CodexSdk,
        model: model.unwrap_or("gpt-5.1-codex-mini").to_string(),
        permission_mode: None,
        role_edit_permission_mode: None,
        setting_sources: None,
        allowed_tools: None,
        disallowed_tools: None,
    }
}

pub fn instantiate_workspace(
    template: &WorkspaceTemplate,
    instance: &WorkspaceInstanceParams,
    profile: &WorkspaceProfile,
) -> WorkspaceSpec {
    let roles = template
        .roles
        .iter()
        .map(|role| instantiate_role(role, profile))
        .collect::<Vec<_>>();

    let derived_allowed_tools = unique_strings(
        roles
            .iter()
            .flat_map(|role| role.agent.tools.clone().unwrap_or_default())
            .collect(),
    );

    WorkspaceSpec {
        id: instance.id.clone(),
        name: instance.name.clone(),
        provider: profile.provider,
        model: profile.model.clone(),
        cwd: instance.cwd.clone(),
        orchestrator_prompt: template.orchestrator_prompt.clone(),
        allowed_tools: profile
            .allowed_tools
            .clone()
            .or_else(|| (!derived_allowed_tools.is_empty()).then_some(derived_allowed_tools)),
        disallowed_tools: profile.disallowed_tools.clone(),
        permission_mode: profile.permission_mode,
        setting_sources: profile.setting_sources.clone(),
        roles,
        default_role_id: template.default_role_id.clone(),
        coordinator_role_id: template.coordinator_role_id.clone(),
        claim_policy: template.claim_policy.clone(),
        activity_policy: template.activity_policy.clone(),
        workflow_vote_policy: template.workflow_vote_policy.clone(),
        workflow: template.workflow.clone(),
        artifacts: template.artifacts.clone(),
        completion_policy: template.completion_policy.clone(),
    }
}

fn node(id: &str, node_type: WorkflowNodeType) -> WorkflowNodeSpec {
    WorkflowNodeSpec {
        id: id.to_string(),
        node_type,
        title: None,
        role_id: None,
        reviewer_role_id: None,
        candidate_role_ids: None,
        command: None,
        evaluator: None,
        prompt: None,
        timeout_ms: None,
        retry: None,
        requires_artifacts: None,
        produces_artifacts: None,
        visibility: None,
        stage_id: None,
    }
}

fn edge(from: &str, to: &str, when: WorkflowEdgeCondition) -> WorkflowEdgeSpec {
    WorkflowEdgeSpec {
        from: from.to_string(),
        to: to.to_string(),
        when,
    }
}

fn artifact(
    id: &str,
    kind: WorkflowArtifactKind,
    path: &str,
    owner_role_id: Option<&str>,
    description: &str,
) -> WorkflowArtifactSpec {
    WorkflowArtifactSpec {
        id: id.to_string(),
        kind,
        path: path.to_string(),
        owner_role_id: owner_role_id.map(str::to_string),
        required: Some(true),
        description: Some(description.to_string()),
    }
}

fn instantiate_role(role: &TemplateRoleSpec, profile: &WorkspaceProfile) -> RoleSpec {
    RoleSpec {
        id: role.id.clone(),
        name: role.name.clone(),
        description: role.description.clone(),
        direct: role.direct,
        output_root: role.output_root.clone(),
        agent: RoleAgentSpec {
            description: role.agent.description.clone(),
            prompt: role.agent.prompt.clone(),
            provider: role.agent.provider,
            tools: role
                .agent
                .capabilities
                .clone()
                .map(|capabilities| map_capabilities(&capabilities))
                .filter(|mapped| !mapped.is_empty()),
            disallowed_tools: None,
            model: role.agent.model.clone(),
            skills: role.agent.skills.clone(),
            mcp_servers: None,
            initial_prompt: role.agent.initial_prompt.clone(),
            permission_mode: if role.agent.requires_edit_access.unwrap_or(false) {
                profile.role_edit_permission_mode
            } else {
                None
            },
        },
    }
}

fn map_capabilities(capabilities: &[AgentCapability]) -> Vec<String> {
    unique_strings(
        capabilities
            .iter()
            .map(|capability| match capability {
                AgentCapability::Read => "Read",
                AgentCapability::Write => "Write",
                AgentCapability::Edit => "Edit",
                AgentCapability::Glob => "Glob",
                AgentCapability::Grep => "Grep",
                AgentCapability::Shell => "Bash",
                AgentCapability::WebFetch => "WebFetch",
                AgentCapability::WebSearch => "WebSearch",
            })
            .map(str::to_string)
            .collect(),
    )
}

fn unique_strings(values: Vec<String>) -> Vec<String> {
    let mut values = values;
    values.sort();
    values.dedup();
    values
}

pub fn create_coding_studio_template() -> WorkspaceTemplate {
    WorkspaceTemplate {
        template_id: "coding-studio".to_string(),
        template_name: "Coding Studio".to_string(),
        description: Some("A software delivery workspace with fixed specialist roles.".to_string()),
        default_role_id: Some("pm".to_string()),
        coordinator_role_id: Some("pm".to_string()),
        orchestrator_prompt: Some(
            "You are the orchestrator for a software delivery workspace. Keep the team aligned, route work to the correct role agent, and summarize progress crisply.".to_string(),
        ),
        claim_policy: Some(ClaimPolicy {
            mode: ClaimMode::Claim,
            claim_timeout_ms: Some(30000),
            max_assignees: Some(1),
            allow_supporting_claims: Some(true),
            fallback_role_id: Some("pm".to_string()),
        }),
        activity_policy: Some(ActivityPolicy {
            publish_user_messages: Some(true),
            publish_coordinator_messages: Some(true),
            publish_dispatch_lifecycle: Some(true),
            publish_member_messages: Some(true),
            default_visibility: Some(WorkspaceVisibility::Public),
        }),
        workflow_vote_policy: Some(WorkflowVotePolicy {
            timeout_ms: Some(30_000),
            minimum_approvals: Some(1),
            required_approval_ratio: Some(1),
            candidate_role_ids: None,
        }),
        workflow: Some(WorkflowSpec {
            mode: WorkflowMode::ReviewLoop,
            entry_node_id: "claim_scope".to_string(),
            stages: Some(vec![
                WorkflowStageSpec {
                    id: "scope".to_string(),
                    name: "Scope".to_string(),
                    description: Some("Claim the request, draft the PRD, and get it accepted.".to_string()),
                    entry_node_id: Some("claim_scope".to_string()),
                    exit_node_ids: Some(vec!["review_prd".to_string()]),
                },
                WorkflowStageSpec {
                    id: "delivery".to_string(),
                    name: "Delivery".to_string(),
                    description: Some("Design, implement, test, and review the change.".to_string()),
                    entry_node_id: Some("architecture".to_string()),
                    exit_node_ids: Some(vec!["release_review".to_string(), "complete".to_string()]),
                },
            ]),
            nodes: vec![
                WorkflowNodeSpec {
                    candidate_role_ids: Some(vec!["pm".to_string(), "prd".to_string()]),
                    title: Some("Broadcast request and collect claim".to_string()),
                    stage_id: Some("scope".to_string()),
                    ..node("claim_scope", WorkflowNodeType::Claim)
                },
                WorkflowNodeSpec {
                    role_id: Some("prd".to_string()),
                    title: Some("Draft PRD".to_string()),
                    produces_artifacts: Some(vec!["prd_doc".to_string()]),
                    stage_id: Some("scope".to_string()),
                    ..node("draft_prd", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    reviewer_role_id: Some("reviewer".to_string()),
                    title: Some("Review PRD".to_string()),
                    requires_artifacts: Some(vec!["prd_doc".to_string()]),
                    stage_id: Some("scope".to_string()),
                    ..node("review_prd", WorkflowNodeType::Review)
                },
                WorkflowNodeSpec {
                    role_id: Some("architect".to_string()),
                    title: Some("Create architecture plan".to_string()),
                    requires_artifacts: Some(vec!["prd_doc".to_string()]),
                    produces_artifacts: Some(vec!["arch_doc".to_string()]),
                    stage_id: Some("delivery".to_string()),
                    ..node("architecture", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    role_id: Some("coder".to_string()),
                    title: Some("Implement change".to_string()),
                    requires_artifacts: Some(vec!["prd_doc".to_string(), "arch_doc".to_string()]),
                    produces_artifacts: Some(vec!["code_change".to_string()]),
                    stage_id: Some("delivery".to_string()),
                    ..node("implement", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    role_id: Some("tester".to_string()),
                    title: Some("Run validation".to_string()),
                    requires_artifacts: Some(vec!["code_change".to_string()]),
                    produces_artifacts: Some(vec!["test_report".to_string()]),
                    stage_id: Some("delivery".to_string()),
                    ..node("test", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    reviewer_role_id: Some("reviewer".to_string()),
                    title: Some("Final release review".to_string()),
                    requires_artifacts: Some(vec!["prd_doc".to_string(), "arch_doc".to_string(), "test_report".to_string()]),
                    stage_id: Some("delivery".to_string()),
                    ..node("release_review", WorkflowNodeType::Review)
                },
                WorkflowNodeSpec {
                    title: Some("Finish delivery".to_string()),
                    stage_id: Some("delivery".to_string()),
                    ..node("complete", WorkflowNodeType::Complete)
                },
            ],
            edges: vec![
                edge("claim_scope", "draft_prd", WorkflowEdgeCondition::Success),
                edge("draft_prd", "review_prd", WorkflowEdgeCondition::Success),
                edge("review_prd", "architecture", WorkflowEdgeCondition::Approved),
                edge("review_prd", "draft_prd", WorkflowEdgeCondition::Rejected),
                edge("architecture", "implement", WorkflowEdgeCondition::Success),
                edge("implement", "test", WorkflowEdgeCondition::Success),
                edge("test", "release_review", WorkflowEdgeCondition::Pass),
                edge("test", "implement", WorkflowEdgeCondition::Fail),
                edge("release_review", "complete", WorkflowEdgeCondition::Approved),
                edge("release_review", "implement", WorkflowEdgeCondition::Rejected),
            ],
        }),
        artifacts: Some(vec![
            artifact("prd_doc", WorkflowArtifactKind::Doc, "10-prd/", Some("prd"), "Implementation-ready PRD markdown."),
            artifact("arch_doc", WorkflowArtifactKind::Doc, "30-arch/", Some("architect"), "Architecture and interface notes."),
            artifact("code_change", WorkflowArtifactKind::Code, "40-code/", Some("coder"), "Code changes required to satisfy the request."),
            artifact("test_report", WorkflowArtifactKind::Report, "50-test/", Some("tester"), "Verification evidence and residual risks."),
        ]),
        completion_policy: Some(CompletionPolicy {
            success_node_ids: Some(vec!["complete".to_string()]),
            failure_node_ids: Some(vec![]),
            max_iterations: Some(8),
            default_status: Some(CompletionStatus::Stuck),
        }),
        roles: vec![
            TemplateRoleSpec {
                id: "pm".to_string(),
                name: "PM".to_string(),
                description: None,
                direct: None,
                output_root: Some("00-management/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Plans scope, sequencing, and acceptance criteria.".to_string(),
                    prompt: "You are a product/project manager. Clarify scope, break work into milestones, and keep handoffs explicit. Prefer concise plans with acceptance criteria.".to_string(),
                    provider: None,
                    capabilities: Some(vec![AgentCapability::Read, AgentCapability::Glob, AgentCapability::Grep]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: None,
                },
            },
            TemplateRoleSpec {
                id: "prd".to_string(),
                name: "PRD".to_string(),
                description: None,
                direct: None,
                output_root: Some("10-prd/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Writes product requirement docs and task definitions.".to_string(),
                    prompt: "You write implementation-ready PRDs. Always produce a concrete markdown deliverable instead of notes. Include explicit sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria, and make the content specific enough for downstream implementation.".to_string(),
                    provider: None,
                    capabilities: Some(vec![
                        AgentCapability::Read,
                        AgentCapability::Write,
                        AgentCapability::Edit,
                        AgentCapability::Glob,
                        AgentCapability::Grep,
                    ]),
                    model: None,
                    skills: None,
                    initial_prompt: Some(
                        "Default PRD contract: write the deliverable under `10-prd/` unless the task gives another file path. Use these exact markdown section headings: `## Goal`, `## User Story`, `## Scope`, `## Non-Goals`, `## Acceptance Criteria`. Do not stop at an overview.".to_string(),
                    ),
                    requires_edit_access: None,
                },
            },
            TemplateRoleSpec {
                id: "architect".to_string(),
                name: "Architect".to_string(),
                description: None,
                direct: None,
                output_root: Some("30-arch/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Designs implementation plans and system changes.".to_string(),
                    prompt: "You are a software architect. Produce pragmatic design notes, data flow decisions, interfaces, and risks before coding starts.".to_string(),
                    provider: None,
                    capabilities: Some(vec![
                        AgentCapability::Read,
                        AgentCapability::Write,
                        AgentCapability::Edit,
                        AgentCapability::Glob,
                        AgentCapability::Grep,
                    ]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: None,
                },
            },
            TemplateRoleSpec {
                id: "coder".to_string(),
                name: "Coder".to_string(),
                description: None,
                direct: None,
                output_root: Some("40-code/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Implements code changes and keeps diffs focused.".to_string(),
                    prompt: "You are an implementation specialist. Make the requested change with minimal churn, explain assumptions briefly, and keep code consistent with the repository style.".to_string(),
                    provider: None,
                    capabilities: Some(vec![
                        AgentCapability::Read,
                        AgentCapability::Write,
                        AgentCapability::Edit,
                        AgentCapability::Glob,
                        AgentCapability::Grep,
                        AgentCapability::Shell,
                    ]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: Some(true),
                },
            },
            TemplateRoleSpec {
                id: "tester".to_string(),
                name: "Tester".to_string(),
                description: None,
                direct: None,
                output_root: Some("50-test/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Runs tests, validates behavior, and reports regressions.".to_string(),
                    prompt: "You are a verification specialist. Run the narrowest useful checks first, surface failures clearly, and report residual risks if full coverage is not possible.".to_string(),
                    provider: None,
                    capabilities: Some(vec![
                        AgentCapability::Read,
                        AgentCapability::Write,
                        AgentCapability::Edit,
                        AgentCapability::Glob,
                        AgentCapability::Grep,
                        AgentCapability::Shell,
                    ]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: None,
                },
            },
            TemplateRoleSpec {
                id: "reviewer".to_string(),
                name: "Reviewer".to_string(),
                description: None,
                direct: None,
                output_root: Some("60-review/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Reviews changes for bugs, regressions, and missing tests.".to_string(),
                    prompt: "You perform code review with a bug-finding mindset. Prioritize correctness, regressions, and missing validation over style commentary.".to_string(),
                    provider: None,
                    capabilities: Some(vec![AgentCapability::Read, AgentCapability::Glob, AgentCapability::Grep]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: None,
                },
            },
        ],
    }
}

pub fn create_opc_solo_company_template() -> WorkspaceTemplate {
    WorkspaceTemplate {
        template_id: "opc-solo-company".to_string(),
        template_name: "OPC Solo Company".to_string(),
        description: Some("A one-person company staffed by specialist digital operators.".to_string()),
        default_role_id: Some("ceo".to_string()),
        coordinator_role_id: Some("ceo".to_string()),
        orchestrator_prompt: Some(
            "You orchestrate a one-person company staffed by specialist digital operators. Route work to the best role, keep recommendations practical, and prefer concrete operating documents over abstract advice.".to_string(),
        ),
        claim_policy: Some(ClaimPolicy {
            mode: ClaimMode::CoordinatorOnly,
            claim_timeout_ms: None,
            max_assignees: Some(1),
            allow_supporting_claims: Some(false),
            fallback_role_id: Some("ceo".to_string()),
        }),
        activity_policy: Some(ActivityPolicy {
            publish_user_messages: Some(true),
            publish_coordinator_messages: Some(true),
            publish_dispatch_lifecycle: Some(true),
            publish_member_messages: Some(true),
            default_visibility: Some(WorkspaceVisibility::Public),
        }),
        workflow_vote_policy: Some(WorkflowVotePolicy {
            timeout_ms: Some(30_000),
            minimum_approvals: Some(1),
            required_approval_ratio: Some(1),
            candidate_role_ids: None,
        }),
        workflow: Some(WorkflowSpec {
            mode: WorkflowMode::Pipeline,
            entry_node_id: "intake".to_string(),
            stages: Some(vec![
                WorkflowStageSpec {
                    id: "intake".to_string(),
                    name: "Intake".to_string(),
                    description: Some("CEO frames the request and routes it to the right operator.".to_string()),
                    entry_node_id: Some("intake".to_string()),
                    exit_node_ids: Some(vec!["route_specialist".to_string()]),
                },
                WorkflowStageSpec {
                    id: "operations".to_string(),
                    name: "Operations".to_string(),
                    description: Some("Specialist operators prepare concrete operating artifacts.".to_string()),
                    entry_node_id: Some("route_specialist".to_string()),
                    exit_node_ids: Some(vec!["ceo_review".to_string(), "complete".to_string()]),
                },
            ]),
            nodes: vec![
                WorkflowNodeSpec {
                    role_id: Some("ceo".to_string()),
                    title: Some("Frame request and operating goal".to_string()),
                    stage_id: Some("intake".to_string()),
                    ..node("intake", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    title: Some("Route to specialist".to_string()),
                    candidate_role_ids: Some(vec![
                        "finance".to_string(),
                        "tax".to_string(),
                        "admin".to_string(),
                        "recruiter".to_string(),
                    ]),
                    stage_id: Some("operations".to_string()),
                    ..node("route_specialist", WorkflowNodeType::Claim)
                },
                WorkflowNodeSpec {
                    role_id: Some("finance".to_string()),
                    title: Some("Prepare finance deliverable".to_string()),
                    produces_artifacts: Some(vec!["finance_doc".to_string()]),
                    stage_id: Some("operations".to_string()),
                    ..node("finance_work", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    role_id: Some("tax".to_string()),
                    title: Some("Prepare tax deliverable".to_string()),
                    produces_artifacts: Some(vec!["tax_doc".to_string()]),
                    stage_id: Some("operations".to_string()),
                    ..node("tax_work", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    role_id: Some("admin".to_string()),
                    title: Some("Prepare admin deliverable".to_string()),
                    produces_artifacts: Some(vec!["admin_doc".to_string()]),
                    stage_id: Some("operations".to_string()),
                    ..node("admin_work", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    role_id: Some("recruiter".to_string()),
                    title: Some("Prepare recruiting deliverable".to_string()),
                    produces_artifacts: Some(vec!["recruit_doc".to_string()]),
                    stage_id: Some("operations".to_string()),
                    ..node("recruit_work", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    reviewer_role_id: Some("ceo".to_string()),
                    title: Some("CEO review and approve".to_string()),
                    stage_id: Some("operations".to_string()),
                    ..node("ceo_review", WorkflowNodeType::Review)
                },
                WorkflowNodeSpec {
                    title: Some("Finish operating workflow".to_string()),
                    stage_id: Some("operations".to_string()),
                    ..node("complete", WorkflowNodeType::Complete)
                },
            ],
            edges: vec![
                edge("intake", "route_specialist", WorkflowEdgeCondition::Success),
                edge("route_specialist", "finance_work", WorkflowEdgeCondition::Success),
                edge("route_specialist", "tax_work", WorkflowEdgeCondition::Success),
                edge("route_specialist", "admin_work", WorkflowEdgeCondition::Success),
                edge("route_specialist", "recruit_work", WorkflowEdgeCondition::Success),
                edge("finance_work", "ceo_review", WorkflowEdgeCondition::Success),
                edge("tax_work", "ceo_review", WorkflowEdgeCondition::Success),
                edge("admin_work", "ceo_review", WorkflowEdgeCondition::Success),
                edge("recruit_work", "ceo_review", WorkflowEdgeCondition::Success),
                edge("ceo_review", "complete", WorkflowEdgeCondition::Approved),
                edge("ceo_review", "route_specialist", WorkflowEdgeCondition::Rejected),
            ],
        }),
        artifacts: Some(vec![
            artifact("finance_doc", WorkflowArtifactKind::Report, "company/10-finance/", Some("finance"), "Finance checklist, budget, or operating summary."),
            artifact("tax_doc", WorkflowArtifactKind::Report, "company/20-tax/", Some("tax"), "Tax filing checklist or compliance note."),
            artifact("admin_doc", WorkflowArtifactKind::Doc, "company/30-admin/", Some("admin"), "Administrative SOP or operator checklist."),
            artifact("recruit_doc", WorkflowArtifactKind::Doc, "company/40-recruiting/", Some("recruiter"), "Hiring brief, scorecard, or interview plan."),
        ]),
        completion_policy: Some(CompletionPolicy {
            success_node_ids: Some(vec!["complete".to_string()]),
            failure_node_ids: Some(vec![]),
            max_iterations: Some(4),
            default_status: Some(CompletionStatus::Stuck),
        }),
        roles: vec![
            role_with_capabilities(
                "ceo",
                "CEO",
                "company/00-ceo/",
                "Owns priorities, approvals, and operating decisions for the solo company.",
                "You are the CEO of a one-person software company. Frame decisions clearly, make tradeoffs explicit, and turn fuzzy requests into concrete next actions.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "finance",
                "Finance",
                "company/10-finance/",
                "Prepares cash, revenue, budget, and monthly operating documents.",
                "You are a finance operator for a lean software business. Produce concise, audit-friendly checklists, budgets, and summaries with concrete assumptions and numbers where possible.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "tax",
                "Tax",
                "company/20-tax/",
                "Prepares filing checklists, tax calendars, and compliance notes.",
                "You are a tax operations specialist for a solo company. Focus on deadlines, supporting documents, risks, and what needs accountant review.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "admin",
                "Admin",
                "company/30-admin/",
                "Handles administrative SOPs, vendor coordination, and internal operations.",
                "You are an operations administrator. Turn messy business tasks into checklists, SOPs, and lightweight systems that a solo founder can actually maintain.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "recruiter",
                "Recruiter",
                "company/40-recruiting/",
                "Drafts hiring briefs, scorecards, and interview plans when the company needs help.",
                "You are a pragmatic recruiting partner for a small company. Keep hiring materials specific, lightweight, and aligned with the business stage.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
        ],
    }
}

pub fn create_autoresearch_template() -> WorkspaceTemplate {
    WorkspaceTemplate {
        template_id: "autoresearch".to_string(),
        template_name: "Autoresearch".to_string(),
        description: Some("A research-oriented workspace for scouting and synthesis.".to_string()),
        default_role_id: Some("lead".to_string()),
        coordinator_role_id: Some("lead".to_string()),
        orchestrator_prompt: Some(
            "You orchestrate an autonomous research workspace. Keep hypotheses explicit, separate evidence from interpretation, and favor compact research artifacts that can feed later evaluation loops.".to_string(),
        ),
        claim_policy: Some(ClaimPolicy {
            mode: ClaimMode::Claim,
            claim_timeout_ms: Some(30000),
            max_assignees: Some(2),
            allow_supporting_claims: Some(true),
            fallback_role_id: Some("lead".to_string()),
        }),
        activity_policy: Some(ActivityPolicy {
            publish_user_messages: Some(true),
            publish_coordinator_messages: Some(true),
            publish_dispatch_lifecycle: Some(true),
            publish_member_messages: Some(true),
            default_visibility: Some(WorkspaceVisibility::Public),
        }),
        workflow_vote_policy: Some(WorkflowVotePolicy {
            timeout_ms: Some(30_000),
            minimum_approvals: Some(1),
            required_approval_ratio: Some(1),
            candidate_role_ids: None,
        }),
        workflow: Some(WorkflowSpec {
            mode: WorkflowMode::Loop,
            entry_node_id: "frame_hypothesis".to_string(),
            stages: Some(vec![
                WorkflowStageSpec {
                    id: "research".to_string(),
                    name: "Research".to_string(),
                    description: Some("Frame hypothesis, gather evidence, and design the next experiment.".to_string()),
                    entry_node_id: Some("frame_hypothesis".to_string()),
                    exit_node_ids: Some(vec!["decide_outcome".to_string()]),
                },
                WorkflowStageSpec {
                    id: "iteration".to_string(),
                    name: "Iteration".to_string(),
                    description: Some("Keep improvements, discard regressions, and loop.".to_string()),
                    entry_node_id: Some("decide_outcome".to_string()),
                    exit_node_ids: Some(vec!["loop_next".to_string(), "discard".to_string()]),
                },
            ]),
            nodes: vec![
                WorkflowNodeSpec {
                    role_id: Some("lead".to_string()),
                    title: Some("Frame the current hypothesis".to_string()),
                    produces_artifacts: Some(vec!["research_brief".to_string()]),
                    stage_id: Some("research".to_string()),
                    ..node("frame_hypothesis", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    candidate_role_ids: Some(vec!["scout".to_string(), "critic".to_string()]),
                    title: Some("Claim evidence gathering".to_string()),
                    stage_id: Some("research".to_string()),
                    ..node("claim_evidence", WorkflowNodeType::Claim)
                },
                WorkflowNodeSpec {
                    role_id: Some("scout".to_string()),
                    title: Some("Collect evidence".to_string()),
                    requires_artifacts: Some(vec!["research_brief".to_string()]),
                    produces_artifacts: Some(vec!["evidence_pack".to_string()]),
                    stage_id: Some("research".to_string()),
                    ..node("collect_evidence", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    role_id: Some("experimenter".to_string()),
                    title: Some("Run experiment".to_string()),
                    command: Some("uv run train.py > run.log 2>&1".to_string()),
                    timeout_ms: Some(600000),
                    requires_artifacts: Some(vec!["evidence_pack".to_string()]),
                    produces_artifacts: Some(vec!["run_log".to_string()]),
                    stage_id: Some("research".to_string()),
                    ..node("run_experiment", WorkflowNodeType::Shell)
                },
                WorkflowNodeSpec {
                    title: Some("Evaluate experiment metrics".to_string()),
                    evaluator: Some("parse_autoresearch_run_log".to_string()),
                    requires_artifacts: Some(vec!["run_log".to_string()]),
                    produces_artifacts: Some(vec!["experiment_result".to_string()]),
                    stage_id: Some("research".to_string()),
                    ..node("evaluate_results", WorkflowNodeType::Evaluate)
                },
                WorkflowNodeSpec {
                    reviewer_role_id: Some("lead".to_string()),
                    title: Some("Decide keep or discard".to_string()),
                    requires_artifacts: Some(vec!["experiment_result".to_string()]),
                    stage_id: Some("iteration".to_string()),
                    ..node("decide_outcome", WorkflowNodeType::Review)
                },
                WorkflowNodeSpec {
                    title: Some("Keep winning experiment".to_string()),
                    stage_id: Some("iteration".to_string()),
                    ..node("keep", WorkflowNodeType::Commit)
                },
                WorkflowNodeSpec {
                    title: Some("Discard regression".to_string()),
                    stage_id: Some("iteration".to_string()),
                    ..node("discard", WorkflowNodeType::Revert)
                },
                WorkflowNodeSpec {
                    title: Some("Advance to next iteration".to_string()),
                    retry: Some(crate::WorkflowRetryPolicy { max_attempts: Some(100) }),
                    stage_id: Some("iteration".to_string()),
                    ..node("loop_next", WorkflowNodeType::Loop)
                },
            ],
            edges: vec![
                edge("frame_hypothesis", "claim_evidence", WorkflowEdgeCondition::Success),
                edge("claim_evidence", "collect_evidence", WorkflowEdgeCondition::Success),
                edge("collect_evidence", "run_experiment", WorkflowEdgeCondition::Success),
                edge("run_experiment", "evaluate_results", WorkflowEdgeCondition::Success),
                edge("run_experiment", "discard", WorkflowEdgeCondition::Failure),
                edge("run_experiment", "discard", WorkflowEdgeCondition::Timeout),
                edge("evaluate_results", "keep", WorkflowEdgeCondition::Improved),
                edge("evaluate_results", "discard", WorkflowEdgeCondition::EqualOrWorse),
                edge("evaluate_results", "discard", WorkflowEdgeCondition::Crash),
                edge("keep", "loop_next", WorkflowEdgeCondition::Success),
                edge("discard", "loop_next", WorkflowEdgeCondition::Success),
                edge("loop_next", "frame_hypothesis", WorkflowEdgeCondition::Retry),
            ],
        }),
        artifacts: Some(vec![
            artifact("research_brief", WorkflowArtifactKind::Doc, "research/00-lead/", Some("lead"), "Current hypothesis and success criteria."),
            artifact("evidence_pack", WorkflowArtifactKind::Evidence, "research/10-scout/", Some("scout"), "Evidence pack with cited sources and observations."),
            artifact("run_log", WorkflowArtifactKind::Result, "run.log", Some("experimenter"), "Raw experiment log from the latest run."),
            artifact("experiment_result", WorkflowArtifactKind::Metric, "results.tsv", Some("lead"), "Parsed outcome used to decide keep or discard."),
        ]),
        completion_policy: Some(CompletionPolicy {
            success_node_ids: Some(vec!["keep".to_string()]),
            failure_node_ids: Some(vec!["discard".to_string()]),
            max_iterations: Some(100),
            default_status: Some(CompletionStatus::Done),
        }),
        roles: vec![
            role_with_capabilities(
                "lead",
                "Lead",
                "research/00-lead/",
                "Frames the research question and decides what evidence is worth collecting next.",
                "You are a research lead. Turn vague topics into testable questions, define success criteria, and keep each loop scoped tightly.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "scout",
                "Scout",
                "research/10-scout/",
                "Collects outside signals, references, and raw observations.",
                "You are a web research scout. Gather high-signal evidence, cite sources inline, and keep notes concise enough for downstream synthesis.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                    AgentCapability::WebSearch,
                    AgentCapability::WebFetch,
                ],
            ),
            role_with_capabilities(
                "experimenter",
                "Experimenter",
                "research/20-experiments/",
                "Turns a hypothesis into a measurable experiment design.",
                "You design small, measurable experiments. Define variables, success metrics, instrumentation, and stopping criteria with minimal ceremony.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                    AgentCapability::Shell,
                ],
            ),
            role_with_capabilities(
                "critic",
                "Critic",
                "research/30-critic/",
                "Challenges assumptions, spots confounders, and tightens reasoning.",
                "You are a skeptical research critic. Look for weak evidence, missing controls, and untested assumptions before the team moves on.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
        ],
    }
}

pub fn create_edict_governance_template() -> WorkspaceTemplate {
    WorkspaceTemplate {
        template_id: "edict-governance".to_string(),
        template_name: "Three Departments Six Ministries".to_string(),
        description: Some(
            "A governance-style multi-agent workspace for coordinated planning, review, execution, and oversight."
                .to_string(),
        ),
        default_role_id: Some("shangshu".to_string()),
        coordinator_role_id: Some("shangshu".to_string()),
        orchestrator_prompt: Some(
            "You coordinate a governance-style multi-agent workspace. Keep responsibilities crisp, route work to the right ministry, and enforce review before completion.".to_string(),
        ),
        claim_policy: Some(ClaimPolicy {
            mode: ClaimMode::Claim,
            claim_timeout_ms: Some(30000),
            max_assignees: Some(2),
            allow_supporting_claims: Some(true),
            fallback_role_id: Some("shangshu".to_string()),
        }),
        activity_policy: Some(ActivityPolicy {
            publish_user_messages: Some(true),
            publish_coordinator_messages: Some(true),
            publish_dispatch_lifecycle: Some(true),
            publish_member_messages: Some(true),
            default_visibility: Some(WorkspaceVisibility::Public),
        }),
        workflow_vote_policy: Some(WorkflowVotePolicy {
            timeout_ms: Some(30_000),
            minimum_approvals: Some(1),
            required_approval_ratio: Some(1),
            candidate_role_ids: None,
        }),
        workflow: Some(WorkflowSpec {
            mode: WorkflowMode::ReviewLoop,
            entry_node_id: "draft_order".to_string(),
            stages: Some(vec![
                WorkflowStageSpec {
                    id: "draft".to_string(),
                    name: "Draft".to_string(),
                    description: Some("Draft task order and clear initial review.".to_string()),
                    entry_node_id: Some("draft_order".to_string()),
                    exit_node_ids: Some(vec!["review_order".to_string()]),
                },
                WorkflowStageSpec {
                    id: "execution".to_string(),
                    name: "Execution".to_string(),
                    description: Some("Dispatch ministries, gather outputs, and clear oversight.".to_string()),
                    entry_node_id: Some("coordinate_execution".to_string()),
                    exit_node_ids: Some(vec!["final_review".to_string(), "complete".to_string()]),
                },
            ]),
            nodes: vec![
                WorkflowNodeSpec {
                    role_id: Some("zhongshu".to_string()),
                    title: Some("Draft task order".to_string()),
                    produces_artifacts: Some(vec!["task_order".to_string()]),
                    stage_id: Some("draft".to_string()),
                    ..node("draft_order", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    reviewer_role_id: Some("menxia".to_string()),
                    title: Some("Review task order".to_string()),
                    requires_artifacts: Some(vec!["task_order".to_string()]),
                    stage_id: Some("draft".to_string()),
                    ..node("review_order", WorkflowNodeType::Review)
                },
                WorkflowNodeSpec {
                    role_id: Some("shangshu".to_string()),
                    title: Some("Coordinate ministry execution".to_string()),
                    requires_artifacts: Some(vec!["task_order".to_string()]),
                    stage_id: Some("execution".to_string()),
                    ..node("coordinate_execution", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    candidate_role_ids: Some(vec![
                        "gongbu".to_string(),
                        "hubu".to_string(),
                        "libu".to_string(),
                        "xingbu".to_string(),
                        "bingbu".to_string(),
                    ]),
                    title: Some("Claim specialist ministry work".to_string()),
                    stage_id: Some("execution".to_string()),
                    ..node("claim_ministry", WorkflowNodeType::Claim)
                },
                WorkflowNodeSpec {
                    role_id: Some("gongbu".to_string()),
                    title: Some("Execute implementation work".to_string()),
                    produces_artifacts: Some(vec!["implementation_output".to_string()]),
                    stage_id: Some("execution".to_string()),
                    ..node("implement_work", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    role_id: Some("hubu".to_string()),
                    title: Some("Assess resources and constraints".to_string()),
                    produces_artifacts: Some(vec!["resource_report".to_string()]),
                    stage_id: Some("execution".to_string()),
                    ..node("resource_review", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    role_id: Some("xingbu".to_string()),
                    title: Some("Perform compliance review".to_string()),
                    requires_artifacts: Some(vec!["implementation_output".to_string(), "resource_report".to_string()]),
                    produces_artifacts: Some(vec!["compliance_report".to_string()]),
                    stage_id: Some("execution".to_string()),
                    ..node("compliance_review", WorkflowNodeType::Review)
                },
                WorkflowNodeSpec {
                    role_id: Some("bingbu".to_string()),
                    title: Some("Assess operational readiness".to_string()),
                    produces_artifacts: Some(vec!["ops_plan".to_string()]),
                    stage_id: Some("execution".to_string()),
                    ..node("ops_readiness", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    role_id: Some("libu".to_string()),
                    title: Some("Package communication artifact".to_string()),
                    produces_artifacts: Some(vec!["communication_brief".to_string()]),
                    stage_id: Some("execution".to_string()),
                    ..node("communication", WorkflowNodeType::Assign)
                },
                WorkflowNodeSpec {
                    reviewer_role_id: Some("menxia".to_string()),
                    title: Some("Final review".to_string()),
                    requires_artifacts: Some(vec![
                        "task_order".to_string(),
                        "implementation_output".to_string(),
                        "resource_report".to_string(),
                        "compliance_report".to_string(),
                        "ops_plan".to_string(),
                    ]),
                    stage_id: Some("execution".to_string()),
                    ..node("final_review", WorkflowNodeType::Review)
                },
                WorkflowNodeSpec {
                    title: Some("Close governance workflow".to_string()),
                    stage_id: Some("execution".to_string()),
                    ..node("complete", WorkflowNodeType::Complete)
                },
            ],
            edges: vec![
                edge("draft_order", "review_order", WorkflowEdgeCondition::Success),
                edge("review_order", "coordinate_execution", WorkflowEdgeCondition::Approved),
                edge("review_order", "draft_order", WorkflowEdgeCondition::Rejected),
                edge("coordinate_execution", "claim_ministry", WorkflowEdgeCondition::Success),
                edge("claim_ministry", "implement_work", WorkflowEdgeCondition::Success),
                edge("claim_ministry", "resource_review", WorkflowEdgeCondition::Success),
                edge("implement_work", "compliance_review", WorkflowEdgeCondition::Success),
                edge("resource_review", "compliance_review", WorkflowEdgeCondition::Success),
                edge("compliance_review", "ops_readiness", WorkflowEdgeCondition::Approved),
                edge("compliance_review", "implement_work", WorkflowEdgeCondition::Rejected),
                edge("ops_readiness", "communication", WorkflowEdgeCondition::Success),
                edge("communication", "final_review", WorkflowEdgeCondition::Success),
                edge("final_review", "complete", WorkflowEdgeCondition::Approved),
                edge("final_review", "coordinate_execution", WorkflowEdgeCondition::Rejected),
            ],
        }),
        artifacts: Some(vec![
            artifact("task_order", WorkflowArtifactKind::TaskOrder, "governance/10-zhongshu/", Some("zhongshu"), "Mission brief and task order."),
            artifact("implementation_output", WorkflowArtifactKind::Result, "governance/30-gongbu/", Some("gongbu"), "Concrete implementation output."),
            artifact("resource_report", WorkflowArtifactKind::Report, "governance/40-hubu/", Some("hubu"), "Resource and tradeoff report."),
            artifact("compliance_report", WorkflowArtifactKind::Report, "governance/60-xingbu/", Some("xingbu"), "Compliance and quality review."),
            artifact("ops_plan", WorkflowArtifactKind::Doc, "governance/70-bingbu/", Some("bingbu"), "Operational rollout and fallback plan."),
            artifact("communication_brief", WorkflowArtifactKind::Doc, "governance/50-libu/", Some("libu"), "External or internal communication brief."),
        ]),
        completion_policy: Some(CompletionPolicy {
            success_node_ids: Some(vec!["complete".to_string()]),
            failure_node_ids: Some(vec![]),
            max_iterations: Some(6),
            default_status: Some(CompletionStatus::Stuck),
        }),
        roles: vec![
            role_with_capabilities(
                "shangshu",
                "Shangshu",
                "governance/00-shangshu/",
                "Coordinates ministries, sequence, and final closure.",
                "You are the chief coordinator. Route work, close loops, demand concrete outputs, and make sure every major task ends with a clear disposition.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "zhongshu",
                "Zhongshu",
                "governance/10-zhongshu/",
                "Drafts mission briefs, task orders, and structured plans.",
                "You draft precise task orders, plans, and briefs. Convert vague goals into crisp instructions with deliverables and milestones.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "menxia",
                "Menxia",
                "governance/20-menxia/",
                "Reviews proposals, challenges assumptions, and enforces red-team scrutiny.",
                "You are the review gate. Challenge weak reasoning, surface risks, and reject plans that are not yet executable or safe.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            TemplateRoleSpec {
                id: "gongbu".to_string(),
                name: "Gongbu".to_string(),
                description: None,
                direct: None,
                output_root: Some("governance/30-gongbu/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Executes implementation, build-out, and tooling work.".to_string(),
                    prompt: "You are the implementation ministry. Build concrete outputs, keep execution disciplined, and report exact deliverables.".to_string(),
                    provider: None,
                    capabilities: Some(vec![
                        AgentCapability::Read,
                        AgentCapability::Write,
                        AgentCapability::Edit,
                        AgentCapability::Glob,
                        AgentCapability::Grep,
                        AgentCapability::Shell,
                    ]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: Some(true),
                },
            },
            role_with_capabilities(
                "hubu",
                "Hubu",
                "governance/40-hubu/",
                "Tracks resources, budgets, dependencies, and allocation tradeoffs.",
                "You manage resources and constraints. Quantify budget, headcount, token, or time tradeoffs and keep plans grounded in capacity.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "libu",
                "Libu",
                "governance/50-libu/",
                "Prepares communication, docs, release notes, and external-facing materials.",
                "You own communication and documentation. Package decisions and outputs into clear artifacts others can consume quickly.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "xingbu",
                "Xingbu",
                "governance/60-xingbu/",
                "Owns compliance, safety, and rule enforcement.",
                "You enforce quality, compliance, and safety constraints. Flag violations early and insist on auditable fixes.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "bingbu",
                "Bingbu",
                "governance/70-bingbu/",
                "Handles operations, release readiness, incident response, and escalation.",
                "You manage operational readiness. Focus on rollout, incident handling, fallback plans, and postmortem discipline.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                    AgentCapability::Shell,
                ],
            ),
        ],
    }
}

pub fn create_task_gate_coding_manual_template() -> WorkspaceTemplate {
    WorkspaceTemplate {
        template_id: "task-gate-coding-manual".to_string(),
        template_name: "Task Gate Coding (Manual Tasks)".to_string(),
        description: Some(
            "Consume a hand-authored task list, have a coder execute one task at a time, require reviewer approval, and commit approved work items one by one."
                .to_string(),
        ),
        default_role_id: Some("reviewer".to_string()),
        coordinator_role_id: Some("reviewer".to_string()),
        orchestrator_prompt: Some(
            "You coordinate a gated implementation workflow from a task list ledger. Route each task to the coder, require reviewer approval before each commit, and stop on the first unresolved task failure.".to_string(),
        ),
        claim_policy: Some(ClaimPolicy {
            mode: ClaimMode::CoordinatorOnly,
            claim_timeout_ms: None,
            max_assignees: Some(1),
            allow_supporting_claims: Some(false),
            fallback_role_id: Some("reviewer".to_string()),
        }),
        activity_policy: Some(ActivityPolicy {
            publish_user_messages: Some(true),
            publish_coordinator_messages: Some(true),
            publish_dispatch_lifecycle: Some(true),
            publish_member_messages: Some(true),
            default_visibility: Some(WorkspaceVisibility::Public),
        }),
        workflow_vote_policy: Some(WorkflowVotePolicy {
            timeout_ms: Some(30_000),
            minimum_approvals: Some(1),
            required_approval_ratio: Some(1),
            candidate_role_ids: None,
        }),
        workflow: None,
        artifacts: None,
        completion_policy: Some(CompletionPolicy {
            success_node_ids: Some(vec![]),
            failure_node_ids: Some(vec![]),
            max_iterations: Some(8),
            default_status: Some(CompletionStatus::Stuck),
        }),
        roles: vec![
            TemplateRoleSpec {
                id: "reviewer".to_string(),
                name: "Reviewer".to_string(),
                description: Some(
                    "Reviews each completed task against the hand-authored task list before it can be committed."
                        .to_string(),
                ),
                direct: None,
                output_root: Some("00-management/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description:
                        "Reviews each completed task against the hand-authored task list before it can be committed.".to_string(),
                    prompt:
                        "You are the reviewer for a gated coding workflow. Trust the provided task list as the source of truth, inspect the changed files carefully, approve only when the task is fully satisfied, and give concrete rejection feedback when it is not.".to_string(),
                    provider: None,
                    capabilities: Some(vec![
                        AgentCapability::Read,
                        AgentCapability::Write,
                        AgentCapability::Edit,
                        AgentCapability::Glob,
                        AgentCapability::Grep,
                    ]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: None,
                },
            },
            TemplateRoleSpec {
                id: "coder".to_string(),
                name: "Coder".to_string(),
                description: Some(
                    "Implements one approved task at a time and commits it after reviewer approval."
                        .to_string(),
                ),
                direct: None,
                output_root: Some("40-code/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description:
                        "Implements one approved task at a time and commits it after reviewer approval.".to_string(),
                    prompt:
                        "You are the coding agent for a gated workflow. Execute exactly one task at a time, make the smallest complete change that satisfies the task, and commit only after the reviewer approves.".to_string(),
                    provider: None,
                    capabilities: Some(vec![
                        AgentCapability::Read,
                        AgentCapability::Write,
                        AgentCapability::Edit,
                        AgentCapability::Glob,
                        AgentCapability::Grep,
                        AgentCapability::Shell,
                    ]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: Some(true),
                },
            },
        ],
    }
}

fn role_with_capabilities(
    id: &str,
    name: &str,
    output_root: &str,
    description: &str,
    prompt: &str,
    capabilities: Vec<AgentCapability>,
) -> TemplateRoleSpec {
    TemplateRoleSpec {
        id: id.to_string(),
        name: name.to_string(),
        description: None,
        direct: None,
        output_root: Some(output_root.to_string()),
        agent: TemplateRoleAgentSpec {
            description: description.to_string(),
            prompt: prompt.to_string(),
            provider: None,
            capabilities: Some(capabilities),
            model: None,
            skills: None,
            initial_prompt: None,
            requires_edit_access: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instantiate_workspace_applies_profile_and_maps_capabilities() {
        let template = create_coding_studio_template();
        let instance = WorkspaceInstanceParams {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            cwd: Some("/tmp/demo".to_string()),
        };
        let profile = create_claude_workspace_profile(None);

        let spec = instantiate_workspace(&template, &instance, &profile);

        assert_eq!(spec.provider, MultiAgentProvider::ClaudeAgentSdk);
        assert_eq!(spec.model, "claude-sonnet-4-5");
        assert_eq!(spec.default_role_id.as_deref(), Some("pm"));
        assert_eq!(spec.setting_sources, Some(vec![SettingSource::Project]));
        assert_eq!(
            spec.workflow.as_ref().unwrap().mode,
            WorkflowMode::ReviewLoop
        );
        assert!(
            spec.artifacts
                .as_ref()
                .unwrap()
                .iter()
                .any(|artifact| artifact.id == "prd_doc")
        );
        assert_eq!(
            spec.completion_policy
                .as_ref()
                .and_then(|policy| policy.default_status),
            Some(CompletionStatus::Stuck)
        );

        let coder = spec.roles.iter().find(|role| role.id == "coder").unwrap();
        assert_eq!(
            coder.agent.tools.as_ref().unwrap(),
            &vec![
                "Bash".to_string(),
                "Edit".to_string(),
                "Glob".to_string(),
                "Grep".to_string(),
                "Read".to_string(),
                "Write".to_string(),
            ]
        );
        assert_eq!(
            coder.agent.permission_mode,
            Some(PermissionMode::AcceptEdits)
        );
    }

    #[test]
    fn codex_profile_uses_codex_provider_defaults() {
        let template = create_autoresearch_template();
        let instance = WorkspaceInstanceParams {
            id: "research".to_string(),
            name: "Research".to_string(),
            cwd: None,
        };
        let profile = create_codex_workspace_profile(None);

        let spec = instantiate_workspace(&template, &instance, &profile);

        assert_eq!(spec.provider, MultiAgentProvider::CodexSdk);
        assert_eq!(spec.model, "gpt-5.1-codex-mini");
        assert!(
            spec.allowed_tools
                .as_ref()
                .unwrap()
                .contains(&"WebSearch".to_string())
        );
        assert!(
            spec.allowed_tools
                .as_ref()
                .unwrap()
                .contains(&"WebFetch".to_string())
        );
        assert_eq!(spec.workflow.as_ref().unwrap().mode, WorkflowMode::Loop);
    }
}
