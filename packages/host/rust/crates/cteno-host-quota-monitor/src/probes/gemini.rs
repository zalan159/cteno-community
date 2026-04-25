//! Gemini probe: Google OAuth credentials from `~/.gemini/oauth_creds.json`.
//!
//! The ACP channel doesn't expose quota, so we call Google's private
//! Code-Assist API directly. Flow per poll:
//!
//!   1. Load + refresh `access_token` if expired (writes back to creds file).
//!   2. Call `loadCodeAssist` to resolve `cloudaicompanionProject` on first
//!      boot; cache in-memory afterward.
//!   3. Call `retrieveUserQuota` with the project id. Translate `buckets[]`
//!      into our per-model `QuotaBucket` shape (`usedPercent = (1 -
//!      remainingFraction) * 100`).
//!
//! Only Google-OAuth-personal accounts have quota data here. Vertex-AI and
//! API-key users will fail `loadCodeAssist` — we report that as a user-
//! facing `error` rather than retrying forever.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::probe::VendorQuotaProbe;
use crate::store::{QuotaBucket, VendorId, VendorQuota};

/// Gemini CLI's published OAuth client id/secret (public desktop app credentials).
/// Hardcoded in gemini-cli as of 0.38.2; we reuse them so tokens refresh
/// against the same client that minted them.
const GEMINI_OAUTH_CLIENT_ID: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
const GEMINI_OAUTH_CLIENT_SECRET: &str = "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";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const CODE_ASSIST_BASE: &str = "https://cloudcode-pa.googleapis.com/v1internal";

pub struct GeminiProbe {
    client: reqwest::Client,
    /// Cached `cloudaicompanionProject` — populated on first successful
    /// `loadCodeAssist`.
    project_id: Arc<Mutex<Option<String>>>,
}

impl GeminiProbe {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("reqwest client build");
        Self {
            client,
            project_id: Arc::new(Mutex::new(None)),
        }
    }

    async fn ensure_project_id(&self, access_token: &str) -> Result<String, String> {
        {
            let guard = self.project_id.lock().await;
            if let Some(p) = guard.clone() {
                return Ok(p);
            }
        }
        let resp = self
            .client
            .post(format!("{}:loadCodeAssist", CODE_ASSIST_BASE))
            .bearer_auth(access_token)
            .json(&json!({
                "metadata": {
                    "ideType": "IDE_UNSPECIFIED",
                    "platform": "PLATFORM_UNSPECIFIED",
                    "pluginType": "GEMINI",
                }
            }))
            .send()
            .await
            .map_err(|e| format!("loadCodeAssist request failed: {}", e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("loadCodeAssist HTTP {}: {}", status, body));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("loadCodeAssist JSON parse failed: {}", e))?;
        let project = body
            .get("cloudaicompanionProject")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                "loadCodeAssist response missing cloudaicompanionProject (account not provisioned)"
                    .to_string()
            })?
            .to_string();
        let mut guard = self.project_id.lock().await;
        *guard = Some(project.clone());
        Ok(project)
    }
}

impl Default for GeminiProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VendorQuotaProbe for GeminiProbe {
    fn vendor(&self) -> VendorId {
        VendorId::Gemini
    }

    async fn poll(&self) -> Result<VendorQuota, String> {
        let creds_path = gemini_creds_path()?;
        let creds = match load_oauth_creds(&creds_path) {
            Ok(c) => c,
            Err(e) => return Ok(VendorQuota::error(VendorId::Gemini, e)),
        };

        let access_token = match ensure_fresh_token(&self.client, &creds_path, creds).await {
            Ok(t) => t,
            Err(e) => return Ok(VendorQuota::error(VendorId::Gemini, e)),
        };

        let project_id = match self.ensure_project_id(&access_token).await {
            Ok(p) => p,
            Err(e) => return Ok(VendorQuota::error(VendorId::Gemini, e)),
        };

        let resp = self
            .client
            .post(format!("{}:retrieveUserQuota", CODE_ASSIST_BASE))
            .bearer_auth(&access_token)
            .json(&json!({ "project": project_id }))
            .send()
            .await
            .map_err(|e| format!("retrieveUserQuota request failed: {}", e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Ok(VendorQuota::error(
                VendorId::Gemini,
                format!("retrieveUserQuota HTTP {}: {}", status, body),
            ));
        }
        let payload: Value = resp
            .json()
            .await
            .map_err(|e| format!("retrieveUserQuota JSON parse failed: {}", e))?;
        Ok(translate_quota(&payload))
    }
}

fn gemini_creds_path() -> Result<std::path::PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "cannot resolve home dir".to_string())?;
    Ok(home.join(".gemini").join("oauth_creds.json"))
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct OAuthCreds {
    access_token: String,
    refresh_token: String,
    /// Google stores this as epoch ms; legacy / some refreshed versions use
    /// seconds. We accept either.
    #[serde(default)]
    expiry_date: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
}

fn load_oauth_creds(path: &std::path::Path) -> Result<OAuthCreds, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("no gemini creds at {}: {}", path.display(), e))?;
    serde_json::from_str::<OAuthCreds>(&raw)
        .map_err(|e| format!("invalid gemini creds at {}: {}", path.display(), e))
}

async fn ensure_fresh_token(
    client: &reqwest::Client,
    path: &std::path::Path,
    creds: OAuthCreds,
) -> Result<String, String> {
    if !is_expired(creds.expiry_date) {
        return Ok(creds.access_token);
    }

    let resp = client
        .post(GOOGLE_TOKEN_URL)
        .form(&[
            ("client_id", GEMINI_OAUTH_CLIENT_ID),
            ("client_secret", GEMINI_OAUTH_CLIENT_SECRET),
            ("refresh_token", &creds.refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| format!("token refresh request failed: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("token refresh HTTP {}: {}", status, body));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("token refresh JSON parse failed: {}", e))?;
    let new_access = body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "refresh response missing access_token".to_string())?
        .to_string();
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_i64)
        .unwrap_or(3600);
    let new_expiry_ms = (chrono::Utc::now().timestamp() + expires_in) * 1000;

    // Rewrite creds file atomically so concurrent gemini-cli runs see the
    // fresh token too. Preserve unknown/untouched keys (id_token, scope,
    // etc.) by round-tripping through the original JSON value.
    let mut parsed: Value = serde_json::from_str(
        &std::fs::read_to_string(path).map_err(|e| format!("re-read creds file failed: {}", e))?,
    )
    .map_err(|e| format!("re-parse creds file failed: {}", e))?;
    if let Value::Object(ref mut map) = parsed {
        map.insert(
            "access_token".to_string(),
            Value::String(new_access.clone()),
        );
        map.insert("expiry_date".to_string(), Value::from(new_expiry_ms));
        if let Some(t) = body.get("id_token").and_then(Value::as_str) {
            map.insert("id_token".to_string(), Value::String(t.to_string()));
        }
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(&parsed).unwrap_or_default(),
    )
    .map_err(|e| format!("write refreshed creds tmp failed: {}", e))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename refreshed creds failed: {}", e))?;

    Ok(new_access)
}

fn is_expired(expiry_date: Option<i64>) -> bool {
    let Some(expiry) = expiry_date else {
        return true;
    };
    // Heuristic: values < 1e12 look like seconds, everything else is ms.
    let expiry_ms = if expiry < 1_000_000_000_000 {
        expiry * 1000
    } else {
        expiry
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    // 60s safety buffer so we refresh before an in-flight request 401s.
    now_ms + 60_000 >= expiry_ms
}

fn translate_quota(response: &Value) -> VendorQuota {
    let mut out = VendorQuota::new_buckets(VendorId::Gemini);
    let buckets = response
        .get("buckets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for bucket in buckets {
        let model_id = bucket
            .get("modelId")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let token_type = bucket
            .get("tokenType")
            .and_then(Value::as_str)
            .unwrap_or("REQUESTS")
            .to_string();
        let remaining_fraction = bucket
            .get("remainingFraction")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        let used_percent = ((1.0 - remaining_fraction) * 100.0).clamp(0.0, 100.0);
        let resets_at = bucket
            .get("resetTime")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc).timestamp());
        let remaining_amount = bucket
            .get("remainingAmount")
            .and_then(Value::as_str)
            .map(String::from);
        out.buckets.push(QuotaBucket {
            model_id,
            token_type,
            used_percent,
            resets_at,
            remaining_amount,
        });
    }

    if out.buckets.is_empty() {
        out.error = Some("gemini retrieveUserQuota returned no buckets".to_string());
    } else {
        // Sort buckets for stable ordering (pro models first, then flash).
        out.buckets.sort_by(|a, b| a.model_id.cmp(&b.model_id));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_typical_response() {
        let response = json!({
            "buckets": [
                { "modelId": "gemini-2.5-pro",          "tokenType": "REQUESTS", "remainingFraction": 1.0,     "resetTime": "2026-04-21T16:13:19Z" },
                { "modelId": "gemini-3-flash-preview",  "tokenType": "REQUESTS", "remainingFraction": 0.98,    "resetTime": "2026-04-20T19:33:20Z" },
                { "modelId": "gemini-2.5-flash-lite",   "tokenType": "REQUESTS", "remainingFraction": 0.97625, "resetTime": "2026-04-20T19:33:18Z" }
            ]
        });
        let u = translate_quota(&response);
        assert!(u.error.is_none());
        assert_eq!(u.buckets.len(), 3);
        // Sorted by model id.
        assert_eq!(u.buckets[0].model_id, "gemini-2.5-flash-lite");
        assert!((u.buckets[0].used_percent - 2.375).abs() < 1e-6);
        assert!(u.buckets[0].resets_at.is_some());
    }

    #[test]
    fn translate_empty_buckets_reports_error() {
        let u = translate_quota(&json!({ "buckets": [] }));
        assert!(u.error.is_some());
    }

    #[test]
    fn is_expired_handles_seconds_and_millis() {
        // Far past in both units.
        assert!(is_expired(Some(1_000_000_000)));
        assert!(is_expired(Some(1_000_000_000_000)));
        // Far future.
        let future_s = chrono::Utc::now().timestamp() + 3600;
        let future_ms = future_s * 1000;
        assert!(!is_expired(Some(future_s)));
        assert!(!is_expired(Some(future_ms)));
        assert!(is_expired(None));
    }
}
