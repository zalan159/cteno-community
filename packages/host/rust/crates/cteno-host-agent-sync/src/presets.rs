//! Canonical spec builders for host-managed resources.
//!
//! `cteno-memory` is kept here only for old migrations/tests. New cross-vendor
//! memory access is exposed through `ctenoctl memory` instructions in
//! `AGENTS.md`, so vendor sessions do not spawn per-session memory MCP
//! subprocesses.

use std::{collections::BTreeMap, path::Path};

use crate::schemas::{McpSpec, McpTransport};

/// Build the canonical `cteno-memory` MCP server spec for a given project.
///
/// - `memory_bin`      — absolute path to the `cteno-memory-mcp` binary
///                        (ship-time bundled with Cteno's daemon).
/// - `project_dir`     — project root (Cteno attaches `.cteno/memory/` under it).
/// - `global_dir`      — optional custom global memory dir; defaults to
///                        `~/.cteno/memory` as resolved by the bin itself
///                        when omitted.
pub fn memory_mcp_spec(
    memory_bin: &Path,
    project_dir: &Path,
    global_dir: Option<&Path>,
) -> McpSpec {
    let mut args = vec![
        "--project-dir".to_string(),
        project_dir.display().to_string(),
    ];
    if let Some(g) = global_dir {
        args.push("--global-dir".into());
        args.push(g.display().to_string());
    }
    McpSpec {
        name: "cteno-memory".into(),
        command: memory_bin.display().to_string(),
        args,
        env: BTreeMap::new(),
        transport: McpTransport::Stdio,
        host_managed: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn builds_command_and_args() {
        let spec = memory_mcp_spec(
            Path::new("/usr/local/bin/cteno-memory-mcp"),
            Path::new("/home/u/proj"),
            None,
        );
        assert_eq!(spec.name, "cteno-memory");
        assert_eq!(spec.command, "/usr/local/bin/cteno-memory-mcp");
        assert_eq!(
            spec.args,
            vec!["--project-dir".to_string(), "/home/u/proj".to_string()]
        );
        assert!(spec.host_managed);
    }

    #[test]
    fn optional_global_dir_appends_args() {
        let spec = memory_mcp_spec(
            Path::new("/bin/cteno-memory-mcp"),
            Path::new("/p"),
            Some(&PathBuf::from("/custom/global")),
        );
        assert_eq!(
            spec.args,
            vec![
                "--project-dir".to_string(),
                "/p".to_string(),
                "--global-dir".to_string(),
                "/custom/global".to_string(),
            ]
        );
    }
}
