use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use multi_agent_protocol::{
    DispatchStatus, WorkspaceActivityKind, WorkspaceInstanceParams, WorkspaceMode,
    WorkspaceTurnRequest, create_autoresearch_template, create_codex_workspace_profile,
    create_coding_studio_template, create_opc_solo_company_template,
};
use multi_agent_runtime_codex::{CodexSandboxMode, CodexWorkspace, CodexWorkspaceOptions};

#[tokio::test]
#[ignore = "requires local Codex CLI auth and a live model run"]
async fn codex_live_workspace_turn_coding_delegates_to_prd() {
    let cwd = create_repo_local_temp_dir("multi-agent-runtime-codex-live-coding");
    fs::create_dir_all(cwd.join("10-prd")).expect("should create temp workspace");

    let mut template = create_coding_studio_template();
    template.orchestrator_prompt = None;
    if let Some(prd_role) = template.roles.iter_mut().find(|role| role.id == "prd") {
        prd_role.agent.prompt =
            "You are a PRD writer. Complete the requested markdown file directly with minimal tool usage and no extra exploration.".to_string();
        prd_role.agent.description = "Writes concise markdown PRDs.".to_string();
    }

    let mut workspace = build_workspace("codex-live-coding", &cwd, template);
    let startup_events = workspace.start();
    assert!(startup_events.len() >= 2);

    let output_file = cwd.join("10-prd/group-mentions.md");
    let turn = workspace
        .run_workspace_turn(WorkspaceTurnRequest {
            message: "We need a short PRD for a group-chat mention feature. Please create it at 10-prd/group-mentions.md with sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria. Keep it under 250 words.".to_string(),
            visibility: None,
            max_assignments: None,
            prefer_role_id: None,
        })
        .await
        .expect("coding workspace turn should succeed");

    assert!(turn.workflow_vote_window.is_none());
    assert_eq!(turn.plan.assignments.len(), 1);
    assert_eq!(turn.plan.assignments[0].role_id, "prd");
    assert_eq!(turn.dispatches.len(), 1);
    assert_eq!(turn.dispatches[0].status, DispatchStatus::Completed);
    assert_eq!(turn.state.workflow_runtime.mode, WorkspaceMode::GroupChat);
    assert!(
        turn.events.iter().any(|event| matches!(
            event,
            multi_agent_protocol::WorkspaceEvent::ActivityPublished { activity, .. }
                if activity.kind == WorkspaceActivityKind::DispatchCompleted
        )),
        "expected public dispatch completion activity"
    );

    let file_text = fs::read_to_string(&output_file).expect("expected PRD output file");
    assert!(file_text.contains("Goal"));
    assert!(file_text.contains("User Story"));
    assert!(file_text.contains("Acceptance Criteria"));
}

#[tokio::test]
#[ignore = "requires local Codex CLI auth and a live model run"]
async fn codex_live_workspace_turn_opc_delegates_to_finance() {
    let cwd = create_repo_local_temp_dir("multi-agent-runtime-codex-live-opc");
    fs::create_dir_all(cwd.join("company/10-finance")).expect("should create temp workspace");

    let mut workspace = build_workspace("codex-live-opc", &cwd, create_opc_solo_company_template());
    let startup_events = workspace.start();
    assert!(startup_events.len() >= 2);

    let output_file = cwd.join("company/10-finance/monthly-close-checklist.md");
    let turn = workspace
        .run_workspace_turn(WorkspaceTurnRequest {
            message: "Please prepare a compact monthly close checklist for a solo SaaS founder and write it to company/10-finance/monthly-close-checklist.md. Keep it concise, around 12-18 actionable checklist items total, while still covering cash review, invoices, subscriptions, payroll or contractors, tax prep handoff, and KPI review.".to_string(),
            visibility: None,
            max_assignments: None,
            prefer_role_id: None,
        })
        .await
        .expect("opc workspace turn should succeed");

    assert!(turn.workflow_vote_window.is_none());
    assert_eq!(turn.plan.assignments.len(), 1);
    assert_eq!(turn.plan.assignments[0].role_id, "finance");
    assert_eq!(turn.dispatches.len(), 1);
    assert_eq!(turn.dispatches[0].status, DispatchStatus::Completed);

    let file_text =
        fs::read_to_string(&output_file).expect("expected finance checklist output file");
    assert!(matches_any(
        &file_text,
        &[
            "Cash Review",
            "Cash & Banking",
            "Cash & Accounts",
            "cash balance",
            "cash on hand",
            "bank and card balances",
            "bank and card transactions",
            "bank reconciliation",
            "cash flow",
            "cash runway",
        ]
    ));
    assert!(matches_any(
        &file_text,
        &["Invoices", "Receivables", "invoice"]
    ));
    assert!(matches_any(
        &file_text,
        &[
            "Tax Prep Handoff",
            "Tax Preparation",
            "Payroll & Taxes",
            "Tax & Compliance",
            "sales tax",
            "VAT",
            "tax filing",
            "tax handoff",
        ]
    ));
}

#[tokio::test]
#[ignore = "requires local Codex CLI auth and a live model run"]
async fn codex_live_workspace_turn_autoresearch_enters_workflow_mode() {
    let cwd = create_repo_local_temp_dir("multi-agent-runtime-codex-live-autoresearch");
    fs::create_dir_all(cwd.join("research/00-lead")).expect("should create temp workspace");

    let mut workspace = build_workspace(
        "codex-live-autoresearch",
        &cwd,
        create_autoresearch_template(),
    );
    let startup_events = workspace.start();
    assert!(startup_events.len() >= 2);

    let output_file = cwd.join("research/00-lead/mention-hypothesis.md");
    let turn = workspace
        .run_workspace_turn(WorkspaceTurnRequest {
            message: "Start the autoresearch workflow for group-chat mention semantics. Frame the current hypothesis for how collaboration tools like Slack and GitHub handle @mentions, and write the initial hypothesis brief to research/00-lead/mention-hypothesis.md with sections for Hypothesis, Success Criteria, and Next Experiment.".to_string(),
            visibility: None,
            max_assignments: None,
            prefer_role_id: None,
        })
        .await
        .expect("autoresearch workspace turn should succeed");

    assert!(turn.workflow_vote_window.is_some());
    assert!(!turn.workflow_vote_responses.is_empty());
    assert!(
        turn.workflow_vote_responses.iter().any(
            |response| response.decision == multi_agent_protocol::WorkflowVoteDecision::Approve
        )
    );
    assert_eq!(
        turn.state.workflow_runtime.mode,
        WorkspaceMode::WorkflowRunning
    );
    assert!(
        matches!(
            turn.state.workflow_runtime.active_node_id.as_deref(),
            Some("frame_hypothesis") | Some("claim_evidence")
        ),
        "expected workflow to be at entry or immediately queued follow-up node"
    );
    assert_eq!(turn.plan.assignments.len(), 1);
    assert_eq!(turn.plan.assignments[0].role_id, "lead");
    assert_eq!(
        turn.plan.assignments[0].workflow_node_id.as_deref(),
        Some("frame_hypothesis")
    );
    assert_eq!(turn.dispatches.len(), 1);
    assert_eq!(turn.dispatches[0].status, DispatchStatus::Completed);
    assert!(
        turn.events.iter().any(|event| matches!(
            event,
            multi_agent_protocol::WorkspaceEvent::WorkflowStarted { .. }
        )),
        "expected workflow started event"
    );

    let file_text = fs::read_to_string(&output_file).expect("expected workflow entry brief");
    assert!(file_text.contains("Hypothesis"));
    assert!(file_text.contains("Success Criteria"));
    assert!(file_text.contains("Next Experiment"));
}

fn build_workspace(
    prefix: &str,
    cwd: &Path,
    template: multi_agent_protocol::WorkspaceTemplate,
) -> CodexWorkspace {
    let model = std::env::var("MULTI_AGENT_TEST_CODEX_MODEL")
        .unwrap_or_else(|_| "gpt-5.4-mini".to_string());
    let profile = create_codex_workspace_profile(Some(model.as_str()));
    let instance = WorkspaceInstanceParams {
        id: format!("{prefix}-{}", unique_suffix()),
        name: prefix.to_string(),
        cwd: Some(cwd.to_string_lossy().to_string()),
    };

    CodexWorkspace::from_template(
        &template,
        &instance,
        &profile,
        CodexWorkspaceOptions {
            sandbox_mode: CodexSandboxMode::DangerFullAccess,
            working_directory: Some(cwd.to_path_buf()),
            turn_timeout: Duration::from_secs(300),
            ..Default::default()
        },
    )
}

fn create_repo_local_temp_dir(prefix: &str) -> PathBuf {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..");
    repo_root
        .join(".tmp")
        .join(format!("{prefix}-{}", unique_suffix()))
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_millis()
}

fn matches_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}
