//! Neutral RPC Handler System
//!
//! Provides RPC handler registration and message routing, shared by community
//! and commercial editions. Commercial-only encrypted RPC wrappers live in
//! `cteno-happy-client-rpc`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub type RpcHandlerFuture = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;
pub type RpcHandler = Arc<dyn Fn(Value) -> RpcHandlerFuture + Send + Sync>;

pub struct RpcRegistry {
    handlers: Arc<RwLock<HashMap<String, RpcHandler>>>,
    persistent_handlers: Arc<RwLock<HashMap<String, RpcHandler>>>,
}

impl RpcRegistry {
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(RwLock::new(HashMap::new())),
            persistent_handlers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register<F, Fut>(&self, method: impl Into<String>, handler: F)
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, String>> + Send + 'static,
    {
        let method = method.into();
        log::info!("Registering RPC handler: {}", method);

        let mut handlers = self.handlers.write().await;
        handlers.insert(
            method,
            Arc::new(move |params: Value| -> RpcHandlerFuture { Box::pin(handler(params)) }),
        );
    }

    pub async fn register_persistent<F, Fut>(&self, method: impl Into<String>, handler: F)
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, String>> + Send + 'static,
    {
        let method = method.into();
        log::info!("Registering persistent RPC handler: {}", method);

        let mut handlers = self.persistent_handlers.write().await;
        handlers.insert(
            method,
            Arc::new(move |params: Value| -> RpcHandlerFuture { Box::pin(handler(params)) }),
        );
    }

    pub async fn register_sync<F>(&self, method: impl Into<String>, handler: F)
    where
        F: Fn(Value) -> Result<Value, String> + Send + Sync + 'static,
    {
        self.register(method, move |params: Value| {
            let result = handler(params);
            async move { result }
        })
        .await;
    }

    pub async fn unregister(&self, method: &str) {
        log::info!("Unregistering RPC handler: {}", method);

        let mut handlers = self.handlers.write().await;
        handlers.remove(method);
    }

    pub async fn clear_all(&self) {
        let mut handlers = self.handlers.write().await;
        let count = handlers.len();
        handlers.clear();
        let persistent_count = self.persistent_handlers.read().await.len();
        log::info!(
            "Cleared {} transient RPC handlers ({} persistent remain)",
            count,
            persistent_count
        );
    }

    pub async fn handle(&self, request: RpcRequest) -> RpcResponse {
        log::debug!(
            "Handling RPC request: {} ({})",
            request.method,
            request.request_id
        );

        let handler = {
            let handlers = self.handlers.read().await;
            if let Some(handler) = handlers.get(&request.method).cloned() {
                Some(handler)
            } else {
                drop(handlers);
                let persistent = self.persistent_handlers.read().await;
                persistent.get(&request.method).cloned()
            }
        };

        match handler {
            Some(handler) => match handler(request.params).await {
                Ok(result) => RpcResponse {
                    request_id: request.request_id,
                    result: Some(result),
                    error: None,
                },
                Err(error) => {
                    log::error!("RPC handler error for {}: {}", request.method, error);
                    RpcResponse {
                        request_id: request.request_id,
                        result: None,
                        error: Some(error),
                    }
                }
            },
            None => {
                log::warn!("No handler registered for method: {}", request.method);
                RpcResponse {
                    request_id: request.request_id,
                    result: None,
                    error: Some(format!("Unknown method: {}", request.method)),
                }
            }
        }
    }

    pub async fn list_methods(&self) -> Vec<String> {
        let handlers = self.handlers.read().await;
        handlers.keys().cloned().collect()
    }

    pub async fn has_method(&self, method: &str) -> bool {
        let handlers = self.handlers.read().await;
        handlers.contains_key(method)
    }
}

impl Default for RpcRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_rpc_registry() {
        let registry = RpcRegistry::new();

        registry
            .register("test.echo", |params| async move {
                Ok(json!({ "echoed": params }))
            })
            .await;

        let request = RpcRequest {
            request_id: "test-123".to_string(),
            method: "test.echo".to_string(),
            params: json!({ "message": "hello" }),
        };

        let response = registry.handle(request).await;
        assert!(response.error.is_none());
        assert!(response.result.is_some());

        let request = RpcRequest {
            request_id: "test-456".to_string(),
            method: "unknown.method".to_string(),
            params: json!({}),
        };

        let response = registry.handle(request).await;
        assert!(response.error.is_some());
        assert!(response.result.is_none());
    }

    #[tokio::test]
    async fn test_list_methods() {
        let registry = RpcRegistry::new();

        registry
            .register("method1", |_| async { Ok(json!({})) })
            .await;
        registry
            .register("method2", |_| async { Ok(json!({})) })
            .await;

        let methods = registry.list_methods().await;
        assert_eq!(methods.len(), 2);
        assert!(methods.contains(&"method1".to_string()));
        assert!(methods.contains(&"method2".to_string()));
    }
}
