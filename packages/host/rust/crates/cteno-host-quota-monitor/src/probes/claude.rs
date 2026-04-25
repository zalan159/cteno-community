//! Claude probe: OAuth token from macOS Keychain (or `~/.claude/.credentials.json`
//! fallback), direct `POST api.anthropic.com/v1/messages` with `max_tokens: 1`.
//!
//! The response headers carry `anthropic-ratelimit-unified-{5h,7d}-utilization`
//! / `-reset` / `-status` unconditionally (even for `allowed` state, unlike the
//! stream-json `rate_limit_event` which only fires near thresholds). That's why
//! this route is preferred over stream-json.
//!
//! Cost: one input + one output token per poll. At a 60s cadence that's ~60
//! haiku pings/hour, which is trivially cheap compared to actual quota.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::probe::VendorQuotaProbe;
use crate::store::{QuotaWindow, VendorId, VendorQuota};

const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

pub struct ClaudeProbe {
    client: reqwest::Client,
}

impl ClaudeProbe {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("reqwest client build");
        Self { client }
    }
}

impl Default for ClaudeProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VendorQuotaProbe for ClaudeProbe {
    fn vendor(&self) -> VendorId {
        VendorId::Claude
    }

    async fn poll(&self) -> Result<VendorQuota, String> {
        let creds = match load_credentials() {
            Ok(c) => c,
            Err(e) => {
                log::debug!("[claude-probe] credentials unavailable: {}", e);
                return Ok(VendorQuota::error(VendorId::Claude, e));
            }
        };

        let response = self
            .client
            .post(ANTHROPIC_MESSAGES_URL)
            .bearer_auth(&creds.access_token)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("content-type", "application/json")
            .json(&json!({
                "model": "claude-haiku-4-5",
                "max_tokens": 1,
                "messages": [ { "role": "user", "content": "." } ]
            }))
            .send()
            .await
            .map_err(|e| format!("claude probe request failed: {}", e))?;

        let status = response.status();
        // Capture headers before consuming body so we don't lose them on 4xx.
        let headers = response.headers().clone();

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let msg = if status.as_u16() == 401 || status.as_u16() == 403 {
                format!(
                    "claude auth rejected ({}): {}",
                    status,
                    truncate(&body, 200)
                )
            } else {
                format!("claude HTTP {}: {}", status, truncate(&body, 200))
            };
            return Ok(VendorQuota::error(VendorId::Claude, msg));
        }

        // Drain the body to free the connection; we don't actually need its
        // content (the rate-limit signal is all in headers).
        let _ = response.bytes().await;

        let mut quota = VendorQuota::new_windows(VendorId::Claude);
        if let Some(subtype) = creds.subscription_type {
            quota.plan_type = Some(subtype);
        }

        let mut seen_any = false;
        for (claim, key) in [("5h", "fiveHour"), ("7d", "weekly")] {
            if let Some(win) = extract_window(&headers, claim) {
                quota.windows.insert(key.to_string(), win);
                seen_any = true;
            }
        }
        // Opus/Sonnet-specific weekly buckets — only present on certain plans.
        // Claude CLI uses shorthand claim names via headers too (no known
        // per-model shorthand as of 2026-04), so we skip these until we see
        // them in the wild rather than invent header names.

        if !seen_any {
            quota.error = Some(
                "anthropic-ratelimit headers missing; probably an API-key account".to_string(),
            );
        }

        Ok(quota)
    }
}

struct ClaudeCredentials {
    access_token: String,
    subscription_type: Option<String>,
}

fn load_credentials() -> Result<ClaudeCredentials, String> {
    #[cfg(target_os = "macos")]
    if let Ok(c) = load_from_macos_keychain() {
        return Ok(c);
    }
    load_from_credentials_file()
}

#[cfg(target_os = "macos")]
fn load_from_macos_keychain() -> Result<ClaudeCredentials, String> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .map_err(|e| format!("failed to run `security`: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "keychain entry {} missing (status={})",
            KEYCHAIN_SERVICE, output.status
        ));
    }
    let raw = String::from_utf8(output.stdout)
        .map_err(|e| format!("keychain payload not UTF-8: {}", e))?;
    parse_credentials_json(raw.trim())
}

fn load_from_credentials_file() -> Result<ClaudeCredentials, String> {
    let home = dirs::home_dir().ok_or_else(|| "cannot resolve home dir".to_string())?;
    let path = home.join(".claude").join(".credentials.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("no credentials at {}: {}", path.display(), e))?;
    parse_credentials_json(&raw)
}

fn parse_credentials_json(raw: &str) -> Result<ClaudeCredentials, String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|e| format!("claude credentials json invalid: {}", e))?;
    let oauth = value
        .get("claudeAiOauth")
        .ok_or_else(|| "claude credentials missing `claudeAiOauth` object".to_string())?;
    let access_token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .ok_or_else(|| "claude credentials missing `accessToken`".to_string())?
        .to_string();
    let subscription_type = oauth
        .get("subscriptionType")
        .and_then(Value::as_str)
        .map(String::from);
    Ok(ClaudeCredentials {
        access_token,
        subscription_type,
    })
}

fn extract_window(headers: &reqwest::header::HeaderMap, claim: &str) -> Option<QuotaWindow> {
    let prefix = format!("anthropic-ratelimit-unified-{}", claim);
    let utilization: f64 = headers
        .get(format!("{}-utilization", prefix))?
        .to_str()
        .ok()?
        .parse::<f64>()
        .ok()?;
    let used_percent = (utilization * 100.0).clamp(0.0, 100.0);
    let resets_at = headers
        .get(format!("{}-reset", prefix))
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok());
    let status = headers
        .get(format!("{}-status", prefix))
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let window_duration_mins = match claim {
        "5h" => Some(300),
        "7d" => Some(10_080),
        _ => None,
    };
    Some(QuotaWindow {
        used_percent,
        resets_at,
        window_duration_mins,
        status,
        limit_type: Some(claim.to_string()),
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut out = s[..max].to_string();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    #[test]
    fn extract_5h_from_headers() {
        let mut h = HeaderMap::new();
        h.insert(
            "anthropic-ratelimit-unified-5h-utilization",
            HeaderValue::from_static("0.01"),
        );
        h.insert(
            "anthropic-ratelimit-unified-5h-reset",
            HeaderValue::from_static("1776718800"),
        );
        h.insert(
            "anthropic-ratelimit-unified-5h-status",
            HeaderValue::from_static("allowed"),
        );
        let w = extract_window(&h, "5h").expect("window");
        assert_eq!(w.used_percent as i64, 1);
        assert_eq!(w.resets_at, Some(1_776_718_800));
        assert_eq!(w.status.as_deref(), Some("allowed"));
        assert_eq!(w.window_duration_mins, Some(300));
    }

    #[test]
    fn missing_headers_return_none() {
        let h = HeaderMap::new();
        assert!(extract_window(&h, "5h").is_none());
    }

    #[test]
    fn parse_credentials_from_keychain_blob() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-x","refreshToken":"r","expiresAt":1,"scopes":[],"subscriptionType":"pro","rateLimitTier":"t"}}"#;
        let c = parse_credentials_json(raw).unwrap();
        assert_eq!(c.access_token, "sk-ant-oat01-x");
        assert_eq!(c.subscription_type.as_deref(), Some("pro"));
    }
}
