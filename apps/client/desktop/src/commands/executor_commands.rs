//! Tauri commands exposing the multi-vendor `ExecutorRegistry` to the frontend.
//!
//! The RN UI (`VendorSelector`, `SetupWizard`) reads this list to gate per-vendor
//! capabilities (model switch, permission mode, abort, compact). Keep the DTOs
//! aligned with `apps/client/app/sync/ops.ts` (`VendorMeta` / `AgentCapabilities`).

use crate::executor_registry::{VendorConnectionProbe, VendorConnectionProbeState};
use multi_agent_runtime_core::PermissionModeKind;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::{sleep, timeout, Duration};

const VENDOR_MODEL_LIST_TIMEOUT: Duration = Duration::from_secs(12);
const PROCESS_GROUP_SHUTDOWN_GRACE: Duration = Duration::from_millis(250);

#[cfg(unix)]
fn place_in_own_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn place_in_own_process_group(_command: &mut Command) {}

#[cfg(unix)]
async fn kill_process_group(pid: u32) {
    let group = format!("-{pid}");
    let _ = Command::new("/bin/kill")
        .arg("-TERM")
        .arg(&group)
        .status()
        .await;
    sleep(PROCESS_GROUP_SHUTDOWN_GRACE).await;
    let _ = Command::new("/bin/kill")
        .arg("-KILL")
        .arg(&group)
        .status()
        .await;
}

#[cfg(not(unix))]
async fn kill_process_group(_pid: u32) {}

async fn terminate_child_group(child: &mut Child) {
    if let Some(pid) = child.id() {
        kill_process_group(pid).await;
    } else {
        let _ = child.kill().await;
    }
    let _ = child.wait().await;
}

/// Capability flags consumed by the RN UI (`AgentCapabilities` in
/// `apps/client/app/sync/ops.ts`). Values are serialised as camelCase.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilitiesDto {
    pub set_model: bool,
    pub set_permission_mode: bool,
    pub set_sandbox_policy: bool,
    pub abort: bool,
    pub compact: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VendorInstallStateDto {
    Installed,
    NotInstalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VendorAuthStateDto {
    Unknown,
    NotRequired,
    LoggedOut,
    LoggedIn,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VendorConnectionStateDto {
    Unknown,
    Probing,
    Connected,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VendorConnectionDto {
    pub state: VendorConnectionStateDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub checked_at_unix_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

impl VendorConnectionDto {
    fn unknown() -> Self {
        Self {
            state: VendorConnectionStateDto::Unknown,
            reason: None,
            checked_at_unix_ms: 0,
            latency_ms: None,
        }
    }
}

fn probe_to_dto(probe: VendorConnectionProbe) -> VendorConnectionDto {
    VendorConnectionDto {
        state: match probe.state {
            VendorConnectionProbeState::Unknown => VendorConnectionStateDto::Unknown,
            VendorConnectionProbeState::Probing => VendorConnectionStateDto::Probing,
            VendorConnectionProbeState::Connected => VendorConnectionStateDto::Connected,
            VendorConnectionProbeState::Failed => VendorConnectionStateDto::Failed,
        },
        reason: probe.reason,
        checked_at_unix_ms: probe.checked_at_unix_ms,
        latency_ms: probe.latency_ms,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VendorStatusDto {
    pub install_state: VendorInstallStateDto,
    pub auth_state: VendorAuthStateDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_authenticated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_authenticated: Option<bool>,
}

/// Matches the legacy `VendorMeta` shape in the frontend while reserving
/// explicit install/auth fields for progressive selector upgrades.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VendorInfoDto {
    pub name: String,
    pub available: bool,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logged_in: Option<bool>,
    pub capabilities: AgentCapabilitiesDto,
    pub status: VendorStatusDto,
    /// Most-recent connection probe outcome for this vendor. Populated from
    /// `ExecutorRegistry::snapshot_probes()`; defaults to `Unknown` when the
    /// vendor has never been probed this run.
    pub connection: VendorConnectionDto,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VendorModelInfoDto {
    pub id: String,
    pub model: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub vendor: String,
    pub api_format: &'static str,
    pub is_default: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub supported_reasoning_efforts: Vec<String>,
    pub supports_vision: bool,
    pub supports_computer_use: bool,
}

/// All known vendor names. The UI shows all of them; unavailable ones are
/// greyed out with "not installed".
const ALL_KNOWN_VENDORS: &[&str] = &["cteno", "claude", "codex", "gemini"];

#[derive(Debug, Clone)]
struct VendorAuthProbe {
    auth_state: VendorAuthStateDto,
    logged_in: Option<bool>,
    account_authenticated: Option<bool>,
    machine_authenticated: Option<bool>,
}

impl VendorAuthProbe {
    fn unknown() -> Self {
        Self {
            auth_state: VendorAuthStateDto::Unknown,
            logged_in: None,
            account_authenticated: None,
            machine_authenticated: None,
        }
    }

    fn not_required() -> Self {
        Self {
            auth_state: VendorAuthStateDto::NotRequired,
            logged_in: None,
            account_authenticated: None,
            machine_authenticated: None,
        }
    }

    fn logged_in() -> Self {
        Self {
            auth_state: VendorAuthStateDto::LoggedIn,
            logged_in: Some(true),
            account_authenticated: None,
            machine_authenticated: None,
        }
    }

    fn logged_out() -> Self {
        Self {
            auth_state: VendorAuthStateDto::LoggedOut,
            logged_in: Some(false),
            account_authenticated: None,
            machine_authenticated: None,
        }
    }
}

fn default_capabilities() -> AgentCapabilitiesDto {
    AgentCapabilitiesDto {
        set_model: false,
        set_permission_mode: false,
        set_sandbox_policy: false,
        abort: false,
        compact: false,
    }
}

fn capabilities_from_executor(
    executor: &dyn multi_agent_runtime_core::AgentExecutor,
) -> AgentCapabilitiesDto {
    let caps = executor.capabilities();
    AgentCapabilitiesDto {
        set_model: caps.supports_runtime_set_model,
        set_permission_mode: matches!(caps.permission_mode_kind, PermissionModeKind::Dynamic),
        set_sandbox_policy: true,
        abort: caps.supports_interrupt,
        compact: false,
    }
}

fn current_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn codex_auth_path(home_dir: &Path) -> PathBuf {
    home_dir.join(".codex").join("auth.json")
}

fn codex_auth_value_has_login(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };

    object
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
        || object
            .get("tokens")
            .and_then(Value::as_object)
            .map(|tokens| !tokens.is_empty())
            .unwrap_or(false)
}

fn detect_cteno_auth(app_data_dir: &Path) -> VendorAuthProbe {
    let account_authenticated = crate::headless_auth::load_account_auth(app_data_dir)
        .ok()
        .flatten()
        .is_some();
    let machine_authenticated =
        crate::auth_store_boot::machine_auth_cache_path(app_data_dir).exists();

    VendorAuthProbe {
        auth_state: if account_authenticated {
            VendorAuthStateDto::LoggedIn
        } else {
            VendorAuthStateDto::LoggedOut
        },
        logged_in: Some(account_authenticated),
        account_authenticated: Some(account_authenticated),
        machine_authenticated: Some(machine_authenticated),
    }
}

fn detect_codex_auth(home_dir: Option<&Path>) -> VendorAuthProbe {
    let Some(home_dir) = home_dir else {
        return VendorAuthProbe::unknown();
    };
    let auth_path = codex_auth_path(home_dir);
    let raw = match std::fs::read_to_string(&auth_path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return VendorAuthProbe::logged_out()
        }
        Err(_) => return VendorAuthProbe::unknown(),
    };
    let value: Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(_) => return VendorAuthProbe::unknown(),
    };
    if codex_auth_value_has_login(&value) {
        VendorAuthProbe::logged_in()
    } else {
        VendorAuthProbe::logged_out()
    }
}

/// Claude Code stores credentials differently per platform:
/// - macOS: macOS Keychain service `Claude Code-credentials`
/// - Linux/others: `~/.claude/.credentials.json` (Electron safeStorage fallback)
/// Env vars `ANTHROPIC_API_KEY` / `CLAUDE_CODE_OAUTH_TOKEN` also satisfy auth.
fn detect_claude_auth(home_dir: Option<&Path>) -> VendorAuthProbe {
    if std::env::var_os("ANTHROPIC_API_KEY")
        .or_else(|| std::env::var_os("CLAUDE_CODE_OAUTH_TOKEN"))
        .is_some()
    {
        return VendorAuthProbe::logged_in();
    }

    if let Some(home) = home_dir {
        let creds = home.join(".claude").join(".credentials.json");
        if let Ok(meta) = std::fs::metadata(&creds) {
            if meta.len() > 2 {
                return VendorAuthProbe::logged_in();
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if matches!(status, Ok(s) if s.success()) {
            return VendorAuthProbe::logged_in();
        }
    }

    #[cfg(target_os = "windows")]
    {
        // `cmdkey /list:Claude Code-credentials*` returns exit 0 whether or not
        // a match exists; the "no match" path prints "* NONE *" in stdout.
        // Treat presence of the target prefix in stdout as logged_in.
        let output = std::process::Command::new("cmdkey")
            .args(["/list:Claude Code-credentials*"])
            .stderr(std::process::Stdio::null())
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if stdout.contains("Claude Code-credentials") {
                return VendorAuthProbe::logged_in();
            }
        }
    }

    VendorAuthProbe::logged_out()
}

/// Gemini CLI stores OAuth creds at `~/.gemini/oauth_creds.json`.
/// API-key users export `GEMINI_API_KEY` / `GOOGLE_API_KEY`.
fn detect_gemini_auth(home_dir: Option<&Path>) -> VendorAuthProbe {
    if std::env::var_os("GEMINI_API_KEY")
        .or_else(|| std::env::var_os("GOOGLE_API_KEY"))
        .is_some()
    {
        return VendorAuthProbe::logged_in();
    }

    let Some(home) = home_dir else {
        return VendorAuthProbe::unknown();
    };
    let creds = home.join(".gemini").join("oauth_creds.json");
    match std::fs::metadata(&creds) {
        Ok(meta) if meta.len() > 2 => VendorAuthProbe::logged_in(),
        Ok(_) => VendorAuthProbe::logged_out(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => VendorAuthProbe::logged_out(),
        Err(_) => VendorAuthProbe::unknown(),
    }
}

fn detect_vendor_auth(
    vendor: &str,
    app_data_dir: &Path,
    home_dir: Option<&Path>,
) -> VendorAuthProbe {
    match vendor {
        "cteno" => detect_cteno_auth(app_data_dir),
        "codex" => detect_codex_auth(home_dir),
        "claude" => detect_claude_auth(home_dir),
        "gemini" => detect_gemini_auth(home_dir),
        _ => VendorAuthProbe::not_required(),
    }
}

fn normalize_reasoning_effort(value: &str) -> Option<String> {
    match value.trim() {
        "low" | "medium" | "high" | "xhigh" | "max" => Some(value.trim().to_string()),
        _ => None,
    }
}

fn repo_root_from_manifest() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.ancestors().nth(3).map(PathBuf::from)
}

fn multi_agent_runtime_package_dir() -> Option<PathBuf> {
    let candidate = repo_root_from_manifest()?
        .join("packages")
        .join("multi-agent-runtime");
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

async fn write_json_rpc_request(
    stdin: &mut ChildStdin,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), String> {
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_string(&request)
        .map_err(|e| format!("Failed to serialize {method} request: {e}"))?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| format!("Failed to write {method} request: {e}"))
}

async fn write_json_rpc_notification(
    stdin: &mut ChildStdin,
    method: &str,
    params: Value,
) -> Result<(), String> {
    let request = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_string(&request)
        .map_err(|e| format!("Failed to serialize {method} notification: {e}"))?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| format!("Failed to write {method} notification: {e}"))
}

async fn wait_json_rpc_response(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    expected_id: u64,
) -> Result<Value, String> {
    loop {
        let mut line = String::new();
        let bytes_read = stdout
            .read_line(&mut line)
            .await
            .map_err(|e| format!("Failed to read JSON-RPC response: {e}"))?;
        if bytes_read == 0 {
            return Err("Child process closed stdout before returning a response".to_string());
        }

        let payload: Value = match serde_json::from_str(line.trim()) {
            Ok(payload) => payload,
            Err(_) => continue,
        };
        let Some(object) = payload.as_object() else {
            continue;
        };

        if object
            .get("id")
            .and_then(Value::as_u64)
            .is_some_and(|id| id == expected_id)
        {
            if let Some(error) = object.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                let code = error
                    .get("code")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                return Err(format!("{message} (code={code})"));
            }
            return Ok(object.get("result").cloned().unwrap_or(Value::Null));
        }

        if let (Some(id), Some(method)) = (
            object.get("id").and_then(Value::as_u64),
            object.get("method").and_then(Value::as_str),
        ) {
            let response = match method {
                "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                    json!({ "decision": "decline" })
                }
                "item/permissions/requestApproval" => json!({ "permissions": {} }),
                "mcpServer/elicitation/request" => json!({
                    "action": "decline",
                    "content": Value::Null,
                    "_meta": Value::Null,
                }),
                _ => json!({}),
            };
            let reply = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": response,
            });
            let mut line = serde_json::to_string(&reply)
                .map_err(|e| format!("Failed to serialize JSON-RPC response: {e}"))?;
            line.push('\n');
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| format!("Failed to write JSON-RPC response: {e}"))?;
        }
    }
}

async fn wait_json_rpc_response_with_timeout(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    expected_id: u64,
    operation: &str,
) -> Result<Value, String> {
    timeout(
        VENDOR_MODEL_LIST_TIMEOUT,
        wait_json_rpc_response(stdin, stdout, expected_id),
    )
    .await
    .map_err(|_| {
        format!(
            "{operation} timed out after {}s",
            VENDOR_MODEL_LIST_TIMEOUT.as_secs()
        )
    })?
}

fn codex_model_from_value(value: &Value) -> Option<VendorModelInfoDto> {
    let model = value
        .get("model")
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    if model.is_empty() {
        return None;
    }

    let display_name = value
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
        .unwrap_or(model.as_str())
        .to_string();

    let supported_reasoning_efforts = value
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            value
                .as_str()
                .or_else(|| value.get("reasoningEffort").and_then(Value::as_str))
        })
        .filter_map(normalize_reasoning_effort)
        .collect::<Vec<_>>();

    let supports_vision = value
        .get("inputModalities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|modality| modality.eq_ignore_ascii_case("image"));

    Some(VendorModelInfoDto {
        id: model.clone(),
        model,
        display_name,
        description: value
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|candidate| !candidate.is_empty())
            .map(ToOwned::to_owned),
        vendor: "codex".to_string(),
        api_format: "openai",
        is_default: value
            .get("isDefault")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        default_reasoning_effort: value
            .get("defaultReasoningEffort")
            .and_then(Value::as_str)
            .and_then(normalize_reasoning_effort),
        supported_reasoning_efforts,
        supports_vision,
        supports_computer_use: false,
    })
}

fn parse_simple_toml_string_value(line: &str, key: &str) -> Option<String> {
    let (candidate_key, raw_value) = line.split_once('=')?;
    if candidate_key.trim() != key {
        return None;
    }

    let value = raw_value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.split_once('"').map(|(value, _)| value))
        .unwrap_or_else(|| value.split('#').next().unwrap_or(value).trim());
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_codex_config_model(raw: &str) -> Option<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .find_map(|line| parse_simple_toml_string_value(line, "model"))
}

fn read_codex_config_model(home_dir: Option<&Path>) -> Option<String> {
    let home_dir = home_dir?;
    let raw = std::fs::read_to_string(home_dir.join(".codex").join("config.toml")).ok()?;
    parse_codex_config_model(&raw)
}

fn apply_codex_configured_default(
    mut models: Vec<VendorModelInfoDto>,
    configured_model: Option<String>,
) -> Vec<VendorModelInfoDto> {
    let Some(configured_model) = configured_model else {
        return models;
    };
    let configured_model = configured_model.trim();
    if configured_model.is_empty()
        || !models
            .iter()
            .any(|model| model.id == configured_model || model.model == configured_model)
    {
        return models;
    }

    for model in models.iter_mut() {
        model.is_default = model.id == configured_model || model.model == configured_model;
    }
    models
}

async fn collect_codex_models() -> Result<Vec<VendorModelInfoDto>, String> {
    let codex_path = crate::executor_registry::resolve_codex_path()
        .ok_or_else(|| "Codex CLI not installed on this host".to_string())?;
    let mut command = Command::new(codex_path);
    command
        .arg("app-server")
        .arg("--listen")
        .arg("stdio://")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    place_in_own_process_group(&mut command);
    command.kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|e| format!("Failed to spawn codex app-server: {e}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Codex app-server missing stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Codex app-server missing stdout".to_string())?;
    let mut stdout_reader = BufReader::new(stdout);

    let result = async {
        write_json_rpc_request(
            &mut stdin,
            1,
            "initialize",
            json!({
                "clientInfo": {
                    "name": "cteno-desktop",
                    "title": "cteno-desktop",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "experimentalApi": true,
                },
            }),
        )
        .await?;
        let _ = wait_json_rpc_response_with_timeout(
            &mut stdin,
            &mut stdout_reader,
            1,
            "codex model initialize",
        )
        .await?;
        write_json_rpc_notification(&mut stdin, "initialized", Value::Null).await?;
        write_json_rpc_request(
            &mut stdin,
            2,
            "model/list",
            json!({
                "limit": 100,
                "includeHidden": false,
            }),
        )
        .await?;
        wait_json_rpc_response_with_timeout(&mut stdin, &mut stdout_reader, 2, "codex model/list")
            .await
    }
    .await;

    drop(stdin);
    drop(stdout_reader);
    terminate_child_group(&mut child).await;
    let result = result?;

    let models = result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(codex_model_from_value)
        .collect();
    Ok(apply_codex_configured_default(
        models,
        read_codex_config_model(current_home_dir().as_deref()),
    ))
}

fn claude_sdk_script() -> &'static str {
    r#"
import { execFileSync } from 'node:child_process';
import { query } from '@anthropic-ai/claude-agent-sdk';

function resolveClaudeCodeExecutable() {
  const explicit = process.env.CLAUDE_CODE_EXECUTABLE?.trim();
  if (explicit) {
    return explicit;
  }
  try {
    return execFileSync('which', ['claude'], { encoding: 'utf8' }).trim() || undefined;
  } catch {
    return undefined;
  }
}

const options = {};
const executable = resolveClaudeCodeExecutable();
if (executable) {
  options.pathToClaudeCodeExecutable = executable;
}

const q = query({ prompt: '', options });
try {
  const models = await q.supportedModels();
  process.stdout.write(JSON.stringify(models));
} finally {
  try {
    await q.return();
  } catch {}
}
"#
}

fn claude_model_from_value(value: &Value) -> Option<VendorModelInfoDto> {
    let model = value
        .get("value")
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    if model.is_empty() {
        return None;
    }

    let display_name = value
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
        .unwrap_or(model.as_str())
        .to_string();

    let supported_reasoning_efforts = value
        .get("supportedEffortLevels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(normalize_reasoning_effort)
        .collect::<Vec<_>>();

    Some(VendorModelInfoDto {
        id: model.clone(),
        model,
        display_name,
        description: value
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|candidate| !candidate.is_empty())
            .map(ToOwned::to_owned),
        vendor: "claude".to_string(),
        api_format: "anthropic",
        is_default: value.get("value").and_then(Value::as_str) == Some("default"),
        default_reasoning_effort: None,
        supported_reasoning_efforts,
        supports_vision: true,
        supports_computer_use: false,
    })
}

async fn collect_claude_models() -> Result<Vec<VendorModelInfoDto>, String> {
    let package_dir = multi_agent_runtime_package_dir().ok_or_else(|| {
        "Unable to locate packages/multi-agent-runtime for Claude SDK".to_string()
    })?;
    let mut command = Command::new("node");
    command
        .arg("--input-type=module")
        .arg("-e")
        .arg(claude_sdk_script())
        .current_dir(&package_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    place_in_own_process_group(&mut command);
    command.kill_on_drop(true);
    let child = command
        .spawn()
        .map_err(|e| format!("Failed to invoke Claude SDK via node: {e}"))?;
    let child_pid = child.id();
    let output = match timeout(VENDOR_MODEL_LIST_TIMEOUT, child.wait_with_output()).await {
        Ok(result) => result.map_err(|e| format!("Failed to invoke Claude SDK via node: {e}"))?,
        Err(_) => {
            if let Some(pid) = child_pid {
                kill_process_group(pid).await;
            }
            return Err(format!(
                "Claude SDK model listing timed out after {}s",
                VENDOR_MODEL_LIST_TIMEOUT.as_secs()
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "Claude SDK model listing failed".to_string()
        } else {
            format!("Claude SDK model listing failed: {stderr}")
        });
    }

    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse Claude SDK model list: {e}"))?;
    Ok(value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(claude_model_from_value)
        .collect())
}

pub async fn collect_vendor_models(vendor: &str) -> Result<Vec<VendorModelInfoDto>, String> {
    match vendor.trim() {
        "codex" => collect_codex_models().await,
        "claude" => collect_claude_models().await,
        "gemini" => Ok(collect_gemini_models().await),
        "cteno" => Ok(Vec::new()),
        other => Err(format!("Unsupported vendor '{other}'")),
    }
}

/// Gemini's ACP server advertises its model list inside every `session/new`
/// response (`result.models.availableModels`). The adapter caches the union
/// across sessions on a per-connection basis; if a connection already exists,
/// we surface what it saw. Otherwise we fall back to a baked-in list matching
/// what `gemini --acp` 0.38.x returns — this prevents the UI from showing an
/// empty picker on a cold daemon (which previously caused `is_vendor_native_model_id`
/// to return `false` for every id and triggered the `[500] Requested entity
/// was not found.` symptom by routing non-gemini profile ids through
/// `session/set_model`).
async fn collect_gemini_models() -> Vec<VendorModelInfoDto> {
    // The adapter caches `session/new` responses' `availableModels` on each
    // connection, but plumbing a live downcast through
    // `Arc<dyn AgentExecutor>` would require a non-trivial trait extension.
    // For now we return a static baseline that tracks what `gemini --acp`
    // 0.38.x actually reports — good enough for `is_vendor_native_model_id`
    // to stop mis-routing non-gemini profile_ids into `session/set_model`.
    let ids: Vec<String> = GEMINI_DEFAULT_MODEL_IDS
        .iter()
        .map(|s| s.to_string())
        .collect();

    let default_id = ids
        .iter()
        .find(|id| id.starts_with("auto-gemini"))
        .cloned()
        .or_else(|| ids.first().cloned());

    ids.into_iter()
        .map(|id| {
            let is_default = default_id.as_deref() == Some(id.as_str());
            VendorModelInfoDto {
                id: id.clone(),
                model: id.clone(),
                display_name: id.clone(),
                description: None,
                vendor: "gemini".to_string(),
                api_format: "gemini",
                is_default,
                default_reasoning_effort: None,
                supported_reasoning_efforts: Vec::new(),
                supports_vision: true,
                supports_computer_use: false,
            }
        })
        .collect()
}

/// Snapshot of `gemini --acp` 0.38.x `session/new` `models.availableModels`
/// list. Used as a fallback when the adapter hasn't yet observed a real
/// session/new response. Kept in sync with the live protocol by the
/// `gemini-model-gate.md` eval suite.
const GEMINI_DEFAULT_MODEL_IDS: &[&str] = &[
    "auto-gemini-3",
    "auto-gemini-2.5",
    "gemini-3.1-pro-preview",
    "gemini-3-flash-preview",
    "gemini-3.1-flash-lite-preview",
    "gemini-2.5-pro",
    "gemini-2.5-flash",
    "gemini-2.5-flash-lite",
];

/// Collect the vendor list by trying to resolve each known vendor from the
/// `ExecutorRegistry`. `available` intentionally keeps the legacy "installed
/// on this host" meaning so existing frontend call sites do not change
/// behaviour; new `installed` / `loggedIn` / `status` fields carry the finer
/// install-auth contract for gradual adoption.
///
/// Returns `Err("ExecutorRegistry not installed")` when service init has not
/// finished (or failed to locate `cteno-agent`).
pub async fn collect_vendor_infos() -> Result<Vec<VendorInfoDto>, String> {
    let registry = crate::local_services::executor_registry()
        .map_err(|_| "ExecutorRegistry not installed".to_string())?;
    let app_data_dir = crate::headless_auth::resolve_app_data_dir();
    let home_dir = current_home_dir();

    let probes = registry.snapshot_probes().await;

    let mut out = Vec::new();
    for &name in ALL_KNOWN_VENDORS {
        let installed = registry.is_vendor_installed(name)?;
        let auth = detect_vendor_auth(name, &app_data_dir, home_dir.as_deref());
        let capabilities = match registry.resolve(name) {
            Ok(executor) => capabilities_from_executor(executor.as_ref()),
            Err(_) => default_capabilities(),
        };

        let connection = match probes.get(name).cloned() {
            Some(probe) => probe_to_dto(probe),
            None => VendorConnectionDto::unknown(),
        };

        out.push(VendorInfoDto {
            name: name.to_string(),
            available: installed,
            installed,
            logged_in: auth.logged_in,
            capabilities,
            status: VendorStatusDto {
                install_state: if installed {
                    VendorInstallStateDto::Installed
                } else {
                    VendorInstallStateDto::NotInstalled
                },
                auth_state: auth.auth_state,
                account_authenticated: auth.account_authenticated,
                machine_authenticated: auth.machine_authenticated,
            },
            connection,
        });
    }
    Ok(out)
}

/// Tauri command: enumerate available executor vendors with capability flags.
///
/// Mirrors the `{machineId}:list_available_vendors` machine RPC registered in
/// `happy_client/manager.rs` so both the direct Tauri invoke path and the
/// `apiSocket.machineRPC` socket-fallback path return the same payload.
#[tauri::command]
pub async fn list_available_vendors() -> Result<Vec<VendorInfoDto>, String> {
    collect_vendor_infos().await
}

#[tauri::command]
pub async fn list_vendor_models(vendor: String) -> Result<Vec<VendorModelInfoDto>, String> {
    collect_vendor_models(&vendor).await
}

/// Force-refresh a vendor's connection probe. Closes any cached handle and
/// re-probes so the UI sees the live state.
#[tauri::command]
pub async fn probe_vendor_connection(vendor: String) -> Result<VendorConnectionDto, String> {
    let registry = crate::local_services::executor_registry()
        .map_err(|_| "ExecutorRegistry not installed".to_string())?;
    let canonical = match vendor.as_str() {
        "cteno" => "cteno",
        "claude" => "claude",
        "codex" => "codex",
        "gemini" => "gemini",
        other => return Err(format!("unknown vendor '{other}'")),
    };
    let probe = registry.refresh(canonical).await;
    Ok(probe_to_dto(probe))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless_auth::{save_account_auth, HeadlessAccountAuth};
    use tempfile::tempdir;

    #[test]
    fn codex_config_model_parser_reads_current_default() {
        assert_eq!(
            parse_codex_config_model(
                r#"
model = "gpt-5.5"
model_reasoning_effort = "medium"
"#
            ),
            Some("gpt-5.5".to_string())
        );
    }

    #[test]
    fn codex_configured_default_only_marks_advertised_models() {
        let models = vec![
            VendorModelInfoDto {
                id: "gpt-5.4".to_string(),
                model: "gpt-5.4".to_string(),
                display_name: "gpt-5.4".to_string(),
                description: None,
                vendor: "codex".to_string(),
                api_format: "openai",
                is_default: true,
                default_reasoning_effort: Some("medium".to_string()),
                supported_reasoning_efforts: vec!["medium".to_string(), "high".to_string()],
                supports_vision: true,
                supports_computer_use: false,
            },
            VendorModelInfoDto {
                id: "gpt-5.5".to_string(),
                model: "gpt-5.5".to_string(),
                display_name: "GPT-5.5".to_string(),
                description: None,
                vendor: "codex".to_string(),
                api_format: "openai",
                is_default: false,
                default_reasoning_effort: Some("medium".to_string()),
                supported_reasoning_efforts: vec!["medium".to_string(), "high".to_string()],
                supports_vision: true,
                supports_computer_use: false,
            },
        ];

        let merged = apply_codex_configured_default(models, Some("gpt-5.5".to_string()));

        assert!(!merged[0].is_default);
        assert!(merged[1].is_default);
        assert_eq!(
            apply_codex_configured_default(merged, Some("gpt-5.6".to_string()))
                .iter()
                .filter(|model| model.is_default)
                .count(),
            1
        );
    }

    #[test]
    fn codex_auth_probe_reads_auth_json_shape() {
        let dir = tempdir().unwrap();
        let codex_dir = dir.path().join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();
        std::fs::write(
            codex_dir.join("auth.json"),
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"secret"}}"#,
        )
        .unwrap();

        let probe = detect_codex_auth(Some(dir.path()));

        assert_eq!(probe.auth_state, VendorAuthStateDto::LoggedIn);
        assert_eq!(probe.logged_in, Some(true));
    }

    #[test]
    fn codex_auth_probe_reports_logged_out_when_file_missing() {
        let dir = tempdir().unwrap();

        let probe = detect_codex_auth(Some(dir.path()));

        assert_eq!(probe.auth_state, VendorAuthStateDto::LoggedOut);
        assert_eq!(probe.logged_in, Some(false));
    }

    #[test]
    fn gemini_auth_probe_reads_oauth_creds_file() {
        let dir = tempdir().unwrap();
        let gemini_dir = dir.path().join(".gemini");
        std::fs::create_dir_all(&gemini_dir).unwrap();
        std::fs::write(
            gemini_dir.join("oauth_creds.json"),
            r#"{"access_token":"ya29.secret","refresh_token":"1//refresh","expiry":"2099-01-01T00:00:00Z"}"#,
        )
        .unwrap();

        let probe = detect_gemini_auth(Some(dir.path()));

        assert_eq!(probe.auth_state, VendorAuthStateDto::LoggedIn);
        assert_eq!(probe.logged_in, Some(true));
    }

    #[test]
    fn gemini_auth_probe_reports_logged_out_without_creds() {
        let dir = tempdir().unwrap();
        // Ensure we're not picking up an ambient env var from the host.
        // SAFETY: single-threaded test context; process env is local here.
        unsafe {
            std::env::remove_var("GEMINI_API_KEY");
            std::env::remove_var("GOOGLE_API_KEY");
        }

        let probe = detect_gemini_auth(Some(dir.path()));

        assert_eq!(probe.auth_state, VendorAuthStateDto::LoggedOut);
        assert_eq!(probe.logged_in, Some(false));
    }

    #[test]
    fn claude_auth_probe_reads_linux_credentials_fallback() {
        let dir = tempdir().unwrap();
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        // Electron safeStorage writes an opaque blob; we only check file exists + non-trivial size.
        std::fs::write(
            claude_dir.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"opaque"}}"#,
        )
        .unwrap();

        let probe = detect_claude_auth(Some(dir.path()));

        assert_eq!(probe.auth_state, VendorAuthStateDto::LoggedIn);
        assert_eq!(probe.logged_in, Some(true));
    }

    #[test]
    fn cteno_auth_probe_reads_headless_auth_store() {
        let dir = tempdir().unwrap();
        save_account_auth(
            dir.path(),
            &HeadlessAccountAuth {
                access_token: Some("access".to_string()),
                refresh_token: Some("refresh".to_string()),
                user_id: Some("user".to_string()),
                machine_id: Some("machine".to_string()),
                access_expires_at_ms: Some(1),
                refresh_expires_at_ms: Some(2),
            },
        )
        .unwrap();

        let probe = detect_cteno_auth(dir.path());

        assert_eq!(probe.auth_state, VendorAuthStateDto::LoggedIn);
        assert_eq!(probe.logged_in, Some(true));
        assert_eq!(probe.account_authenticated, Some(true));
        assert_eq!(probe.machine_authenticated, Some(true));
    }
}
