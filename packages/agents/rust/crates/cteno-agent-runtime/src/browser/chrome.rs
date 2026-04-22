//! Chrome Discovery, Profile Copying, and Process Launch

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Files/dirs to copy from Chrome profile for login state preservation.
const PROFILE_ITEMS: &[&str] = &[
    "Default/Cookies",
    "Default/Login Data",
    "Default/Web Data",
    "Default/Preferences",
    "Default/Network",
    "Local State",
];

/// Find a Chromium-based browser executable on the system.
///
/// Strategy:
/// 1. Collect all installed Chromium-based browsers
/// 2. If only one found, use it
/// 3. If multiple found, prefer the system's default browser (if it's Chromium-based)
/// 4. If none found, return an error prompting the user to install one
pub fn find_chrome() -> Result<PathBuf, String> {
    let installed = find_installed_chromium_browsers();

    if installed.is_empty() {
        return Err(
            "未找到 Chromium 内核浏览器。请安装 Chrome、Edge、Brave 或其他 Chromium 内核浏览器。"
                .to_string(),
        );
    }

    if installed.len() == 1 {
        log::info!("[Browser] Single browser found: {}", installed[0].display());
        return Ok(installed[0].clone());
    }

    // Multiple browsers installed — try to prefer the system default
    log::info!(
        "[Browser] Found {} Chromium browsers, checking system default...",
        installed.len()
    );
    if let Some(default_hint) = detect_system_default_browser() {
        for candidate in &installed {
            if is_same_browser(candidate, &default_hint) {
                log::info!(
                    "[Browser] Using system default browser: {}",
                    candidate.display()
                );
                return Ok(candidate.clone());
            }
        }
        log::info!(
            "[Browser] System default ({}) is not Chromium-based, using first found",
            default_hint.display()
        );
    }

    log::info!("[Browser] Using first found: {}", installed[0].display());
    Ok(installed[0].clone())
}

/// Collect all installed Chromium-based browsers on this system.
fn find_installed_chromium_browsers() -> Vec<PathBuf> {
    let mut found = Vec::new();

    #[cfg(target_os = "macos")]
    {
        let candidates = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/Applications/Arc.app/Contents/MacOS/Arc",
            "/Applications/Vivaldi.app/Contents/MacOS/Vivaldi",
            "/Applications/Opera.app/Contents/MacOS/Opera",
            "/Applications/Opera GX.app/Contents/MacOS/Opera GX",
            "/Applications/Comet.app/Contents/MacOS/Comet",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ];
        for p in &candidates {
            let path = PathBuf::from(p);
            if path.exists() {
                found.push(path);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let pf = std::env::var("ProgramFiles").unwrap_or_default();
        let pf86 = std::env::var("ProgramFiles(x86)").unwrap_or_default();
        let lad = std::env::var("LocalAppData").unwrap_or_default();

        let candidates: Vec<String> = vec![
            // Google Chrome
            format!(r"{}\Google\Chrome\Application\chrome.exe", pf),
            format!(r"{}\Google\Chrome\Application\chrome.exe", pf86),
            format!(r"{}\Google\Chrome\Application\chrome.exe", lad),
            // Microsoft Edge
            format!(r"{}\Microsoft\Edge\Application\msedge.exe", pf),
            format!(r"{}\Microsoft\Edge\Application\msedge.exe", pf86),
            // Brave
            format!(r"{}\BraveSoftware\Brave-Browser\Application\brave.exe", pf),
            format!(r"{}\BraveSoftware\Brave-Browser\Application\brave.exe", lad),
            // Vivaldi
            format!(r"{}\Vivaldi\Application\vivaldi.exe", lad),
            // Opera
            format!(r"{}\Opera Software\Opera Stable\opera.exe", lad),
            // 360 安全浏览器 / 极速浏览器
            format!(r"{}\360Chrome\Chrome\Application\360chrome.exe", lad),
            format!(r"{}\360Chrome\Chrome\Application\360chrome.exe", pf),
            format!(r"{}\360\360se6\Application\360se.exe", pf),
            format!(r"{}\360\360se6\Application\360se.exe", pf86),
            format!(r"{}\360ChromeX\Chrome\Application\360chromex.exe", lad),
            // 搜狗浏览器
            format!(r"{}\SogouExplorer\SogouExplorer.exe", lad),
            format!(r"{}\SogouExplorer\SogouExplorer.exe", pf),
            // QQ 浏览器
            format!(r"{}\Tencent\QQBrowser\QQBrowser.exe", lad),
            format!(r"{}\Tencent\QQBrowser\QQBrowser.exe", pf),
            // 2345 浏览器
            format!(r"{}\2345Explorer\2345Explorer.exe", lad),
            // 猎豹浏览器
            format!(r"{}\liebao\liebao.exe", lad),
            // Comet
            format!(r"{}\Comet\Application\comet.exe", lad),
        ];
        // Deduplicate: same exe can appear via multiple env var combos
        let mut seen = std::collections::HashSet::new();
        for p in &candidates {
            let path = PathBuf::from(p);
            if path.exists() {
                let canonical = path.to_string_lossy().to_lowercase();
                if seen.insert(canonical) {
                    found.push(path);
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        for name in &[
            "google-chrome",
            "google-chrome-stable",
            "microsoft-edge",
            "microsoft-edge-stable",
            "brave-browser",
            "vivaldi",
            "opera",
            "chromium-browser",
            "chromium",
            "comet",
        ] {
            if let Ok(path) = which::which(name) {
                found.push(path);
            }
        }
    }

    if !found.is_empty() {
        log::info!(
            "[Browser] Installed Chromium browsers: {:?}",
            found
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
        );
    }
    found
}

// ---------------------------------------------------------------------------
// System default browser detection (platform-specific)
// ---------------------------------------------------------------------------

/// Detect the system's default browser. Returns a path hint that can be
/// compared against the installed Chromium list via `is_same_browser()`.
///
/// On macOS: returns the .app bundle path (e.g. `/Applications/Google Chrome.app/`).
/// On Windows: returns the .exe path from the registry.
/// On Linux: returns the resolved binary path from xdg-settings.
fn detect_system_default_browser() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        // Use NSWorkspace via osascript to find the default HTTPS handler
        let output = Command::new("osascript")
            .args([
                "-e",
                r#"use framework "AppKit"
set appURL to current application's NSWorkspace's sharedWorkspace()'s URLForApplicationToOpenURL:(current application's |NSURL|'s URLWithString:"https://example.com")
return POSIX path of appURL"#,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }
        let path_str = String::from_utf8(output.stdout).ok()?.trim().to_string();
        if path_str.is_empty() {
            return None;
        }
        log::info!("[Browser] System default browser app: {}", path_str);
        return Some(PathBuf::from(path_str));
    }

    #[cfg(target_os = "windows")]
    {
        // Read the ProgId for HTTPS from registry
        let output = Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice",
                "/v",
                "ProgId",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8(output.stdout).ok()?;
        let prog_id = stdout
            .lines()
            .find(|l| l.contains("ProgId"))?
            .split_whitespace()
            .last()?
            .to_string();

        // Look up the open command for this ProgId
        let output2 = Command::new("reg")
            .args([
                "query",
                &format!(r"HKEY_CLASSES_ROOT\{}\shell\open\command", prog_id),
                "/ve",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;

        if !output2.status.success() {
            return None;
        }
        let stdout2 = String::from_utf8(output2.stdout).ok()?;
        // Parse: `"C:\...\chrome.exe" --single-argument %1`
        let cmd_line = stdout2.lines().find(|l| l.contains("REG_SZ"))?;
        let exe_path = if let Some(start) = cmd_line.find('"') {
            let rest = &cmd_line[start + 1..];
            rest.split('"').next()?
        } else {
            return None;
        };

        let path = PathBuf::from(exe_path);
        if path.exists() {
            log::info!("[Browser] System default browser exe: {}", path.display());
            return Some(path);
        }
        return None;
    }

    #[cfg(target_os = "linux")]
    {
        // xdg-settings returns e.g. "google-chrome.desktop"
        let output = Command::new("xdg-settings")
            .args(["get", "default-web-browser"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }
        let desktop = String::from_utf8(output.stdout).ok()?.trim().to_string();
        let name = desktop.strip_suffix(".desktop").unwrap_or(&desktop);
        if let Ok(path) = which::which(name) {
            log::info!("[Browser] System default browser: {}", path.display());
            return Some(path);
        }
        return None;
    }

    #[allow(unreachable_code)]
    None
}

/// Check if a candidate executable belongs to the same browser as the default hint.
fn is_same_browser(candidate: &Path, default_hint: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        // default_hint: "/Applications/Google Chrome.app/" (or without trailing slash)
        // candidate:    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        let hint_str = default_hint.to_string_lossy();
        let hint_trimmed = hint_str.trim_end_matches('/');
        return candidate.to_string_lossy().starts_with(hint_trimmed);
    }

    #[cfg(target_os = "windows")]
    {
        // Direct exe path comparison (case-insensitive)
        return candidate.to_string_lossy().to_lowercase()
            == default_hint.to_string_lossy().to_lowercase();
    }

    #[cfg(target_os = "linux")]
    {
        return candidate == default_hint;
    }

    #[allow(unreachable_code)]
    false
}

/// Get the default Chrome user data directory for the current platform.
pub fn default_profile_dir() -> PathBuf {
    profile_dir_for_browser(None)
}

/// Detect the user data directory for a specific browser executable.
/// Falls back to Google Chrome's profile dir if the browser is not recognized.
pub fn profile_dir_for_browser(browser_exe: Option<&Path>) -> PathBuf {
    let exe_lower = browser_exe
        .map(|p| p.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir().unwrap_or_default();
        let app_support = home.join("Library/Application Support");

        if exe_lower.contains("microsoft edge") {
            return app_support.join("Microsoft Edge");
        } else if exe_lower.contains("brave") {
            return app_support.join("BraveSoftware/Brave-Browser");
        } else if exe_lower.contains("vivaldi") {
            return app_support.join("Vivaldi");
        } else if exe_lower.contains("opera") {
            return app_support.join("com.operasoftware.Opera");
        } else if exe_lower.contains("arc") {
            return app_support.join("Arc/User Data");
        } else if exe_lower.contains("comet") {
            return app_support.join("Comet/User Data");
        } else if exe_lower.contains("chromium") {
            return app_support.join("Chromium");
        }
        // Default: Google Chrome
        app_support.join("Google/Chrome")
    }

    #[cfg(target_os = "windows")]
    {
        let lad = std::env::var("LocalAppData").unwrap_or_default();

        if exe_lower.contains("msedge") || exe_lower.contains("edge") {
            return PathBuf::from(format!(r"{}\Microsoft\Edge\User Data", lad));
        } else if exe_lower.contains("brave") {
            return PathBuf::from(format!(r"{}\BraveSoftware\Brave-Browser\User Data", lad));
        } else if exe_lower.contains("vivaldi") {
            return PathBuf::from(format!(r"{}\Vivaldi\User Data", lad));
        } else if exe_lower.contains("opera") {
            return PathBuf::from(format!(r"{}\Opera Software\Opera Stable", lad));
        } else if exe_lower.contains("360chrome") || exe_lower.contains("360se") {
            return PathBuf::from(format!(r"{}\360Chrome\Chrome\User Data", lad));
        } else if exe_lower.contains("comet") {
            return PathBuf::from(format!(r"{}\Comet\User Data", lad));
        }
        // Default: Google Chrome
        PathBuf::from(format!(r"{}\Google\Chrome\User Data", lad))
    }

    #[cfg(target_os = "linux")]
    {
        let home = dirs::home_dir().unwrap_or_default();

        if exe_lower.contains("microsoft-edge") {
            return home.join(".config/microsoft-edge");
        } else if exe_lower.contains("brave") {
            return home.join(".config/BraveSoftware/Brave-Browser");
        } else if exe_lower.contains("vivaldi") {
            return home.join(".config/vivaldi");
        } else if exe_lower.contains("opera") {
            return home.join(".config/opera");
        } else if exe_lower.contains("chromium") {
            return home.join(".config/chromium");
        } else if exe_lower.contains("comet") {
            return home.join(".config/comet");
        }
        // Default: Google Chrome
        home.join(".config/google-chrome")
    }
}

/// Copy key profile files to a temporary directory for CDP use.
/// `browser_exe` is used to detect the correct source profile directory.
/// Returns the path to the temp profile directory.
pub fn copy_profile(session_id: &str, browser_exe: Option<&Path>) -> Result<PathBuf, String> {
    let src = profile_dir_for_browser(browser_exe);
    log::info!(
        "[Browser] Copying profile from {:?} (browser: {:?})",
        src,
        browser_exe
    );
    let tmp_dir = std::env::temp_dir().join(format!("cdp_{}", session_id));

    // Create Default subdirectory
    std::fs::create_dir_all(tmp_dir.join("Default"))
        .map_err(|e| format!("Failed to create temp profile dir: {}", e))?;

    for item in PROFILE_ITEMS {
        let src_path = src.join(item);
        let dst_path = tmp_dir.join(item);

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path).ok();
        } else if src_path.is_file() {
            if let Some(parent) = dst_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::copy(&src_path, &dst_path).ok();
        }
    }

    log::info!("[Browser] Profile copied to {:?}", tmp_dir);
    Ok(tmp_dir)
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("Failed to create dir {:?}: {}", dst, e))?;

    for entry in
        std::fs::read_dir(src).map_err(|e| format!("Failed to read dir {:?}: {}", src, e))?
    {
        let entry = entry.map_err(|e| format!("Dir entry error: {}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).ok();
        }
    }
    Ok(())
}

/// Launch Chrome with CDP debugging enabled.
/// Returns (child process, browser executable path).
pub fn launch_chrome(
    profile_dir: &Path,
    port: u16,
    headless: bool,
) -> Result<(Child, PathBuf), String> {
    let chrome_path = find_chrome()?;

    let mut args = vec![
        format!("--remote-debugging-port={}", port),
        format!("--user-data-dir={}", profile_dir.display()),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--window-size=1280,900".to_string(),
        // Disable "Chrome is being controlled by automated test software" banner
        "--disable-infobars".to_string(),
    ];

    if headless {
        args.push("--headless=new".to_string());
    }

    log::info!(
        "[Browser] Launching {} on port {} (headless: {})",
        chrome_path.display(),
        port,
        headless
    );

    let child = Command::new(&chrome_path)
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to launch Chrome: {}", e))?;

    Ok((child, chrome_path))
}

/// Wait for CDP to become available on the given port.
pub async fn wait_for_cdp(port: u16, timeout_secs: u64) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);
    let url = format!("http://127.0.0.1:{}/json/version", port);

    loop {
        match reqwest::get(&url).await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            _ => {}
        }

        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "Chrome CDP not ready on port {} after {}s",
                port, timeout_secs
            ));
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }
}

/// Clean up temp profile directory.
pub fn cleanup_profile(profile_dir: &Path) {
    if profile_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(profile_dir) {
            log::warn!(
                "[Browser] Failed to remove temp profile {:?}: {}",
                profile_dir,
                e
            );
        }
    }
}
