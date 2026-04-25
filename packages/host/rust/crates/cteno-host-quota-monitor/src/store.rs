//! Vendor-neutral quota snapshot types.
//!
//! The store holds one `VendorQuota` per known vendor. Frontend reads the
//! whole map in one RPC call and picks the entry for the session's current
//! vendor.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VendorId {
    Claude,
    Codex,
    Gemini,
}

impl VendorId {
    pub fn as_str(&self) -> &'static str {
        match self {
            VendorId::Claude => "claude",
            VendorId::Codex => "codex",
            VendorId::Gemini => "gemini",
        }
    }
}

/// Which data shape this vendor reports. Different vendors model quota
/// differently — time windows vs per-model buckets — so the frontend picks
/// the matching field based on this tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VendorQuotaShape {
    /// Claude / Codex — rolling time windows.
    Windows,
    /// Gemini — per-model request-count buckets.
    Buckets,
}

/// One time window (Claude: five_hour/seven_day/...; Codex: primary/secondary).
///
/// `used_percent` is 0–100 "已用"。前端负责翻成剩余 = 100 - used_percent。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub used_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_duration_mins: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_type: Option<String>,
}

/// One per-model request/token bucket (Gemini). Like `QuotaWindow` but
/// stored under a model id rather than a named time window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaBucket {
    pub model_id: String,
    /// `REQUESTS` or `TOKENS`.
    pub token_type: String,
    /// 0–100，统一成"已用"，跟 QuotaWindow.used_percent 对齐。
    pub used_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_amount: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaCredits {
    pub has_credits: bool,
    pub unlimited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<String>,
}

/// One vendor's entire reported state at a point in time. `error` is set
/// when the last poll failed; prior windows/buckets are cleared on error
/// so the UI shows the user a clean "unavailable" state rather than stale
/// values that no longer represent reality.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VendorQuota {
    pub provider: VendorId,
    pub shape: VendorQuotaShape,
    /// Keyed by window name: `fiveHour`, `weekly`, `weeklyOpus`,
    /// `weeklySonnet`, `overage`, etc. Only populated when
    /// `shape == Windows`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub windows: HashMap<String, QuotaWindow>,
    /// Only populated when `shape == Buckets`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buckets: Vec<QuotaBucket>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<QuotaCredits>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_model: Option<String>,
    /// Unix seconds when this snapshot was produced by the daemon.
    pub updated_at: i64,
    /// Human-readable error when last poll failed (auth missing, 401,
    /// unsupported plan type, etc.). `None` on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl VendorQuota {
    pub fn new_windows(provider: VendorId) -> Self {
        Self {
            provider,
            shape: VendorQuotaShape::Windows,
            windows: HashMap::new(),
            buckets: Vec::new(),
            credits: None,
            plan_type: None,
            primary_model: None,
            updated_at: chrono::Utc::now().timestamp(),
            error: None,
        }
    }

    pub fn new_buckets(provider: VendorId) -> Self {
        Self {
            provider,
            shape: VendorQuotaShape::Buckets,
            windows: HashMap::new(),
            buckets: Vec::new(),
            credits: None,
            plan_type: None,
            primary_model: None,
            updated_at: chrono::Utc::now().timestamp(),
            error: None,
        }
    }

    pub fn error(provider: VendorId, message: impl Into<String>) -> Self {
        // Preserve the canonical shape so the frontend keeps rendering the
        // right skeleton; data lists stay empty.
        let shape = match provider {
            VendorId::Claude | VendorId::Codex => VendorQuotaShape::Windows,
            VendorId::Gemini => VendorQuotaShape::Buckets,
        };
        Self {
            provider,
            shape,
            windows: HashMap::new(),
            buckets: Vec::new(),
            credits: None,
            plan_type: None,
            primary_model: None,
            updated_at: chrono::Utc::now().timestamp(),
            error: Some(message.into()),
        }
    }
}

/// Simple owner map. Keyed by vendor so at most one entry per vendor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VendorQuotaMap {
    #[serde(flatten)]
    pub entries: HashMap<String, VendorQuota>,
}

impl VendorQuotaMap {
    pub fn insert(&mut self, vendor: VendorId, quota: VendorQuota) {
        self.entries.insert(vendor.as_str().to_string(), quota);
    }

    pub fn get(&self, vendor: VendorId) -> Option<&VendorQuota> {
        self.entries.get(vendor.as_str())
    }
}
