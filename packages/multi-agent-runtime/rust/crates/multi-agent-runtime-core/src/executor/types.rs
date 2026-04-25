//! Shared data types for the `AgentExecutor` trait.
//!
//! These types are vendor-agnostic. Vendor-specific payloads are either packed
//! into the `vendor_specific` escape hatches (free-form JSON) or emitted via
//! [`ExecutorEvent::NativeEvent`](super::event::ExecutorEvent::NativeEvent).

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Opaque identifier used by the vendor to address one of its own sessions.
///
/// Shape is vendor-defined — Cteno uses ULID-like ids, Claude uses UUIDv4,
/// Codex uses its own session handle. The session layer never parses this.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NativeSessionId(pub String);

impl NativeSessionId {
    /// Wrap an existing string id.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Borrow as `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NativeSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Token identifying a subprocess owned by the executor runtime.
///
/// The bare token is opaque to the session layer; the executor uses it to
/// look up the owning `Child` handle inside its own registry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProcessHandleToken(pub Uuid);

impl ProcessHandleToken {
    /// Generate a fresh token.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ProcessHandleToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifier for a reusable vendor connection (spawn + initialize handshake)
/// that can host multiple sessions. Registry keys its cache by this id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConnectionHandleId(pub Uuid);

impl ConnectionHandleId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ConnectionHandleId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ConnectionHandleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// Parameters for opening a reusable vendor connection.
///
/// Most vendors just need `env`. Callers issuing a lightweight health probe
/// set `probe = true` so adapters may short-circuit (skip auth flows, use
/// shorter timeouts).
#[derive(Debug, Clone, Default)]
pub struct ConnectionSpec {
    pub env: BTreeMap<String, String>,
    /// When true, caller only needs a handshake dry-run. Adapters MAY exit
    /// early once handshake confirms. Payload-heavy setup (MCP servers,
    /// initialize hooks) may be skipped.
    pub probe: bool,
}

/// Health status of a live connection. Returned by `check_connection`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionHealth {
    /// Transport still responsive.
    Healthy,
    /// Transport is dead (subprocess exited, stdin closed, handshake channel
    /// broken, etc.). Registry should re-open before next session start.
    Dead { reason: String },
}

/// Handle returned by `spawn_session` / `resume_session` that identifies a
/// live session plus the subprocess backing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    /// Vendor-native session id.
    pub id: NativeSessionId,
    /// Vendor name (`"cteno"` / `"claude"` / `"codex"`).
    pub vendor: &'static str,
    /// Executor-internal handle for the subprocess.
    pub process_handle: ProcessHandleToken,
    /// When the session was spawned (UTC).
    pub spawned_at: DateTime<Utc>,
    /// Absolute workspace directory the agent was launched in.
    pub workdir: PathBuf,
}

/// Permission-mode identifiers shared across vendors.
///
/// Not every vendor supports every mode — callers should check
/// [`AgentCapabilities`](super::capabilities::AgentCapabilities) first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Default prompting mode: every destructive action asks first.
    Default,
    /// Vendor-defined automatic permission selection.
    Auto,
    /// Auto-accept file-edit tool calls within workspace.
    AcceptEdits,
    /// Bypass all permission prompts (dangerous).
    BypassPermissions,
    /// Vendor-defined "never ask" mode.
    DontAsk,
    /// Plan-only mode; no tool execution.
    Plan,
    /// Read-only mode; no mutations.
    ReadOnly,
    /// Auto-allow writes within the configured workspace, prompt outside.
    WorkspaceWrite,
    /// Full system access with no sandboxing.
    DangerFullAccess,
}

impl PermissionMode {
    /// Stable core semantics for this permission mode.
    pub const fn semantics(self) -> PermissionModeSemantics {
        match self {
            Self::Default => PermissionModeSemantics {
                access_scope: PermissionAccessScope::WorkspaceWrite,
                prompt_behavior: PermissionPromptBehavior::OnRequest,
                allows_tool_calls: true,
                allows_mutation: true,
            },
            Self::Auto => PermissionModeSemantics {
                access_scope: PermissionAccessScope::VendorDefined,
                prompt_behavior: PermissionPromptBehavior::OnRequest,
                allows_tool_calls: true,
                allows_mutation: true,
            },
            Self::AcceptEdits => PermissionModeSemantics {
                access_scope: PermissionAccessScope::WorkspaceWrite,
                prompt_behavior: PermissionPromptBehavior::Never,
                allows_tool_calls: true,
                allows_mutation: true,
            },
            Self::BypassPermissions => PermissionModeSemantics {
                access_scope: PermissionAccessScope::FullAccess,
                prompt_behavior: PermissionPromptBehavior::Never,
                allows_tool_calls: true,
                allows_mutation: true,
            },
            Self::DontAsk => PermissionModeSemantics {
                access_scope: PermissionAccessScope::VendorDefined,
                prompt_behavior: PermissionPromptBehavior::Never,
                allows_tool_calls: true,
                allows_mutation: true,
            },
            Self::Plan => PermissionModeSemantics {
                access_scope: PermissionAccessScope::None,
                prompt_behavior: PermissionPromptBehavior::Disabled,
                allows_tool_calls: false,
                allows_mutation: false,
            },
            Self::ReadOnly => PermissionModeSemantics {
                access_scope: PermissionAccessScope::ReadOnly,
                prompt_behavior: PermissionPromptBehavior::Never,
                allows_tool_calls: true,
                allows_mutation: false,
            },
            Self::WorkspaceWrite => PermissionModeSemantics {
                access_scope: PermissionAccessScope::WorkspaceWrite,
                prompt_behavior: PermissionPromptBehavior::OnRequest,
                allows_tool_calls: true,
                allows_mutation: true,
            },
            Self::DangerFullAccess => PermissionModeSemantics {
                access_scope: PermissionAccessScope::FullAccess,
                prompt_behavior: PermissionPromptBehavior::Never,
                allows_tool_calls: true,
                allows_mutation: true,
            },
        }
    }

    /// Whether the mode permits tool execution at all.
    pub const fn allows_tool_calls(self) -> bool {
        self.semantics().allows_tool_calls
    }

    /// Whether the mode permits mutations.
    pub const fn allows_mutation(self) -> bool {
        self.semantics().allows_mutation
    }
}

/// Coarse access scope implied by a [`PermissionMode`].
///
/// `VendorDefined` intentionally preserves room for legacy modes whose exact
/// sandbox boundary differs by adapter; this keeps core semantics vendor-neutral
/// without pretending every mode maps to the same filesystem scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionAccessScope {
    /// No tool execution surface is available.
    None,
    /// Tool execution is constrained to read-only access.
    ReadOnly,
    /// Tool execution may write within the workspace boundary.
    WorkspaceWrite,
    /// Tool execution may access the full system.
    FullAccess,
    /// The mode exists in core, but the precise sandbox boundary is adapter-defined.
    VendorDefined,
}

/// Stable prompting behavior carried by a [`PermissionMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPromptBehavior {
    /// Tool execution is unavailable, so there is no approval surface.
    Disabled,
    /// Sensitive or out-of-scope actions may trigger an approval prompt.
    OnRequest,
    /// Tool calls run without interactive approval prompts.
    Never,
}

/// Stable, vendor-neutral semantics carried by a [`PermissionMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionModeSemantics {
    /// Coarse access boundary implied by the mode.
    pub access_scope: PermissionAccessScope,
    /// Whether the executor may still interrupt for approval.
    pub prompt_behavior: PermissionPromptBehavior,
    /// Whether the mode allows tools to run.
    pub allows_tool_calls: bool,
    /// Whether the mode allows mutating actions.
    pub allows_mutation: bool,
}

/// Decision for a pending permission prompt.
///
/// The first three variants are the cross-vendor baseline the cteno-agent
/// stdio protocol agreed on. `SelectedOption` is an additive variant for
/// vendors that expose their own option list (currently gemini's ACP
/// `session/request_permission.options[]`). Adapters that don't understand
/// the vendor-specific option id fall through to [`PermissionDecision::Allow`]
/// semantics — the string is opaque to non-supporting vendors.
///
/// Wire format mirrors the cteno-agent stdio convention: the simple variants
/// serialize as lowercase strings; `SelectedOption` serializes as
/// `{"type":"selected_option","option_id":"..."}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionDecision {
    /// Allow the tool call to proceed.
    Allow,
    /// Deny the tool call; the executor surfaces a ToolResult error.
    Deny,
    /// Abort the current turn entirely.
    Abort,
    /// Vendor-specific option id picked from the request's `options[]` array.
    /// Gemini ACP is the reference consumer; Claude / Codex / Cteno adapters
    /// may treat this as `Allow` if they don't expose equivalent semantics.
    SelectedOption {
        #[serde(rename = "option_id")]
        option_id: String,
    },
}

/// Model selection descriptor for `set_model` and `SpawnSessionSpec`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSpec {
    /// Provider id (e.g. `"anthropic"`, `"openai"`, `"deepseek"`).
    pub provider: String,
    /// Vendor-specific model id (e.g. `"claude-sonnet-4-5"`).
    pub model_id: String,
    /// Legacy compatibility field for provider-specific reasoning hints.
    ///
    /// New code should prefer [`ModelSpec::effort`] or [`NormalizedModelSpec`]
    /// so the core layer can carry stable cross-vendor effort semantics while
    /// preserving the original string on legacy adapters.
    pub reasoning_effort: Option<String>,
    /// Optional sampling temperature override.
    pub temperature: Option<f32>,
}

impl ModelSpec {
    /// Return the normalized core effort tier, if present.
    pub fn effort(&self) -> Option<Effort> {
        self.reasoning_effort
            .as_deref()
            .map(Effort::from_vendor_value)
    }

    /// Convert the legacy model spec into the additive normalized DTO.
    pub fn normalized(&self) -> NormalizedModelSpec {
        self.into()
    }
}

/// Stable cross-vendor reasoning effort tiers with a custom escape hatch.
///
/// This lives alongside `ModelSpec.reasoning_effort` so the core crate can
/// migrate additively without forcing every adapter to change in lockstep.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Effort {
    Minimal,
    Low,
    Medium,
    High,
    /// Provider-specific tier that does not map to a stable core preset yet.
    Custom(String),
}

impl Effort {
    /// Parse a provider or vendor string into the closest stable effort tier.
    pub fn from_vendor_value(value: impl Into<String>) -> Self {
        let raw = value.into();
        let trimmed = raw.trim();
        match trimmed.to_ascii_lowercase().as_str() {
            "minimal" => Self::Minimal,
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            _ => Self::Custom(trimmed.to_string()),
        }
    }

    /// Return the normalized string form used on the wire.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Custom(value) => value.as_str(),
        }
    }
}

impl Serialize for Effort {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Effort {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_vendor_value(value))
    }
}

impl From<&str> for Effort {
    fn from(value: &str) -> Self {
        Self::from_vendor_value(value)
    }
}

impl From<String> for Effort {
    fn from(value: String) -> Self {
        Self::from_vendor_value(value)
    }
}

impl From<Effort> for String {
    fn from(value: Effort) -> Self {
        value.as_str().to_string()
    }
}

/// Additive normalized model DTO for future adapter migration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedModelSpec {
    /// Provider id (e.g. `"anthropic"`, `"openai"`, `"deepseek"`).
    pub provider: String,
    /// Vendor-specific model id (e.g. `"claude-sonnet-4-5"`).
    pub model_id: String,
    /// Stable core reasoning effort tier, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    /// Optional sampling temperature override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

impl From<&ModelSpec> for NormalizedModelSpec {
    fn from(value: &ModelSpec) -> Self {
        Self {
            provider: value.provider.clone(),
            model_id: value.model_id.clone(),
            effort: value.effort(),
            temperature: value.temperature,
        }
    }
}

impl From<ModelSpec> for NormalizedModelSpec {
    fn from(value: ModelSpec) -> Self {
        Self::from(&value)
    }
}

impl From<NormalizedModelSpec> for ModelSpec {
    fn from(value: NormalizedModelSpec) -> Self {
        Self {
            provider: value.provider,
            model_id: value.model_id,
            reasoning_effort: value.effort.map(String::from),
            temperature: value.temperature,
        }
    }
}

/// Outcome of an attempt to change the model on a live session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelChangeOutcome {
    /// Change applied without disruption.
    Applied,
    /// The vendor requires closing and resuming the session to apply the change.
    RestartRequired {
        /// Human-readable reason explaining why a restart is needed.
        reason: String,
    },
    /// The vendor does not permit runtime model change.
    Unsupported,
}

/// Tool declared out-of-band by the caller and exposed to the agent for this turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectedToolSpec {
    /// Tool identifier.
    pub name: String,
    /// Human-readable description surfaced to the LLM.
    pub description: String,
    /// JSON-Schema for the tool's input payload.
    pub input_schema: serde_json::Value,
}

/// Classification of an attachment payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    /// Inline image (base64 or reference).
    Image,
    /// Text blob.
    Text,
    /// File reference (absolute path or URL).
    File,
    /// Free-form binary / other payload.
    Other,
}

/// Attachment bundled with a user message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Classification hint.
    pub kind: AttachmentKind,
    /// Declared MIME type, if known.
    pub mime_type: Option<String>,
    /// Source URI or file path.
    pub source: Option<String>,
    /// Inline payload (UTF-8 text or base64 binary).
    pub data: Option<String>,
}

/// User-originated input for `send_message`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserMessage {
    /// Primary text content of the message.
    pub content: String,
    /// Optional host task id for this turn. Vendors that can emit native ACP
    /// lifecycle records may use this to close the exact task started by the
    /// host UI.
    #[serde(default)]
    pub task_id: Option<String>,
    /// Optional attachments (images, files, etc.).
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// When the message is a tool-result follow-up, id of the originating
    /// tool-use that is being answered.
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    /// Additional caller-injected tools available for this turn only.
    #[serde(default)]
    pub injected_tools: Vec<InjectedToolSpec>,
}

/// Hint payload for `resume_session` — carries vendor-specific resume state
/// without leaking vendor details into the trait.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeHints {
    /// Opaque vendor-specific cursor / conversation id to splice back in.
    #[serde(default)]
    pub vendor_cursor: Option<String>,
    /// Optional absolute workdir override.
    #[serde(default)]
    pub workdir: Option<PathBuf>,
    /// Caller-provided metadata forwarded untouched to the vendor adapter.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

/// Full specification for spawning a brand-new agent session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpawnSessionSpec {
    /// Absolute workspace directory.
    pub workdir: PathBuf,
    /// Optional system prompt override.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Optional model selection at spawn time.
    #[serde(default)]
    pub model: Option<ModelSpec>,
    /// Permission mode the session should start in.
    pub permission_mode: PermissionMode,
    /// Explicit allow-list of tool ids; `None` means "all built-ins".
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Additional directories to expose to the agent beyond `workdir`.
    #[serde(default)]
    pub additional_directories: Vec<PathBuf>,
    /// Environment variables injected into the subprocess.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Free-form vendor-specific config (Cteno feature flags, Claude profile, etc.).
    #[serde(default)]
    pub agent_config: serde_json::Value,
    /// When present, the executor should resume from this native session
    /// instead of starting fresh.
    #[serde(default)]
    pub resume_hint: Option<ResumeHints>,
}

impl SpawnSessionSpec {
    /// Return the additive normalized model selection, if one was requested.
    pub fn normalized_model(&self) -> Option<NormalizedModelSpec> {
        self.model.as_ref().map(NormalizedModelSpec::from)
    }
}

/// Filter used when listing sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFilter {
    /// Restrict to the given workspace / project root.
    #[serde(default)]
    pub workdir: Option<PathBuf>,
    /// Restrict by session status.
    #[serde(default)]
    pub status: Option<SessionStatusFilter>,
    /// Maximum number of results.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Pagination cursor (vendor-defined).
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Session status buckets for filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatusFilter {
    /// Session is currently running.
    Active,
    /// Session has completed or been closed.
    Completed,
    /// Session terminated due to an error.
    Errored,
    /// Any status.
    Any,
}

/// Aggregate token usage accounting for a turn or session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input (prompt) tokens.
    pub input_tokens: u64,
    /// Output (completion) tokens.
    pub output_tokens: u64,
    /// Cache-creation tokens, if reported.
    #[serde(default)]
    pub cache_creation_tokens: u64,
    /// Cache-read tokens, if reported.
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// Reasoning / thinking tokens, if reported separately.
    #[serde(default)]
    pub reasoning_tokens: u64,
}

/// Lightweight summary of a session returned by `list_sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Vendor-native id.
    pub id: NativeSessionId,
    /// Workspace path the session is scoped to.
    pub workdir: PathBuf,
    /// When the session was first spawned.
    pub created_at: DateTime<Utc>,
    /// Latest activity timestamp.
    pub updated_at: DateTime<Utc>,
    /// Optional short title / first-message preview.
    #[serde(default)]
    pub title: Option<String>,
}

/// Full detail record returned by `get_session_info`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Base metadata.
    pub meta: SessionMeta,
    /// Current permission mode, if known.
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    /// Current model binding, if known.
    #[serde(default)]
    pub model: Option<ModelSpec>,
    /// Cumulative usage across the session.
    #[serde(default)]
    pub usage: TokenUsage,
    /// Vendor-specific extras.
    #[serde(default)]
    pub extras: serde_json::Value,
}

impl SessionInfo {
    /// Return the additive normalized model binding, if one is known.
    pub fn normalized_model(&self) -> Option<NormalizedModelSpec> {
        self.model.as_ref().map(NormalizedModelSpec::from)
    }
}

/// Pagination cursor for `get_session_messages`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pagination {
    /// Maximum page size.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Opaque cursor emitted by a previous call.
    #[serde(default)]
    pub cursor: Option<String>,
    /// If `true`, return oldest-first; otherwise newest-first.
    #[serde(default)]
    pub ascending: bool,
}

/// Native (per-vendor) message representation returned by `get_session_messages`.
///
/// The content is intentionally JSON — the app-level normalizer is responsible
/// for translating into UI / ACP shapes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeMessage {
    /// Stable id assigned by the vendor (or synthesized by the adapter).
    pub id: String,
    /// Role string as reported by the vendor (`"user"`, `"assistant"`, `"tool"`, …).
    pub role: String,
    /// Raw vendor payload untouched.
    pub payload: serde_json::Value,
    /// Timestamp reported by the vendor, if any.
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
}
