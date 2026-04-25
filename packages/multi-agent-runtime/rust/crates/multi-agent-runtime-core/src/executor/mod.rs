//! Unified session-level executor abstraction shared by all agent vendors.
//!
//! See `agent_executor_plan.md` §2 for the design rationale.
//!
//! This module contains **only** trait + data-type definitions. Concrete
//! implementations live in the vendor-specific crates:
//!
//! - `multi-agent-runtime-cteno`
//! - `multi-agent-runtime-claude`
//! - `multi-agent-runtime-codex`

pub mod capabilities;
pub mod error;
pub mod event;
pub mod session_store;
pub mod trait_def;
pub mod types;

pub use capabilities::{
    AgentCapabilities, ExecutorSemanticCapabilities, ModelSelectionLifecycle, ModelSupport,
    PermissionModeKind, PermissionModeLifecycle, PermissionModeSupport,
};
pub use error::AgentExecutorError;
pub use event::{DeltaKind, EventStream, ExecutorEvent};
pub use session_store::{SessionRecord, SessionStoreProvider};
pub use trait_def::{AgentExecutor, AutonomousTurnHandler, ConnectionHandle};
pub use types::{
    Attachment, AttachmentKind, ConnectionHandleId, ConnectionHealth, ConnectionSpec, Effort,
    InjectedToolSpec, ModelChangeOutcome, ModelSpec, NativeMessage, NativeSessionId,
    NormalizedModelSpec, Pagination, PermissionAccessScope, PermissionDecision, PermissionMode,
    PermissionModeSemantics, PermissionPromptBehavior, ProcessHandleToken, ResumeHints,
    SessionFilter, SessionInfo, SessionMeta, SessionRef, SessionStatusFilter, SpawnSessionSpec,
    TokenUsage, UserMessage,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn agent_capabilities_serializes() {
        let caps = AgentCapabilities {
            name: "cteno".into(),
            protocol_version: "0.1".into(),
            supports_list_sessions: true,
            supports_get_messages: true,
            supports_runtime_set_model: false,
            permission_mode_kind: PermissionModeKind::Static,
            supports_resume: true,
            supports_multi_session_per_process: false,
            supports_injected_tools: true,
            supports_permission_closure: true,
            supports_interrupt: true,
            autonomous_turn: false,
        };
        let json = serde_json::to_string(&caps).expect("ser");
        let decoded: AgentCapabilities = serde_json::from_str(&json).expect("de");
        assert_eq!(caps, decoded);
        assert!(json.contains("\"cteno\""));
        assert!(json.contains("\"static\""));
    }

    #[test]
    fn executor_error_display() {
        let e = AgentExecutorError::Unsupported {
            capability: "list_sessions".to_string(),
        };
        assert!(e.to_string().contains("list_sessions"));

        let e = AgentExecutorError::SessionNotFound("abc123".into());
        assert!(e.to_string().contains("abc123"));

        let e = AgentExecutorError::Timeout {
            operation: "spawn_session".into(),
            seconds: 30,
        };
        let s = e.to_string();
        assert!(s.contains("spawn_session"));
        assert!(s.contains("30"));
    }

    #[test]
    fn permission_decision_serde_lowercase() {
        // cteno-agent stdio protocol requires lowercase strings.
        assert_eq!(
            serde_json::to_string(&PermissionDecision::Allow).unwrap(),
            r#""allow""#
        );
        assert_eq!(
            serde_json::to_string(&PermissionDecision::Deny).unwrap(),
            r#""deny""#
        );
        assert_eq!(
            serde_json::to_string(&PermissionDecision::Abort).unwrap(),
            r#""abort""#
        );

        let d: PermissionDecision = serde_json::from_str(r#""allow""#).unwrap();
        assert_eq!(d, PermissionDecision::Allow);
    }

    #[test]
    fn permission_mode_snake_case() {
        assert_eq!(
            serde_json::to_string(&PermissionMode::AcceptEdits).unwrap(),
            r#""accept_edits""#
        );
        assert_eq!(
            serde_json::to_string(&PermissionMode::BypassPermissions).unwrap(),
            r#""bypass_permissions""#
        );
        assert_eq!(
            serde_json::to_string(&PermissionMode::DangerFullAccess).unwrap(),
            r#""danger_full_access""#
        );
    }

    #[test]
    fn permission_mode_kind_lowercase() {
        assert_eq!(
            serde_json::to_string(&PermissionModeKind::Dynamic).unwrap(),
            r#""dynamic""#
        );
        assert_eq!(
            serde_json::to_string(&PermissionModeKind::Static).unwrap(),
            r#""static""#
        );
        assert_eq!(
            serde_json::to_string(&PermissionModeKind::None).unwrap(),
            r#""none""#
        );
    }

    #[test]
    fn permission_semantic_capabilities_derive_from_legacy_fields() {
        let caps = AgentCapabilities {
            name: "claude".into(),
            protocol_version: "0.1".into(),
            supports_list_sessions: true,
            supports_get_messages: true,
            supports_runtime_set_model: false,
            permission_mode_kind: PermissionModeKind::Static,
            supports_resume: true,
            supports_multi_session_per_process: false,
            supports_injected_tools: false,
            supports_permission_closure: true,
            supports_interrupt: true,
            autonomous_turn: false,
        };

        let semantic = caps.semantic_capabilities();
        assert_eq!(
            semantic.model.lifecycle,
            ModelSelectionLifecycle::SessionBound
        );
        assert_eq!(
            semantic.permission_mode.lifecycle,
            PermissionModeLifecycle::SessionBound
        );
        assert!(semantic.model.supported_efforts.is_none());
        assert!(semantic.permission_mode.supported_modes.is_none());
    }

    #[test]
    fn model_change_outcome_tagged() {
        let applied = serde_json::to_string(&ModelChangeOutcome::Applied).unwrap();
        assert!(applied.contains("\"kind\":\"applied\""));
        let restart = serde_json::to_string(&ModelChangeOutcome::RestartRequired {
            reason: "codex cli needs cold start".into(),
        })
        .unwrap();
        assert!(restart.contains("\"kind\":\"restart_required\""));
        assert!(restart.contains("codex cli"));
    }

    #[test]
    fn effort_round_trips_as_string() {
        assert_eq!(serde_json::to_string(&Effort::High).unwrap(), r#""high""#);
        assert_eq!(
            serde_json::from_str::<Effort>(r#""medium""#).unwrap(),
            Effort::Medium
        );
        assert_eq!(
            serde_json::from_str::<Effort>(r#""VeryHigh""#).unwrap(),
            Effort::Custom("VeryHigh".to_string())
        );
    }

    #[test]
    fn model_spec_normalizes_effort() {
        let spec = ModelSpec {
            provider: "openai".into(),
            model_id: "gpt-5.4".into(),
            reasoning_effort: Some("high".into()),
            temperature: Some(0.2),
        };

        assert_eq!(spec.effort(), Some(Effort::High));

        let normalized = spec.normalized();
        assert_eq!(
            normalized,
            NormalizedModelSpec {
                provider: "openai".into(),
                model_id: "gpt-5.4".into(),
                effort: Some(Effort::High),
                temperature: Some(0.2),
            }
        );

        let legacy: ModelSpec = normalized.into();
        assert_eq!(legacy.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn permission_mode_semantics_capture_core_contract() {
        assert_eq!(
            PermissionMode::Plan.semantics(),
            PermissionModeSemantics {
                access_scope: PermissionAccessScope::None,
                prompt_behavior: PermissionPromptBehavior::Disabled,
                allows_tool_calls: false,
                allows_mutation: false,
            }
        );
        assert_eq!(
            PermissionMode::WorkspaceWrite.semantics(),
            PermissionModeSemantics {
                access_scope: PermissionAccessScope::WorkspaceWrite,
                prompt_behavior: PermissionPromptBehavior::OnRequest,
                allows_tool_calls: true,
                allows_mutation: true,
            }
        );
        assert_eq!(
            PermissionMode::ReadOnly.semantics().access_scope,
            PermissionAccessScope::ReadOnly
        );
        assert_eq!(
            PermissionMode::Default.semantics().prompt_behavior,
            PermissionPromptBehavior::OnRequest
        );
        assert_eq!(
            PermissionMode::AcceptEdits.semantics().prompt_behavior,
            PermissionPromptBehavior::Never
        );
        assert_eq!(
            PermissionMode::BypassPermissions.semantics(),
            PermissionModeSemantics {
                access_scope: PermissionAccessScope::FullAccess,
                prompt_behavior: PermissionPromptBehavior::Never,
                allows_tool_calls: true,
                allows_mutation: true,
            }
        );
    }

    #[test]
    fn spawn_session_spec_defaults() {
        let spec = SpawnSessionSpec {
            workdir: PathBuf::from("/tmp/ws"),
            system_prompt: None,
            model: None,
            permission_mode: PermissionMode::Default,
            allowed_tools: None,
            additional_directories: Vec::new(),
            env: Default::default(),
            agent_config: serde_json::Value::Null,
            resume_hint: None,
        };
        let json = serde_json::to_string(&spec).unwrap();
        let back: SpawnSessionSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
    }

    #[test]
    fn executor_event_round_trip_text_delta() {
        let ev = ExecutorEvent::StreamDelta {
            kind: DeltaKind::Text,
            content: "hello".into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"stream_delta\""));
        assert!(json.contains("\"kind\":\"text\""));
        let back: ExecutorEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn native_session_id_string_transparent() {
        let id = NativeSessionId::new("sess-abc");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, r#""sess-abc""#);
    }

    #[test]
    fn io_error_converts_to_executor_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: AgentExecutorError = io_err.into();
        assert!(matches!(err, AgentExecutorError::Io(_)));
    }

    /// Compile-time witness that `AgentExecutor` is object-safe.
    #[allow(dead_code)]
    fn assert_object_safe(_: &dyn AgentExecutor) {}
}
