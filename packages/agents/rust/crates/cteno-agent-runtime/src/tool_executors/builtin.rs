//! Batch registration of runtime-native built-in tool executors.
//!
//! This is the single source of truth for the list of tools that the runtime
//! can construct without any host-side hooks beyond what [`crate::hooks`]
//! ships. It is called from:
//!
//! - `cteno-agent-stdio` (standalone binary): populates its ToolRegistry with
//!   the full built-in set so the ReAct loop has real tools.
//! - Future Tauri host (`apps/client/desktop`): can call the same function to
//!   avoid duplicating the tool inventory; host-specific tools (memory,
//!   skill, wait, a2ui_render, start_subagent, dispatch_task, …) are
//!   registered separately with their respective provider hooks.
//!
//! Tools that *require* a host provider (SkillRegistryProvider,
//! PersonaDispatchProvider, A2uiStoreProvider, MachineSocketProvider,
//! SpawnConfigProvider, SessionWaker, SubagentBootstrapProvider,
//! AgentOwnerProvider, CommandInterceptorProvider) are intentionally absent
//! from this list. Registering them without their hooks would surface
//! "hook not installed" errors at call time — better to omit them entirely
//! so the LLM never sees them.

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use serde_json::{json, Value};
use tokio::sync::RwLock as TokioRwLock;

use crate::browser::BrowserManager;
use crate::llm_profile::{ApiFormat, LlmEndpoint, LlmProfile, ProfileStore};
use crate::runs::RunManager;
use crate::tool::registry::ToolRegistry;
use crate::tool::{ToolCategory, ToolConfig};
use crate::tool_executors::oss_upload::OssUploader;
use crate::tool_executors::{
    BrowserActionExecutor, BrowserAdapterExecutor, BrowserCdpExecutor, BrowserManageExecutor,
    BrowserNavigateExecutor, BrowserNetworkExecutor, ComputerUseExecutor, CoordScale, EditExecutor,
    FetchExecutor, GetSessionOutputExecutor, GlobExecutor, GrepExecutor, ImageGenerationExecutor,
    QuerySubAgentExecutor, ReadExecutor, RunManagerExecutor, ScreenshotExecutor, ShellExecutor,
    StopSubAgentExecutor, ToolSearchExecutor, UpdatePlanExecutor, UploadArtifactExecutor,
    WebSearchExecutor, WriteExecutor,
};

/// Register every runtime-native built-in executor into `registry`.
///
/// * `data_dir` — writable scratch/state directory (used for runs, machine
///   auth cache, websearch cache, adapter defaults, etc.).
/// * `run_manager` — shared background-run manager (ShellExecutor,
///   RunManagerExecutor, UploadArtifactExecutor and ImageGenerationExecutor
///   all write into it).
///
/// Returns the number of tools registered (matches `registry.count()` when
/// the registry started empty).
pub fn register_all_builtin_executors(
    registry: &mut ToolRegistry,
    data_dir: PathBuf,
    run_manager: Arc<RunManager>,
) -> usize {
    let start = registry.count();

    // --- plain file / shell / search tools ---
    registry.register(
        sys("shell", "Execute a shell command.", shell_schema()),
        Arc::new(ShellExecutor::new(run_manager.clone())),
    );
    registry.register(
        sys("read", "Read a file from disk.", read_schema()),
        Arc::new(ReadExecutor::new()),
    );
    registry.register(
        sys(
            "write",
            "Write content to a file, creating it if necessary.",
            write_schema(),
        ),
        Arc::new(WriteExecutor::new()),
    );
    registry.register(
        sys(
            "edit",
            "Apply a string replacement edit to a file.",
            edit_schema(),
        ),
        Arc::new(EditExecutor::new()),
    );
    registry.register(
        sys("grep", "Search files for a regex pattern.", grep_schema()),
        Arc::new(GrepExecutor::new()),
    );
    registry.register(
        sys("glob", "Find files matching a glob pattern.", glob_schema()),
        Arc::new(GlobExecutor::new()),
    );

    // --- network / fetch ---
    // FetchExecutor wants a ProfileStore + a global api-key handle; for
    // hook-less runtime use we install empty stubs. The public unauth path
    // works fine for basic HTTP fetches.
    registry.register(
        sys(
            "fetch",
            "HTTP GET / POST a URL and return the response body.",
            fetch_schema(),
        ),
        Arc::new(FetchExecutor::new(
            stub_profile_store(),
            Arc::new(TokioRwLock::new(String::new())),
        )),
    );
    registry.register(
        sys("websearch", "Run a web search.", websearch_schema()),
        Arc::new(WebSearchExecutor::new(data_dir.clone())),
    );

    // --- planning ---
    registry.register(
        sys(
            "update_plan",
            "Update the agent's task plan (TodoWrite equivalent).",
            update_plan_schema(),
        ),
        Arc::new(UpdatePlanExecutor::new()),
    );

    // --- background-run management ---
    registry.register(
        sys(
            "run_manager",
            "Manage background runs (list / get / logs / cancel).",
            run_manager_schema(),
        ),
        Arc::new(RunManagerExecutor::new(run_manager.clone())),
    );

    // --- tool discovery ---
    registry.register(
        sys(
            "tool_search",
            "Discover deferred tools on demand.",
            tool_search_schema(),
        ),
        Arc::new(ToolSearchExecutor::new()),
    );

    // --- session introspection ---
    registry.register(
        sys(
            "get_session_output",
            "Retrieve the last N messages from a task session.",
            get_session_output_schema(),
        ),
        Arc::new(GetSessionOutputExecutor::new(data_dir.clone())),
    );

    // --- subagent query / stop (start_subagent lives host-side) ---
    registry.register(
        sys(
            "query_subagent",
            "Query status and results of SubAgent tasks.",
            query_subagent_schema(),
        ),
        Arc::new(QuerySubAgentExecutor::new()),
    );
    registry.register(
        sys(
            "stop_subagent",
            "Stop a running SubAgent task.",
            stop_subagent_schema(),
        ),
        Arc::new(StopSubAgentExecutor::new()),
    );

    // --- happy-server-assisted tools ---
    // Both need machine_auth.json + HAPPY_SERVER_URL at call time, but the
    // executor constructors themselves are cheap. If those are missing the
    // execute() call returns a clear error.
    registry.register(
        sys(
            "upload_artifact",
            "Upload a local file to object storage via happy-server STS.",
            upload_artifact_schema(),
        ),
        Arc::new(UploadArtifactExecutor::new(
            run_manager.clone(),
            data_dir.clone(),
        )),
    );
    registry.register(
        sys(
            "image_generation",
            "Generate an image via happy-server image proxy (background).",
            image_generation_schema(),
        ),
        Arc::new(ImageGenerationExecutor::new(
            run_manager.clone(),
            data_dir.clone(),
        )),
    );

    // --- screenshot + computer_use (share CoordScale + OssUploader) ---
    let shared_oss_uploader = Arc::new(OssUploader::new(data_dir.clone()));
    let shared_coord_scale = Arc::new(StdMutex::new(CoordScale::default()));

    registry.register(
        sys(
            "screenshot",
            "Capture the desktop screen as a PNG and upload to object storage.",
            json!({"type":"object","properties":{}}),
        ),
        Arc::new(ScreenshotExecutor::new(
            data_dir.clone(),
            shared_coord_scale.clone(),
            shared_oss_uploader.clone(),
        )),
    );
    registry.register(
        sys(
            "computer_use",
            "Simulate mouse and keyboard actions on the desktop.",
            computer_use_schema(),
        ),
        Arc::new(ComputerUseExecutor::new(
            data_dir.clone(),
            shared_coord_scale.clone(),
        )),
    );

    // --- browser tools (shared BrowserManager) ---
    let browser_manager = Arc::new(BrowserManager::new());
    registry.register(
        sys(
            "browser_navigate",
            "Open a URL in Chrome via CDP.",
            browser_navigate_schema(),
        ),
        Arc::new(BrowserNavigateExecutor::new(browser_manager.clone())),
    );
    registry.register(
        sys(
            "browser_action",
            "Interactive browser action (click, type, scroll, ...).",
            action_schema(),
        ),
        Arc::new(BrowserActionExecutor::new(
            browser_manager.clone(),
            data_dir.clone(),
            shared_oss_uploader.clone(),
        )),
    );
    registry.register(
        sys(
            "browser_manage",
            "Manage browser tabs and lifecycle.",
            action_schema(),
        ),
        Arc::new(BrowserManageExecutor::new(browser_manager.clone())),
    );
    registry.register(
        sys(
            "browser_network",
            "Monitor browser network requests via CDP.",
            action_schema(),
        ),
        Arc::new(BrowserNetworkExecutor::new(browser_manager.clone())),
    );
    registry.register(
        sys(
            "browser_cdp",
            "Send a raw Chrome DevTools Protocol command.",
            browser_cdp_schema(),
        ),
        Arc::new(BrowserCdpExecutor::new(browser_manager.clone())),
    );
    registry.register(
        sys(
            "browser_adapter",
            "Run a site-specific browser automation adapter.",
            browser_adapter_schema(),
        ),
        Arc::new(BrowserAdapterExecutor::new(
            browser_manager.clone(),
            data_dir.join("default_adapters"),
        )),
    );

    registry.count() - start
}

// ---------------------------------------------------------------------------
// Minimal ToolConfig helper + inline schemas.
// Real metadata lives in `tools/<id>/TOOL.md` in the workspace; runtime-only
// callers (stdio) ship without those, so we inline enough to satisfy the LLM.
// ---------------------------------------------------------------------------

fn sys(id: &str, description: &str, schema: Value) -> ToolConfig {
    ToolConfig {
        id: id.to_string(),
        name: id.to_string(),
        description: description.to_string(),
        category: ToolCategory::System,
        input_schema: schema,
        instructions: String::new(),
        supports_background: false,
        should_defer: false,
        always_load: true,
        search_hint: None,
        is_read_only: false,
        is_concurrency_safe: false,
    }
}

fn shell_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "Shell command to execute."
            },
            "workdir": {
                "type": "string",
                "description": "Working directory. Auto-injected from the session when omitted; pass only if you need to override."
            },
            "timeout": {
                "type": "integer",
                "description": "Timeout in seconds for synchronous runs. Required when background=false."
            },
            "background": {
                "type": "boolean",
                "description": "Run in background and return a run_id instead of blocking."
            },
            "wait_timeout_secs": {
                "type": "integer",
                "description": "Only when background=true: seconds to wait for completion before returning run_id."
            },
            "hard_timeout_secs": {
                "type": "integer",
                "description": "Only when background=true: hard kill after N seconds (0 = no hard timeout)."
            },
            "notify": {
                "type": "boolean",
                "description": "Only when background=true: notify the agent when the run finishes."
            }
        },
        "required": ["command", "timeout"]
    })
}
fn read_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "file_path": {"type": "string"},
            "offset": {"type": "integer"},
            "limit": {"type": "integer"}
        },
        "required": ["file_path"]
    })
}
fn write_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "file_path": {"type": "string"},
            "content": {"type": "string"}
        },
        "required": ["file_path", "content"]
    })
}
fn edit_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "file_path": {"type": "string"},
            "old_string": {"type": "string"},
            "new_string": {"type": "string"},
            "replace_all": {"type": "boolean"}
        },
        "required": ["file_path", "old_string", "new_string"]
    })
}
fn grep_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "pattern": {"type": "string"},
            "path": {"type": "string"},
            "glob": {"type": "string"}
        },
        "required": ["pattern"]
    })
}
fn glob_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "pattern": {"type": "string"},
            "path": {"type": "string"}
        },
        "required": ["pattern"]
    })
}
fn fetch_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "url": {"type": "string"},
            "method": {"type": "string"},
            "body": {"type": "string"}
        },
        "required": ["url"]
    })
}
fn websearch_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"query": {"type": "string"}},
        "required": ["query"]
    })
}
fn update_plan_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"todos": {"type": "array"}},
        "required": ["todos"]
    })
}
fn run_manager_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "op": {"type": "string"},
            "run_id": {"type": "string"}
        },
        "required": ["op"]
    })
}
fn tool_search_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {"type": "string"},
            "max_results": {"type": "integer"}
        },
        "required": ["query"]
    })
}
fn get_session_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "session_id": {"type": "string"},
            "last_n": {"type": "integer"}
        },
        "required": ["session_id"]
    })
}
fn query_subagent_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {"type": "string"},
            "list": {"type": "boolean"}
        }
    })
}
fn stop_subagent_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"id": {"type": "string"}},
        "required": ["id"]
    })
}
fn upload_artifact_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "required": ["path"]
    })
}
fn image_generation_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "model": {"type": "string"},
            "prompt": {"type": "string"},
            "negative_prompt": {"type": "string"},
            "size": {"type": "string"},
            "seed": {"type": "integer"}
        },
        "required": ["prompt"]
    })
}
fn computer_use_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"action": {"type": "string"}},
        "required": ["action"]
    })
}
fn browser_navigate_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "url": {"type": "string"},
            "headless": {"type": "boolean"},
            "wait_seconds": {"type": "number"}
        },
        "required": ["url"]
    })
}
fn browser_cdp_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "method": {"type": "string"},
            "params": {"type": "object"},
            "timeout": {"type": "integer"}
        },
        "required": ["method"]
    })
}
fn browser_adapter_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "adapter": {"type": "string"},
            "op": {"type": "string"}
        },
        "required": ["op"]
    })
}
fn action_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"action": {"type": "string"}},
        "required": ["action"]
    })
}

// ---------------------------------------------------------------------------
// FetchExecutor needs *some* ProfileStore. The runtime default is the one
// embedded stub.
// ---------------------------------------------------------------------------

fn stub_profile_store() -> Arc<TokioRwLock<ProfileStore>> {
    let endpoint = LlmEndpoint {
        api_key: String::new(),
        base_url: String::new(),
        model: String::new(),
        temperature: 0.0,
        max_tokens: 0,
        context_window_tokens: None,
    };
    let profile = LlmProfile {
        id: "runtime-stub".to_string(),
        name: "runtime-stub".to_string(),
        chat: endpoint.clone(),
        compress: endpoint,
        supports_vision: false,
        supports_computer_use: false,
        api_format: ApiFormat::Anthropic,
        thinking: false,
        is_free: false,
        supports_function_calling: false,
        supports_image_output: false,
    };
    let store = ProfileStore {
        profiles: vec![profile],
        default_profile_id: "runtime-stub".to_string(),
    };
    Arc::new(TokioRwLock::new(store))
}
