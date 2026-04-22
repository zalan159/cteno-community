//! Shared push notification utility.
//!
//! Sends push notifications to user's mobile devices via Expo Push API.
//! Extracted from `happy_client::permission` for reuse across modules.

use serde_json::{json, Value};

pub fn compact_notification_body(body: &str, max_chars: usize) -> Option<String> {
    let normalized = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }

    let mut compact = normalized.chars().take(max_chars).collect::<String>();
    if normalized.chars().count() > max_chars {
        compact.push('…');
    }

    Some(compact)
}

/// Send a local desktop notification (macOS/Windows).
/// Fire-and-forget, logs errors.
pub fn send_local_notification(title: &str, body: &str) {
    let title = title.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        if let Some(provider) = crate::hooks::local_notification() {
            match provider.send_local_notification(&title, &body) {
                Ok(()) => return,
                Err(err) => {
                    log::warn!(
                        "[PushNotification] Host local notification failed, falling back: {}",
                        err
                    );
                }
            }
        }

        send_local_notification_fallback(&title, &body);
    });
}

fn send_local_notification_fallback(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            body.replace('\\', "\\\\").replace('"', "\\\""),
            title.replace('\\', "\\\\").replace('"', "\\\""),
        );
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output();
    }
    #[cfg(target_os = "windows")]
    {
        // PowerShell toast notification
        let script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
             $xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
             $text = $xml.GetElementsByTagName('text'); \
             $text[0].AppendChild($xml.CreateTextNode('{}')) > $null; \
             $text[1].AppendChild($xml.CreateTextNode('{}')) > $null; \
             $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Cteno').Show($toast)",
            title.replace('\'', "''"),
            body.replace('\'', "''"),
        );
        let _ = std::process::Command::new("powershell")
            .arg("-Command")
            .arg(&script)
            .output();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (title, body);
    }
}

pub fn send_agent_reply_notification(title: &str, body: &str) {
    if let Some(compact_body) = compact_notification_body(body, 220) {
        send_local_notification(title, &compact_body);
    }
}

/// Send a push notification to all registered devices (fire-and-forget).
///
/// Fetches push tokens from the server, then sends via Expo Push API.
/// Also sends a local desktop notification.
/// Logs errors but never propagates them.
pub async fn send_push(server_url: &str, auth_token: &str, title: &str, body: &str, data: Value) {
    // Local desktop notification (macOS/Windows)
    send_local_notification(title, body);

    let client = reqwest::Client::new();

    // Step 1: Get push tokens
    let tokens_url = format!("{}/v1/push-tokens", server_url);
    let tokens_response = match client
        .get(&tokens_url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            log::warn!("[PushNotification] Failed to fetch push tokens: {}", e);
            return;
        }
    };

    let tokens_json: Value = match tokens_response.json().await {
        Ok(j) => j,
        Err(e) => {
            log::warn!(
                "[PushNotification] Failed to parse push tokens response: {}",
                e
            );
            return;
        }
    };

    let tokens = match tokens_json.get("tokens").and_then(|t| t.as_array()) {
        Some(t) => t.clone(),
        None => {
            log::debug!("[PushNotification] No push tokens found");
            return;
        }
    };

    if tokens.is_empty() {
        log::debug!("[PushNotification] No push tokens to notify");
        return;
    }

    // Step 2: Send push notifications via Expo Push API
    let expo_url = "https://exp.host/--/api/v2/push/send";

    let messages: Vec<Value> = tokens
        .iter()
        .filter_map(|t| t.get("token").and_then(|v| v.as_str()))
        .map(|token| {
            json!({
                "to": token,
                "title": title,
                "body": body,
                "data": data,
                "sound": "default",
                "priority": "high",
            })
        })
        .collect();

    if messages.is_empty() {
        return;
    }

    match client.post(expo_url).json(&messages).send().await {
        Ok(resp) => {
            log::info!(
                "[PushNotification] Push sent ({} devices), status: {}",
                messages.len(),
                resp.status()
            );
        }
        Err(e) => {
            log::warn!("[PushNotification] Failed to send push notification: {}", e);
        }
    }
}
