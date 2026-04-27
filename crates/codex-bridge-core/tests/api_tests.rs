//! Local API tests.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use codex_bridge_core::{
    api::build_router,
    conversation_history::{HistoryMessage, HistoryQueryResult},
    lane_manager::{LaneRuntimeState, LaneSnapshot, RuntimeSlotSnapshot, RuntimeSlotState},
    reply_context::ActiveReplyContext,
    service::{ServiceCommand, ServiceState, SessionSnapshot, SessionStatus},
};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tower::ServiceExt;

#[tokio::test]
async fn health_route_returns_ok() {
    let state = ServiceState::for_tests();
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("health response");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn session_route_returns_current_snapshot() {
    let state = ServiceState::for_tests();
    state
        .set_session(SessionSnapshot {
            status: SessionStatus::Connected,
            self_id: Some(2993013575),
            nickname: Some("离殇".to_string()),
            qq_pid: Some(12345),
        })
        .await;

    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri("/api/session")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("session response");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn send_private_route_rejects_empty_text() {
    let state = ServiceState::for_tests();
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/messages/private")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"user_id":2394626220,"text":"   "}"#))
                .expect("request"),
        )
        .await
        .expect("send response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn status_route_returns_lane_and_slot_snapshots() {
    let state = ServiceState::for_tests();
    state
        .set_runtime_snapshot(codex_bridge_core::lane_manager::RuntimeSnapshot {
            lanes: vec![LaneSnapshot {
                conversation_key: "private:42".to_string(),
                thread_id: Some("thread-a".to_string()),
                state: LaneRuntimeState::Running,
                pending_turn_count: 2,
                active_task_id: Some("task-1".to_string()),
                active_since: Some("2026-04-18T11:00:00Z".to_string()),
                last_progress_at: Some("2026-04-18T11:01:00Z".to_string()),
                last_terminal_summary: Some("已完成".to_string()),
            }],
            runtime_slots: vec![RuntimeSlotSnapshot {
                slot_id: 0,
                state: RuntimeSlotState::Busy,
                assigned_conversation_key: Some("private:42".to_string()),
            }],
            ready_lane_count: 1,
            total_pending_turn_count: 2,
            last_retryable_conversation_key: None,
            prompt_file: Some(".run/default/prompt/system_prompt.md".to_string()),
        })
        .await;

    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri("/api/status")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("status response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["snapshot"]["lanes"][0]["conversation_key"], "private:42");
    assert_eq!(json["snapshot"]["lanes"][0]["state"], "running");
    assert_eq!(json["snapshot"]["runtime_slots"][0]["state"], "busy");
    assert_eq!(json["snapshot"]["prompt_file"], ".run/default/prompt/system_prompt.md");
}

#[tokio::test]
async fn queue_route_reflects_snapshot() {
    let state = ServiceState::for_tests();
    state
        .set_runtime_snapshot(codex_bridge_core::lane_manager::RuntimeSnapshot {
            ready_lane_count: 1,
            total_pending_turn_count: 2,
            ..Default::default()
        })
        .await;

    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri("/api/queue")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("queue response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("queue json");
    assert_eq!(json["text"], "等待中的会话：1，待处理 turn：2");
}

#[tokio::test]
async fn cancel_route_rejects_without_running_conversation() {
    let state = ServiceState::for_tests();
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri("/api/tasks/cancel")
                .method("POST")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("cancel response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn cancel_route_sends_when_running_conversation_exists() {
    let state = ServiceState::for_tests();
    state
        .set_runtime_snapshot(codex_bridge_core::lane_manager::RuntimeSnapshot {
            lanes: vec![LaneSnapshot {
                conversation_key: "private:42".to_string(),
                state: LaneRuntimeState::Running,
                ..Default::default()
            }],
            ..Default::default()
        })
        .await;

    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri("/api/tasks/cancel")
                .method("POST")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("cancel response");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn retry_last_route_sends_when_running_conversation_exists() {
    let state = ServiceState::for_tests();
    state
        .set_runtime_snapshot(codex_bridge_core::lane_manager::RuntimeSnapshot {
            lanes: vec![LaneSnapshot {
                conversation_key: "private:42".to_string(),
                state: LaneRuntimeState::Running,
                ..Default::default()
            }],
            ..Default::default()
        })
        .await;

    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri("/api/tasks/retry-last")
                .method("POST")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("retry response");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn retry_last_route_uses_last_retryable_conversation_when_idle() {
    let state = ServiceState::for_tests();
    state
        .set_runtime_snapshot(codex_bridge_core::lane_manager::RuntimeSnapshot {
            last_retryable_conversation_key: Some("private:42".to_string()),
            ..Default::default()
        })
        .await;

    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri("/api/tasks/retry-last")
                .method("POST")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("retry response");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn queue_route_rejects_control_command_without_running_conversation() {
    let state = ServiceState::for_tests();
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri("/api/queue")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("queue response");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn reply_route_sends_text_when_token_is_active() {
    let state = ServiceState::for_tests();
    let tempdir = TempDir::new().expect("tempdir");
    let repo_root = tempdir.path().to_path_buf();
    let artifacts_dir = repo_root.join(".run/artifacts");
    std::fs::create_dir_all(&artifacts_dir).expect("create artifacts dir");
    state
        .activate_reply_context(ActiveReplyContext {
            token: "token-1".to_string(),
            conversation_key: "private:42".to_string(),
            is_group: false,
            reply_target_id: 42,
            source_message_id: 9001,
            source_sender_id: 42,
            source_sender_name: "alice".to_string(),
            repo_root,
            artifacts_dir,
        })
        .await
        .expect("activate reply context");

    let response = build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/reply")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"token":"token-1","text":"hello"}"#))
                .expect("request"),
        )
        .await
        .expect("reply response");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn reply_route_rejects_files_outside_artifacts() {
    let state = ServiceState::for_tests();
    let tempdir = TempDir::new().expect("tempdir");
    let repo_root = tempdir.path().to_path_buf();
    let artifacts_dir = repo_root.join(".run/artifacts");
    let outside_file = repo_root.join("report.md");
    std::fs::create_dir_all(&artifacts_dir).expect("create artifacts dir");
    std::fs::write(&outside_file, "# report\n").expect("write outside file");
    state
        .activate_reply_context(ActiveReplyContext {
            token: "token-2".to_string(),
            conversation_key: "private:42".to_string(),
            is_group: false,
            reply_target_id: 42,
            source_message_id: 9001,
            source_sender_id: 42,
            source_sender_name: "alice".to_string(),
            repo_root,
            artifacts_dir,
        })
        .await
        .expect("activate reply context");

    let response = build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/reply")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    "{{\"token\":\"token-2\",\"file\":\"{}\"}}",
                    outside_file.display()
                )))
                .expect("request"),
        )
        .await
        .expect("reply response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn history_query_route_uses_lane_scoped_token() {
    let tempdir = TempDir::new().expect("tempdir");
    let (command_tx, mut command_rx) = mpsc::channel(8);
    let (control_tx, _control_rx) = mpsc::channel(8);
    let state = ServiceState::with_control_and_reply_context_paths(
        command_tx,
        control_tx,
        tempdir.path().join("contexts"),
    );
    state
        .set_session(SessionSnapshot {
            status: SessionStatus::Connected,
            self_id: Some(99),
            nickname: Some("bot".to_string()),
            qq_pid: None,
        })
        .await;
    state
        .activate_reply_context(ActiveReplyContext {
            token: "token-history".to_string(),
            conversation_key: "group:123".to_string(),
            is_group: true,
            reply_target_id: 123,
            source_message_id: 9001,
            source_sender_id: 42,
            source_sender_name: "alice".to_string(),
            repo_root: tempdir.path().to_path_buf(),
            artifacts_dir: tempdir.path().join(".run/artifacts"),
        })
        .await
        .expect("activate reply context");

    let command_handle = tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            if let ServiceCommand::FetchConversationHistory {
                is_group,
                target_id,
                query,
                respond_to,
                ..
            } = command
            {
                assert!(is_group);
                assert_eq!(target_id, 123);
                assert_eq!(query.query.as_deref(), Some("找部署那句"));
                let _ = respond_to.send(Ok(HistoryQueryResult {
                    messages: vec![HistoryMessage {
                        message_id: 11,
                        timestamp: 1_744_970_800,
                        sender_id: 42,
                        sender_name: "alice".to_string(),
                        text: "部署今天下午做".to_string(),
                    }],
                    truncated: false,
                }));
                break;
            }
        }
    });

    let response = build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/history/query")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"token":"token-history","query":"找部署那句","limit":50}"#))
                .expect("request"),
        )
        .await
        .expect("history query response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["result"]["messages"][0]["message_id"], 11);
    assert_eq!(json["result"]["messages"][0]["text"], "部署今天下午做");

    command_handle.await.expect("command handler");
}

#[tokio::test]
async fn capability_invoke_returns_stub_text() {
    use std::sync::Arc;

    use async_trait::async_trait;
    use codex_bridge_core::{
        model_capabilities::ModelRegistry,
        model_capability::{
            CapabilityError, CapabilityInput, CapabilityKind, CapabilityOutput, ModelCapability,
        },
    };
    use http_body_util::BodyExt;
    use serde_json::json;

    #[derive(Debug)]
    struct EchoCapability;
    #[async_trait]
    impl ModelCapability for EchoCapability {
        fn id(&self) -> &str {
            "echo"
        }
        fn kind(&self) -> CapabilityKind {
            CapabilityKind::Text
        }
        fn display_name(&self) -> &str {
            "Echo stub"
        }
        fn scenario(&self) -> &str {
            "test fixture"
        }
        fn tags(&self) -> &[&'static str] {
            &["test"]
        }
        async fn invoke(
            &self,
            input: &CapabilityInput,
        ) -> Result<CapabilityOutput, CapabilityError> {
            Ok(CapabilityOutput::Text {
                text: format!("echoed: {}", input.prompt),
            })
        }
    }

    let state = ServiceState::for_tests();
    let mut registry = ModelRegistry::empty();
    registry.insert(Arc::new(EchoCapability)).expect("insert");
    state.set_capabilities(Arc::new(registry)).await;

    let body = json!({ "id": "echo", "prompt": "hello" }).to_string();
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/capability/invoke")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("capability response");

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let payload: serde_json::Value = serde_json::from_slice(&bytes).expect("decode");
    assert_eq!(payload["kind"], "text");
    assert_eq!(payload["id"], "echo");
    assert_eq!(payload["text"], "echoed: hello");
}

#[tokio::test]
async fn capability_invoke_rejects_unknown_id() {
    use serde_json::json;

    let state = ServiceState::for_tests();
    let body = json!({ "id": "does-not-exist", "prompt": "hi" }).to_string();
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/capability/invoke")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("capability response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn capability_reload_picks_up_new_toml() {
    use std::fs;

    let tmp = TempDir::new().expect("tmpdir");
    let config_path = tmp.path().join("model_capabilities.toml");

    // First generation: one capability.
    fs::write(
        &config_path,
        r#"
            [[capabilities]]
            id = "claude-kiro"
            kind = "anthropic_messages"
            display_name = "Claude via Kiro"
            scenario = "human-tone replies"
            base_url = "http://127.0.0.1:39180/api/kiro-gateway"
            api_key = "sf-kiro-test"
            model = "claude-sonnet-4-6"
            max_tokens = 512
        "#,
    )
    .expect("seed initial config");

    let state = ServiceState::for_tests();
    state.set_capabilities_file(config_path.clone());

    // Trigger initial reload via the endpoint and verify the registry
    // plus the shared prompt block were populated.
    let response = build_router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/capability/reload")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("reload response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .expect("collect body")
        .to_bytes();
    let payload: serde_json::Value = serde_json::from_slice(&bytes).expect("decode");
    assert_eq!(payload["capability_count"], 1);

    {
        let block_handle = state.capabilities_prompt_block_handle();
        let guard = block_handle.read().expect("read prompt block");
        let block = guard.clone().expect("block rendered");
        assert!(block.contains("claude-kiro"), "block body: {block}");
    }

    // Second generation: replace the file with two capabilities and
    // reload again. The new registry must be visible and the prompt
    // block updated in place.
    fs::write(
        &config_path,
        r#"
            [[capabilities]]
            id = "claude-kiro"
            kind = "anthropic_messages"
            display_name = "Claude via Kiro"
            scenario = "human-tone replies"
            base_url = "http://127.0.0.1:39180/api/kiro-gateway"
            api_key = "sf-kiro-test"
            model = "claude-sonnet-4-6"
            max_tokens = 512

            [[capabilities]]
            id = "claude-kiro-translate"
            kind = "anthropic_messages"
            display_name = "Claude translator"
            scenario = "translation"
            base_url = "http://127.0.0.1:39180/api/kiro-gateway"
            api_key = "sf-kiro-test"
            model = "claude-sonnet-4-6"
            max_tokens = 512
        "#,
    )
    .expect("overwrite config");

    let response = build_router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/capability/reload")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("second reload");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .expect("collect body")
        .to_bytes();
    let payload: serde_json::Value = serde_json::from_slice(&bytes).expect("decode");
    assert_eq!(payload["capability_count"], 2);

    let block_handle = state.capabilities_prompt_block_handle();
    let guard = block_handle.read().expect("read prompt block");
    let block = guard.clone().expect("block rendered");
    assert!(block.contains("claude-kiro-translate"));
}

#[tokio::test]
async fn capability_reload_fails_when_no_file_configured() {
    let state = ServiceState::for_tests();
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/capability/reload")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("reload response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn capability_reload_rejects_invalid_toml() {
    use std::fs;

    let tmp = TempDir::new().expect("tmpdir");
    let config_path = tmp.path().join("model_capabilities.toml");
    fs::write(&config_path, "this is not valid toml === %%%").expect("seed config");

    let state = ServiceState::for_tests();
    state.set_capabilities_file(config_path);

    let response = build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/capability/reload")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("reload response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
