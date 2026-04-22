//! Data models for the notification watcher system.

use serde::{Deserialize, Serialize};

/// A subscription linking a persona to a macOS app's notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSubscription {
    pub id: String,
    pub persona_id: String,
    pub app_identifier: String,
    pub app_display_name: String,
    pub enabled: bool,
    pub created_at: i64,
}

/// A notification record read from the macOS notification center database.
/// Transient — not persisted in our DB.
#[derive(Debug, Clone)]
pub struct NotificationRecord {
    pub rec_id: i64,
    pub app_identifier: String,
    pub title: String,
    pub body: String,
    pub iden: String,
    pub category: String,
    pub delivered_date: f64,
}

/// An app entry from the macOS notification center database.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationApp {
    pub app_id: i64,
    pub identifier: String,
    pub display_name: String,
}
