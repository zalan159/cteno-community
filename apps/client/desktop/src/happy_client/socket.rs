//! Feature-gated session transport façade.

#[cfg(feature = "commercial-cloud")]
pub use cteno_happy_client_transport::{
    auth_hook, current_llm_key, install_auth_refresh_hook, install_llm_key_refresh_hook,
    install_machine_runtime, llm_key_refresh_hook, reconnect_machine_runtime, server_is_reachable,
    spawn_llm_key_refresh_guard, AuthRefreshHook, ConnectionWatchdog, HappySocket,
    HeartbeatManager, LlmKeyGuardCancel, LlmKeyRefreshHook, LocalEventSink, LocalEventSinkArc,
    MachineReconnectState, MachineRuntimeState, SessionRecoveryHooks, SocketRpcHandler,
    WatchdogState,
};

#[cfg(not(feature = "commercial-cloud"))]
mod community {
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, OnceLock};
    use tokio::sync::Mutex;

    use cteno_host_session_codec::EncryptionVariant;
    use cteno_host_session_wire::{ConnectionType, EphemeralEvent, UpdatePayload};

    use crate::happy_client::RpcRegistry;

    pub mod auth_hook {
        use std::sync::{Arc, OnceLock};

        pub trait AuthRefreshHook: Send + Sync + 'static {
            fn refresh_now(
                &self,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>>;
            fn notify_require_login(&self);
            fn current_access_token(&self) -> Option<String>;
        }

        static HOOK: OnceLock<Arc<dyn AuthRefreshHook>> = OnceLock::new();

        pub fn install_auth_refresh_hook(hook: Arc<dyn AuthRefreshHook>) {
            let _ = HOOK.set(hook);
        }
    }

    pub use auth_hook::AuthRefreshHook;

    pub type LocalEventSinkArc = Arc<dyn LocalEventSink>;

    pub trait LocalEventSink: Send + Sync + 'static {
        fn on_message(&self, session_id: &str, encrypted_message: &str, local_id: Option<&str>);
        fn on_transient_message(&self, session_id: &str, encrypted_message: &str);
        fn on_state_update(&self, session_id: &str, encrypted_state: Option<&str>, version: u32);
        fn on_metadata_update(&self, session_id: &str, encrypted_metadata: &str, version: u32);
        fn on_session_alive(
            &self,
            _session_id: &str,
            _thinking: Option<bool>,
            _thinking_status: Option<&str>,
            _context_tokens: u32,
            _compression_threshold: u32,
        ) {
        }
    }

    pub struct HappySocket {
        connection_type: ConnectionType,
        local_sink: OnceLock<LocalEventSinkArc>,
    }

    pub type SocketRpcHandler = Arc<
        dyn Fn(
                String,
                String,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
            + Send
            + Sync,
    >;

    impl HappySocket {
        fn unavailable() -> String {
            "Happy Server remote transport is unavailable in community builds".to_string()
        }

        pub async fn connect(
            _server_url: &str,
            _token: impl Into<String>,
            connection_type: ConnectionType,
        ) -> Result<Self, String> {
            Ok(Self::local(connection_type))
        }

        pub fn local(connection_type: ConnectionType) -> Self {
            Self {
                connection_type,
                local_sink: OnceLock::new(),
            }
        }

        pub fn install_local_sink(&self, sink: LocalEventSinkArc) {
            let _ = self.local_sink.set(sink);
        }

        fn local_sink(&self) -> Option<&LocalEventSinkArc> {
            self.local_sink.get()
        }

        pub async fn on_update<F>(&self, _callback: F)
        where
            F: Fn(UpdatePayload) + Send + Sync + 'static,
        {
        }

        pub async fn on_ephemeral<F>(&self, _callback: F)
        where
            F: Fn(EphemeralEvent) + Send + Sync + 'static,
        {
        }

        pub async fn on_connect<F>(&self, _callback: F)
        where
            F: Fn() + Send + Sync + 'static,
        {
        }

        pub async fn on_disconnect<F>(&self, _callback: F)
        where
            F: Fn() + Send + Sync + 'static,
        {
        }

        pub async fn on_rpc_request<F, Fut>(&self, _callback: F)
        where
            F: Fn(String, String) -> Fut + Send + Sync + 'static,
            Fut: std::future::Future<Output = String> + Send + 'static,
        {
        }

        pub async fn on_token_near_expiry<F, Fut>(&self, _callback: F)
        where
            F: Fn(i64) -> Fut + Send + Sync + 'static,
            Fut: std::future::Future<Output = ()> + Send + 'static,
        {
        }

        pub async fn on_relay_session_messages_request<F, Fut>(&self, _callback: F)
        where
            F: Fn(serde_json::Value) -> Fut + Send + Sync + 'static,
            Fut: std::future::Future<Output = ()> + Send + 'static,
        {
        }

        pub async fn wait_for_connect(&self) -> Result<(), String> {
            Err(Self::unavailable())
        }

        pub async fn register_rpc_method(&self, _method: &str) -> Result<(), String> {
            Err(Self::unavailable())
        }

        pub async fn unregister_rpc_method(&self, _method: &str) -> Result<(), String> {
            Err(Self::unavailable())
        }

        pub async fn send_message(
            &self,
            session_id: &str,
            encrypted_message: &str,
            local_id: Option<String>,
        ) -> Result<(), String> {
            if let Some(sink) = self.local_sink() {
                sink.on_message(session_id, encrypted_message, local_id.as_deref());
            }
            Ok(())
        }

        pub async fn send_transient_message(
            &self,
            session_id: &str,
            encrypted_message: &str,
        ) -> Result<(), String> {
            if let Some(sink) = self.local_sink() {
                sink.on_transient_message(session_id, encrypted_message);
            }
            Ok(())
        }

        pub async fn update_session_metadata(
            &self,
            session_id: &str,
            encrypted_metadata: &str,
            expected_version: u32,
        ) -> Result<serde_json::Value, String> {
            if let Some(sink) = self.local_sink() {
                sink.on_metadata_update(session_id, encrypted_metadata, expected_version);
            }
            Ok(json!({"result": "success"}))
        }

        pub async fn update_session_state(
            &self,
            session_id: &str,
            encrypted_state: Option<&str>,
            expected_version: u32,
        ) -> Result<serde_json::Value, String> {
            if let Some(sink) = self.local_sink() {
                sink.on_state_update(session_id, encrypted_state, expected_version);
            }
            Ok(json!({"result": "success"}))
        }

        pub async fn session_alive(
            &self,
            session_id: &str,
            thinking: Option<bool>,
            thinking_status: Option<&str>,
            context_tokens: u32,
            compression_threshold: u32,
        ) -> Result<(), String> {
            if let Some(sink) = self.local_sink() {
                sink.on_session_alive(
                    session_id,
                    thinking,
                    thinking_status,
                    context_tokens,
                    compression_threshold,
                );
            }
            Ok(())
        }

        pub async fn emit_usage_report(
            &self,
            _session_id: &str,
            _model: &str,
            _input_tokens: u32,
            _output_tokens: u32,
            _total_tokens: u32,
        ) -> Result<(), String> {
            Ok(())
        }

        pub async fn emit_session_end(&self, _session_id: &str) -> Result<(), String> {
            Ok(())
        }

        pub async fn emit(&self, _event: &str, _payload: serde_json::Value) -> Result<(), String> {
            Ok(())
        }

        pub async fn machine_alive(&self, _machine_id: &str) -> Result<(), String> {
            Err(Self::unavailable())
        }

        pub async fn ping(&self, _timeout: std::time::Duration) -> Result<(), String> {
            Err(Self::unavailable())
        }

        pub async fn update_machine_state(
            &self,
            _machine_id: &str,
            _encrypted_state: &str,
            _expected_version: u32,
        ) -> Result<serde_json::Value, String> {
            Err(Self::unavailable())
        }

        pub async fn disconnect(&self) -> Result<(), String> {
            Ok(())
        }

        pub fn connection_type(&self) -> &ConnectionType {
            &self.connection_type
        }

        pub fn is_local(&self) -> bool {
            true
        }

        pub fn unavailable_response() -> serde_json::Value {
            json!({ "error": Self::unavailable() })
        }
    }

    pub struct HeartbeatManager {
        running: Arc<AtomicBool>,
    }

    impl HeartbeatManager {
        pub fn new(_socket: Arc<HappySocket>, _machine_id: impl Into<String>) -> Self {
            Self {
                running: Arc::new(AtomicBool::new(false)),
            }
        }

        pub fn new_with_failures(
            socket: Arc<HappySocket>,
            machine_id: impl Into<String>,
            _consecutive_failures: Arc<AtomicU32>,
        ) -> Self {
            Self::new(socket, machine_id)
        }

        pub fn with_interval(self, _interval_secs: u64) -> Self {
            self
        }

        pub async fn start(&self) {
            self.running.store(true, Ordering::SeqCst);
        }

        pub fn stop(&self) {
            self.running.store(false, Ordering::SeqCst);
        }

        pub fn is_running(&self) -> bool {
            self.running.load(Ordering::SeqCst)
        }

        pub fn consecutive_failures_arc(&self) -> Arc<AtomicU32> {
            Arc::new(AtomicU32::new(0))
        }

        pub fn consecutive_failures(&self) -> u32 {
            0
        }

        pub fn set_consecutive_failures(&self, _value: u32) {}
    }

    #[async_trait::async_trait]
    pub trait SessionRecoveryHooks: Send + Sync {
        async fn reconnect_dead_sessions(&self) -> Result<(), String>;
        async fn discover_new_sessions(&self) -> Result<(), String>;
    }

    pub struct WatchdogState {
        pub machine_id: String,
        pub server_url: String,
        pub auth_token: String,
        pub encryption_key: [u8; 32],
        pub encryption_variant: EncryptionVariant,
        pub rpc_methods: Vec<String>,
        pub started_at: i64,
        pub daemon_state_version: Arc<AtomicU32>,
        pub machine_socket: Arc<Mutex<Option<Arc<HappySocket>>>>,
        pub rpc_registry: Arc<RpcRegistry>,
        pub heartbeat_manager: Arc<Mutex<Option<HeartbeatManager>>>,
        pub heartbeat_failures: Arc<AtomicU32>,
        pub session_hooks: Arc<dyn SessionRecoveryHooks>,
    }

    pub struct ConnectionWatchdog;

    impl ConnectionWatchdog {
        pub fn start(_state: Arc<WatchdogState>) -> Self {
            Self
        }

        pub fn stop(&self) {}
    }

    pub async fn reconnect_machine_runtime<T>(_state: &T) -> Result<(), String> {
        Ok(())
    }

    pub async fn server_is_reachable(_server_url: &str) -> bool {
        false
    }
}

#[cfg(not(feature = "commercial-cloud"))]
pub use community::*;
