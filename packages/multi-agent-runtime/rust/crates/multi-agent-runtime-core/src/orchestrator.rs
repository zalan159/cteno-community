use async_trait::async_trait;
use multi_agent_protocol::{TaskDispatch, WorkspaceActivity};
use serde_json::Value;
use thiserror::Error;

use crate::{RuntimeError, WorkspaceShell};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRequestMode {
    Work,
    Claim,
    WorkflowVote,
    CoordinatorDecision,
}

#[async_trait]
pub trait SessionMessenger: Send + Sync {
    async fn send_to_session(
        &self,
        session_id: &str,
        message: &str,
    ) -> Result<(), OrchestratorError>;

    async fn request_response(
        &self,
        session_id: &str,
        message: &str,
        mode: SessionRequestMode,
    ) -> Result<String, OrchestratorError>;
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OrchestratorResponse {
    pub activities: Vec<WorkspaceActivity>,
    pub dispatches: Vec<TaskDispatch>,
    pub template_state: Value,
}

#[derive(Debug, Error)]
pub enum OrchestratorError {
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("messaging error: {0}")]
    Messaging(String),
    #[error("invalid orchestrator state: {0}")]
    InvalidState(String),
    #[error("serialization error: {0}")]
    Serialization(String),
}

#[async_trait]
pub trait WorkspaceOrchestrator: Send + Sync {
    fn orchestrator_type(&self) -> &str;

    async fn handle_user_message(
        &mut self,
        shell: &WorkspaceShell,
        messenger: &dyn SessionMessenger,
        message: &str,
        target_role: Option<&str>,
    ) -> Result<OrchestratorResponse, OrchestratorError>;

    async fn on_role_completed(
        &mut self,
        shell: &WorkspaceShell,
        messenger: &dyn SessionMessenger,
        role_id: &str,
        result: &str,
        success: bool,
    ) -> Result<OrchestratorResponse, OrchestratorError>;

    fn template_state(&self) -> Value;

    fn serialize_state(&self) -> Value;

    fn restore_state(&mut self, state: Value) -> Result<(), OrchestratorError>;
}
