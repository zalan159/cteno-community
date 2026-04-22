#![cfg_attr(not(feature = "tauri-commands"), allow(dead_code))]

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[cfg(feature = "tauri-commands")]
pub mod commands;

fn get_archive_dir(app_dir: &Path) -> Result<PathBuf, String> {
    let sessions_dir = app_dir.join("sessions");
    fs::create_dir_all(&sessions_dir)
        .map_err(|e| format!("Failed to create sessions dir: {}", e))?;
    Ok(sessions_dir)
}

pub(crate) fn archive_append_line_core(
    app_dir: &Path,
    path: &str,
    line: &str,
) -> Result<(), String> {
    let archive_dir = get_archive_dir(app_dir)?;
    let file_path = archive_dir.join(&path);

    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create parent dir: {}", e))?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
        .map_err(|e| format!("Failed to open archive file: {}", e))?;

    writeln!(file, "{line}").map_err(|e| format!("Failed to write to archive: {}", e))?;
    Ok(())
}

pub(crate) fn archive_read_lines_core(app_dir: &Path, path: &str) -> Result<Vec<String>, String> {
    let archive_dir = get_archive_dir(app_dir)?;
    let file_path = archive_dir.join(&path);

    if !file_path.exists() {
        return Ok(Vec::new());
    }

    let file = File::open(&file_path).map_err(|e| format!("Failed to open archive file: {}", e))?;
    let reader = BufReader::new(file);
    let lines: Result<Vec<String>, _> = reader.lines().collect();
    lines.map_err(|e| format!("Failed to read archive lines: {}", e))
}

pub(crate) fn archive_exists_core(app_dir: &Path, path: &str) -> Result<bool, String> {
    let archive_dir = get_archive_dir(app_dir)?;
    let file_path = archive_dir.join(&path);
    Ok(file_path.exists())
}

pub(crate) fn archive_list_files_core(
    app_dir: &Path,
    pattern: Option<String>,
) -> Result<Vec<String>, String> {
    let archive_dir = get_archive_dir(app_dir)?;

    if !archive_dir.exists() {
        return Ok(Vec::new());
    }

    let entries =
        fs::read_dir(&archive_dir).map_err(|e| format!("Failed to read archive dir: {}", e))?;

    let mut files = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();

        if path.is_file() {
            if let Some(file_name) = path.file_name() {
                let name = file_name.to_string_lossy().to_string();

                if let Some(ref pat) = pattern {
                    if pat == "*.jsonl" {
                        if name.ends_with(".jsonl") {
                            files.push(name);
                        }
                    } else {
                        files.push(name);
                    }
                } else {
                    files.push(name);
                }
            }
        }
    }

    Ok(files)
}

pub(crate) fn archive_delete_file_core(app_dir: &Path, path: &str) -> Result<(), String> {
    let archive_dir = get_archive_dir(app_dir)?;
    let file_path = archive_dir.join(&path);

    if file_path.exists() {
        fs::remove_file(&file_path).map_err(|e| format!("Failed to delete archive file: {}", e))?;
    }

    Ok(())
}
