use super::session::{reconcile_default_profile_store, SessionRegistry};
use crate::happy_client::runtime::{ProfileRpcHooks, RuntimeFuture};
use crate::llm_profile::{self, LlmProfile, ProfileStore};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

pub(crate) fn build_profile_rpc_hooks(
    session_connections: SessionRegistry,
    profile_store: Arc<RwLock<ProfileStore>>,
    proxy_profiles: Arc<RwLock<Vec<LlmProfile>>>,
    app_data_dir: PathBuf,
    server_url: String,
    api_key: String,
) -> ProfileRpcHooks {
    ProfileRpcHooks {
        list_profiles: Arc::new({
            let api_key = api_key.clone();
            let profile_store = profile_store.clone();
            let proxy_profiles = proxy_profiles.clone();
            move || {
                let api_key = api_key.clone();
                let profile_store = profile_store.clone();
                let proxy_profiles = proxy_profiles.clone();
                Box::pin(async move {
                    let store = profile_store.read().await;
                    let proxy = proxy_profiles.read().await;
                    Ok(store.to_display(&api_key, &proxy))
                }) as RuntimeFuture<Result<Value, String>>
            }
        }),
        refresh_proxy_profiles: Arc::new({
            let app_data_dir = app_data_dir.clone();
            let profile_store = profile_store.clone();
            let proxy_profiles = proxy_profiles.clone();
            let server_url = server_url.clone();
            move || {
                let app_data_dir = app_data_dir.clone();
                let profile_store = profile_store.clone();
                let proxy_profiles = proxy_profiles.clone();
                let server_url = server_url.clone();
                Box::pin(async move {
                    let profiles =
                        llm_profile::fetch_proxy_profiles_from_server(&server_url, &app_data_dir)
                            .await;
                    let count = profiles.len();
                    *proxy_profiles.write().await = profiles;
                    let default_profile_id = {
                        let proxy = proxy_profiles.read().await;
                        let mut store = profile_store.write().await;
                        reconcile_default_profile_store(
                            &mut store,
                            &proxy,
                            &app_data_dir,
                            "[refresh-proxy-profiles] ",
                        );
                        store.default_profile_id.clone()
                    };
                    log::info!(
                        "[refresh-proxy-profiles] Refreshed {} proxy profiles",
                        count
                    );
                    Ok(json!({
                        "success": true,
                        "count": count,
                        "defaultProfileId": default_profile_id,
                    }))
                }) as RuntimeFuture<Result<Value, String>>
            }
        }),
        export_profiles: Arc::new({
            let profile_store = profile_store.clone();
            move || {
                let profile_store = profile_store.clone();
                Box::pin(async move {
                    let store = profile_store.read().await;
                    let profiles_json = serde_json::to_value(&store.profiles)
                        .map_err(|e| format!("Failed to serialize profiles: {}", e))?;
                    Ok(json!({
                        "profiles": profiles_json,
                        "defaultProfileId": store.default_profile_id,
                    }))
                }) as RuntimeFuture<Result<Value, String>>
            }
        }),
        save_profile: Arc::new({
            let app_data_dir = app_data_dir.clone();
            let profile_store = profile_store.clone();
            move |profile_val: Value| {
                let app_data_dir = app_data_dir.clone();
                let profile_store = profile_store.clone();
                Box::pin(async move {
                    let profile: LlmProfile = match serde_json::from_value(profile_val) {
                        Ok(profile) => profile,
                        Err(e) => {
                            return Ok(
                                json!({ "success": false, "error": format!("Invalid profile: {}", e) }),
                            );
                        }
                    };

                    let mut store = profile_store.write().await;
                    let id = profile.id.clone();
                    store.save_profile(profile);
                    if let Err(e) = llm_profile::save_profiles(&app_data_dir, &store) {
                        log::error!("Failed to save profiles: {}", e);
                        return Ok(json!({ "success": false, "error": e }));
                    }

                    Ok(json!({ "success": true, "id": id }))
                }) as RuntimeFuture<Result<Value, String>>
            }
        }),
        save_coding_plan_profiles: Arc::new({
            let app_data_dir = app_data_dir.clone();
            let profile_store = profile_store.clone();
            move |payload: Value| {
                let app_data_dir = app_data_dir.clone();
                let profile_store = profile_store.clone();
                Box::pin(async move {
                    let profiles_val = payload
                        .get("profiles")
                        .cloned()
                        .unwrap_or_else(|| Value::Array(Vec::new()));
                    let profiles: Vec<LlmProfile> = match serde_json::from_value(profiles_val) {
                        Ok(profiles) => profiles,
                        Err(e) => {
                            return Ok(json!({
                                "success": false,
                                "error": format!("Invalid Coding Plan profiles: {}", e),
                            }));
                        }
                    };

                    if profiles.is_empty() {
                        return Ok(json!({
                            "success": false,
                            "error": "No Coding Plan profiles provided",
                        }));
                    }

                    let default_profile_id = payload
                        .get("defaultProfileId")
                        .and_then(Value::as_str)
                        .filter(|id| profiles.iter().any(|profile| profile.id == *id))
                        .unwrap_or(&profiles[0].id)
                        .to_string();
                    let profile_count = profiles.len();

                    let mut store = profile_store.write().await;
                    for profile in profiles {
                        store.save_profile(profile);
                    }
                    store.default_profile_id = default_profile_id.clone();
                    if let Err(e) = llm_profile::save_profiles(&app_data_dir, &store) {
                        log::error!("Failed to save Coding Plan profiles: {}", e);
                        return Ok(json!({ "success": false, "error": e }));
                    }

                    Ok(json!({
                        "success": true,
                        "count": profile_count,
                        "defaultProfileId": default_profile_id,
                    }))
                }) as RuntimeFuture<Result<Value, String>>
            }
        }),
        delete_profile: Arc::new({
            let app_data_dir = app_data_dir.clone();
            let profile_store = profile_store.clone();
            move |profile_id: String| {
                let app_data_dir = app_data_dir.clone();
                let profile_store = profile_store.clone();
                Box::pin(async move {
                    let mut store = profile_store.write().await;
                    if !store.delete_profile(&profile_id) {
                        return Ok(
                            json!({ "success": false, "error": "Cannot delete default profile or profile not found" }),
                        );
                    }
                    if let Err(e) = llm_profile::save_profiles(&app_data_dir, &store) {
                        log::error!("Failed to save profiles after delete: {}", e);
                        return Ok(json!({ "success": false, "error": e }));
                    }

                    Ok(json!({ "success": true }))
                }) as RuntimeFuture<Result<Value, String>>
            }
        }),
        switch_session_model: Arc::new({
            let session_connections = session_connections.clone();
            move |session_id: String, profile_id: String, reasoning_effort: Option<String>| {
                let session_connections = session_connections.clone();
                Box::pin(async move {
                    if let Some(conn) = session_connections.get(&session_id).await {
                        let outcome = conn
                            .switch_profile(profile_id.clone(), reasoning_effort)
                            .await?;
                        let (outcome_kind, reason) = match outcome {
                            multi_agent_runtime_core::ModelChangeOutcome::Applied => {
                                ("applied", None)
                            }
                            multi_agent_runtime_core::ModelChangeOutcome::RestartRequired {
                                reason,
                            } => ("restart_required", Some(reason)),
                            multi_agent_runtime_core::ModelChangeOutcome::Unsupported => {
                                ("unsupported", None)
                            }
                        };
                        Ok(json!({
                            "success": true,
                            "sessionId": session_id,
                            "modelId": profile_id,
                            "outcome": outcome_kind,
                            "reason": reason,
                        }))
                    } else {
                        Ok(
                            json!({ "success": false, "error": "Session not found or not connected" }),
                        )
                    }
                }) as RuntimeFuture<Result<Value, String>>
            }
        }),
    }
}
