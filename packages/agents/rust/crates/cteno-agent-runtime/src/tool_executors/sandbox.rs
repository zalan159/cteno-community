//! Sandbox Policy — workspace boundary enforcement for tool executors.
//!
//! Provides the second axis of the two-axis permission model:
//!   PermissionMode (when to ask)  ×  SandboxPolicy (what's accessible)
//!
//! Even in BypassPermissions mode, the sandbox still restricts writes to the workspace.

use super::path_resolver::{expand_tilde, normalize_lexical, resolve_workdir};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Sandbox policy controlling where tools can write.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SandboxPolicy {
    /// Writes restricted to workdir + additional roots + system tmp (default).
    WorkspaceWrite {
        #[serde(default)]
        additional_writable_roots: Vec<PathBuf>,
    },
    /// No workspace restrictions. System-protected paths still blocked.
    Unrestricted,
    /// All writes blocked.
    ReadOnly,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        SandboxPolicy::WorkspaceWrite {
            additional_writable_roots: vec![],
        }
    }
}

/// Result of a sandbox check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxCheckResult {
    Allowed,
    Denied(String),
}

/// Pre-computed sandbox context for a session.
#[derive(Debug, Clone)]
pub struct SandboxContext {
    pub policy: SandboxPolicy,
    pub workdir: PathBuf,
    /// workdir + additional_writable_roots + system temp dir, all normalized absolute paths.
    pub writable_roots: Vec<PathBuf>,
}

// ---------------------------------------------------------------------------
// System-level protected paths (all policies, including Unrestricted)
// ---------------------------------------------------------------------------

/// System directories that should never be written to by an AI agent.
const SYSTEM_PROTECTED_PREFIXES: &[&str] = &[
    "/etc", "/usr", "/bin", "/sbin", "/boot", "/proc", "/sys", // macOS specific
    "/System", "/Library",
    // /var is blocked but /var/tmp and /var/folders are allowed via SYSTEM_ALLOWED_SUBPATHS.
    "/var",
];

/// Sub-paths under SYSTEM_PROTECTED_PREFIXES that we allow writes to (temp dirs).
const SYSTEM_ALLOWED_SUBPATHS: &[&str] = &[
    "/var/tmp",
    "/var/folders", // macOS per-user temp
];

// ---------------------------------------------------------------------------
// Workspace-level protected patterns (WorkspaceWrite mode only)
// ---------------------------------------------------------------------------

/// File/directory names that are protected inside the workspace.
/// Matching is done on individual path components.
const PROTECTED_NAMES: &[&str] = &[
    ".git",
    ".env",
    ".env.local",
    ".env.production",
    ".env.development",
    ".bashrc",
    ".zshrc",
    ".profile",
    ".bash_profile",
    ".ssh",
    ".gnupg",
];

// ---------------------------------------------------------------------------
// SandboxContext construction
// ---------------------------------------------------------------------------

impl SandboxContext {
    /// Build context from policy and raw workdir string.
    pub fn new(policy: SandboxPolicy, workdir_raw: Option<&str>) -> Self {
        let workdir = resolve_workdir(workdir_raw);
        let mut writable_roots = vec![workdir.clone()];

        // System temp directory
        let tmp = std::env::temp_dir();
        let tmp_normalized = normalize_lexical(&tmp);
        if !writable_roots.contains(&tmp_normalized) {
            writable_roots.push(tmp_normalized);
        }

        // On macOS, /tmp is a symlink to /private/tmp — add both forms
        #[cfg(target_os = "macos")]
        {
            let private_tmp = PathBuf::from("/private/tmp");
            let private_tmp_normalized = normalize_lexical(&private_tmp);
            if !writable_roots.contains(&private_tmp_normalized) {
                writable_roots.push(private_tmp_normalized);
            }
        }

        if let SandboxPolicy::WorkspaceWrite {
            ref additional_writable_roots,
        } = policy
        {
            for root in additional_writable_roots {
                let expanded = expand_tilde(&root.to_string_lossy());
                let normalized = if expanded.is_absolute() {
                    normalize_lexical(&expanded)
                } else {
                    normalize_lexical(&workdir.join(expanded))
                };
                if !writable_roots.contains(&normalized) {
                    writable_roots.push(normalized);
                }
            }
        }

        Self {
            policy,
            workdir,
            writable_roots,
        }
    }

    /// Construct from tool input JSON. Reads `__sandbox_policy` and `workdir` fields.
    /// Falls back to `SandboxPolicy::default()` (WorkspaceWrite) when absent.
    pub fn from_input(input: &serde_json::Value) -> Self {
        let policy = input
            .get("__sandbox_policy")
            .and_then(|v| serde_json::from_value::<SandboxPolicy>(v.clone()).ok())
            .unwrap_or_default();

        let workdir_raw = input.get("workdir").and_then(|v| v.as_str());
        Self::new(policy, workdir_raw)
    }
}

// ---------------------------------------------------------------------------
// Core check functions
// ---------------------------------------------------------------------------

/// Check if a path targets a system-protected directory.
/// Returns the matched prefix on denial.
fn is_system_protected(path: &Path) -> Option<&'static str> {
    let path_str = path.to_string_lossy();

    // First check allowed sub-paths (e.g. /var/tmp)
    for allowed in SYSTEM_ALLOWED_SUBPATHS {
        if path_str.starts_with(allowed) {
            return None;
        }
    }

    for prefix in SYSTEM_PROTECTED_PREFIXES {
        if path_str.starts_with(prefix)
            && (path_str.len() == prefix.len()
                || path_str.as_bytes().get(prefix.len()) == Some(&b'/'))
        {
            return Some(prefix);
        }
    }
    None
}

/// Check if a path targets a protected file/directory within the workspace.
/// Only applies in WorkspaceWrite mode.
fn is_protected_path(path: &Path) -> Option<String> {
    for component in path.components() {
        if let std::path::Component::Normal(name) = component {
            let name_str = name.to_string_lossy();
            for pattern in PROTECTED_NAMES {
                // Exact match: component == ".git"
                if name_str == *pattern {
                    return Some(format!(
                        "Protected path: writing to '{}' is blocked (matched '{}'). \
                         These files control security-sensitive configuration.",
                        path.display(),
                        pattern
                    ));
                }
                // Prefix match for .env variants: ".env.staging" starts with ".env"
                if *pattern == ".env"
                    && pattern.len() < name_str.len()
                    && name_str.starts_with(pattern)
                {
                    let next_char = name_str.as_bytes()[pattern.len()];
                    if next_char == b'.' {
                        return Some(format!(
                            "Protected path: writing to '{}' is blocked (matched '{}*'). \
                             Environment files may contain secrets.",
                            path.display(),
                            pattern
                        ));
                    }
                }
            }
        }
    }
    None
}

/// Main write-path check. Called by `resolve_file_path_sandboxed`.
pub fn check_write_path(path: &Path, ctx: &SandboxContext) -> SandboxCheckResult {
    let normalized = normalize_lexical(path);

    // Layer 1: System-protected paths — always enforced, even Unrestricted.
    if let Some(prefix) = is_system_protected(&normalized) {
        return SandboxCheckResult::Denied(format!(
            "System-protected path: writing to '{}' is blocked (under '{}')",
            normalized.display(),
            prefix
        ));
    }

    // Layer 2: Policy-specific checks.
    match &ctx.policy {
        SandboxPolicy::Unrestricted => SandboxCheckResult::Allowed,

        SandboxPolicy::ReadOnly => {
            SandboxCheckResult::Denied("Read-only sandbox: all writes are blocked".to_string())
        }

        SandboxPolicy::WorkspaceWrite { .. } => {
            // Layer 2a: Protected files within workspace.
            if let Some(reason) = is_protected_path(&normalized) {
                return SandboxCheckResult::Denied(reason);
            }

            // Layer 2b: Workspace boundary.
            for root in &ctx.writable_roots {
                if normalized.starts_with(root) {
                    return SandboxCheckResult::Allowed;
                }
            }

            SandboxCheckResult::Denied(format!(
                "Path '{}' is outside the workspace. Writable roots: [{}]",
                normalized.display(),
                ctx.writable_roots
                    .iter()
                    .map(|r| r.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Shell command redirection detection
// ---------------------------------------------------------------------------

/// Extract file paths from shell output redirections (best-effort regex).
fn extract_redirection_targets(command: &str) -> Vec<String> {
    let mut targets = Vec::new();

    // Match: > /path, >> /path, 2> /path, 2>> /path, &> /path, &>> /path
    // Also handles quoted paths: > "/path with spaces"
    if let Ok(re) = regex::Regex::new(r#"(?:\d?&?)>{1,2}\s*"([^"]+)"|(?:\d?&?)>{1,2}\s*(\S+)"#) {
        for cap in re.captures_iter(command) {
            if let Some(m) = cap.get(1).or_else(|| cap.get(2)) {
                let target = m.as_str();
                // Skip /dev/null and stdout/stderr descriptors
                if target != "/dev/null" && !target.starts_with('&') {
                    targets.push(target.to_string());
                }
            }
        }
    }

    // Match: | tee /path, | tee -a /path, | sudo tee /path
    if let Ok(re) = regex::Regex::new(
        r#"\|\s*(?:sudo\s+)?tee\s+(?:-[a-zA-Z]\s+)*"([^"]+)"|\|\s*(?:sudo\s+)?tee\s+(?:-[a-zA-Z]\s+)*(\S+)"#,
    ) {
        for cap in re.captures_iter(command) {
            if let Some(m) = cap.get(1).or_else(|| cap.get(2)) {
                let target = m.as_str();
                if target != "/dev/null" {
                    targets.push(target.to_string());
                }
            }
        }
    }

    targets
}

/// Resolve a shell path (may be relative) against workdir.
fn resolve_shell_path(target: &str, workdir: &Path) -> PathBuf {
    let expanded = expand_tilde(target);
    if expanded.is_absolute() {
        normalize_lexical(&expanded)
    } else {
        normalize_lexical(&workdir.join(expanded))
    }
}

/// Check a shell command for sandbox violations via output redirection detection.
pub fn check_shell_command(command: &str, ctx: &SandboxContext) -> SandboxCheckResult {
    match &ctx.policy {
        SandboxPolicy::Unrestricted => {
            // Even in unrestricted mode, check for system-protected redirect targets
            for target in extract_redirection_targets(command) {
                let target_path = resolve_shell_path(&target, &ctx.workdir);
                if let Some(prefix) = is_system_protected(&target_path) {
                    return SandboxCheckResult::Denied(format!(
                        "Shell redirection to system-protected path: '{}' (under '{}')",
                        target, prefix
                    ));
                }
            }
            SandboxCheckResult::Allowed
        }

        SandboxPolicy::ReadOnly => {
            // In read-only, block commands with obvious write indicators.
            let redirections = extract_redirection_targets(command);
            if !redirections.is_empty() {
                return SandboxCheckResult::Denied(format!(
                    "Read-only sandbox: shell output redirection blocked (targets: {})",
                    redirections.join(", ")
                ));
            }
            SandboxCheckResult::Allowed
        }

        SandboxPolicy::WorkspaceWrite { .. } => {
            for target in extract_redirection_targets(command) {
                let target_path = resolve_shell_path(&target, &ctx.workdir);
                match check_write_path(&target_path, ctx) {
                    SandboxCheckResult::Allowed => {}
                    SandboxCheckResult::Denied(reason) => {
                        return SandboxCheckResult::Denied(format!(
                            "Shell redirection target blocked: '{}' — {}",
                            target, reason
                        ));
                    }
                }
            }
            SandboxCheckResult::Allowed
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_ctx(workdir: &str) -> SandboxContext {
        SandboxContext::new(SandboxPolicy::default(), Some(workdir))
    }

    fn workspace_ctx_with_roots(workdir: &str, roots: Vec<&str>) -> SandboxContext {
        SandboxContext::new(
            SandboxPolicy::WorkspaceWrite {
                additional_writable_roots: roots.into_iter().map(PathBuf::from).collect(),
            },
            Some(workdir),
        )
    }

    fn unrestricted_ctx(workdir: &str) -> SandboxContext {
        SandboxContext::new(SandboxPolicy::Unrestricted, Some(workdir))
    }

    fn readonly_ctx(workdir: &str) -> SandboxContext {
        SandboxContext::new(SandboxPolicy::ReadOnly, Some(workdir))
    }

    // ── Workspace boundary ──────────────────────────────────────────────

    #[test]
    fn workspace_allows_path_inside_workdir() {
        let ctx = workspace_ctx("/tmp/project");
        let r = check_write_path(Path::new("/tmp/project/src/main.rs"), &ctx);
        assert_eq!(r, SandboxCheckResult::Allowed);
    }

    #[test]
    fn workspace_allows_path_at_workdir_root() {
        let ctx = workspace_ctx("/tmp/project");
        let r = check_write_path(Path::new("/tmp/project/file.txt"), &ctx);
        assert_eq!(r, SandboxCheckResult::Allowed);
    }

    #[test]
    fn workspace_denies_path_outside_workdir() {
        let ctx = workspace_ctx("/tmp/project");
        let r = check_write_path(Path::new("/home/user/secret.txt"), &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn workspace_denies_parent_directory() {
        let ctx = workspace_ctx("/tmp/project");
        let r = check_write_path(Path::new("/tmp/other/file.txt"), &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn workspace_allows_additional_writable_root() {
        let ctx = workspace_ctx_with_roots("/tmp/project", vec!["/home/user/data"]);
        let r = check_write_path(Path::new("/home/user/data/output.csv"), &ctx);
        assert_eq!(r, SandboxCheckResult::Allowed);
    }

    #[test]
    fn workspace_allows_temp_dir() {
        let tmp = std::env::temp_dir();
        let ctx = workspace_ctx("/home/user/project");
        let target = tmp.join("cteno-test.txt");
        let r = check_write_path(&target, &ctx);
        assert_eq!(r, SandboxCheckResult::Allowed);
    }

    // ── System-protected paths ──────────────────────────────────────────

    #[test]
    fn system_protected_blocks_etc() {
        let ctx = unrestricted_ctx("/tmp/project");
        let r = check_write_path(Path::new("/etc/passwd"), &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn system_protected_blocks_usr() {
        let ctx = unrestricted_ctx("/tmp/project");
        let r = check_write_path(Path::new("/usr/local/bin/evil"), &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn system_protected_blocks_bin() {
        let ctx = workspace_ctx("/tmp/project");
        let r = check_write_path(Path::new("/bin/sh"), &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn system_protected_allows_home() {
        let ctx = unrestricted_ctx("/tmp/project");
        let home = super::super::path_resolver::user_home_dir();
        let target = home.join("Documents/file.txt");
        let r = check_write_path(&target, &ctx);
        assert_eq!(r, SandboxCheckResult::Allowed);
    }

    #[test]
    fn system_protected_allows_var_tmp() {
        let ctx = unrestricted_ctx("/tmp/project");
        let r = check_write_path(Path::new("/var/tmp/test.txt"), &ctx);
        assert_eq!(r, SandboxCheckResult::Allowed);
    }

    #[test]
    fn system_protected_blocks_var_log() {
        let ctx = unrestricted_ctx("/tmp/project");
        let r = check_write_path(Path::new("/var/log/syslog"), &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn unrestricted_still_blocks_system_paths() {
        let ctx = unrestricted_ctx("/tmp/project");
        assert!(matches!(
            check_write_path(Path::new("/etc/hosts"), &ctx),
            SandboxCheckResult::Denied(_)
        ));
        assert!(matches!(
            check_write_path(Path::new("/System/Library/file"), &ctx),
            SandboxCheckResult::Denied(_)
        ));
    }

    // ── Protected files within workspace ────────────────────────────────

    #[test]
    fn protected_blocks_dot_git() {
        let ctx = workspace_ctx("/tmp/project");
        let r = check_write_path(Path::new("/tmp/project/.git/hooks/pre-commit"), &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn protected_blocks_dot_env() {
        let ctx = workspace_ctx("/tmp/project");
        let r = check_write_path(Path::new("/tmp/project/.env"), &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn protected_blocks_dot_env_production() {
        let ctx = workspace_ctx("/tmp/project");
        let r = check_write_path(Path::new("/tmp/project/.env.production"), &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn protected_blocks_dot_env_staging() {
        // .env.staging matches the ".env" prefix pattern
        let ctx = workspace_ctx("/tmp/project");
        let r = check_write_path(Path::new("/tmp/project/.env.staging"), &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn protected_allows_gitignore() {
        // .gitignore is NOT in PROTECTED_NAMES
        let ctx = workspace_ctx("/tmp/project");
        let r = check_write_path(Path::new("/tmp/project/.gitignore"), &ctx);
        assert_eq!(r, SandboxCheckResult::Allowed);
    }

    #[test]
    fn protected_allows_config_env() {
        // config.env doesn't match ".env" pattern (no leading dot)
        let ctx = workspace_ctx("/tmp/project");
        let r = check_write_path(Path::new("/tmp/project/config.env"), &ctx);
        assert_eq!(r, SandboxCheckResult::Allowed);
    }

    #[test]
    fn protected_allows_env_directory_content() {
        // src/env/config.ts — "env" component doesn't match ".env"
        let ctx = workspace_ctx("/tmp/project");
        let r = check_write_path(Path::new("/tmp/project/src/env/config.ts"), &ctx);
        assert_eq!(r, SandboxCheckResult::Allowed);
    }

    #[test]
    fn protected_blocks_ssh() {
        let ctx = workspace_ctx("/tmp/project");
        let r = check_write_path(Path::new("/tmp/project/.ssh/id_rsa"), &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    // ── ReadOnly mode ───────────────────────────────────────────────────

    #[test]
    fn readonly_denies_all_writes() {
        let ctx = readonly_ctx("/tmp/project");
        assert!(matches!(
            check_write_path(Path::new("/tmp/project/file.txt"), &ctx),
            SandboxCheckResult::Denied(_)
        ));
    }

    // ── Default trait ───────────────────────────────────────────────────

    #[test]
    fn default_policy_is_workspace_write() {
        let policy = SandboxPolicy::default();
        assert!(matches!(
            policy,
            SandboxPolicy::WorkspaceWrite {
                additional_writable_roots
            } if additional_writable_roots.is_empty()
        ));
    }

    // ── Shell redirection detection ─────────────────────────────────────

    #[test]
    fn extract_redirect_simple() {
        let targets = extract_redirection_targets("echo hello > /tmp/out.txt");
        assert_eq!(targets, vec!["/tmp/out.txt"]);
    }

    #[test]
    fn extract_redirect_append() {
        let targets = extract_redirection_targets("echo hello >> /tmp/out.txt");
        assert_eq!(targets, vec!["/tmp/out.txt"]);
    }

    #[test]
    fn extract_redirect_stderr() {
        let targets = extract_redirection_targets("cmd 2> /tmp/err.log");
        assert_eq!(targets, vec!["/tmp/err.log"]);
    }

    #[test]
    fn extract_redirect_tee() {
        let targets = extract_redirection_targets("echo hello | tee /tmp/out.txt");
        assert_eq!(targets, vec!["/tmp/out.txt"]);
    }

    #[test]
    fn extract_redirect_sudo_tee() {
        let targets = extract_redirection_targets("echo hello | sudo tee /etc/hosts");
        assert_eq!(targets, vec!["/etc/hosts"]);
    }

    #[test]
    fn extract_redirect_dev_null_ignored() {
        let targets = extract_redirection_targets("cmd > /dev/null 2>&1");
        assert!(targets.is_empty());
    }

    #[test]
    fn extract_no_redirect() {
        let targets = extract_redirection_targets("ls -la");
        assert!(targets.is_empty());
    }

    #[test]
    fn shell_check_workspace_denies_redirect_outside() {
        let ctx = workspace_ctx("/home/user/project");
        let r = check_shell_command("echo x > /etc/hosts", &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn shell_check_workspace_allows_redirect_inside() {
        let ctx = workspace_ctx("/tmp/project");
        let r = check_shell_command("echo x > /tmp/project/out.txt", &ctx);
        assert_eq!(r, SandboxCheckResult::Allowed);
    }

    #[test]
    fn shell_check_workspace_denies_tee_outside() {
        let ctx = workspace_ctx("/home/user/project");
        let r = check_shell_command("cat file | tee /etc/passwd", &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn shell_check_workspace_denies_sudo_tee_outside() {
        let ctx = workspace_ctx("/home/user/project");
        let r = check_shell_command("echo data | sudo tee /etc/hosts", &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn shell_check_allows_no_redirect() {
        let ctx = workspace_ctx("/tmp/project");
        let r = check_shell_command("ls -la && grep foo bar", &ctx);
        assert_eq!(r, SandboxCheckResult::Allowed);
    }

    #[test]
    fn shell_check_unrestricted_blocks_system_redirect() {
        let ctx = unrestricted_ctx("/tmp/project");
        let r = check_shell_command("echo x > /etc/hosts", &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    #[test]
    fn shell_check_unrestricted_allows_normal_redirect() {
        let ctx = unrestricted_ctx("/tmp/project");
        let r = check_shell_command("echo x > /home/user/out.txt", &ctx);
        assert_eq!(r, SandboxCheckResult::Allowed);
    }

    #[test]
    fn shell_check_readonly_blocks_redirect() {
        let ctx = readonly_ctx("/tmp/project");
        let r = check_shell_command("echo x > /tmp/project/out.txt", &ctx);
        assert!(matches!(r, SandboxCheckResult::Denied(_)));
    }

    // ── SandboxContext::from_input ──────────────────────────────────────

    #[test]
    fn from_input_default_when_absent() {
        let input = serde_json::json!({"workdir": "/tmp/project"});
        let ctx = SandboxContext::from_input(&input);
        assert!(matches!(ctx.policy, SandboxPolicy::WorkspaceWrite { .. }));
        assert_eq!(ctx.workdir, normalize_lexical(Path::new("/tmp/project")));
    }

    #[test]
    fn from_input_unrestricted() {
        let input = serde_json::json!({
            "workdir": "/tmp/project",
            "__sandbox_policy": {"type": "unrestricted"}
        });
        let ctx = SandboxContext::from_input(&input);
        assert_eq!(ctx.policy, SandboxPolicy::Unrestricted);
    }

    #[test]
    fn from_input_workspace_with_roots() {
        let input = serde_json::json!({
            "workdir": "/tmp/project",
            "__sandbox_policy": {
                "type": "workspace_write",
                "additional_writable_roots": ["/home/data"]
            }
        });
        let ctx = SandboxContext::from_input(&input);
        assert!(ctx
            .writable_roots
            .contains(&normalize_lexical(Path::new("/home/data"))));
    }

    // ── Serde round-trip ────────────────────────────────────────────────

    #[test]
    fn serde_roundtrip_workspace_write() {
        let policy = SandboxPolicy::WorkspaceWrite {
            additional_writable_roots: vec![PathBuf::from("/data")],
        };
        let json = serde_json::to_string(&policy).unwrap();
        let parsed: SandboxPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, parsed);
    }

    #[test]
    fn serde_roundtrip_unrestricted() {
        let policy = SandboxPolicy::Unrestricted;
        let json = serde_json::to_string(&policy).unwrap();
        let parsed: SandboxPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, parsed);
    }

    #[test]
    fn serde_roundtrip_readonly() {
        let policy = SandboxPolicy::ReadOnly;
        let json = serde_json::to_string(&policy).unwrap();
        let parsed: SandboxPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, parsed);
    }
}
