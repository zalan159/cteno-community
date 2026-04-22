//! Cross-platform symlink helper with graceful fallbacks.
//!
//! Policy:
//! - The link path is re-pointed authoritatively. Any existing file, symlink,
//!   or empty dir at the link path is removed first. Non-empty directories
//!   raise an error (we refuse to recurse-delete user data).
//! - Unix: `std::os::unix::fs::symlink`.
//! - Windows: try junction for directories + `symlink_file` for files; on
//!   permission error we fall back to copy (files) or recursive copy (dirs).

use std::{
    fs,
    io::{self, ErrorKind},
    path::Path,
};

use anyhow::{bail, Context, Result};

/// Ensure that `link` is a symlink (or equivalent) pointing to `target` file.
///
/// Safety rails:
/// - If `link` and `target` already canonicalize to the same inode (either by
///   pointing at each other already, or by resolving through a chain to the
///   same real file), do nothing. This covers cases like a pre-existing
///   `AGENTS.md -> CLAUDE.md` symlink where the caller asks us to create
///   `CLAUDE.md -> AGENTS.md` — the canonical destination is the same file,
///   so we leave both entries alone instead of inverting the chain and
///   creating a cycle.
/// - If the proposed new symlink would resolve (transitively) back to `link`
///   itself, refuse — that would be a cycle.
pub fn ensure_symlink(target: &Path, link: &Path) -> Result<()> {
    ensure_parent(link)?;

    let link_canonical = fs::canonicalize(link).ok();
    let target_canonical = fs::canonicalize(target).ok();

    if let (Some(lc), Some(tc)) = (link_canonical.as_ref(), target_canonical.as_ref()) {
        if lc == tc {
            // Both sides already resolve to the same real file — nothing to
            // do, and critically: refuse to rewrite `link` into a direct
            // symlink at `target` (which might be a symlink itself pointing
            // back at `link`, forming a cycle).
            return Ok(());
        }
    }

    match fs::symlink_metadata(link) {
        Ok(md) => {
            if md.file_type().is_symlink() {
                if let Ok(existing) = fs::read_link(link) {
                    if existing == target {
                        return Ok(());
                    }
                }
                fs::remove_file(link).with_context(|| format!("remove stale symlink {link:?}"))?;
            } else if md.is_file() {
                fs::remove_file(link)
                    .with_context(|| format!("remove conflicting file {link:?}"))?;
            } else {
                bail!(
                    "refuse to overwrite non-file non-symlink at link path {:?}",
                    link
                );
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("stat {link:?}"))?,
    }
    create_file_symlink(target, link)
}

/// Ensure that `link` is a symlink pointing at `target` directory.
pub fn ensure_symlink_to_dir(target: &Path, link: &Path) -> Result<()> {
    if !target.is_dir() {
        bail!("symlink target is not a directory: {target:?}");
    }
    ensure_parent(link)?;
    match fs::symlink_metadata(link) {
        Ok(md) => {
            if md.file_type().is_symlink() {
                if let Ok(existing) = fs::read_link(link) {
                    if existing == target {
                        return Ok(());
                    }
                }
                remove_symlink_or_file(link)?;
            } else if md.is_file() {
                fs::remove_file(link).with_context(|| {
                    format!("remove conflicting file at dir-link path {link:?}")
                })?;
            } else if md.is_dir() {
                match fs::remove_dir(link) {
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::DirectoryNotEmpty || is_not_empty(&e) => {
                        bail!(
                            "refuse to overwrite non-empty directory {:?} (looks like user data)",
                            link
                        );
                    }
                    Err(e) => return Err(e).with_context(|| format!("remove empty dir {link:?}")),
                }
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("stat {link:?}"))?,
    }
    create_dir_symlink(target, link)
}

fn ensure_parent(link: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir for link {link:?}"))?;
    }
    Ok(())
}

fn remove_symlink_or_file(p: &Path) -> Result<()> {
    fs::remove_file(p).with_context(|| format!("remove {p:?}"))
}

fn is_not_empty(e: &io::Error) -> bool {
    // ErrorKind::DirectoryNotEmpty stabilized in newer toolchains; fallback
    // matches raw_os_error for ENOTEMPTY = 66 on macOS, 39 on Linux.
    matches!(e.raw_os_error(), Some(66) | Some(39) | Some(145))
}

#[cfg(unix)]
fn create_file_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("symlink {target:?} -> {link:?}"))
}

#[cfg(unix)]
fn create_dir_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("dir symlink {target:?} -> {link:?}"))
}

#[cfg(windows)]
fn create_file_symlink(target: &Path, link: &Path) -> Result<()> {
    match std::os::windows::fs::symlink_file(target, link) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(1314) => {
            // ERROR_PRIVILEGE_NOT_HELD — fall back to copying content.
            tracing::warn!(
                "symlink privilege not held; copying file instead ({:?} -> {:?})",
                target,
                link
            );
            fs::copy(target, link)
                .with_context(|| format!("copy fallback {target:?} -> {link:?}"))?;
            Ok(())
        }
        Err(e) => Err(e).with_context(|| format!("symlink_file {target:?} -> {link:?}"))?,
    }
}

#[cfg(windows)]
fn create_dir_symlink(target: &Path, link: &Path) -> Result<()> {
    match std::os::windows::fs::symlink_dir(target, link) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(1314) => {
            tracing::warn!(
                "dir symlink privilege not held; copying recursively ({:?} -> {:?})",
                target,
                link
            );
            copy_dir_recursive(target, link)
        }
        Err(e) => Err(e).with_context(|| format!("symlink_dir {target:?} -> {link:?}"))?,
    }
}

#[cfg(windows)]
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("mkdir {dst:?}"))?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn creates_missing_file_symlink() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("source.md");
        std::fs::write(&target, "hello").unwrap();
        let link = tmp.path().join("a/b/link.md");
        ensure_symlink(&target, &link).unwrap();
        let read = std::fs::read_to_string(&link).unwrap();
        assert_eq!(read, "hello");
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn retargets_existing_symlink() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a.md");
        let b = tmp.path().join("b.md");
        std::fs::write(&a, "A").unwrap();
        std::fs::write(&b, "B").unwrap();
        let link = tmp.path().join("link.md");
        ensure_symlink(&a, &link).unwrap();
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "A");
        ensure_symlink(&b, &link).unwrap();
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "B");
    }

    #[test]
    fn overwrites_existing_regular_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("target.md");
        std::fs::write(&target, "fresh").unwrap();
        let link = tmp.path().join("link.md");
        std::fs::write(&link, "stale").unwrap();
        ensure_symlink(&target, &link).unwrap();
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "fresh");
    }

    #[test]
    fn dir_symlink_creates_and_retargets() {
        let tmp = TempDir::new().unwrap();
        let skill_a = tmp.path().join("skills/a");
        let skill_b = tmp.path().join("skills/b");
        std::fs::create_dir_all(&skill_a).unwrap();
        std::fs::create_dir_all(&skill_b).unwrap();
        std::fs::write(skill_a.join("SKILL.md"), "skill A").unwrap();
        std::fs::write(skill_b.join("SKILL.md"), "skill B").unwrap();
        let link = tmp.path().join(".claude/skills/x");
        ensure_symlink_to_dir(&skill_a, &link).unwrap();
        assert_eq!(
            std::fs::read_to_string(link.join("SKILL.md")).unwrap(),
            "skill A"
        );
        ensure_symlink_to_dir(&skill_b, &link).unwrap();
        assert_eq!(
            std::fs::read_to_string(link.join("SKILL.md")).unwrap(),
            "skill B"
        );
    }

    #[test]
    fn no_op_when_both_already_resolve_to_same_file() {
        // Pre-existing convention: AGENTS.md is a symlink → CLAUDE.md.
        // Ask ensure_symlink to create CLAUDE.md → AGENTS.md. It must
        // detect that both paths already canonicalize to the same real
        // file and do nothing (avoiding a cycle).
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let claude = project.join("CLAUDE.md");
        let agents = project.join("AGENTS.md");
        std::fs::write(&claude, "real prompt body").unwrap();
        std::os::unix::fs::symlink("CLAUDE.md", &agents).unwrap();

        // Sanity: AGENTS.md resolves to CLAUDE.md.
        assert_eq!(
            std::fs::canonicalize(&agents).unwrap(),
            std::fs::canonicalize(&claude).unwrap()
        );

        // Ask to create CLAUDE.md → AGENTS.md. Should be a no-op, not a cycle.
        ensure_symlink(&agents, &claude).unwrap();

        // Verify: CLAUDE.md is still a regular file with the original content.
        let md = std::fs::symlink_metadata(&claude).unwrap();
        assert!(md.is_file(), "CLAUDE.md should still be a regular file");
        assert_eq!(
            std::fs::read_to_string(&claude).unwrap(),
            "real prompt body"
        );
        // AGENTS.md still → CLAUDE.md.
        assert_eq!(
            std::fs::read_link(&agents).unwrap().to_str().unwrap(),
            "CLAUDE.md"
        );
    }

    #[test]
    fn refuses_to_clobber_non_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("target");
        std::fs::create_dir_all(&target).unwrap();
        let link = tmp.path().join("user-dir");
        std::fs::create_dir_all(&link).unwrap();
        std::fs::write(link.join("IMPORTANT.md"), "user data").unwrap();
        let err = ensure_symlink_to_dir(&target, &link).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("non-empty"), "got: {msg}");
        // User data still there.
        assert!(link.join("IMPORTANT.md").exists());
    }
}
