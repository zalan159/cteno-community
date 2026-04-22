use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use multi_agent_protocol::{
    ClaimStatus, DispatchStatus, RoleSpec, RoleTaskRequest, TaskDispatch, WorkspaceEvent,
    WorkspaceInstanceParams, WorkspaceProfile, WorkspaceSpec, WorkspaceState, WorkspaceTemplate,
    WorkspaceTurnPlan, WorkspaceTurnRequest, WorkspaceWorkflowVoteResponse,
    WorkspaceWorkflowVoteWindow, build_workflow_entry_plan, decide_coordinator_action,
    direct_workspace_turn_plan, instantiate_workspace, resolve_workflow_vote_candidate_role_ids,
    should_approve_workflow_vote, synthesize_workflow_vote_response,
};
use multi_agent_runtime_core::{RuntimeError, WorkspaceRuntime};
use multi_agent_runtime_local::{
    LocalPersistenceError, LocalWorkspacePersistence, PersistedProviderState,
};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexSandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl CodexSandboxMode {
    fn as_cli_value(self) -> &'static str {
        match self {
            CodexSandboxMode::ReadOnly => "read-only",
            CodexSandboxMode::WorkspaceWrite => "workspace-write",
            CodexSandboxMode::DangerFullAccess => "danger-full-access",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexApprovalPolicy {
    Untrusted,
    OnFailure,
    OnRequest,
    Never,
}

impl CodexApprovalPolicy {
    fn as_cli_value(self) -> &'static str {
        match self {
            CodexApprovalPolicy::Untrusted => "untrusted",
            CodexApprovalPolicy::OnFailure => "on-failure",
            CodexApprovalPolicy::OnRequest => "on-request",
            CodexApprovalPolicy::Never => "never",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CodexWorkspaceOptions {
    pub codex_path: PathBuf,
    pub sandbox_mode: CodexSandboxMode,
    pub approval_policy: CodexApprovalPolicy,
    pub working_directory: Option<PathBuf>,
    pub additional_directories: Vec<PathBuf>,
    pub temp_directory_name: String,
    pub skip_git_repo_check: bool,
    pub turn_timeout: Duration,
    pub max_workflow_followups: usize,
}

impl Default for CodexWorkspaceOptions {
    fn default() -> Self {
        Self {
            codex_path: PathBuf::from("codex"),
            sandbox_mode: CodexSandboxMode::WorkspaceWrite,
            approval_policy: CodexApprovalPolicy::Never,
            working_directory: None,
            additional_directories: Vec::new(),
            temp_directory_name: ".codex-tmp".to_string(),
            skip_git_repo_check: true,
            turn_timeout: Duration::from_secs(240),
            max_workflow_followups: 0,
        }
    }
}

#[derive(Debug)]
pub struct CodexRoleTaskRun {
    pub dispatch: TaskDispatch,
    pub events: Vec<WorkspaceEvent>,
}

#[derive(Debug)]
pub struct CodexWorkspaceTurnRun {
    pub request: WorkspaceTurnRequest,
    pub plan: WorkspaceTurnPlan,
    pub workflow_vote_window: Option<WorkspaceWorkflowVoteWindow>,
    pub workflow_vote_responses: Vec<WorkspaceWorkflowVoteResponse>,
    pub dispatches: Vec<TaskDispatch>,
    pub events: Vec<WorkspaceEvent>,
    pub state: WorkspaceState,
}

#[derive(Debug, Error)]
pub enum CodexAdapterError {
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("unknown role: {0}")]
    UnknownRole(String),
    #[error("codex process missing stdin")]
    MissingStdin,
    #[error("codex process missing stdout")]
    MissingStdout,
    #[error("codex process missing stderr")]
    MissingStderr,
    #[error("codex io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("codex task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("codex stderr: {0}")]
    Process(String),
    #[error("codex failed turn: {0}")]
    TurnFailed(String),
    #[error("codex timed out after {timeout:?}\n{debug}")]
    TimedOut { timeout: Duration, debug: String },
    #[error("local persistence error: {0}")]
    LocalPersistence(#[from] LocalPersistenceError),
}

pub struct CodexWorkspace {
    runtime: WorkspaceRuntime,
    options: CodexWorkspaceOptions,
    started: bool,
    role_thread_ids: BTreeMap<String, String>,
    persistence: Option<LocalWorkspacePersistence>,
    restored_from_persistence: bool,
}

impl CodexWorkspace {
    pub fn new(spec: WorkspaceSpec, options: CodexWorkspaceOptions) -> Self {
        let persistence = LocalWorkspacePersistence::from_spec(&spec).ok();
        Self {
            runtime: WorkspaceRuntime::new(spec),
            options,
            started: false,
            role_thread_ids: BTreeMap::new(),
            persistence,
            restored_from_persistence: false,
        }
    }

    pub fn from_template(
        template: &WorkspaceTemplate,
        instance: &WorkspaceInstanceParams,
        profile: &WorkspaceProfile,
        options: CodexWorkspaceOptions,
    ) -> Self {
        Self::new(instantiate_workspace(template, instance, profile), options)
    }

    pub fn restore_from_local(
        cwd: impl AsRef<std::path::Path>,
        workspace_id: &str,
        options: CodexWorkspaceOptions,
    ) -> Result<Self, CodexAdapterError> {
        let persistence = LocalWorkspacePersistence::from_workspace(cwd, workspace_id);
        let spec = persistence.load_workspace_spec()?;
        let state = persistence.load_workspace_state()?;
        let history = persistence.load_events()?;
        let provider_state = persistence.load_provider_state()?;

        let mut workspace = Self::new(spec, options);
        workspace.runtime.restore_snapshot(state, history);
        workspace.role_thread_ids = provider_state
            .member_bindings
            .into_iter()
            .map(|(role_id, binding)| (role_id, binding.provider_conversation_id))
            .collect();
        workspace.restored_from_persistence = true;
        Ok(workspace)
    }

    pub fn runtime(&self) -> &WorkspaceRuntime {
        &self.runtime
    }

    pub fn persistence_root(&self) -> Option<&std::path::Path> {
        self.persistence.as_ref().map(|p| p.root())
    }

    pub fn start(&mut self) -> Vec<WorkspaceEvent> {
        if self.started {
            return Vec::new();
        }

        self.started = true;
        if !self.restored_from_persistence {
            if let Some(persistence) = self.persistence.as_ref() {
                let _ = persistence.ensure_workspace_initialized(self.runtime.spec());
            }
        }
        let mut emitted = Vec::new();
        emitted.extend(self.runtime.start().emitted);
        emitted.extend(
            self.runtime
                .initialize(
                    None,
                    self.runtime
                        .spec()
                        .roles
                        .iter()
                        .map(|role| role.id.clone())
                        .collect(),
                    self.runtime
                        .spec()
                        .allowed_tools
                        .clone()
                        .unwrap_or_default(),
                    Some(vec!["exec".to_string(), "resume".to_string()]),
                )
                .emitted,
        );
        let _ = self.persist_runtime(&emitted);
        emitted
    }

    pub fn delete_workspace(&mut self) -> Result<(), CodexAdapterError> {
        self.started = false;
        self.role_thread_ids.clear();
        if let Some(persistence) = self.persistence.as_ref() {
            persistence.delete_workspace()?;
        }
        Ok(())
    }

    pub async fn run_role_task(
        &mut self,
        request: RoleTaskRequest,
    ) -> Result<CodexRoleTaskRun, CodexAdapterError> {
        let run = self.execute_assignment(request, None).await?;
        self.persist_runtime(&run.events)?;
        Ok(run)
    }

    pub async fn run_workspace_turn(
        &mut self,
        request: WorkspaceTurnRequest,
    ) -> Result<CodexWorkspaceTurnRun, CodexAdapterError> {
        let mut events = self
            .runtime
            .publish_user_message(request.message.clone())
            .emitted;
        let coordinator_decision = decide_coordinator_action(self.runtime.spec(), &request);
        if !coordinator_decision.response_text.trim().is_empty() {
            events.extend(
                self.runtime
                    .record_role_message(
                        &self
                            .runtime
                            .spec()
                            .coordinator_role_id
                            .clone()
                            .or_else(|| self.runtime.spec().default_role_id.clone())
                            .unwrap_or_else(|| "coordinator".to_string()),
                        coordinator_decision.response_text.clone(),
                        multi_agent_protocol::WorkspaceVisibility::Public,
                        None,
                        None,
                    )?
                    .emitted,
            );
        }

        let mut workflow_vote_window = None;
        let mut workflow_vote_responses = Vec::new();
        let plan = match coordinator_decision.kind {
            multi_agent_protocol::CoordinatorDecisionKind::Respond => WorkspaceTurnPlan {
                coordinator_role_id: self
                    .runtime
                    .spec()
                    .coordinator_role_id
                    .clone()
                    .or_else(|| self.runtime.spec().default_role_id.clone())
                    .unwrap_or_else(|| "coordinator".to_string()),
                response_text: coordinator_decision.response_text.clone(),
                assignments: Vec::new(),
                rationale: coordinator_decision.rationale.clone(),
            },
            multi_agent_protocol::CoordinatorDecisionKind::Delegate => {
                if let Some(target_role_id) = coordinator_decision.target_role_id.clone() {
                    direct_workspace_turn_plan(self.runtime.spec(), &request, &target_role_id)
                } else {
                    multi_agent_protocol::plan_workspace_turn(self.runtime.spec(), &request)
                }
            }
            multi_agent_protocol::CoordinatorDecisionKind::ProposeWorkflow => {
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
                for role_id in candidate_role_ids {
                    if let Some(role) = self
                        .runtime
                        .spec()
                        .roles
                        .iter()
                        .find(|role| role.id == role_id)
                    {
                        let response = synthesize_workflow_vote_response(
                            self.runtime.spec(),
                            &request,
                            &coordinator_decision,
                            role,
                        );
                        workflow_vote_responses.push(response.clone());
                        if let Some(vote_window) = vote_window.as_ref() {
                            events.extend(
                                self.runtime
                                    .record_workflow_vote_response(vote_window, response)?
                                    .emitted,
                            );
                        }
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
                    let plan = build_workflow_entry_plan(self.runtime.spec(), &request);
                    let first_assignment = plan.assignments.first();
                    events.extend(
                        self.runtime
                            .start_workflow(
                                coordinator_decision.clone(),
                                workflow_vote_window.clone(),
                                Some(request.message.clone()),
                                first_assignment
                                    .and_then(|assignment| assignment.workflow_node_id.clone()),
                                first_assignment.and_then(|assignment| assignment.stage_id.clone()),
                            )
                            .emitted,
                    );
                    plan
                } else {
                    WorkspaceTurnPlan {
                        coordinator_role_id: self
                            .runtime
                            .spec()
                            .coordinator_role_id
                            .clone()
                            .or_else(|| self.runtime.spec().default_role_id.clone())
                            .unwrap_or_else(|| "coordinator".to_string()),
                        response_text: coordinator_decision.response_text.clone(),
                        assignments: Vec::new(),
                        rationale: Some(
                            "Workflow vote rejected; staying in group chat mode.".to_string(),
                        ),
                    }
                }
            }
        };

        let mut dispatches = Vec::new();
        for assignment in &plan.assignments {
            let (mut chained_dispatches, chained_events) = self
                .execute_assignment_chain(
                    RoleTaskRequest {
                        role_id: assignment.role_id.clone(),
                        instruction: assignment.instruction.clone(),
                        summary: assignment.summary.clone(),
                        visibility: assignment.visibility,
                        source_role_id: Some(plan.coordinator_role_id.clone()),
                        workflow_node_id: assignment.workflow_node_id.clone(),
                        stage_id: assignment.stage_id.clone(),
                    },
                    Some("Claimed by runtime routing".to_string()),
                )
                .await?;
            events.extend(chained_events);
            dispatches.append(&mut chained_dispatches);
        }

        let run = CodexWorkspaceTurnRun {
            request,
            plan,
            workflow_vote_window,
            workflow_vote_responses,
            dispatches,
            events,
            state: self.runtime.snapshot(),
        };
        self.persist_runtime(&run.events)?;
        Ok(run)
    }

    async fn execute_assignment(
        &mut self,
        request: RoleTaskRequest,
        claim_note: Option<String>,
    ) -> Result<CodexRoleTaskRun, CodexAdapterError> {
        let role = self
            .runtime
            .spec()
            .roles
            .iter()
            .find(|role| role.id == request.role_id)
            .cloned()
            .ok_or_else(|| CodexAdapterError::UnknownRole(request.role_id.clone()))?;

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
                        &role.id,
                        ClaimStatus::Claimed,
                        claim_note,
                    )?
                    .emitted,
            );
        }

        let provider_result = self.execute_provider_turn(&role, &dispatch).await?;
        emitted.extend(provider_result.events);

        let snapshot = self.runtime.snapshot();
        let final_dispatch = snapshot
            .dispatches
            .get(&dispatch.dispatch_id)
            .cloned()
            .expect("dispatch should exist after provider turn");

        Ok(CodexRoleTaskRun {
            dispatch: final_dispatch,
            events: emitted,
        })
    }

    async fn execute_assignment_chain(
        &mut self,
        request: RoleTaskRequest,
        claim_note: Option<String>,
    ) -> Result<(Vec<TaskDispatch>, Vec<WorkspaceEvent>), CodexAdapterError> {
        let mut dispatches = Vec::new();
        let mut events = Vec::new();
        let mut pending = vec![(request, claim_note)];

        let mut followup_budget = self.options.max_workflow_followups;
        while let Some((request, claim_note)) = pending.pop() {
            let run = self.execute_assignment(request, claim_note).await?;
            let provider_task_id = run.dispatch.provider_task_id.clone();
            events.extend(run.events);
            dispatches.push(run.dispatch.clone());

            if let Some(provider_task_id) = provider_task_id {
                let (advance_tick, mut followups) = self
                    .runtime
                    .advance_workflow_after_dispatch(&provider_task_id)?;
                events.extend(advance_tick.emitted);
                while followup_budget > 0 {
                    let Some(followup) = followups.pop() else {
                        break;
                    };
                    followup_budget -= 1;
                    pending.push((
                        followup,
                        Some("Claimed by workflow progression".to_string()),
                    ));
                }
            }
        }

        Ok((dispatches, events))
    }

    async fn execute_provider_turn(
        &mut self,
        role: &RoleSpec,
        dispatch: &TaskDispatch,
    ) -> Result<CodexRoleTaskRun, CodexAdapterError> {
        let role_thread_id = self.role_thread_ids.get(&role.id).cloned();
        let effective_working_directory = self
            .options
            .working_directory
            .clone()
            .or_else(|| self.runtime.spec().cwd.as_ref().map(PathBuf::from));
        let temp_directory = effective_working_directory
            .as_ref()
            .map(|working_directory| working_directory.join(&self.options.temp_directory_name));
        let stdout_tail = Arc::new(Mutex::new(Vec::<String>::new()));
        let stderr_tail = Arc::new(Mutex::new(Vec::<String>::new()));

        if let Some(temp_directory) = temp_directory.as_ref() {
            fs::create_dir_all(temp_directory)?;
        }

        let mut command = Command::new(&self.options.codex_path);
        command
            .arg("exec")
            .arg("--experimental-json")
            .arg("--model")
            .arg(&self.runtime.spec().model)
            .arg("--sandbox")
            .arg(self.options.sandbox_mode.as_cli_value())
            .arg("--config")
            .arg(format!(
                "approval_policy=\"{}\"",
                self.options.approval_policy.as_cli_value()
            ));
        if let Some(working_directory) = effective_working_directory.as_ref() {
            command.arg("--cd").arg(working_directory);
        }

        for additional_directory in &self.options.additional_directories {
            command.arg("--add-dir").arg(additional_directory);
        }

        if self.options.skip_git_repo_check {
            command.arg("--skip-git-repo-check");
        }

        if let Some(thread_id) = role_thread_id.as_deref() {
            command.arg("resume").arg(thread_id);
        } else {
        }

        if let Some(temp_directory) = temp_directory.as_ref() {
            command.env("TMPDIR", temp_directory);
        }
        command.env(
            "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
            "multi_agent_runtime_rust",
        );
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn()?;
        let mut stdin = child.stdin.take().ok_or(CodexAdapterError::MissingStdin)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(CodexAdapterError::MissingStdout)?;
        let stderr = child
            .stderr
            .take()
            .ok_or(CodexAdapterError::MissingStderr)?;
        let command_line = render_command_line(&self.options.codex_path, &command);

        let prompt = format!(
            "{}\n",
            build_dispatch_prompt(self.runtime.spec(), role, dispatch)
        );
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.shutdown().await?;
        drop(stdin);

        let stderr_tail_for_task = Arc::clone(&stderr_tail);
        let stderr_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut buffer = Vec::new();
            while let Some(line) = lines.next_line().await? {
                push_tail(&stderr_tail_for_task, line.clone());
                buffer.push(line);
            }
            Ok::<Vec<String>, std::io::Error>(buffer)
        });

        let stdout_tail_for_processing = Arc::clone(&stdout_tail);
        let processing = async {
            let mut stdout_lines = BufReader::new(stdout).lines();
            let mut emitted = Vec::new();
            let mut final_result_text: Option<String> = None;
            let mut saw_turn_completion = false;
            let mut saw_turn_failure = None::<String>;

            while let Some(line) = stdout_lines.next_line().await? {
                push_tail(&stdout_tail_for_processing, line.clone());
                let trimmed = line.trim();
                if !trimmed.starts_with('{') {
                    continue;
                }

                let event: CodexJsonEvent = match serde_json::from_str(trimmed) {
                    Ok(event) => event,
                    Err(_) => continue,
                };

                match event {
                    CodexJsonEvent::ThreadStarted { thread_id } => {
                        self.role_thread_ids
                            .insert(role.id.clone(), thread_id.clone());
                        emitted.extend(
                            self.runtime
                                .start_next_dispatch(
                                    thread_id.clone(),
                                    dispatch
                                        .summary
                                        .clone()
                                        .unwrap_or_else(|| dispatch.instruction.clone()),
                                    Some(format!("codex-thread:{thread_id}")),
                                )?
                                .1
                                .emitted,
                        );
                    }
                    CodexJsonEvent::ItemStarted { item } => {
                        if let CodexItem::CommandExecution { id, command, .. } = item {
                            let _ = id;
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        command,
                                        Some("Codex is executing a shell command.".to_string()),
                                        Some("Bash".to_string()),
                                    )?
                                    .emitted,
                            );
                        }
                    }
                    CodexJsonEvent::ItemUpdated { item } => {
                        if let CodexItem::TodoList { items, .. } = item {
                            let remaining = items.iter().filter(|item| !item.completed).count();
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        "todo_list",
                                        Some(format!(
                                            "Codex is tracking {remaining} remaining todo item(s)."
                                        )),
                                        Some("TodoList".to_string()),
                                    )?
                                    .emitted,
                            );
                        }
                    }
                    CodexJsonEvent::ItemCompleted { item } => match item {
                        CodexItem::CommandExecution {
                            command, exit_code, ..
                        } => {
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        command,
                                        Some(match exit_code {
                                            Some(0) => {
                                                "Codex completed a shell command.".to_string()
                                            }
                                            Some(code) => {
                                                format!("Codex command exited with code {code}.")
                                            }
                                            None => "Codex completed a shell command.".to_string(),
                                        }),
                                        Some("Bash".to_string()),
                                    )?
                                    .emitted,
                            );
                        }
                        CodexItem::AgentMessage { text, .. } => {
                            final_result_text = Some(text.clone());
                            emitted.push(WorkspaceEvent::Message {
                                timestamp: Utc::now().to_rfc3339(),
                                workspace_id: self.runtime.spec().id.clone(),
                                role: "assistant".to_string(),
                                text,
                                visibility: Some(multi_agent_protocol::WorkspaceVisibility::Public),
                                member_id: Some(role.id.clone()),
                                session_id: self.role_thread_ids.get(&role.id).cloned(),
                                parent_tool_use_id: None,
                            });
                        }
                        CodexItem::FileChange { changes, .. } => {
                            let description = changes
                                .iter()
                                .map(|change| change.path.clone())
                                .collect::<Vec<_>>()
                                .join(", ");
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        description,
                                        Some("Codex applied file changes.".to_string()),
                                        Some("ApplyPatch".to_string()),
                                    )?
                                    .emitted,
                            );
                        }
                        CodexItem::McpToolCall {
                            server,
                            tool,
                            status,
                            error,
                            ..
                        } => {
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        format!("{server}.{tool}"),
                                        Some(match status.as_deref() {
                                            Some("failed") => error
                                                .and_then(|error| error.message)
                                                .unwrap_or_else(|| {
                                                    "Codex MCP call failed.".to_string()
                                                }),
                                            _ => "Codex completed an MCP tool call.".to_string(),
                                        }),
                                        Some(tool),
                                    )?
                                    .emitted,
                            );
                        }
                        CodexItem::DynamicToolCall { .. } => {}
                        CodexItem::WebSearch { query, .. } => {
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        query,
                                        Some("Codex completed a web search.".to_string()),
                                        Some("WebSearch".to_string()),
                                    )?
                                    .emitted,
                            );
                        }
                        CodexItem::Error { message, .. } => {
                            emitted.push(WorkspaceEvent::Error {
                                timestamp: Utc::now().to_rfc3339(),
                                workspace_id: self.runtime.spec().id.clone(),
                                error: message,
                            });
                        }
                        CodexItem::Reasoning { text, .. } => {
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        "reasoning",
                                        Some(text),
                                        Some("Reasoning".to_string()),
                                    )?
                                    .emitted,
                            );
                        }
                        CodexItem::TodoList { .. } => {}
                    },
                    CodexJsonEvent::TurnCompleted { .. } => {
                        saw_turn_completion = true;
                    }
                    CodexJsonEvent::TurnFailed { error } => {
                        saw_turn_failure = Some(error.message);
                    }
                    CodexJsonEvent::Error { message } => {
                        emitted.push(WorkspaceEvent::Error {
                            timestamp: Utc::now().to_rfc3339(),
                            workspace_id: self.runtime.spec().id.clone(),
                            error: message,
                        });
                    }
                    CodexJsonEvent::TurnStarted => {}
                }
            }

            let status = child.wait().await?;

            Ok::<_, CodexAdapterError>((
                emitted,
                final_result_text,
                saw_turn_completion,
                saw_turn_failure,
                status.success(),
            ))
        };

        let (
            mut emitted,
            final_result_text,
            saw_turn_completion,
            saw_turn_failure,
            process_success,
        ) = match timeout(self.options.turn_timeout, processing).await {
            Ok(result) => result?,
            Err(_) => {
                let _ = child.kill().await;
                stderr_task.abort();
                return Err(CodexAdapterError::TimedOut {
                    timeout: self.options.turn_timeout,
                    debug: render_debug_context(
                        &command_line,
                        &prompt,
                        effective_working_directory.as_ref(),
                        temp_directory.as_ref(),
                        &stdout_tail,
                        &stderr_tail,
                    ),
                });
            }
        };

        let stderr_lines = stderr_task.await??;
        if !process_success {
            return Err(CodexAdapterError::Process(stderr_lines.join("\n")));
        }
        if let Some(message) = saw_turn_failure {
            return Err(CodexAdapterError::TurnFailed(message));
        }

        let provider_task_id = self
            .role_thread_ids
            .get(&role.id)
            .cloned()
            .unwrap_or_else(|| dispatch.dispatch_id.to_string());

        if saw_turn_completion {
            emitted.extend(
                self.runtime
                    .complete_dispatch(
                        &provider_task_id,
                        DispatchStatus::Completed,
                        None,
                        "Codex completed the turn.".to_string(),
                    )?
                    .emitted,
            );
            if let Some(result_text) = final_result_text {
                emitted.extend(
                    self.runtime
                        .attach_result_text(&provider_task_id, result_text)?
                        .emitted,
                );
            }
        }

        let snapshot = self.runtime.snapshot();
        let final_dispatch = snapshot
            .dispatches
            .get(&dispatch.dispatch_id)
            .cloned()
            .expect("dispatch should exist after codex turn");

        Ok(CodexRoleTaskRun {
            dispatch: final_dispatch,
            events: emitted,
        })
    }

    fn build_provider_state(&self) -> PersistedProviderState {
        PersistedProviderState {
            workspace_id: self.runtime.spec().id.clone(),
            provider: multi_agent_protocol::MultiAgentProvider::CodexSdk,
            root_conversation_id: self.runtime.snapshot().session_id,
            member_bindings: self
                .role_thread_ids
                .iter()
                .map(|(role_id, thread_id)| {
                    (
                        role_id.clone(),
                        multi_agent_runtime_local::PersistedProviderBinding {
                            role_id: role_id.clone(),
                            provider_conversation_id: thread_id.clone(),
                            kind: multi_agent_runtime_local::ProviderConversationKind::Thread,
                            updated_at: Utc::now().to_rfc3339(),
                        },
                    )
                })
                .collect(),
            metadata: None,
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    fn persist_runtime(&self, events: &[WorkspaceEvent]) -> Result<(), CodexAdapterError> {
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

fn build_dispatch_prompt(spec: &WorkspaceSpec, role: &RoleSpec, dispatch: &TaskDispatch) -> String {
    let mut parts = vec![format!(
        "You are the {} role in the workspace \"{}\".",
        role.name, spec.name
    )];

    parts.push(
        "The current working directory is the workspace root for this task. Create or edit files using paths relative to the current directory, and avoid exploring unrelated directories.".to_string(),
    );
    parts.push(
        "If a shell command needs a temporary directory, use .codex-tmp/ inside the workspace rather than relying on the system temp directory.".to_string(),
    );

    if let Some(description) = role.description.as_ref() {
        parts.push(format!("Role description: {description}"));
    }
    parts.push(format!(
        "Follow this role-specific instruction set strictly:\n{}",
        role.agent.prompt
    ));
    if let Some(orchestrator_prompt) = spec.orchestrator_prompt.as_ref() {
        parts.push(format!(
            "Workspace orchestration context:\n{}",
            orchestrator_prompt
        ));
    }
    if let Some(output_root) = role.output_root.as_ref() {
        parts.push(format!(
            "Preferred output root for this role: {output_root}"
        ));
    }
    if let Some(summary) = dispatch.summary.as_ref() {
        parts.push(format!("Task summary: {summary}"));
    }
    parts.push(format!("Task instruction:\n{}", dispatch.instruction));
    parts.push(
        "Return a concise final answer after completing the task. If you create or edit files, mention the key output paths in the final answer."
            .to_string(),
    );

    parts.join("\n\n")
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum CodexJsonEvent {
    #[serde(rename = "thread.started")]
    ThreadStarted { thread_id: String },
    #[serde(rename = "turn.started")]
    TurnStarted,
    #[serde(rename = "turn.completed")]
    TurnCompleted {
        #[serde(default)]
        usage: Option<Value>,
    },
    #[serde(rename = "turn.failed")]
    TurnFailed { error: CodexTurnError },
    #[serde(rename = "item.started")]
    ItemStarted { item: CodexItem },
    #[serde(rename = "item.updated")]
    ItemUpdated { item: CodexItem },
    #[serde(rename = "item.completed")]
    ItemCompleted { item: CodexItem },
    #[serde(rename = "error")]
    Error { message: String },
}

#[derive(Debug, Deserialize)]
struct CodexTurnError {
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum CodexItem {
    #[serde(rename = "command_execution")]
    CommandExecution {
        id: String,
        command: String,
        #[serde(default)]
        aggregated_output: Option<String>,
        #[serde(default)]
        exit_code: Option<i32>,
        #[serde(default)]
        status: Option<String>,
    },
    #[serde(rename = "agent_message")]
    AgentMessage { id: String, text: String },
    #[serde(rename = "reasoning")]
    Reasoning { id: String, text: String },
    #[serde(rename = "file_change")]
    FileChange {
        id: String,
        changes: Vec<CodexFileChange>,
        #[serde(default)]
        status: Option<String>,
    },
    #[serde(rename = "mcp_tool_call")]
    McpToolCall {
        id: String,
        server: String,
        tool: String,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        error: Option<CodexItemError>,
    },
    #[serde(rename = "dynamicToolCall", alias = "dynamic_tool_call")]
    DynamicToolCall {
        id: String,
        tool: String,
        #[serde(default)]
        arguments: Option<Value>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default, alias = "content_items")]
        content_items: Vec<Value>,
        #[serde(default)]
        error: Option<CodexItemError>,
    },
    #[serde(rename = "web_search")]
    WebSearch { id: String, query: String },
    #[serde(rename = "todo_list")]
    TodoList {
        id: String,
        items: Vec<CodexTodoItem>,
    },
    #[serde(rename = "error")]
    Error { id: String, message: String },
}

#[derive(Debug, Deserialize)]
struct CodexFileChange {
    path: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
struct CodexItemError {
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexTodoItem {
    text: String,
    completed: bool,
}

fn current_task_id<'a>(
    role_thread_ids: &BTreeMap<String, String>,
    role_id: &str,
    dispatch: &TaskDispatch,
) -> String {
    role_thread_ids
        .get(role_id)
        .cloned()
        .unwrap_or_else(|| dispatch.dispatch_id.to_string())
}

fn push_tail(buffer: &Arc<Mutex<Vec<String>>>, line: String) {
    let mut guard = buffer
        .lock()
        .expect("tail buffer mutex should not be poisoned");
    guard.push(line);
    if guard.len() > 40 {
        let overflow = guard.len() - 40;
        guard.drain(0..overflow);
    }
}

fn render_command_line(program: &PathBuf, command: &Command) -> String {
    let mut parts = vec![program.display().to_string()];
    parts.extend(
        command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string()),
    );
    parts.join(" ")
}

fn render_debug_context(
    command_line: &str,
    prompt: &str,
    working_directory: Option<&PathBuf>,
    temp_directory: Option<&PathBuf>,
    stdout_tail: &Arc<Mutex<Vec<String>>>,
    stderr_tail: &Arc<Mutex<Vec<String>>>,
) -> String {
    let stdout_tail = stdout_tail
        .lock()
        .expect("stdout tail mutex should not be poisoned")
        .join("\n");
    let stderr_tail = stderr_tail
        .lock()
        .expect("stderr tail mutex should not be poisoned")
        .join("\n");
    let prompt_preview = prompt.lines().take(24).collect::<Vec<_>>().join("\n");

    format!(
        "command: {command_line}\nworking_directory: {}\ntemp_directory: {}\nprompt_preview:\n{prompt_preview}\nstdout_tail:\n{stdout_tail}\nstderr_tail:\n{stderr_tail}",
        working_directory
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        temp_directory
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<none>".to_string()),
    )
}
