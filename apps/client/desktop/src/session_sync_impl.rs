use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use cteno_host_runtime::{install_session_sync_service, SessionSyncMessage, SessionSyncService};
use cteno_host_session_codec::EncryptionVariant;
use cteno_host_session_wire::{ConnectionType, UpdateEvent};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::happy_client::socket::HappySocket;
use crate::session_message_codec::SessionMessageCodec;

const SESSION_MESSAGE_RELAY_EVENT: &str = "session-message-relay";

#[derive(Clone, Default)]
pub struct HappyServerSyncService {
    sessions: Arc<RwLock<HashMap<String, RemoteSessionSyncState>>>,
}

#[derive(Clone)]
struct RemoteSessionSyncState {
    remote_session_id: String,
    socket: Arc<HappySocket>,
}

struct SyncContext {
    server_url: String,
    auth_token: String,
    encryption_key: [u8; 32],
    encryption_variant: EncryptionVariant,
    data_key_public: Option<[u8; 32]>,
    machine_id: String,
    profile_id: String,
}

fn parse_cloud_session_sync_enabled(value: Option<&str>) -> bool {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    !matches!(
        value.to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off" | "disabled"
    )
}

pub(crate) fn cloud_session_sync_enabled() -> bool {
    if let Ok(value) = std::env::var("CTENO_CLOUD_SYNC_ENABLED") {
        return parse_cloud_session_sync_enabled(Some(&value));
    }
    if let Ok(value) = std::env::var("EXPO_PUBLIC_CLOUD_SYNC_ENABLED") {
        return parse_cloud_session_sync_enabled(Some(&value));
    }
    true
}

pub fn install() {
    if !cloud_session_sync_enabled() {
        log::info!("SessionSyncService not installed: cloud session sync disabled");
        return;
    }

    let service: Arc<dyn SessionSyncService> = Arc::new(HappyServerSyncService::default());
    if install_session_sync_service(service).is_err() {
        log::debug!("SessionSyncService already installed");
    }
}

fn decode_payload(payload: &Value, message_codec: &SessionMessageCodec) -> Result<Value, String> {
    let Some(content_type) = payload.get("t").and_then(|value| value.as_str()) else {
        return Ok(payload.clone());
    };
    let content = payload
        .get("c")
        .ok_or_else(|| format!("{content_type} sync payload missing content"))?;
    message_codec
        .decode_message_content(content_type, content)
        .map_err(|error| format!("Failed to decode sync payload: {error}"))
}

fn extract_message_fields(
    payload: &Value,
) -> Option<(String, Vec<Value>, Option<String>, Option<String>)> {
    let role = payload
        .get("role")
        .and_then(|value| value.as_str())
        .unwrap_or("user");
    if role != "user" {
        return None;
    }

    let permission_mode = payload
        .get("permissionMode")
        .or_else(|| payload.pointer("/meta/permissionMode"))
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned);
    let local_id = payload
        .get("localId")
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned);

    if let Some(content) = payload.get("content") {
        let mut text = String::new();
        let mut images = Vec::new();

        if let Some(array) = content.as_array() {
            for block in array {
                match block.get("type").and_then(|value| value.as_str()) {
                    Some("text") => {
                        if let Some(block_text) = block.get("text").and_then(|value| value.as_str())
                        {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(block_text);
                        }
                    }
                    Some("image") => images.push(block.clone()),
                    _ => {}
                }
            }
        } else if let Some(content_text) = content.get("text").and_then(|value| value.as_str()) {
            text = content_text.to_string();
        }

        return Some((text, images, permission_mode, local_id));
    }

    let text = payload
        .get("text")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let images = payload
        .get("images")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    Some((text, images, permission_mode, local_id))
}

async fn load_sync_context() -> Result<SyncContext, String> {
    if !cloud_session_sync_enabled() {
        return Err("Cloud session sync disabled".to_string());
    }

    let spawn_config = crate::local_services::spawn_config()?;
    let runtime_ctx = crate::local_services::agent_runtime_context()?;
    let (auth_token, encryption_key, encryption_variant, data_key_public) =
        crate::auth_store_boot::load_persisted_machine_auth(&runtime_ctx.data_dir)?
            .ok_or_else(|| "Happy Server auth is not available for session sync".to_string())?;

    let profile_id = spawn_config.agent_config.profile_id.read().await.clone();
    Ok(SyncContext {
        server_url: crate::resolved_happy_server_url(),
        auth_token,
        encryption_key,
        encryption_variant,
        data_key_public,
        machine_id: spawn_config.machine_id.clone(),
        profile_id,
    })
}

async fn forward_to_local_session(
    session_id: &str,
    payload: &Value,
    message_codec: &SessionMessageCodec,
) -> Result<(), String> {
    let decrypted = decode_payload(payload, message_codec)?;
    let Some((text, images, permission_mode, local_id)) = extract_message_fields(&decrypted) else {
        return Ok(());
    };

    if text.is_empty() && images.is_empty() {
        return Ok(());
    }

    let spawn_config = crate::local_services::spawn_config()?;
    let Some(connection) = spawn_config.session_connections.get(session_id).await else {
        return Err(format!(
            "No local session connection found for {session_id}"
        ));
    };

    connection
        .message_handle()
        .inject_remote_message(text, images, permission_mode, local_id)
        .await
}

fn build_session_message_relay_payload(session_id: &str, message: &Value) -> Value {
    json!({
        "sessionId": session_id,
        "message": message,
    })
}

#[cfg(test)]
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
static RECORDED_RELAYS: OnceLock<Mutex<Vec<(String, Value)>>> = OnceLock::new();
#[cfg(test)]
static RELAY_TEST_GUARD: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(test)]
fn record_session_message_relay(event: &str, payload: &Value) {
    let relays = RECORDED_RELAYS.get_or_init(|| Mutex::new(Vec::new()));
    relays
        .lock()
        .expect("relay recorder poisoned")
        .push((event.to_string(), payload.clone()));
}

#[cfg(test)]
fn take_recorded_session_message_relays() -> Vec<(String, Value)> {
    let relays = RECORDED_RELAYS.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = relays.lock().expect("relay recorder poisoned");
    std::mem::take(&mut *guard)
}

async fn emit_session_message_relay(
    socket: &HappySocket,
    session_id: &str,
    message: &Value,
) -> Result<(), String> {
    let payload = build_session_message_relay_payload(session_id, message);
    #[cfg(test)]
    record_session_message_relay(SESSION_MESSAGE_RELAY_EVENT, &payload);
    socket.emit(SESSION_MESSAGE_RELAY_EVENT, payload).await
}

impl HappyServerSyncService {
    async fn connect_session_socket(
        &self,
        local_session_id: &str,
        remote_session_id: &str,
        context: &SyncContext,
    ) -> Result<Arc<HappySocket>, String> {
        let socket = Arc::new(
            HappySocket::connect(
                &context.server_url,
                &context.auth_token,
                ConnectionType::SessionScoped {
                    session_id: remote_session_id.to_string(),
                },
            )
            .await?,
        );

        let local_session_id = local_session_id.to_string();
        let remote_session_id = remote_session_id.to_string();
        let message_codec = SessionMessageCodec::for_session_messages(
            context.encryption_key,
            context.encryption_variant,
        );
        socket
            .on_update(move |update| {
                if let UpdateEvent::NewMessage(message) = update.body {
                    if message.sid != remote_session_id {
                        return;
                    }

                    let local_session_id = local_session_id.clone();
                    let encrypted_payload = json!({
                        "t": message.message.content.t,
                        "c": message.message.content.c,
                    });
                    tokio::spawn(async move {
                        if let Err(error) = forward_to_local_session(
                            &local_session_id,
                            &encrypted_payload,
                            &message_codec,
                        )
                        .await
                        {
                            log::warn!(
                                "Best-effort session sync receive failed for {}: {}",
                                local_session_id,
                                error
                            );
                        }
                    });
                }
            })
            .await;

        Ok(socket)
    }

    async fn remote_session(&self, session_id: &str) -> Option<RemoteSessionSyncState> {
        self.sessions.read().await.get(session_id).cloned()
    }
}

#[async_trait]
impl SessionSyncService for HappyServerSyncService {
    async fn on_session_created(&self, session_id: &str, workdir: &Path, vendor: &str) {
        if self.remote_session(session_id).await.is_some() {
            return;
        }

        let context = match load_sync_context().await {
            Ok(context) => context,
            Err(error) => {
                log::warn!(
                    "Best-effort session sync create skipped for {}: {}",
                    session_id,
                    error
                );
                return;
            }
        };

        let remote_session_id =
            match crate::happy_client::session_helpers::create_session_on_server(
                &context.server_url,
                &context.auth_token,
                &context.encryption_key,
                context.encryption_variant,
                context.data_key_public,
                &context.machine_id,
                &workdir.to_string_lossy(),
                vendor,
                &context.profile_id,
                None,
            )
            .await
            {
                Ok(remote_session_id) => remote_session_id,
                Err(error) => {
                    log::warn!(
                        "Best-effort session sync create failed for {}: {}",
                        session_id,
                        error
                    );
                    return;
                }
            };

        let socket = match self
            .connect_session_socket(session_id, &remote_session_id, &context)
            .await
        {
            Ok(socket) => socket,
            Err(error) => {
                log::warn!(
                    "Best-effort session sync socket setup failed for {}: {}",
                    session_id,
                    error
                );
                return;
            }
        };

        self.sessions.write().await.insert(
            session_id.to_string(),
            RemoteSessionSyncState {
                remote_session_id,
                socket,
            },
        );
    }

    async fn on_message_sent(&self, session_id: &str, message: &SessionSyncMessage) {
        let Some(remote_session) = self.remote_session(session_id).await else {
            log::warn!(
                "Best-effort session sync send skipped for {}: remote session not connected",
                session_id
            );
            return;
        };

        if let Err(error) = emit_session_message_relay(
            remote_session.socket.as_ref(),
            &remote_session.remote_session_id,
            &message.payload,
        )
        .await
        {
            log::warn!(
                "Best-effort session sync send failed for {}: {}",
                session_id,
                error
            );
        }
    }

    async fn on_message_received(&self, session_id: &str, message: &SessionSyncMessage) {
        let Some(remote_session) = self.remote_session(session_id).await else {
            log::warn!(
                "Best-effort session sync receive skipped for {}: remote session not connected",
                session_id
            );
            return;
        };

        if let Err(error) = emit_session_message_relay(
            remote_session.socket.as_ref(),
            &remote_session.remote_session_id,
            &message.payload,
        )
        .await
        {
            log::warn!(
                "Best-effort session sync receive failed for {}: {}",
                session_id,
                error
            );
        }
    }

    async fn on_session_closed(&self, session_id: &str) {
        let Some(remote_session) = self.sessions.write().await.remove(session_id) else {
            return;
        };

        if let Err(error) = remote_session
            .socket
            .emit_session_end(&remote_session.remote_session_id)
            .await
        {
            log::warn!(
                "Best-effort session sync close failed for {}: {}",
                session_id,
                error
            );
        }

        if let Err(error) = remote_session.socket.disconnect().await {
            log::warn!(
                "Best-effort session sync disconnect failed for {}: {}",
                session_id,
                error
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::happy_client::socket::LocalEventSink;
    use cteno_host_runtime::SessionSyncMessage;
    use cteno_host_session_wire::ConnectionType;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingLocalSink {
        messages: Mutex<Vec<(String, String, Option<String>)>>,
    }

    impl RecordingLocalSink {
        fn take_messages(&self) -> Vec<(String, String, Option<String>)> {
            let mut guard = self.messages.lock().expect("local sink poisoned");
            std::mem::take(&mut *guard)
        }
    }

    impl LocalEventSink for RecordingLocalSink {
        fn on_message(&self, session_id: &str, encrypted_message: &str, local_id: Option<&str>) {
            self.messages.lock().expect("local sink poisoned").push((
                session_id.to_string(),
                encrypted_message.to_string(),
                local_id.map(str::to_owned),
            ));
        }

        fn on_transient_message(&self, _session_id: &str, _encrypted_message: &str) {}

        fn on_state_update(
            &self,
            _session_id: &str,
            _encrypted_state: Option<&str>,
            _version: u32,
        ) {
        }

        fn on_metadata_update(&self, _session_id: &str, _encrypted_metadata: &str, _version: u32) {}
    }

    async fn test_service(
        session_id: &str,
        remote_session_id: &str,
    ) -> (HappyServerSyncService, Arc<RecordingLocalSink>) {
        let service = HappyServerSyncService::default();
        let local_sink = Arc::new(RecordingLocalSink::default());
        let socket = Arc::new(HappySocket::local(ConnectionType::SessionScoped {
            session_id: remote_session_id.to_string(),
        }));
        socket.install_local_sink(local_sink.clone());
        service.sessions.write().await.insert(
            session_id.to_string(),
            RemoteSessionSyncState {
                remote_session_id: remote_session_id.to_string(),
                socket,
            },
        );
        (service, local_sink)
    }

    #[test]
    fn parses_cloud_session_sync_flag() {
        assert!(parse_cloud_session_sync_enabled(None));
        assert!(parse_cloud_session_sync_enabled(Some("true")));
        assert!(parse_cloud_session_sync_enabled(Some("1")));
        assert!(!parse_cloud_session_sync_enabled(Some("false")));
        assert!(!parse_cloud_session_sync_enabled(Some("0")));
        assert!(!parse_cloud_session_sync_enabled(Some("off")));
    }

    #[tokio::test]
    async fn on_message_sent_relays_over_session_socket() {
        let _guard = RELAY_TEST_GUARD
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("relay test guard poisoned");
        let (service, local_sink) = test_service("local-session", "remote-session").await;
        let message = SessionSyncMessage {
            payload: json!({
                "role": "user",
                "content": { "type": "text", "text": "hello" },
                "localId": "local-1",
            }),
        };

        let _ = take_recorded_session_message_relays();
        service.on_message_sent("local-session", &message).await;

        assert!(local_sink.take_messages().is_empty());
        assert_eq!(
            take_recorded_session_message_relays(),
            vec![(
                SESSION_MESSAGE_RELAY_EVENT.to_string(),
                json!({
                    "sessionId": "remote-session",
                    "message": message.payload,
                }),
            )]
        );
    }

    #[tokio::test]
    async fn on_message_received_relays_over_session_socket() {
        let _guard = RELAY_TEST_GUARD
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("relay test guard poisoned");
        let (service, local_sink) = test_service("local-session", "remote-session").await;
        let message = SessionSyncMessage {
            payload: json!({
                "role": "user",
                "text": "hello from mobile",
            }),
        };

        let _ = take_recorded_session_message_relays();
        service.on_message_received("local-session", &message).await;

        assert!(local_sink.take_messages().is_empty());
        assert_eq!(
            take_recorded_session_message_relays(),
            vec![(
                SESSION_MESSAGE_RELAY_EVENT.to_string(),
                json!({
                    "sessionId": "remote-session",
                    "message": message.payload,
                }),
            )]
        );
    }
}
