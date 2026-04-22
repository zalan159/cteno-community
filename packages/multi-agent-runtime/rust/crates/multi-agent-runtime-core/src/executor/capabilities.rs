//! Capability descriptors for `AgentExecutor` implementations.
//!
//! Session-layer callers should inspect these **before** invoking optional
//! methods so they can gracefully degrade. Capabilities are informational —
//! methods still return `AgentExecutorError::Unsupported` when called against
//! an unsupported surface.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::types::{Effort, PermissionMode};

/// Shape of permission-mode support across vendors.
///
/// Legacy compatibility enum retained so existing adapter crates can keep
/// constructing `AgentCapabilities` unchanged. New core code should prefer
/// [`PermissionModeLifecycle`] via [`ExecutorSemanticCapabilities`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionModeKind {
    /// Mode can be toggled at runtime mid-session.
    Dynamic,
    /// Mode is fixed at spawn time only.
    Static,
    /// No permission-mode concept.
    None,
}

/// Stable lifecycle for how an executor supports permission modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionModeLifecycle {
    /// Permission mode can change on a live session.
    Dynamic,
    /// Permission mode is selected when the session starts.
    SessionBound,
    /// The executor has no permission-mode concept.
    Unsupported,
}

impl From<PermissionModeKind> for PermissionModeLifecycle {
    fn from(value: PermissionModeKind) -> Self {
        match value {
            PermissionModeKind::Dynamic => Self::Dynamic,
            PermissionModeKind::Static => Self::SessionBound,
            PermissionModeKind::None => Self::Unsupported,
        }
    }
}

/// Stable lifecycle for model selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSelectionLifecycle {
    /// The model can change on a live session.
    Dynamic,
    /// The model binding is fixed for the lifetime of a session.
    SessionBound,
}

/// Stable, cross-vendor model capability descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSupport {
    /// Whether a live session can accept model changes.
    pub lifecycle: ModelSelectionLifecycle,
    /// Explicit effort tiers, if the adapter has migrated to declare them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_efforts: Option<Vec<Effort>>,
}

/// Stable, cross-vendor permission capability descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionModeSupport {
    /// How permission mode behaves over the session lifecycle.
    pub lifecycle: PermissionModeLifecycle,
    /// Explicit permission modes, if the adapter has migrated to declare them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_modes: Option<Vec<PermissionMode>>,
}

/// Additive semantic capability surface for model / effort / permission logic.
///
/// This is derived from the legacy `AgentCapabilities` fields by default so
/// callers can move to the richer contract without forcing every adapter crate
/// to change in the same step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorSemanticCapabilities {
    pub model: ModelSupport,
    pub permission_mode: PermissionModeSupport,
}

impl ExecutorSemanticCapabilities {
    /// Derive the additive semantic contract from the legacy capability bits.
    pub fn from_legacy(capabilities: &AgentCapabilities) -> Self {
        Self {
            model: ModelSupport {
                lifecycle: if capabilities.supports_runtime_set_model {
                    ModelSelectionLifecycle::Dynamic
                } else {
                    ModelSelectionLifecycle::SessionBound
                },
                supported_efforts: None,
            },
            permission_mode: PermissionModeSupport {
                lifecycle: capabilities.permission_mode_kind.into(),
                supported_modes: None,
            },
        }
    }
}

/// Declarative capability manifest for an `AgentExecutor`.
///
/// All fields are plain-old-data so they can be serialised and shipped to the
/// frontend / RPC layer for UX gating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCapabilities {
    /// Vendor identifier (`"cteno"` / `"claude"` / `"codex"`).
    ///
    /// Stored as `Cow<'static, str>` so executors can construct a capability
    /// manifest with a zero-cost `&'static str` literal while the type still
    /// round-trips through `serde`.
    pub name: Cow<'static, str>,
    /// Semantic version of the wire protocol this executor speaks.
    pub protocol_version: Cow<'static, str>,
    /// Whether [`AgentExecutor::list_sessions`](super::trait_def::AgentExecutor::list_sessions) is implemented.
    pub supports_list_sessions: bool,
    /// Whether [`AgentExecutor::get_session_messages`](super::trait_def::AgentExecutor::get_session_messages) is implemented.
    pub supports_get_messages: bool,
    /// Legacy compatibility bit for runtime model mutation.
    ///
    /// New code should prefer
    /// [`AgentExecutor::semantic_capabilities`](super::trait_def::AgentExecutor::semantic_capabilities)
    /// so callers can distinguish stable core lifecycle semantics from older
    /// adapter-specific wiring.
    pub supports_runtime_set_model: bool,
    /// Legacy compatibility field describing permission-mode mutation timing.
    ///
    /// New code should prefer
    /// [`AgentExecutor::semantic_capabilities`](super::trait_def::AgentExecutor::semantic_capabilities)
    /// and read `permission_mode.lifecycle`.
    pub permission_mode_kind: PermissionModeKind,
    /// Whether resuming a previously-closed session is supported.
    pub supports_resume: bool,
    /// Whether multiple sessions can share a single subprocess.
    pub supports_multi_session_per_process: bool,
    /// Whether caller-injected tool specs (extra tools beyond built-ins) are honoured.
    pub supports_injected_tools: bool,
    /// Whether permission-closure flow (async decision callbacks) is available.
    pub supports_permission_closure: bool,
    /// Whether an in-flight turn can be interrupted via
    /// [`AgentExecutor::interrupt`](super::trait_def::AgentExecutor::interrupt).
    pub supports_interrupt: bool,
}

impl AgentCapabilities {
    /// Derive the additive model / effort / permission contract from the
    /// legacy capability manifest.
    pub fn semantic_capabilities(&self) -> ExecutorSemanticCapabilities {
        ExecutorSemanticCapabilities::from_legacy(self)
    }
}
