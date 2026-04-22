//! Shared path resolution helpers for tool executors.
//!
//! Goals:
//! - Expand `~` consistently.
//! - Resolve relative paths against an optional `workdir` (injected by the agent runtime).
//! - Prevent relative-path traversal from escaping the `workdir`.

use super::sandbox::{self, SandboxCheckResult, SandboxContext};
use std::path::{Component, Path, PathBuf};

pub fn user_home_dir() -> PathBuf {
    dirs::home_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Expand `~` and `~/...` to an absolute path.
///
/// Note: `~username/...` is not supported and is returned as-is.
pub fn expand_tilde(input: &str) -> PathBuf {
    let input = input.trim();
    if input == "~" {
        return user_home_dir();
    }

    if input.starts_with("~/") || input.starts_with("~\\") {
        return user_home_dir().join(&input[2..]);
    }

    PathBuf::from(input)
}

/// Resolve an effective workdir.
/// - If `workdir` is missing/blank: home directory.
/// - If `workdir` is relative: anchored to home directory for deterministic behavior.
pub fn resolve_workdir(workdir: Option<&str>) -> PathBuf {
    let raw = workdir
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("~");
    let expanded = expand_tilde(raw);
    if expanded.is_absolute() {
        normalize_lexical(&expanded)
    } else {
        normalize_lexical(&user_home_dir().join(expanded))
    }
}

/// Lexically normalize a path (no filesystem access):
/// removes `.` and resolves `..` where possible.
pub fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                // Keep root stable: popping a root path is a no-op.
                let _ = out.pop();
            }
            Component::Normal(p) => out.push(p),
        }
    }
    out
}

/// Resolve a file path for tool executors.
///
/// Behavior:
/// - Absolute paths are accepted (after `~` expansion).
/// - Relative paths are resolved against `workdir` (after `~` expansion).
/// - Relative paths may not escape the resolved `workdir` via `..`.
pub fn resolve_file_path(path: &str, workdir: Option<&str>) -> Result<PathBuf, String> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err("path cannot be empty".to_string());
    }

    let expanded = expand_tilde(raw);
    if expanded.is_absolute() {
        return Ok(normalize_lexical(&expanded));
    }

    let base = resolve_workdir(workdir);
    let joined = normalize_lexical(&base.join(expanded));

    // For relative inputs, prevent escaping the base directory.
    if !joined.starts_with(&base) {
        return Err(format!(
            "Relative path escapes workdir: '{}' (workdir: '{}')",
            raw,
            base.display()
        ));
    }

    Ok(joined)
}

/// Resolve a file path with sandbox enforcement.
///
/// For write operations (`is_write = true`), checks the resolved path against
/// the sandbox policy. For read operations, the sandbox is not enforced.
pub fn resolve_file_path_sandboxed(
    path: &str,
    workdir: Option<&str>,
    sandbox: &SandboxContext,
    is_write: bool,
) -> Result<PathBuf, String> {
    let resolved = resolve_file_path(path, workdir)?;
    if !is_write {
        return Ok(resolved);
    }
    match sandbox::check_write_path(&resolved, sandbox) {
        SandboxCheckResult::Allowed => Ok(resolved),
        SandboxCheckResult::Denied(reason) => Err(format!("SANDBOX_DENIED: {}", reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_basic() {
        let home = user_home_dir();
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/a/b"), home.join("a").join("b"));
    }

    #[test]
    fn resolve_workdir_relative_anchors_to_home() {
        let home = user_home_dir();
        let wd = resolve_workdir(Some("tmp/project"));
        assert_eq!(wd, normalize_lexical(&home.join("tmp").join("project")));
    }

    #[test]
    fn resolve_file_path_relative_under_workdir() {
        let base = PathBuf::from("/tmp/work");
        let p = resolve_file_path("a/b.txt", Some(base.to_str().unwrap())).unwrap();
        assert_eq!(p, normalize_lexical(&base.join("a").join("b.txt")));
    }

    #[test]
    fn resolve_file_path_relative_escape_rejected() {
        let base = PathBuf::from("/tmp/work");
        let err = resolve_file_path("../etc/passwd", Some(base.to_str().unwrap())).unwrap_err();
        assert!(err.contains("escapes workdir"));
    }

    // ── Sandboxed resolver tests ────────────────────────────────────────

    #[test]
    fn sandboxed_write_inside_workdir_allowed() {
        let ctx = SandboxContext::new(sandbox::SandboxPolicy::default(), Some("/tmp/project"));
        let r = resolve_file_path_sandboxed("src/main.rs", Some("/tmp/project"), &ctx, true);
        assert!(r.is_ok());
    }

    #[test]
    fn sandboxed_write_outside_workdir_denied() {
        let ctx = SandboxContext::new(sandbox::SandboxPolicy::default(), Some("/tmp/project"));
        let r =
            resolve_file_path_sandboxed("/home/user/secret.txt", Some("/tmp/project"), &ctx, true);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("SANDBOX_DENIED"));
    }

    #[test]
    fn sandboxed_read_outside_workdir_allowed() {
        let ctx = SandboxContext::new(sandbox::SandboxPolicy::default(), Some("/tmp/project"));
        let r =
            resolve_file_path_sandboxed("/home/user/file.txt", Some("/tmp/project"), &ctx, false);
        assert!(r.is_ok());
    }

    #[test]
    fn sandboxed_unrestricted_allows_outside() {
        let ctx = SandboxContext::new(sandbox::SandboxPolicy::Unrestricted, Some("/tmp/project"));
        let home = user_home_dir();
        let target = home.join("file.txt").to_string_lossy().to_string();
        let r = resolve_file_path_sandboxed(&target, Some("/tmp/project"), &ctx, true);
        assert!(r.is_ok());
    }

    #[test]
    fn sandboxed_unrestricted_still_blocks_system() {
        let ctx = SandboxContext::new(sandbox::SandboxPolicy::Unrestricted, Some("/tmp/project"));
        let r = resolve_file_path_sandboxed("/etc/hosts", Some("/tmp/project"), &ctx, true);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("SANDBOX_DENIED"));
    }
}
