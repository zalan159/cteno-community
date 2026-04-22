//! SkillHub registry integration — search, browse, and install skills from SkillHub.
//!
//! Registry endpoints:
//! - Featured: GET https://skillhub-1388575217.cos.ap-guangzhou.myqcloud.com/skills.json
//! - Search:   GET http://lb-3zbg86f6-0gwe3n7q8t4sv2za.clb.gz-tencentclb.com/api/v1/search?q=...&limit=N
//! - Download: GET https://skillhub-1388575217.cos.ap-guangzhou.myqcloud.com/skills/{slug}.zip

use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::Path;

const FEATURED_URL: &str = "https://skillhub-1388575217.cos.ap-guangzhou.myqcloud.com/skills.json";
const SEARCH_URL: &str = "http://lb-3zbg86f6-0gwe3n7q8t4sv2za.clb.gz-tencentclb.com/api/v1/search";
const DOWNLOAD_TEMPLATE: &str =
    "https://skillhub-1388575217.cos.ap-guangzhou.myqcloud.com/skills/{slug}.zip";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallResult {
    pub success: bool,
    pub skill_id: String,
    pub install_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillHubItem {
    pub slug: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub stats: SkillHubStats,
    #[serde(default)]
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillHubStats {
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub stars: u64,
}

// ——— Featured ———

#[derive(Deserialize)]
struct FeaturedResponse {
    #[serde(default)]
    skills: Vec<FeaturedEntry>,
}

#[derive(Deserialize)]
struct FeaturedEntry {
    slug: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    homepage: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    stats: Option<FeaturedStats>,
}

#[derive(Deserialize, Default)]
struct FeaturedStats {
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    stars: u64,
}

pub async fn fetch_featured(installed_ids: &[String]) -> Result<Vec<SkillHubItem>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp: FeaturedResponse = client
        .get(FEATURED_URL)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch featured skills: {}", e))?
        .json()
        .await
        .map_err(|e| format!("Failed to parse featured skills: {}", e))?;

    let items = resp
        .skills
        .into_iter()
        .map(|e| {
            let installed = installed_ids.contains(&e.slug);
            let stats = e.stats.unwrap_or_default();
            SkillHubItem {
                slug: e.slug,
                name: e.name,
                description: e.description,
                version: e.version,
                homepage: e.homepage,
                tags: e.tags,
                stats: SkillHubStats {
                    downloads: stats.downloads,
                    stars: stats.stars,
                },
                installed,
            }
        })
        .collect();

    Ok(items)
}

// ——— Search ———

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchEntry {
    slug: String,
    #[serde(alias = "displayName")]
    name: Option<String>,
    #[serde(alias = "summary")]
    description: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

pub async fn search_skills(
    query: &str,
    limit: usize,
    installed_ids: &[String],
) -> Result<Vec<SkillHubItem>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(vec![]);
    }

    // Try remote search API first
    if let Ok(items) = search_remote(query, limit, installed_ids).await {
        if !items.is_empty() {
            return Ok(items);
        }
    }

    // Fallback: search within featured list
    let featured = fetch_featured(installed_ids).await?;
    let query_lower = query.to_lowercase();
    let filtered: Vec<SkillHubItem> = featured
        .into_iter()
        .filter(|s| {
            s.name.to_lowercase().contains(&query_lower)
                || s.slug.to_lowercase().contains(&query_lower)
                || s.description.to_lowercase().contains(&query_lower)
        })
        .take(limit)
        .collect();

    Ok(filtered)
}

async fn search_remote(
    query: &str,
    limit: usize,
    installed_ids: &[String],
) -> Result<Vec<SkillHubItem>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp = client
        .get(SEARCH_URL)
        .query(&[("q", query), ("limit", &limit.to_string())])
        .send()
        .await
        .map_err(|e| format!("Search API error: {}", e))?;

    let search: SearchResponse = resp
        .json()
        .await
        .map_err(|e| format!("Search parse error: {}", e))?;

    let items = search
        .results
        .into_iter()
        .map(|e| {
            let installed = installed_ids.contains(&e.slug);
            SkillHubItem {
                name: e.name.unwrap_or_else(|| e.slug.clone()),
                description: e.description.unwrap_or_default(),
                version: e.version.unwrap_or_default(),
                installed,
                slug: e.slug,
                homepage: String::new(),
                tags: vec![],
                stats: SkillHubStats::default(),
            }
        })
        .collect();

    Ok(items)
}

// ——— Install ———

pub async fn install_skill(
    slug: &str,
    display_name: Option<&str>,
) -> Result<SkillInstallResult, String> {
    if slug.trim().is_empty() {
        return Err("slug is required".to_string());
    }

    let download_url = DOWNLOAD_TEMPLATE.replace("{slug}", slug);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp = client
        .get(&download_url)
        .send()
        .await
        .map_err(|e| format!("Failed to download skill '{}': {}", slug, e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Download failed for '{}': HTTP {}",
            slug,
            resp.status()
        ));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read download bytes: {}", e))?;

    // Install from zip
    let install_root = resolve_community_skill_dir()?;
    std::fs::create_dir_all(&install_root)
        .map_err(|e| format!("Failed to create skill directory {:?}: {}", install_root, e))?;

    let temp_root =
        std::env::temp_dir().join(format!("cteno-skillhub-install-{}", uuid::Uuid::new_v4()));
    let extract_dir = temp_root.join("extract");
    std::fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("Failed to create temp directory: {}", e))?;

    // Extract zip
    extract_zip_bytes(&bytes, &extract_dir)?;

    // Find skill dir (contains SKILL.md)
    let source_dir = find_skill_dir(&extract_dir).unwrap_or_else(|| extract_dir.clone());

    // Use slug as skill_id
    let skill_id = sanitize_skill_id(slug);
    let target_dir = install_root.join(&skill_id);

    if target_dir.exists() {
        std::fs::remove_dir_all(&target_dir)
            .map_err(|e| format!("Failed to replace existing skill: {}", e))?;
    }

    copy_dir_recursive(&source_dir, &target_dir)?;

    // Write source metadata (including display name for consistent UI)
    let mut source_meta = serde_json::json!({
        "sourceType": "skillhub",
        "sourceKey": slug,
        "installedVersion": read_skill_version(&target_dir),
        "installedAt": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(name) = display_name {
        source_meta["displayName"] = serde_json::Value::String(name.to_string());
    }
    let meta_path = target_dir.join(".cteno-source.json");
    let _ = std::fs::write(
        &meta_path,
        serde_json::to_string_pretty(&source_meta).unwrap_or_default(),
    );

    let result = SkillInstallResult {
        success: true,
        skill_id,
        install_path: target_dir.to_string_lossy().to_string(),
    };

    let _ = std::fs::remove_dir_all(&temp_root);

    Ok(result)
}

// ——— Helpers ———

fn resolve_community_skill_dir() -> Result<std::path::PathBuf, String> {
    dirs::home_dir()
        .map(|h| h.join(".agents").join("skills"))
        .ok_or_else(|| "Failed to resolve home directory".to_string())
}

fn extract_zip_bytes(zip_bytes: &[u8], extract_dir: &Path) -> Result<(), String> {
    let cursor = Cursor::new(zip_bytes.to_vec());
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("Failed to parse zip: {}", e))?;

    for idx in 0..archive.len() {
        let mut entry = archive
            .by_index(idx)
            .map_err(|e| format!("Failed to read zip entry: {}", e))?;

        let enclosed = entry
            .enclosed_name()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| format!("Invalid zip entry path: {}", entry.name()))?;

        let out_path = extract_dir.join(enclosed);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("Failed to create dir {:?}: {}", out_path, e))?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir: {}", e))?;
            }
            let mut file = std::fs::File::create(&out_path)
                .map_err(|e| format!("Failed to create file {:?}: {}", out_path, e))?;
            std::io::copy(&mut entry, &mut file)
                .map_err(|e| format!("Failed to write file {:?}: {}", out_path, e))?;
        }
    }

    Ok(())
}

fn find_skill_dir(search_root: &Path) -> Option<std::path::PathBuf> {
    // Direct SKILL.md
    if search_root.join("SKILL.md").exists() {
        return Some(search_root.to_path_buf());
    }

    // One level deep
    if let Ok(entries) = std::fs::read_dir(search_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("SKILL.md").exists() {
                return Some(path);
            }
        }
    }

    // Two levels deep
    if let Ok(entries) = std::fs::read_dir(search_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Ok(sub_entries) = std::fs::read_dir(&path) {
                    for sub_entry in sub_entries.flatten() {
                        let sub_path = sub_entry.path();
                        if sub_path.is_dir() && sub_path.join("SKILL.md").exists() {
                            return Some(sub_path);
                        }
                    }
                }
            }
        }
    }

    None
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("Failed to create {:?}: {}", dst, e))?;

    for entry in std::fs::read_dir(src).map_err(|e| format!("Failed to read {:?}: {}", src, e))? {
        let entry = entry.map_err(|e| format!("Dir entry error: {}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("Failed to copy {:?}: {}", src_path, e))?;
        }
    }

    Ok(())
}

fn sanitize_skill_id(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn read_skill_version(dir: &Path) -> Option<String> {
    let skill_md = dir.join("SKILL.md");
    if !skill_md.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&skill_md).ok()?;
    // Parse YAML frontmatter
    if !content.starts_with("---") {
        return None;
    }
    let end = content[3..].find("---")?;
    let yaml = &content[3..3 + end];
    for line in yaml.lines() {
        let line = line.trim();
        if line.starts_with("version:") {
            return Some(
                line["version:".len()..]
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            );
        }
    }
    None
}

/// Get list of installed skill IDs from the community skills directory
pub fn get_installed_skill_ids() -> Vec<String> {
    let install_root = match resolve_community_skill_dir() {
        Ok(dir) => dir,
        Err(_) => return vec![],
    };

    if !install_root.exists() {
        return vec![];
    }

    let mut ids = vec![];
    if let Ok(entries) = std::fs::read_dir(&install_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Check if it has a .cteno-source.json with sourceType "skillhub"
                let meta_path = path.join(".cteno-source.json");
                if meta_path.exists() {
                    if let Ok(content) = std::fs::read_to_string(&meta_path) {
                        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&content) {
                            if meta.get("sourceType").and_then(|v| v.as_str()) == Some("skillhub") {
                                if let Some(key) = meta.get("sourceKey").and_then(|v| v.as_str()) {
                                    ids.push(key.to_string());
                                }
                            }
                        }
                    }
                }
                // Also check by directory name (slug matches)
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if !ids.contains(&name.to_string()) {
                        ids.push(name.to_string());
                    }
                }
            }
        }
    }

    ids
}
