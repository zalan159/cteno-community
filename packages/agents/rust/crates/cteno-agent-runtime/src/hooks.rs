//! Runtime hook seams.
//!
//! The agent runtime is a library: it needs several capabilities from the host
//! (tool registry, run manager, browser, etc.) but must not depend on the host
//! crate's concrete types.  Each capability is exposed here as a `trait` +
//! `OnceCell` registry; the host installs impls during boot and the runtime
//! looks them up at call sites.
//!
//! Hook installation pattern (mirrored for every trait):
//!
//! ```ignore
//! pub fn install_foo(h: Arc<dyn FooProvider>) { let _ = FOO.set(h); }
//! pub fn foo() -> Option<Arc<dyn FooProvider>> { FOO.get().cloned() }
//! ```
//!
//! Call sites return a readable error when the hook is not installed, so
//! community builds / tests that skip a subset of providers still fail loudly
//! instead of panicking.

#![allow(deprecated)]

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::OnceCell;
use serde_json::Value;

// ---------------------------------------------------------------------------
// 2.1 ToolRegistryProvider
// ---------------------------------------------------------------------------

/// Surface the host's `ToolRegistry` so the ReAct loop and `tool_search`
/// executor can describe / invoke tools generically.
#[async_trait]
pub trait ToolRegistryProvider: Send + Sync {
    /// Execute a tool by name with the given JSON input.
    async fn execute(&self, tool_name: &str, input: Value) -> Result<String, String>;

    /// List all available tool names.
    async fn list_tools(&self) -> Vec<String>;

    /// Return a structured description (name / schema) of a tool, if known.
    async fn describe(&self, tool_name: &str) -> Option<Value>;
}

// ---------------------------------------------------------------------------
// 2.2 CommandInterceptor
// ---------------------------------------------------------------------------

/// Outcome from intercepting a slash command before it reaches the LLM.
#[derive(Debug, Clone)]
pub struct InterceptedOutcome {
    pub message: String,
    pub stop: bool,
}

#[async_trait]
pub trait CommandInterceptor: Send + Sync {
    async fn intercept(&self, session_id: &str, user_message: &str) -> Option<InterceptedOutcome>;
}

// ---------------------------------------------------------------------------
// 2.3 RunManagerHandle — removed in Wave 2.3b.
// `runs.rs` migrated to runtime in Wave 2.2a; all executors call
// `crate::runs::*` directly, so the trait + RunKind/RunSpec/RunSnapshot DTOs
// were never wired up and are gone.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 2.4 ResolvedUrlProvider
// ---------------------------------------------------------------------------

pub trait ResolvedUrlProvider: Send + Sync {
    fn happy_server_url(&self) -> String;
}

// ---------------------------------------------------------------------------
// 2.X CredentialsProvider (Wave 2.0 — account unification)
// ---------------------------------------------------------------------------
// Host-owned source of truth for the session's auth context: the accessToken
// used for all Happy Server RPC / Socket.IO calls, and the resolved user /
// machine identity. Single accessToken replaces the pre-2.0 bootstrap +
// machine + persistent user triad.
//
// Sub-process agent installs an stdio-backed impl whose slot is fed by the
// `init.auth_token` field and the `token_refreshed` Inbound message. In-
// process runtime installs a store-backed impl that reads from the host's
// AuthStore. Getter is sync because auth state is in-memory.

pub trait CredentialsProvider: Send + Sync {
    /// Current access token (30-minute ephemeral JWT). `None` if user not
    /// logged in. Callers that need cloud capabilities must surface a clear
    /// "not authenticated" error in this case — never panic / never forge.
    fn access_token(&self) -> Option<String>;

    /// Owning user id (opaque server-assigned string). `None` if not logged in.
    fn user_id(&self) -> Option<String>;

    /// This machine's id (host-assigned, stable across restarts). May be
    /// `Some` even when `access_token()` is `None` (machineId is generated
    /// pre-login).
    fn machine_id(&self) -> Option<String>;
}

// ---------------------------------------------------------------------------
// 2.5 SpawnConfigProvider
// ---------------------------------------------------------------------------

/// Minimal summary of spawn configuration (used by `wait` / `start_subagent`
/// executors). Host impl wraps `happy_client::manager::SpawnSessionConfig`.
#[async_trait]
pub trait SpawnConfigProvider: Send + Sync {
    async fn peek_session_message(&self, session_id: &str) -> Option<String>;
}

// ---------------------------------------------------------------------------
// 2.6 AgentOwnerProvider
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SessionOwner {
    pub owner_id: String,
    pub owner_name: Option<String>,
}

pub trait AgentOwnerProvider: Send + Sync {
    fn session_owner(&self, session_id: &str) -> Option<SessionOwner>;
    fn resolve_owner_name(&self, owner_id: &str) -> Option<String>;
    fn record_agent_reply(&self, session_id: &str, message: &str) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// 2.7 MachineSocketProvider — removed after the host-event-bus refactor.
// Producers that used to call `push_to_frontend(channel, payload)` now emit
// typed `HostEvent` values through `HostEventEmitter` /
// `cteno_host_runtime::events`.  See section 2.19.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 2.8 BrowserControlProvider — removed in Wave 2.3b.
// `browser/` migrated to runtime in Wave 2.2a; executors call
// `crate::browser::*` directly.  The trait and its NavigateOptions / NavResult
// / SnapshotMode / Snapshot / NetFilter / NetEntry DTOs never had any caller
// and are gone.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 2.11 SessionWaker
// ---------------------------------------------------------------------------

/// Wake a (possibly hibernated) session so its notification/poll loop can pick
/// up freshly-pushed events. Host impl calls into `happy_client` to reconnect
/// hibernated sessions; community build is a no-op.
///
/// Used by `crate::runs::RunManager` when background runs complete and need
/// to deliver a notification to a session that may have auto-hibernated.
#[async_trait]
pub trait SessionWaker: Send + Sync {
    async fn wake_session(&self, session_id: &str, label: &str) -> bool;
}

// ---------------------------------------------------------------------------
// 2.9 SkillRegistryProvider (Wave 2.3a)
// ---------------------------------------------------------------------------
// Used by the `skill` tool executor to load skills from disk (three-layer
// builtin/global/workspace lookup) and to talk to the remote SkillHub
// registry.  The host owns skill FS layout and network access, so these stay
// as trait methods.

use std::path::Path;

use crate::agent_config::SkillConfig;

#[async_trait]
pub trait SkillRegistryProvider: Send + Sync {
    /// Load all installed skills, merging builtin + global + workspace layers.
    /// `workspace_dir` is the `.cteno/skills` dir of the active workspace.
    fn load_all_skills(&self, workspace_dir: Option<&Path>) -> Vec<SkillConfig>;

    /// Return the set of locally-installed skill IDs (used to tag SkillHub
    /// results as `installed=true`).
    fn installed_skill_ids(&self) -> Vec<String>;

    /// Full-text search against SkillHub.  Returns raw JSON array of
    /// `SkillHubItem`-shaped entries.
    async fn search_skills(&self, query: &str, limit: usize) -> Result<Value, String>;

    /// Browse featured SkillHub entries.
    async fn fetch_featured(&self) -> Result<Value, String>;

    /// Install a skill from SkillHub into the user's global skills directory.
    async fn install_skill(&self, slug: &str) -> Result<Value, String>;
}

// ---------------------------------------------------------------------------
// 2.10 PersonaDispatchProvider (Wave 2.3a)
// ---------------------------------------------------------------------------
// Narrow seam for `skill` executor's fork-context path: when a skill declares
// `context: fork`, we hand the task off to `PersonaManager::dispatch_task`.
// The full PersonaManager surface stays in the app crate; only this single
// method is exposed as a trait.

#[async_trait]
pub trait PersonaDispatchProvider: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_task(
        &self,
        persona_id: &str,
        task_description: &str,
        workdir: Option<&str>,
        profile_id: Option<&str>,
        skill_ids: Option<Vec<String>>,
        agent_type: Option<&str>,
        label: Option<&str>,
    ) -> Result<String, String>;
}

// ---------------------------------------------------------------------------
// 2.12 A2uiStoreProvider (Wave 2.3a)
// ---------------------------------------------------------------------------
// Thin trait wrapping the host's `A2uiStore`.  The store is pure state, but
// it lives in the app crate today (`a2ui::A2uiStore`) and is shared with
// Tauri commands, so the runtime accesses it through this trait.

pub trait A2uiStoreProvider: Send + Sync {
    fn create_surface(&self, agent_id: &str, surface_id: &str, catalog_id: &str) -> u64;
    fn update_components(
        &self,
        agent_id: &str,
        surface_id: &str,
        components: Vec<Value>,
    ) -> Result<u64, String>;
    fn update_data_model(
        &self,
        agent_id: &str,
        surface_id: &str,
        data: Value,
    ) -> Result<u64, String>;
    fn delete_surface(&self, agent_id: &str, surface_id: &str) -> bool;
}

// ---------------------------------------------------------------------------
// 2.13 SubagentBootstrapProvider (Wave 2.3a)
// ---------------------------------------------------------------------------
// Used only by `start_subagent` executor.  Encapsulates the messy profile /
// proxy-auth / db-path resolution that would otherwise force us to expose
// SpawnSessionConfig + profile_store + proxy_profiles + machine_auth_token
// through individual hook methods.  Host impl returns a pre-built
// `(AgentConfig, SubAgentContext)` tuple; executor just spawns.

use crate::agent::executor::SubAgentContext;
use crate::agent_config::AgentConfig;

#[async_trait]
pub trait SubagentBootstrapProvider: Send + Sync {
    /// Resolve `(agent_config, subagent_context)` for spawning a subagent.
    /// - `agent_id` selects the AgentConfig from the builtin+global registry.
    /// - `parent_session_id` is used to inherit `profile_id` from the parent
    ///   session's context when `override_profile_id` is `None`.
    async fn build_subagent_context(
        &self,
        agent_id: &str,
        parent_session_id: &str,
        override_profile_id: Option<&str>,
    ) -> Result<(AgentConfig, SubAgentContext), String>;
}

// ---------------------------------------------------------------------------
// 2.14 NotificationDeliveryProvider (Wave 3.2b)
// ---------------------------------------------------------------------------
// Used by the macOS `notification_watcher` background loop to route an
// incoming system notification to a Persona's chat session.  The watcher
// itself only depends on the runtime (rusqlite / chrono / plist), but the
// actual delivery path needs PersonaManager + SpawnSessionConfig, which live
// in the app crate.  Exposing a single "deliver" entry point keeps the watcher
// host-agnostic.

#[async_trait]
pub trait NotificationDeliveryProvider: Send + Sync {
    /// Deliver a formatted notification to the given persona's chat session.
    /// `app_display_name` is already localised ("微信" / "企业微信" / ...);
    /// `title` and `body` come straight from the macOS notification record.
    async fn deliver_to_persona(
        &self,
        persona_id: &str,
        app_display_name: &str,
        title: &str,
        body: &str,
    );
}

// ---------------------------------------------------------------------------
// 2.18 LocalNotificationProvider (Wave 3.4a)
// ---------------------------------------------------------------------------
// Used by `push_notification` to emit desktop notifications via the host app
// process (e.g. Tauri plugin notification), so the OS sender identity and
// app icon are correct. Runtime keeps a fallback `osascript` / PowerShell
// path when this hook is unavailable.

pub trait LocalNotificationProvider: Send + Sync {
    fn send_local_notification(&self, title: &str, body: &str) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Global registry (one OnceCell per trait)
// ---------------------------------------------------------------------------

macro_rules! hook_slot {
    ($static_name:ident, $trait_ty:ty, $install_fn:ident, $getter:ident) => {
        static $static_name: OnceCell<Arc<$trait_ty>> = OnceCell::new();

        pub fn $install_fn(h: Arc<$trait_ty>) {
            let _ = $static_name.set(h);
        }

        pub fn $getter() -> Option<Arc<$trait_ty>> {
            $static_name.get().cloned()
        }
    };
}

hook_slot!(
    TOOL_REGISTRY,
    dyn ToolRegistryProvider,
    install_tool_registry,
    tool_registry
);
hook_slot!(
    COMMAND_INTERCEPTOR,
    dyn CommandInterceptor,
    install_command_interceptor,
    command_interceptor
);
hook_slot!(
    URL_PROVIDER,
    dyn ResolvedUrlProvider,
    install_url_provider,
    url_provider
);
hook_slot!(
    SPAWN_CONFIG,
    dyn SpawnConfigProvider,
    install_spawn_config,
    spawn_config
);
hook_slot!(
    AGENT_OWNER,
    dyn AgentOwnerProvider,
    install_agent_owner,
    agent_owner
);
hook_slot!(
    SKILL_REGISTRY,
    dyn SkillRegistryProvider,
    install_skill_registry,
    skill_registry
);
hook_slot!(
    PERSONA_DISPATCH,
    dyn PersonaDispatchProvider,
    install_persona_dispatch,
    persona_dispatch
);
hook_slot!(
    A2UI_STORE,
    dyn A2uiStoreProvider,
    install_a2ui_store,
    a2ui_store
);
hook_slot!(
    SUBAGENT_BOOTSTRAP,
    dyn SubagentBootstrapProvider,
    install_subagent_bootstrap,
    subagent_bootstrap
);
hook_slot!(
    SESSION_WAKER,
    dyn SessionWaker,
    install_session_waker,
    session_waker
);
hook_slot!(
    NOTIFICATION_DELIVERY,
    dyn NotificationDeliveryProvider,
    install_notification_delivery,
    notification_delivery
);
hook_slot!(
    CREDENTIALS,
    dyn CredentialsProvider,
    install_credentials,
    credentials
);
hook_slot!(
    LOCAL_NOTIFICATION,
    dyn LocalNotificationProvider,
    install_local_notification,
    local_notification
);

/// Convenience helper: resolved happy-server URL, or a compiled fallback.
pub fn resolved_happy_server_url() -> String {
    url_provider()
        .map(|p| p.happy_server_url())
        .unwrap_or_else(|| {
            std::env::var("HAPPY_SERVER_URL")
                .unwrap_or_else(|_| "https://cteno.frontfidelity.cn".to_string())
        })
}

// ---------------------------------------------------------------------------
// Concrete ToolRegistry handle (used by `tool_search` executor, which needs
// direct access to ToolRegistry methods like `search_deferred_tools` that
// return `crate::llm::Tool` — too rich to reasonably re-declare on a trait).
//
// `ToolRegistry` itself already lives in this crate (since Wave 1), so the
// host's `Arc<RwLock<ToolRegistry>>` is runtime-native and can be handed in
// directly.
// ---------------------------------------------------------------------------

use tokio::sync::RwLock as AsyncRwLock;

use crate::tool::registry::ToolRegistry;

static TOOL_REGISTRY_HANDLE: OnceCell<Arc<AsyncRwLock<ToolRegistry>>> = OnceCell::new();

pub fn install_tool_registry_handle(reg: Arc<AsyncRwLock<ToolRegistry>>) {
    let _ = TOOL_REGISTRY_HANDLE.set(reg);
}

pub fn tool_registry_handle() -> Option<Arc<AsyncRwLock<ToolRegistry>>> {
    TOOL_REGISTRY_HANDLE.get().cloned()
}

// ---------------------------------------------------------------------------
// Generic HostCallDispatcher (Wave 3 — stdio subprocess support)
// ---------------------------------------------------------------------------
//
// Background: every hook trait above is invoked in-process by runtime code,
// and its impl lives either in the app crate (Tauri host) or in the stdio
// binary (`cteno-agent`). When the runtime runs inside a subprocess, those
// impls can no longer be in-process — they must round-trip through the parent
// host. Writing a bespoke protocol message pair per hook (as we did for
// `ToolExecutionRequest/Response`) doesn't scale to 12+ traits.
//
// `HostCallDispatcher` is the generic seam: in-runtime hook impls register a
// dispatcher at boot and, when asked to do something, encode
// `(hook_name, method, params)` as JSON and hand it to the dispatcher. The
// dispatcher is responsible for delivering the call across whatever boundary
// exists (stdio, IPC, direct in-process re-dispatch, ...) and returning the
// method's return value as JSON.
//
// This trait intentionally stays minimal: it does not know about any specific
// hook. Hook-specific marshalling lives in per-hook adapter modules that sit
// on top of this dispatcher (not added by this commit).

/// Generic seam for proxying hook method calls out of the runtime. Installed
/// once at boot by the host; consulted by in-runtime hook adapters that
/// need to round-trip calls to the parent process.
#[async_trait]
pub trait HostCallDispatcher: Send + Sync {
    /// Execute a hook method out-of-process. Returns the method's JSON-encoded
    /// return value on success, or a short error string on failure.
    ///
    /// - `session_id` identifies the session the call belongs to (may be
    ///   empty for global hooks).
    /// - `hook_name` is the logical hook family, e.g. `"agent_owner"`,
    ///   `"skillhub"`, `"local_notification"`.
    /// - `method` is the method within that family, e.g. `"session_owner"`,
    ///   `"list_skills"`, `"send_local_notification"`.
    /// - `params` is an arbitrary JSON object whose shape is defined by the
    ///   adapter for `(hook_name, method)`.
    async fn call(
        &self,
        session_id: &str,
        hook_name: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String>;
}

static HOST_CALL: OnceCell<Arc<dyn HostCallDispatcher>> = OnceCell::new();

/// Install the process-wide host-call dispatcher. First installer wins; later
/// calls are silently ignored so tests / smoke runs don't panic on reinit.
pub fn install_host_call(d: Arc<dyn HostCallDispatcher>) {
    let _ = HOST_CALL.set(d);
}

/// Look up the installed host-call dispatcher, if any. Hook adapters treat a
/// `None` return as "no out-of-process path" and fall back to in-process
/// behaviour (or return a clear error).
pub fn host_call() -> Option<Arc<dyn HostCallDispatcher>> {
    HOST_CALL.get().cloned()
}

// ---------------------------------------------------------------------------
// 2.15 AgentKindResolver (Wave 3.3a)
// ---------------------------------------------------------------------------
// Session-id → AgentKind resolution lives on the app side because it queries
// the `persona_sessions` table and may surface persona metadata.  Runtime
// callers that need the kind (without depending on persona types directly)
// go through this trait.
//
// Persona data is returned as opaque `serde_json::Value` bundles so the
// runtime does not need to depend on `crate::persona::models::*` (which only
// exists in the app crate).

use crate::agent_kind::AgentKind;

#[derive(Debug, Clone)]
pub struct AgentKindResolution {
    pub kind: AgentKind,
    /// Persona session link (shape: `PersonaSessionLink` JSON), if any.
    pub persona_link: Option<Value>,
    /// Owning / Chat persona (shape: `Persona` JSON), if any.
    pub persona: Option<Value>,
}

#[async_trait]
pub trait AgentKindResolver: Send + Sync {
    /// Resolve the agent kind for the given session id.  Always returns a
    /// resolution — if no persona link exists, implementations fall back to
    /// `AgentKind::Worker`.
    async fn resolve(&self, session_id: &str) -> Result<AgentKindResolution, String>;
}

static AGENT_KIND_RESOLVER: OnceCell<Arc<dyn AgentKindResolver>> = OnceCell::new();

pub fn install_agent_kind_resolver(r: Arc<dyn AgentKindResolver>) {
    let _ = AGENT_KIND_RESOLVER.set(r);
}

pub fn agent_kind_resolver() -> Option<Arc<dyn AgentKindResolver>> {
    AGENT_KIND_RESOLVER.get().cloned()
}

// ---------------------------------------------------------------------------
// 2.16 HeadlessAuthPathProvider (Wave 3.3c)
// ---------------------------------------------------------------------------
// Headless account-auth storage lives on disk under a host-owned app data
// directory (macOS ~/Library/Application Support/cteno/..., Linux XDG, etc.).
// The path resolution is entirely a host concern (`host::core::default_headless_app_data_dir`),
// so the runtime exposes a narrow hook for callers that need to inspect or
// clear the account-auth store.
//
// The auth file format + crypto flows stay on the app side because they
// depend on `cteno-happy-client-core` (commercial-only). This seam is only
// for runtime code that needs the *location* of the headless app data dir.

use std::path::PathBuf;

pub trait HeadlessAuthPathProvider: Send + Sync {
    /// Return the absolute path of the headless app data directory (may not
    /// yet exist — callers should `create_dir_all` as needed).
    fn headless_auth_dir(&self) -> Result<PathBuf, String>;
}

static HEADLESS_AUTH_PATH: OnceCell<Arc<dyn HeadlessAuthPathProvider>> = OnceCell::new();

pub fn install_headless_auth_path(p: Arc<dyn HeadlessAuthPathProvider>) {
    let _ = HEADLESS_AUTH_PATH.set(p);
}

pub fn headless_auth_path() -> Option<Arc<dyn HeadlessAuthPathProvider>> {
    HEADLESS_AUTH_PATH.get().cloned()
}

// ---------------------------------------------------------------------------
// 2.17 LlmKeyProvider — OpenRouter subkey access for proxy profiles
// ---------------------------------------------------------------------------
// Proxy profiles (`is_proxy_profile()` = true) route LLM traffic directly to
// `openrouter.ai`. The key is a per-user subkey issued by happy-server via
// the OpenRouter Provisioning API and cached locally (see
// `cteno-host-runtime::LlmKeyStore`). This hook exposes the current
// plaintext key to the profile resolver without the runtime depending on
// either the transport crate or the host crate.
//
// The app-side bridge is trivial: impl `LlmKeyProvider::current()` by
// calling `cteno_happy_client_transport::current_llm_key()` (or directly
// reading the LlmKeyStore).

pub trait LlmKeyProvider: Send + Sync {
    /// Return the current cached OpenRouter subkey plaintext, or `None`
    /// when unavailable (not logged in / never fetched / balance depleted).
    fn current(&self) -> Option<String>;
}

static LLM_KEY_PROVIDER: OnceCell<Arc<dyn LlmKeyProvider>> = OnceCell::new();

pub fn install_llm_key_provider(p: Arc<dyn LlmKeyProvider>) {
    let _ = LLM_KEY_PROVIDER.set(p);
}

pub fn llm_key_provider() -> Option<Arc<dyn LlmKeyProvider>> {
    LLM_KEY_PROVIDER.get().cloned()
}

pub fn current_llm_key() -> Option<String> {
    llm_key_provider().and_then(|p| p.current())
}

// ---------------------------------------------------------------------------
// 2.19 HostEventEmitter — bridge from session-internal code to the host
// event bus (`cteno_host_runtime::events`).
// ---------------------------------------------------------------------------
// Session-internal executors (currently `a2ui_render`) that need to emit a
// host-level domain event go through this trait.  The runtime crate must not
// reverse-depend on the host crate, so the hook surface stays typed per
// event — no generic `push_to_frontend(channel, payload)` stringly-typed
// envelope.  The desktop app installs an impl that forwards to
// `cteno_host_runtime::events::emit(...)`.

#[async_trait]
pub trait HostEventEmitter: Send + Sync {
    /// An `a2ui_render` batch committed changes to the agent's surface.
    async fn emit_a2ui_updated(&self, agent_id: &str);
}

static HOST_EVENT_EMITTER: OnceCell<Arc<dyn HostEventEmitter>> = OnceCell::new();

pub fn install_host_event_emitter(e: Arc<dyn HostEventEmitter>) {
    let _ = HOST_EVENT_EMITTER.set(e);
}

pub fn host_event_emitter() -> Option<Arc<dyn HostEventEmitter>> {
    HOST_EVENT_EMITTER.get().cloned()
}

// ---------------------------------------------------------------------------
// 2.20 TaskGraphEventEmitter — session-internal DAG lifecycle events.
// ---------------------------------------------------------------------------

#[async_trait]
pub trait TaskGraphEventEmitter: Send + Sync {
    async fn emit_task_graph_event(&self, session_id: &str, event: &str, payload: Value);
}

static TASK_GRAPH_EVENT_EMITTER: OnceCell<Arc<dyn TaskGraphEventEmitter>> = OnceCell::new();

pub fn install_task_graph_event_emitter(e: Arc<dyn TaskGraphEventEmitter>) {
    let _ = TASK_GRAPH_EVENT_EMITTER.set(e);
}

pub fn task_graph_event_emitter() -> Option<Arc<dyn TaskGraphEventEmitter>> {
    TASK_GRAPH_EVENT_EMITTER.get().cloned()
}

// ---------------------------------------------------------------------------
// 2.21 SubAgentLifecycleEmitter — push subagent state transitions out of
// the runtime so the host can mirror a SubAgent registry and surface live
// progress in the UI (BackgroundRunsModal). The runtime owns the canonical
// SubAgentManager state; the host-side mirror exists purely so the
// frontend can subscribe to lifecycle Tauri events without polling RPC.
// ---------------------------------------------------------------------------

/// Vendor-neutral lifecycle DTO. Crosses the runtime ↔ host boundary; the
/// stdio crate translates this into `Outbound::SubAgentLifecycle` wire
/// frames, the host-side dispatcher then translates the wire frame into
/// the desktop's `SessionEventSink::on_subagent_lifecycle` call.
///
/// Stays an opaque struct (rather than re-exporting `crate::subagent::SubAgent`)
/// so the runtime stays agnostic about wire encoding and host adapters
/// stay agnostic about the runtime's internal SubAgent struct shape.
#[derive(Debug, Clone)]
pub enum SubAgentLifecycleEventDto {
    Spawned {
        subagent_id: String,
        agent_id: String,
        task: String,
        label: Option<String>,
        created_at_ms: i64,
    },
    Started {
        subagent_id: String,
        started_at_ms: i64,
    },
    Updated {
        /// Periodic progress beacon. Optional; emitter may throttle these.
        subagent_id: String,
        iteration_count: u32,
    },
    Completed {
        subagent_id: String,
        result: Option<String>,
        completed_at_ms: i64,
    },
    Failed {
        subagent_id: String,
        error: String,
        completed_at_ms: i64,
    },
    Stopped {
        subagent_id: String,
        completed_at_ms: i64,
    },
}

pub trait SubAgentLifecycleEmitter: Send + Sync {
    /// Best-effort fire-and-forget; never blocks the SubAgentManager.
    /// Hosts that don't care (e.g. tests, embedded library use) install
    /// nothing and the runtime silently skips emission.
    fn emit(&self, parent_session_id: &str, event: SubAgentLifecycleEventDto);
}

static SUBAGENT_LIFECYCLE_EMITTER: OnceCell<Arc<dyn SubAgentLifecycleEmitter>> = OnceCell::new();

pub fn install_subagent_lifecycle_emitter(e: Arc<dyn SubAgentLifecycleEmitter>) {
    let _ = SUBAGENT_LIFECYCLE_EMITTER.set(e);
}

pub fn subagent_lifecycle_emitter() -> Option<Arc<dyn SubAgentLifecycleEmitter>> {
    SUBAGENT_LIFECYCLE_EMITTER.get().cloned()
}
