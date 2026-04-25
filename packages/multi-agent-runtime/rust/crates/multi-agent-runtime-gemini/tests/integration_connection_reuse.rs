//! End-to-end adapter tests against a mock `gemini --acp` script speaking the
//! real JSON-RPC 2.0 / ndJSON protocol.
//!
//! These tests are the hook for validating that:
//! - initialize + session/new run on the shared connection
//! - two sessions on one connection route their events independently
//! - session/prompt yields StreamDelta + TurnComplete via the normalized
//!   EventStream
//! - permission requests correlate by inbound JSON-RPC id
//! - close_connection kills the child and drains pendings
//!
//! They're intentionally not `#[ignore]` — the mocks are local scripts with
//! no network, no API key, no binary dependency beyond `bash` + `python3`.
//! The separate live-binary test (see the bottom of this file) is gated by
//! `GEMINI_PATH` + `GEMINI_API_KEY` and always ignored by default.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use multi_agent_runtime_core::{
    AgentExecutor, DeltaKind, ExecutorEvent, PermissionMode, SpawnSessionSpec,
};
use multi_agent_runtime_gemini::GeminiAgentExecutor;
use serde_json::Value;

mod mock_harness;
mod store_stub;

use mock_harness::MockHarness;
use store_stub::RecordingStore;

fn make_executor(script_path: PathBuf) -> GeminiAgentExecutor {
    GeminiAgentExecutor::new(script_path, Arc::new(RecordingStore::default()))
        .with_spawn_ready_timeout(Duration::from_secs(5))
        .with_turn_timeout(Duration::from_secs(5))
}

fn default_spec(workdir: PathBuf) -> SpawnSessionSpec {
    SpawnSessionSpec {
        workdir,
        system_prompt: None,
        model: None,
        permission_mode: PermissionMode::Default,
        allowed_tools: None,
        additional_directories: Vec::new(),
        env: Default::default(),
        agent_config: Value::Null,
        resume_hint: None,
    }
}

#[tokio::test]
async fn open_connection_runs_initialize_and_parses_capabilities() {
    let harness = MockHarness::new("basic");
    let executor = make_executor(harness.script_path.clone());

    let session = executor
        .spawn_session(default_spec(harness.root.clone()))
        .await
        .expect("spawn should succeed");
    assert_eq!(session.id.as_str(), "mock-session-1");

    let caps = executor.capabilities();
    assert!(caps.supports_multi_session_per_process);
    assert!(caps.supports_resume);
}

#[tokio::test]
async fn start_session_on_sends_session_new_and_registers() {
    let harness = MockHarness::new("basic");
    let executor = make_executor(harness.script_path.clone());

    let handle = executor
        .open_connection(multi_agent_runtime_core::executor::ConnectionSpec::default())
        .await
        .expect("open_connection");

    let session = executor
        .start_session_on(&handle, default_spec(harness.root.clone()))
        .await
        .expect("start_session_on");
    assert_eq!(session.id.as_str(), "mock-session-1");
}

#[tokio::test]
async fn two_sessions_on_one_connection_route_independent_events() {
    let harness = MockHarness::new("two_sessions");
    let executor = make_executor(harness.script_path.clone());

    let s1 = executor
        .spawn_session(default_spec(harness.root.clone()))
        .await
        .expect("s1");
    assert_eq!(s1.id.as_str(), "session-1");

    let s2 = executor
        .spawn_session(default_spec(harness.root.clone()))
        .await
        .expect("s2");
    assert_eq!(s2.id.as_str(), "session-2");

    let stream1 = executor
        .send_message(
            &s1,
            multi_agent_runtime_core::UserMessage {
                task_id: None,
                content: "hi".to_string(),
                attachments: Vec::new(),
                parent_tool_use_id: None,
                injected_tools: Vec::new(),
            },
        )
        .await
        .expect("send1");

    let stream2 = executor
        .send_message(
            &s2,
            multi_agent_runtime_core::UserMessage {
                task_id: None,
                content: "hi".to_string(),
                attachments: Vec::new(),
                parent_tool_use_id: None,
                injected_tools: Vec::new(),
            },
        )
        .await
        .expect("send2");

    let events1 = collect_events(stream1).await;
    let events2 = collect_events(stream2).await;

    assert!(
        events1.iter().any(|e| matches!(
            e,
            ExecutorEvent::StreamDelta { kind: DeltaKind::Text, content } if content == "from-session-1"
        )),
        "session-1 should see its own text; got {events1:?}"
    );
    assert!(
        events2.iter().any(|e| matches!(
            e,
            ExecutorEvent::StreamDelta { kind: DeltaKind::Text, content } if content == "from-session-2"
        )),
        "session-2 should see its own text; got {events2:?}"
    );
    // Neither stream should see the other's text.
    assert!(!events1.iter().any(
        |e| matches!(e, ExecutorEvent::StreamDelta { content, .. } if content == "from-session-2")
    ));
    assert!(!events2.iter().any(
        |e| matches!(e, ExecutorEvent::StreamDelta { content, .. } if content == "from-session-1")
    ));
}

#[tokio::test]
async fn send_message_completes_with_turn_complete_and_usage() {
    let harness = MockHarness::new("basic");
    let executor = make_executor(harness.script_path.clone());

    let session = executor
        .spawn_session(default_spec(harness.root.clone()))
        .await
        .unwrap();

    let stream = executor
        .send_message(
            &session,
            multi_agent_runtime_core::UserMessage {
                task_id: None,
                content: "hi".to_string(),
                attachments: Vec::new(),
                parent_tool_use_id: None,
                injected_tools: Vec::new(),
            },
        )
        .await
        .unwrap();

    let events = collect_events(stream).await;
    let tc = events.iter().find_map(|e| match e {
        ExecutorEvent::TurnComplete { usage, .. } => Some(usage.clone()),
        _ => None,
    });
    let usage = tc.expect("TurnComplete should be emitted");
    assert_eq!(usage.input_tokens, 3);
    assert_eq!(usage.output_tokens, 1);
}

#[tokio::test]
async fn close_connection_kills_child_and_drains_pending() {
    let harness = MockHarness::new("basic");
    let executor = make_executor(harness.script_path.clone());
    let handle = executor
        .open_connection(multi_agent_runtime_core::executor::ConnectionSpec::default())
        .await
        .unwrap();

    let health = executor.check_connection(&handle).await.unwrap();
    assert_eq!(
        health,
        multi_agent_runtime_core::executor::ConnectionHealth::Healthy
    );

    executor.close_connection(handle.clone()).await.unwrap();

    let health_after = executor.check_connection(&handle).await.unwrap();
    assert!(matches!(
        health_after,
        multi_agent_runtime_core::executor::ConnectionHealth::Dead { .. }
    ));
}

#[tokio::test]
async fn check_connection_reports_dead_after_child_exit() {
    let harness = MockHarness::new("exit_after_init");
    let executor = make_executor(harness.script_path.clone());
    let handle = executor
        .open_connection(multi_agent_runtime_core::executor::ConnectionSpec::default())
        .await
        .unwrap();

    // Give the subprocess time to exit.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let health = executor.check_connection(&handle).await.unwrap();
    assert!(matches!(
        health,
        multi_agent_runtime_core::executor::ConnectionHealth::Dead { .. }
    ));
}

#[tokio::test]
async fn respond_to_permission_correlates_by_inbound_request_id() {
    let harness = MockHarness::new("permission");
    let executor = make_executor(harness.script_path.clone());

    let session = executor
        .spawn_session(default_spec(harness.root.clone()))
        .await
        .unwrap();

    let stream = executor
        .send_message(
            &session,
            multi_agent_runtime_core::UserMessage {
                task_id: None,
                content: "please run".to_string(),
                attachments: Vec::new(),
                parent_tool_use_id: None,
                injected_tools: Vec::new(),
            },
        )
        .await
        .unwrap();

    let session_clone = session.clone();
    let executor_arc: Arc<dyn AgentExecutor> = Arc::new(executor);
    let exec_clone = Arc::clone(&executor_arc);
    tokio::spawn(async move {
        // Wait a beat then approve — we know the permission comes during the
        // turn. Simplest: respond via a dedicated loop.
        // This task subscribes to the stream independently via open_connection
        // is not trivial; we handle correlation inside the main test instead.
        drop((session_clone, exec_clone));
    });

    // Iterate events until we see the PermissionRequest, approve, then expect
    // the "perm=selected" agent_message_chunk + TurnComplete.
    let mut stream = Box::pin(stream);
    let mut saw_perm = false;
    let mut saw_perm_text = false;
    let mut saw_turn_complete = false;
    while let Some(item) = stream.next().await {
        match item.expect("stream error") {
            ExecutorEvent::PermissionRequest { request_id, .. } => {
                saw_perm = true;
                executor_arc
                    .respond_to_permission(
                        &session,
                        request_id,
                        multi_agent_runtime_core::PermissionDecision::Allow,
                    )
                    .await
                    .expect("approve");
            }
            ExecutorEvent::StreamDelta { content, .. } if content.starts_with("perm=") => {
                saw_perm_text = true;
                assert_eq!(
                    content, "perm=selected",
                    "outcome should be selected on Allow"
                );
            }
            ExecutorEvent::TurnComplete { .. } => {
                saw_turn_complete = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_perm, "permission request should have been surfaced");
    assert!(
        saw_perm_text,
        "agent message with perm result should have streamed"
    );
    assert!(saw_turn_complete, "turn should have completed");
}

async fn collect_events(stream: multi_agent_runtime_core::EventStream) -> Vec<ExecutorEvent> {
    let mut out = Vec::new();
    let mut s = Box::pin(stream);
    while let Some(item) = s.next().await {
        out.push(item.unwrap());
    }
    out
}

// -----------------------------------------------------------------------
// Model-gate regression — ensures a non-gemini profile_id (e.g.
// "deepseek-reasoner" from a shared host profile) is NOT forwarded to
// `session/set_model`, because the real Gemini backend accepts the call
// silently and then errors the next `session/prompt` with
// `[500] Requested entity was not found.` See
// `tests/eval/gemini-model-gate.md`.
// -----------------------------------------------------------------------

fn spec_with_model(workdir: PathBuf, provider: &str, model_id: &str) -> SpawnSessionSpec {
    SpawnSessionSpec {
        workdir,
        system_prompt: None,
        model: Some(multi_agent_runtime_core::ModelSpec {
            provider: provider.to_string(),
            model_id: model_id.to_string(),
            reasoning_effort: None,
            temperature: None,
        }),
        permission_mode: PermissionMode::Default,
        allowed_tools: None,
        additional_directories: Vec::new(),
        env: Default::default(),
        agent_config: Value::Null,
        resume_hint: None,
    }
}

fn read_log(root: &std::path::Path) -> String {
    let log = root.join("set_model.log");
    std::fs::read_to_string(&log).unwrap_or_default()
}

#[tokio::test]
async fn apply_model_skips_set_model_when_provider_is_not_gemini() {
    // Shared host profile says "deepseek-reasoner" from an OpenAI-format
    // profile → resolve_spawn_model tags provider="openai". Adapter must
    // refuse to forward that id into session/set_model.
    let harness = MockHarness::new("model_gate");
    let executor = make_executor(harness.script_path.clone());

    let session = executor
        .spawn_session(spec_with_model(
            harness.root.clone(),
            "openai",
            "deepseek-reasoner",
        ))
        .await
        .expect("spawn");

    // Round-trip a prompt — must succeed because no bogus set_model fired.
    let stream = executor
        .send_message(
            &session,
            multi_agent_runtime_core::UserMessage {
                task_id: None,
                content: "hi".to_string(),
                attachments: Vec::new(),
                parent_tool_use_id: None,
                injected_tools: Vec::new(),
            },
        )
        .await
        .expect("send_message");
    let events = collect_events(stream).await;

    let log = read_log(&harness.root);
    assert!(
        log.is_empty(),
        "session/set_model must NOT be sent for provider=openai; got log=\"{log}\""
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ExecutorEvent::TurnComplete { .. })),
        "prompt should complete normally when set_model was skipped; got {events:?}"
    );
}

#[tokio::test]
async fn apply_model_skips_set_model_for_unknown_gemini_id() {
    // Even when provider="gemini", if the id isn't in the server-advertised
    // `availableModels` list we must not forward it — otherwise the session
    // gets poisoned and session/prompt returns [500].
    let harness = MockHarness::new("model_gate");
    let executor = make_executor(harness.script_path.clone());

    let session = executor
        .spawn_session(spec_with_model(
            harness.root.clone(),
            "gemini",
            "gemini-bogus-preview",
        ))
        .await
        .expect("spawn");

    let stream = executor
        .send_message(
            &session,
            multi_agent_runtime_core::UserMessage {
                task_id: None,
                content: "hi".to_string(),
                attachments: Vec::new(),
                parent_tool_use_id: None,
                injected_tools: Vec::new(),
            },
        )
        .await
        .expect("send_message");
    let events = collect_events(stream).await;

    let log = read_log(&harness.root);
    assert!(
        log.is_empty(),
        "unknown gemini id must be filtered out; set_model.log=\"{log}\""
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ExecutorEvent::TurnComplete { .. })),
        "prompt should complete since server default stayed in place; got {events:?}"
    );
}

#[tokio::test]
async fn apply_model_forwards_recognized_gemini_id() {
    // Positive control: a real gemini id from the advertised list must be
    // forwarded to session/set_model.
    let harness = MockHarness::new("model_gate");
    let executor = make_executor(harness.script_path.clone());

    let _session = executor
        .spawn_session(spec_with_model(
            harness.root.clone(),
            "gemini",
            "gemini-2.5-flash",
        ))
        .await
        .expect("spawn");

    let log = read_log(&harness.root);
    assert!(
        log.trim().lines().any(|l| l == "gemini-2.5-flash"),
        "recognized gemini id must be forwarded; log=\"{log}\""
    );
}

#[tokio::test]
async fn apply_model_does_not_poison_shared_connection_across_sessions() {
    // Regression: a bogus model on session A must not prevent session B
    // (on the same shared connection) from running prompts successfully.
    let harness = MockHarness::new("model_gate");
    let executor = make_executor(harness.script_path.clone());

    // Session A — requests a bogus id (would have poisoned the mock).
    let session_a = executor
        .spawn_session(spec_with_model(
            harness.root.clone(),
            "openai",
            "deepseek-reasoner",
        ))
        .await
        .expect("spawn session A");

    // Session B — default (no model).
    let session_b = executor
        .spawn_session(default_spec(harness.root.clone()))
        .await
        .expect("spawn session B");

    // Both prompts must succeed with TurnComplete — no [500] leakage.
    for session in [&session_a, &session_b] {
        let stream = executor
            .send_message(
                session,
                multi_agent_runtime_core::UserMessage {
                task_id: None,
                content: "hi".to_string(),
                    attachments: Vec::new(),
                    parent_tool_use_id: None,
                    injected_tools: Vec::new(),
                },
            )
            .await
            .expect("send");
        let events = collect_events(stream).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ExecutorEvent::TurnComplete { .. })),
            "each session must complete its turn on a shared (non-poisoned) connection; got {events:?}"
        );
    }
}

// -----------------------------------------------------------------------
// Live-binary smoke — ignored by default. Enable with:
//
//     GEMINI_PATH=/opt/homebrew/bin/gemini \
//     GEMINI_API_KEY=... \
//     cargo test -p multi-agent-runtime-gemini -- --ignored live_smoke
//
// Works without GEMINI_API_KEY when the host has cached OAuth creds.
// -----------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn live_smoke_initialize_plus_prompt() {
    let Ok(gemini_path) = std::env::var("GEMINI_PATH") else {
        eprintln!("GEMINI_PATH not set — skipping");
        return;
    };

    let workdir = std::env::temp_dir();
    let mut spec = default_spec(workdir.clone());
    if let Ok(key) = std::env::var("GEMINI_API_KEY") {
        spec.env.insert("GEMINI_API_KEY".to_string(), key);
    }

    let executor = GeminiAgentExecutor::new(
        PathBuf::from(gemini_path),
        Arc::new(RecordingStore::default()),
    )
    .with_spawn_ready_timeout(Duration::from_secs(60))
    .with_turn_timeout(Duration::from_secs(120));

    let session = executor
        .spawn_session(spec)
        .await
        .expect("spawn against live gemini");

    let stream = executor
        .send_message(
            &session,
            multi_agent_runtime_core::UserMessage {
                task_id: None,
                content: "Reply with exactly the single word PONG.".to_string(),
                attachments: Vec::new(),
                parent_tool_use_id: None,
                injected_tools: Vec::new(),
            },
        )
        .await
        .expect("send_message against live gemini");

    let events = collect_events(stream).await;
    assert!(
        events.iter().any(|e| matches!(
            e,
            ExecutorEvent::StreamDelta { kind: DeltaKind::Text, content } if content.contains("PONG")
        )),
        "live gemini should have streamed PONG; got {events:?}"
    );

    executor.close_session(&session).await.ok();
}

/// Reproduces the original failure mode end-to-end: spawn against the real
/// gemini CLI with a non-gemini model tag. Pre-fix this surfaced
/// `[500] Requested entity was not found.` on the next `session/prompt`.
/// Post-fix the bogus id is filtered out and the turn completes normally.
#[tokio::test]
#[ignore]
async fn live_bogus_model_does_not_surface_500() {
    let Ok(gemini_path) = std::env::var("GEMINI_PATH") else {
        eprintln!("GEMINI_PATH not set — skipping");
        return;
    };

    let workdir = std::env::temp_dir();
    let executor = GeminiAgentExecutor::new(
        PathBuf::from(gemini_path),
        Arc::new(RecordingStore::default()),
    )
    .with_spawn_ready_timeout(Duration::from_secs(60))
    .with_turn_timeout(Duration::from_secs(120));

    let session = executor
        .spawn_session(spec_with_model(workdir, "openai", "deepseek-reasoner"))
        .await
        .expect("spawn against live gemini");

    let stream = executor
        .send_message(
            &session,
            multi_agent_runtime_core::UserMessage {
                task_id: None,
                content: "Reply with exactly PONG.".to_string(),
                attachments: Vec::new(),
                parent_tool_use_id: None,
                injected_tools: Vec::new(),
            },
        )
        .await
        .expect("send_message against live gemini");

    let events = collect_events(stream).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ExecutorEvent::TurnComplete { .. })),
        "live prompt should complete even with a non-gemini profile id; got {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            ExecutorEvent::Error { message, .. } if message.contains("Requested entity was not found")
        )),
        "bogus model id must not poison the live session; got {events:?}"
    );

    executor.close_session(&session).await.ok();
}
