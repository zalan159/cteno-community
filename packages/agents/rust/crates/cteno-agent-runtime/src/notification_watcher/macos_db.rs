//! Read-only access to the macOS notification center SQLite database.
//!
//! The database lives at:
//!   ~/Library/Group Containers/group.com.apple.usernoted/db2/db
//!
//! Records are stored in the `record` table with binary plist `data` blobs.
//! Notifications disappear from the database once the user clicks or dismisses them,
//! so we poll frequently and track watermarks.

use plist::Value as PlistValue;
use rusqlite::{params, Connection, OpenFlags};
use std::collections::HashMap;
use std::path::PathBuf;

use super::models::{NotificationApp, NotificationRecord};

/// Known app identifiers → human-readable display names.
fn known_display_names() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    m.insert("com.tencent.weworkmac", "企业微信");
    m.insert("com.tencent.xinWeChat", "微信");
    m.insert("com.apple.MobileSMS", "短信");
    m.insert("com.apple.mail", "邮件");
    m.insert("com.tencent.qq", "QQ");
    m.insert("com.apple.iCal", "日历");
    m.insert("com.apple.reminders", "提醒事项");
    m.insert("com.apple.FaceTime", "FaceTime");
    m
}

/// Path to the macOS notification center database.
pub fn db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/unknown".to_string());
    PathBuf::from(home)
        .join("Library")
        .join("Group Containers")
        .join("group.com.apple.usernoted")
        .join("db2")
        .join("db")
}

/// Open the notification database in read-only mode.
fn open_readonly() -> Result<Connection, String> {
    let path = db_path();
    if !path.exists() {
        return Err(format!(
            "macOS notification DB not found at {}",
            path.display()
        ));
    }
    Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("Failed to open notification DB: {}", e))
}

/// List all apps that have entries in the notification database.
/// Filters out system-internal `_system_center_:` prefixed entries.
pub fn list_apps() -> Result<Vec<NotificationApp>, String> {
    let conn = open_readonly()?;
    let known = known_display_names();

    let mut stmt = conn
        .prepare("SELECT app_id, identifier FROM app ORDER BY identifier")
        .map_err(|e| format!("Failed to query apps: {}", e))?;

    let rows = stmt
        .query_map([], |row| {
            let app_id: i64 = row.get(0)?;
            let identifier: String = row.get(1)?;
            Ok((app_id, identifier))
        })
        .map_err(|e| format!("Failed to iterate apps: {}", e))?;

    let mut apps = Vec::new();
    for row in rows {
        let (app_id, identifier) = row.map_err(|e| e.to_string())?;
        // Skip system center entries
        if identifier.starts_with("_system_center_:") {
            continue;
        }
        let display_name = known
            .get(identifier.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                // Use last component of bundle ID as fallback name
                identifier
                    .split('.')
                    .last()
                    .unwrap_or(&identifier)
                    .to_string()
            });
        apps.push(NotificationApp {
            app_id,
            identifier,
            display_name,
        });
    }
    Ok(apps)
}

/// Get the current maximum rec_id for a given app identifier.
/// Used to initialize the watermark so we don't replay old notifications.
pub fn get_max_rec_id(app_identifier: &str) -> Result<i64, String> {
    let conn = open_readonly()?;
    let result: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(r.rec_id), 0)
             FROM record r JOIN app a ON r.app_id = a.app_id
             WHERE a.identifier = ?1",
            params![app_identifier],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to get max rec_id: {}", e))?;
    Ok(result)
}

/// Fetch notification records newer than `since_rec_id` for the given app.
pub fn fetch_new_records(
    app_identifier: &str,
    since_rec_id: i64,
) -> Result<Vec<NotificationRecord>, String> {
    let conn = open_readonly()?;
    let mut stmt = conn
        .prepare(
            "SELECT r.rec_id, a.identifier, r.data, r.delivered_date
             FROM record r JOIN app a ON r.app_id = a.app_id
             WHERE a.identifier = ?1 AND r.rec_id > ?2
             ORDER BY r.rec_id ASC",
        )
        .map_err(|e| format!("Failed to prepare query: {}", e))?;

    let rows = stmt
        .query_map(params![app_identifier, since_rec_id], |row| {
            let rec_id: i64 = row.get(0)?;
            let identifier: String = row.get(1)?;
            let data: Vec<u8> = row.get(2)?;
            let delivered_date: f64 = row.get(3)?;
            Ok((rec_id, identifier, data, delivered_date))
        })
        .map_err(|e| format!("Failed to query records: {}", e))?;

    let mut records = Vec::new();
    for row in rows {
        let (rec_id, identifier, data, delivered_date) = row.map_err(|e| e.to_string())?;
        match decode_notification_plist(&data) {
            Ok((title, body, iden, category)) => {
                records.push(NotificationRecord {
                    rec_id,
                    app_identifier: identifier,
                    title,
                    body,
                    iden,
                    category,
                    delivered_date,
                });
            }
            Err(e) => {
                log::debug!(
                    "[NotifWatcher] Failed to decode plist for rec_id {}: {}",
                    rec_id,
                    e
                );
            }
        }
    }
    Ok(records)
}

/// Decode a binary plist notification blob.
/// Extracts `req.titl`, `req.body`, `req.iden`, `req.cate` fields.
fn decode_notification_plist(data: &[u8]) -> Result<(String, String, String, String), String> {
    let plist = PlistValue::from_reader(std::io::Cursor::new(data))
        .map_err(|e| format!("plist parse error: {}", e))?;

    let dict = plist
        .as_dictionary()
        .ok_or_else(|| "plist root is not a dictionary".to_string())?;

    let req = dict
        .get("req")
        .and_then(|v| v.as_dictionary())
        .ok_or_else(|| "missing req dictionary".to_string())?;

    let title = req
        .get("titl")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let body = req
        .get("body")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let iden = req
        .get("iden")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let category = req
        .get("cate")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();

    Ok((title, body, iden, category))
}
