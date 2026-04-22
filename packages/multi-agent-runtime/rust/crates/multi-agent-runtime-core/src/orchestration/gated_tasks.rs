use async_trait::async_trait;
use chrono::Utc;
use multi_agent_protocol::{
    DispatchStatus, TaskDispatch, WorkspaceActivity, WorkspaceActivityKind, WorkspaceVisibility,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    OrchestratorError, OrchestratorResponse, SessionMessenger, WorkspaceOrchestrator,
    WorkspaceShell,
};

const ORCHESTRATOR_TYPE: &str = "gated_tasks";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatedTasksPhase {
    Idle,
    Coding,
    Reviewing,
    Committing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatedTaskStatus {
    Pending,
    Coding,
    Reviewing,
    Committing,
    Completed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatedTask {
    pub title: String,
    pub instruction: String,
    pub status: GatedTaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coder_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_result: Option<String>,
}

impl GatedTask {
    fn new(instruction: impl Into<String>) -> Self {
        let instruction = instruction.into();
        Self {
            title: task_title(&instruction),
            instruction,
            status: GatedTaskStatus::Pending,
            feedback: None,
            coder_result: None,
            review_result: None,
            commit_result: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatedTasksOrchestrator {
    pub tasks: Vec<GatedTask>,
    pub current_task_index: Option<usize>,
    pub reviewer_role_id: String,
    pub coder_role_id: String,
    pub current_phase: GatedTasksPhase,
}

impl GatedTasksOrchestrator {
    pub fn new(reviewer_role_id: impl Into<String>, coder_role_id: impl Into<String>) -> Self {
        Self {
            tasks: Vec::new(),
            current_task_index: None,
            reviewer_role_id: reviewer_role_id.into(),
            coder_role_id: coder_role_id.into(),
            current_phase: GatedTasksPhase::Idle,
        }
    }

    pub fn with_tasks<I, S>(
        reviewer_role_id: impl Into<String>,
        coder_role_id: impl Into<String>,
        tasks: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut orchestrator = Self::new(reviewer_role_id, coder_role_id);
        orchestrator
            .tasks
            .extend(tasks.into_iter().map(GatedTask::new));
        orchestrator
    }

    fn create_activity(
        &self,
        shell: &WorkspaceShell,
        kind: WorkspaceActivityKind,
        text: impl Into<String>,
        role_id: Option<&str>,
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
            dispatch_id: None,
            task_id: None,
        }
    }

    fn create_dispatch(
        &self,
        shell: &WorkspaceShell,
        role_id: &str,
        instruction: impl Into<String>,
        summary: impl Into<String>,
    ) -> Result<TaskDispatch, OrchestratorError> {
        if !shell.members.contains_key(role_id) {
            return Err(OrchestratorError::InvalidState(format!(
                "unknown role: {role_id}"
            )));
        }

        Ok(TaskDispatch {
            dispatch_id: Uuid::new_v4(),
            workspace_id: shell.spec.id.clone(),
            role_id: role_id.to_string(),
            instruction: instruction.into(),
            summary: Some(summary.into()),
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
        })
    }

    fn current_task_mut(&mut self) -> Option<&mut GatedTask> {
        self.current_task_index
            .and_then(|index| self.tasks.get_mut(index))
    }

    fn active_or_selected_index(&self) -> Option<usize> {
        if matches!(self.current_phase, GatedTasksPhase::Idle) {
            self.current_task_index
                .filter(|index| {
                    self.tasks.get(*index).map(|task| task.status) == Some(GatedTaskStatus::Pending)
                })
                .or_else(|| self.next_pending_index())
        } else {
            self.current_task_index
        }
    }

    fn next_pending_index(&self) -> Option<usize> {
        self.tasks
            .iter()
            .position(|task| task.status == GatedTaskStatus::Pending)
    }

    fn add_tasks_from_text(&mut self, raw: &str) -> usize {
        let parsed = parse_task_instructions(raw);
        let count = parsed.len();
        self.tasks.extend(parsed.into_iter().map(GatedTask::new));
        count
    }

    fn start_current_or_next_task(
        &mut self,
        shell: &WorkspaceShell,
    ) -> Result<Option<TaskDispatch>, OrchestratorError> {
        let index = self
            .current_task_index
            .filter(|index| {
                self.tasks.get(*index).map(|task| task.status) == Some(GatedTaskStatus::Pending)
            })
            .or_else(|| self.next_pending_index());
        let Some(index) = index else {
            self.current_task_index = None;
            self.current_phase = GatedTasksPhase::Idle;
            return Ok(None);
        };

        self.current_task_index = Some(index);
        self.current_phase = GatedTasksPhase::Coding;
        let task = self.tasks.get_mut(index).ok_or_else(|| {
            OrchestratorError::InvalidState(format!("missing task at index {index}"))
        })?;
        task.status = GatedTaskStatus::Coding;
        let instruction = coding_instruction(task);
        let summary = format!("Code task {}", task.title);
        self.create_dispatch(shell, &self.coder_role_id, instruction, summary)
            .map(Some)
    }

    fn dispatch_review(
        &mut self,
        shell: &WorkspaceShell,
        coder_result: &str,
    ) -> Result<TaskDispatch, OrchestratorError> {
        let (instruction, summary) = {
            let task = self.current_task_mut().ok_or_else(|| {
                OrchestratorError::InvalidState("missing active task for review".to_string())
            })?;
            task.status = GatedTaskStatus::Reviewing;
            task.coder_result = Some(coder_result.to_string());
            (
                review_instruction(task, coder_result),
                format!("Review task {}", task.title),
            )
        };
        self.current_phase = GatedTasksPhase::Reviewing;
        self.create_dispatch(shell, &self.reviewer_role_id, instruction, summary)
    }

    fn dispatch_commit(
        &mut self,
        shell: &WorkspaceShell,
        review_result: &str,
    ) -> Result<TaskDispatch, OrchestratorError> {
        let (instruction, summary) = {
            let task = self.current_task_mut().ok_or_else(|| {
                OrchestratorError::InvalidState("missing active task for commit".to_string())
            })?;
            task.status = GatedTaskStatus::Committing;
            task.review_result = Some(review_result.to_string());
            task.feedback = None;
            (
                commit_instruction(task, review_result),
                format!("Commit task {}", task.title),
            )
        };
        self.current_phase = GatedTasksPhase::Committing;
        self.create_dispatch(shell, &self.coder_role_id, instruction, summary)
    }

    fn retry_current_task(
        &mut self,
        shell: &WorkspaceShell,
        feedback: &str,
    ) -> Result<TaskDispatch, OrchestratorError> {
        let (instruction, summary) = {
            let task = self.current_task_mut().ok_or_else(|| {
                OrchestratorError::InvalidState("missing active task for retry".to_string())
            })?;
            task.status = GatedTaskStatus::Coding;
            task.feedback = Some(feedback.to_string());
            task.review_result = Some(feedback.to_string());
            (
                coding_instruction(task),
                format!("Retry task {}", task.title),
            )
        };
        self.current_phase = GatedTasksPhase::Coding;
        self.create_dispatch(shell, &self.coder_role_id, instruction, summary)
    }

    fn complete_current_task(&mut self, commit_result: &str) -> Result<(), OrchestratorError> {
        let task = self.current_task_mut().ok_or_else(|| {
            OrchestratorError::InvalidState("missing active task for completion".to_string())
        })?;
        task.status = GatedTaskStatus::Completed;
        task.commit_result = Some(commit_result.to_string());
        self.current_phase = GatedTasksPhase::Idle;
        self.current_task_index = None;
        Ok(())
    }

    fn skip_current_task(&mut self) -> Option<String> {
        let index = self.active_or_selected_index()?;
        let title = self.tasks.get(index)?.title.clone();
        let task = self.tasks.get_mut(index)?;
        task.status = GatedTaskStatus::Skipped;
        self.current_phase = GatedTasksPhase::Idle;
        self.current_task_index = None;
        Some(title)
    }

    fn pause_current_task(&mut self) -> Option<String> {
        let index = self.current_task_index?;
        let task = self.tasks.get_mut(index)?;
        if matches!(
            task.status,
            GatedTaskStatus::Coding | GatedTaskStatus::Reviewing | GatedTaskStatus::Committing
        ) {
            task.status = GatedTaskStatus::Pending;
        }
        self.current_phase = GatedTasksPhase::Idle;
        Some(task.title.clone())
    }
}

impl Default for GatedTasksOrchestrator {
    fn default() -> Self {
        Self::new("reviewer", "coder")
    }
}

#[async_trait]
impl WorkspaceOrchestrator for GatedTasksOrchestrator {
    fn orchestrator_type(&self) -> &str {
        ORCHESTRATOR_TYPE
    }

    async fn handle_user_message(
        &mut self,
        shell: &WorkspaceShell,
        _messenger: &dyn SessionMessenger,
        message: &str,
        target_role: Option<&str>,
    ) -> Result<OrchestratorResponse, OrchestratorError> {
        if let Some(role_id) = target_role {
            return Err(OrchestratorError::InvalidState(format!(
                "direct target dispatch is not supported by {ORCHESTRATOR_TYPE}: {role_id}"
            )));
        }

        let mut activities =
            vec![self.create_activity(shell, WorkspaceActivityKind::UserMessage, message, None)];
        let mut dispatches = Vec::new();

        match parse_user_command(message) {
            Some(UserCommand::Add(raw)) => {
                let count = self.add_tasks_from_text(&raw);
                activities.push(self.create_activity(
                    shell,
                    WorkspaceActivityKind::SystemNotice,
                    format!("Added {count} task(s) to the gate."),
                    None,
                ));
            }
            Some(UserCommand::Pause) => {
                let text = self
                    .pause_current_task()
                    .map(|title| format!("Paused task {title}."))
                    .unwrap_or_else(|| "Nothing is currently running.".to_string());
                activities.push(self.create_activity(
                    shell,
                    WorkspaceActivityKind::SystemNotice,
                    text,
                    None,
                ));
            }
            Some(UserCommand::Skip) => {
                let text = self
                    .skip_current_task()
                    .map(|title| format!("Skipped task {title}."))
                    .unwrap_or_else(|| "No task available to skip.".to_string());
                activities.push(self.create_activity(
                    shell,
                    WorkspaceActivityKind::SystemNotice,
                    text,
                    None,
                ));
                if let Some(dispatch) = self.start_current_or_next_task(shell)? {
                    dispatches.push(dispatch);
                }
            }
            Some(UserCommand::Resume) => {
                if let Some(dispatch) = self.start_current_or_next_task(shell)? {
                    dispatches.push(dispatch);
                } else {
                    activities.push(self.create_activity(
                        shell,
                        WorkspaceActivityKind::SystemNotice,
                        "No pending tasks remain.".to_string(),
                        None,
                    ));
                }
            }
            None => {
                if self.tasks.is_empty() {
                    self.add_tasks_from_text(message);
                }
                if self.current_phase == GatedTasksPhase::Idle {
                    if let Some(dispatch) = self.start_current_or_next_task(shell)? {
                        dispatches.push(dispatch);
                    } else {
                        activities.push(self.create_activity(
                            shell,
                            WorkspaceActivityKind::SystemNotice,
                            "No pending tasks remain.".to_string(),
                            None,
                        ));
                    }
                } else {
                    activities.push(self.create_activity(
                        shell,
                        WorkspaceActivityKind::SystemNotice,
                        format!(
                            "Task execution is already in the {:?} phase.",
                            self.current_phase
                        ),
                        None,
                    ));
                }
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
        _messenger: &dyn SessionMessenger,
        role_id: &str,
        result: &str,
        success: bool,
    ) -> Result<OrchestratorResponse, OrchestratorError> {
        let activity_kind = if success {
            WorkspaceActivityKind::MemberDelivered
        } else {
            WorkspaceActivityKind::MemberBlocked
        };
        let mut activities =
            vec![self.create_activity(shell, activity_kind, result, Some(role_id))];
        let mut dispatches = Vec::new();

        match self.current_phase {
            GatedTasksPhase::Coding if role_id == self.coder_role_id => {
                if success {
                    dispatches.push(self.dispatch_review(shell, result)?);
                } else {
                    activities.push(self.create_activity(
                        shell,
                        WorkspaceActivityKind::SystemNotice,
                        "Coder reported a blocking issue.".to_string(),
                        None,
                    ));
                }
            }
            GatedTasksPhase::Reviewing if role_id == self.reviewer_role_id => {
                match parse_review_decision(result, success) {
                    ReviewDecision::Approved => {
                        dispatches.push(self.dispatch_commit(shell, result)?);
                    }
                    ReviewDecision::Rejected(feedback) => {
                        dispatches.push(self.retry_current_task(shell, &feedback)?);
                    }
                }
            }
            GatedTasksPhase::Committing if role_id == self.coder_role_id => {
                if success {
                    self.complete_current_task(result)?;
                    if let Some(dispatch) = self.start_current_or_next_task(shell)? {
                        dispatches.push(dispatch);
                    }
                } else {
                    dispatches.push(self.retry_current_task(shell, result)?);
                }
            }
            _ => {
                activities.push(self.create_activity(
                    shell,
                    WorkspaceActivityKind::SystemNotice,
                    format!(
                        "Ignored completion from @{role_id} while gate is in {:?}.",
                        self.current_phase
                    ),
                    None,
                ));
            }
        }

        Ok(OrchestratorResponse {
            activities,
            dispatches,
            template_state: self.template_state(),
        })
    }

    fn template_state(&self) -> Value {
        json!({
            "type": ORCHESTRATOR_TYPE,
            "currentPhase": self.current_phase,
            "currentTaskIndex": self.current_task_index,
            "reviewerRoleId": self.reviewer_role_id,
            "coderRoleId": self.coder_role_id,
            "tasks": self.tasks,
        })
    }

    fn serialize_state(&self) -> Value {
        json!({
            "type": ORCHESTRATOR_TYPE,
            "tasks": self.tasks,
            "currentTaskIndex": self.current_task_index,
            "reviewerRoleId": self.reviewer_role_id,
            "coderRoleId": self.coder_role_id,
            "currentPhase": self.current_phase,
        })
    }

    fn restore_state(&mut self, state: Value) -> Result<(), OrchestratorError> {
        let Some(object) = state.as_object() else {
            return Err(OrchestratorError::Serialization(
                "gated tasks state must be a JSON object".to_string(),
            ));
        };

        if let Some(orchestrator_type) = object.get("type").and_then(Value::as_str) {
            if orchestrator_type != ORCHESTRATOR_TYPE {
                return Err(OrchestratorError::Serialization(format!(
                    "expected orchestrator type {ORCHESTRATOR_TYPE}, got {orchestrator_type}"
                )));
            }
        }

        self.tasks =
            serde_json::from_value(object.get("tasks").cloned().unwrap_or_else(|| json!([])))
                .map_err(|error| OrchestratorError::Serialization(error.to_string()))?;
        self.current_task_index = serde_json::from_value(
            object
                .get("currentTaskIndex")
                .cloned()
                .unwrap_or(Value::Null),
        )
        .map_err(|error| OrchestratorError::Serialization(error.to_string()))?;
        self.reviewer_role_id = object
            .get("reviewerRoleId")
            .and_then(Value::as_str)
            .unwrap_or("reviewer")
            .to_string();
        self.coder_role_id = object
            .get("coderRoleId")
            .and_then(Value::as_str)
            .unwrap_or("coder")
            .to_string();
        self.current_phase = serde_json::from_value(
            object
                .get("currentPhase")
                .cloned()
                .unwrap_or_else(|| json!("idle")),
        )
        .map_err(|error| OrchestratorError::Serialization(error.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UserCommand {
    Add(String),
    Pause,
    Resume,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReviewDecision {
    Approved,
    Rejected(String),
}

fn parse_user_command(message: &str) -> Option<UserCommand> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.eq_ignore_ascii_case("pause") {
        return Some(UserCommand::Pause);
    }
    if trimmed.eq_ignore_ascii_case("resume") {
        return Some(UserCommand::Resume);
    }
    if trimmed.eq_ignore_ascii_case("skip") {
        return Some(UserCommand::Skip);
    }
    if let Some(rest) = strip_prefix_ignore_ascii_case(trimmed, "add task ") {
        return Some(UserCommand::Add(rest.trim().to_string()));
    }
    if let Some(rest) = strip_prefix_ignore_ascii_case(trimmed, "add ") {
        return Some(UserCommand::Add(rest.trim().to_string()));
    }

    None
}

fn parse_review_decision(result: &str, success: bool) -> ReviewDecision {
    let trimmed = result.trim();
    if trimmed.len() >= "APPROVED".len() && trimmed[..8].eq_ignore_ascii_case("APPROVED") {
        return ReviewDecision::Approved;
    }
    if trimmed.len() >= "REJECTED".len() && trimmed[..8].eq_ignore_ascii_case("REJECTED") {
        let feedback = trimmed
            .split_once(':')
            .map(|(_, rest)| rest.trim())
            .filter(|rest| !rest.is_empty())
            .unwrap_or(trimmed);
        return ReviewDecision::Rejected(feedback.to_string());
    }
    if success {
        ReviewDecision::Rejected(format!(
            "Review response must start with APPROVED: or REJECTED:. Raw response:\n{trimmed}"
        ))
    } else {
        ReviewDecision::Rejected(trimmed.to_string())
    }
}

fn parse_task_instructions(raw: &str) -> Vec<String> {
    let bullet_tasks = raw
        .lines()
        .filter_map(strip_task_bullet)
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    if bullet_tasks.len() > 1 {
        bullet_tasks
    } else {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            Vec::new()
        } else {
            vec![trimmed.to_string()]
        }
    }
}

fn strip_task_bullet(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        return (!rest.trim().is_empty()).then_some(rest.trim());
    }

    let digits = trimmed
        .chars()
        .take_while(|char| char.is_ascii_digit())
        .count();
    if digits == 0 {
        return None;
    }

    let rest = trimmed.get(digits..)?;
    let rest = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))
        .map(str::trim)?;
    (!rest.is_empty()).then_some(rest)
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    (value.len() >= prefix.len() && value[..prefix.len()].eq_ignore_ascii_case(prefix))
        .then(|| &value[prefix.len()..])
}

fn task_title(instruction: &str) -> String {
    instruction
        .lines()
        .find_map(|line| {
            let trimmed = line.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .unwrap_or("Untitled task")
        .chars()
        .take(80)
        .collect()
}

fn coding_instruction(task: &GatedTask) -> String {
    let mut message = format!("Implement task \"{}\".\n\n{}", task.title, task.instruction);
    if let Some(feedback) = task.feedback.as_deref() {
        message.push_str("\n\nReviewer feedback to address before returning:\n");
        message.push_str(feedback);
    }
    message
}

fn review_instruction(task: &GatedTask, coder_result: &str) -> String {
    format!(
        "Review task \"{}\".\n\nTask:\n{}\n\nCoder handoff:\n{}\n\nRespond on the first line with either \"APPROVED:\" or \"REJECTED:\".",
        task.title, task.instruction, coder_result
    )
}

fn commit_instruction(task: &GatedTask, review_result: &str) -> String {
    format!(
        "The reviewer approved task \"{}\".\n\nTask:\n{}\n\nApproval:\n{}\n\nStage and commit only the relevant files, then report the commit hash and summary.",
        task.title, task.instruction, review_result
    )
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use multi_agent_protocol::{MultiAgentProvider, RoleAgentSpec, RoleSpec, WorkspaceSpec};

    use super::*;

    #[derive(Clone, Default)]
    struct FakeMessenger;

    #[async_trait]
    impl SessionMessenger for FakeMessenger {
        async fn send_to_session(
            &self,
            _session_id: &str,
            _message: &str,
        ) -> Result<(), OrchestratorError> {
            Ok(())
        }

        async fn request_response(
            &self,
            _session_id: &str,
            _message: &str,
            _mode: crate::SessionRequestMode,
        ) -> Result<String, OrchestratorError> {
            Ok(String::new())
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
                    id: "reviewer".to_string(),
                    name: "Reviewer".to_string(),
                    description: Some("Reviews changes".to_string()),
                    direct: Some(true),
                    output_root: Some("00-management/".to_string()),
                    agent: RoleAgentSpec {
                        description: "Reviews code".to_string(),
                        prompt: "Review".to_string(),
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
                    description: Some("Implements changes".to_string()),
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
            default_role_id: Some("reviewer".to_string()),
            coordinator_role_id: Some("reviewer".to_string()),
            claim_policy: None,
            activity_policy: None,
            workflow_vote_policy: None,
            workflow: None,
            artifacts: None,
            completion_policy: None,
        };

        let mut shell = WorkspaceShell::new(spec);
        shell.members.get_mut("reviewer").unwrap().session_id =
            Some("session-reviewer".to_string());
        shell.members.get_mut("coder").unwrap().session_id = Some("session-coder".to_string());
        shell
    }

    #[tokio::test]
    async fn first_message_creates_and_dispatches_first_task() {
        let shell = sample_shell();
        let messenger = FakeMessenger;
        let mut orchestrator = GatedTasksOrchestrator::default();

        let response = orchestrator
            .handle_user_message(&shell, &messenger, "Implement mentions RPC", None)
            .await
            .expect("first task should dispatch");

        assert_eq!(response.dispatches.len(), 1);
        assert_eq!(response.dispatches[0].role_id, "coder");
        assert_eq!(orchestrator.current_phase, GatedTasksPhase::Coding);
        assert_eq!(orchestrator.current_task_index, Some(0));
        assert_eq!(orchestrator.tasks.len(), 1);
        assert_eq!(orchestrator.tasks[0].status, GatedTaskStatus::Coding);
        assert_eq!(response.template_state["tasks"][0]["status"], "coding");
    }

    #[tokio::test]
    async fn rejection_loops_back_to_coder_with_feedback() {
        let shell = sample_shell();
        let messenger = FakeMessenger;
        let mut orchestrator =
            GatedTasksOrchestrator::with_tasks("reviewer", "coder", ["Implement mentions RPC"]);

        orchestrator
            .handle_user_message(&shell, &messenger, "start", None)
            .await
            .expect("task should start");
        orchestrator
            .on_role_completed(&shell, &messenger, "coder", "Patch ready", true)
            .await
            .expect("coder completion should dispatch review");

        let response = orchestrator
            .on_role_completed(
                &shell,
                &messenger,
                "reviewer",
                "REJECTED: add regression coverage",
                true,
            )
            .await
            .expect("review rejection should retry");

        assert_eq!(response.dispatches.len(), 1);
        assert_eq!(response.dispatches[0].role_id, "coder");
        assert_eq!(orchestrator.current_phase, GatedTasksPhase::Coding);
        assert_eq!(orchestrator.tasks[0].status, GatedTaskStatus::Coding);
        assert_eq!(
            orchestrator.tasks[0].feedback.as_deref(),
            Some("add regression coverage")
        );
        assert!(
            response.dispatches[0]
                .instruction
                .contains("add regression coverage")
        );
    }

    #[tokio::test]
    async fn approval_commits_and_advances_to_next_task() {
        let shell = sample_shell();
        let messenger = FakeMessenger;
        let mut orchestrator = GatedTasksOrchestrator::with_tasks(
            "reviewer",
            "coder",
            ["Implement mentions RPC", "Register missing bridge RPC"],
        );

        orchestrator
            .handle_user_message(&shell, &messenger, "start", None)
            .await
            .expect("task should start");
        orchestrator
            .on_role_completed(&shell, &messenger, "coder", "Patch ready", true)
            .await
            .expect("coder completion should dispatch review");
        orchestrator
            .on_role_completed(&shell, &messenger, "reviewer", "APPROVED: looks good", true)
            .await
            .expect("approval should dispatch commit");

        let response = orchestrator
            .on_role_completed(&shell, &messenger, "coder", "Committed as abc123", true)
            .await
            .expect("commit completion should advance");

        assert_eq!(orchestrator.tasks[0].status, GatedTaskStatus::Completed);
        assert_eq!(
            orchestrator.tasks[0].commit_result.as_deref(),
            Some("Committed as abc123")
        );
        assert_eq!(orchestrator.current_task_index, Some(1));
        assert_eq!(orchestrator.current_phase, GatedTasksPhase::Coding);
        assert_eq!(orchestrator.tasks[1].status, GatedTaskStatus::Coding);
        assert_eq!(response.dispatches.len(), 1);
        assert_eq!(response.dispatches[0].role_id, "coder");
        assert!(response.template_state["tasks"][0]["status"] == "completed");
    }

    #[tokio::test]
    async fn manual_commands_update_worklist_state() {
        let shell = sample_shell();
        let messenger = FakeMessenger;
        let mut orchestrator =
            GatedTasksOrchestrator::with_tasks("reviewer", "coder", ["Implement mentions RPC"]);

        orchestrator
            .handle_user_message(&shell, &messenger, "start", None)
            .await
            .expect("task should start");

        let add_response = orchestrator
            .handle_user_message(
                &shell,
                &messenger,
                "add task Register missing bridge RPC",
                None,
            )
            .await
            .expect("add should succeed");
        let pause_response = orchestrator
            .handle_user_message(&shell, &messenger, "pause", None)
            .await
            .expect("pause should succeed");
        let skip_response = orchestrator
            .handle_user_message(&shell, &messenger, "skip", None)
            .await
            .expect("skip should move to the next task");

        assert_eq!(orchestrator.tasks.len(), 2);
        assert_eq!(orchestrator.current_task_index, Some(1));
        assert_eq!(orchestrator.current_phase, GatedTasksPhase::Coding);
        assert_eq!(orchestrator.tasks[0].status, GatedTaskStatus::Skipped);
        assert_eq!(orchestrator.tasks[1].status, GatedTaskStatus::Coding);
        assert_eq!(add_response.dispatches.len(), 0);
        assert_eq!(pause_response.dispatches.len(), 0);
        assert_eq!(skip_response.dispatches.len(), 1);
    }
}
