//! Local API tests.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use codex_bridge_core::{
    api::build_router,
    reply_context::ActiveReplyContext,
    service::{ServiceState, SessionSnapshot, SessionStatus, TaskSnapshot},
};
use tempfile::TempDir;
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
async fn status_route_returns_running_snapshot_and_prompt_file() {
    let state = ServiceState::for_tests();
    state
        .set_task_snapshot(TaskSnapshot {
            running_task_id: Some("task-1".to_string()),
            running_conversation_key: Some("private:42".to_string()),
            running_summary: Some("正在执行".to_string()),
            recent_output: vec!["先看 orchestrator".to_string(), "准备改 /status".to_string()],
            queue_len: 2,
            last_terminal_summary: Some("已完成".to_string()),
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
    let body = String::from_utf8(body.to_vec()).expect("utf8");
    assert!(body.contains("最近输出"));
    assert!(body.contains("先看 orchestrator"));
    assert!(body.contains("准备改 /status"));
    assert!(body.contains("Prompt file: .run/default/prompt/system_prompt.md"));
    assert!(!body.contains("Prompt version"));
}

#[tokio::test]
async fn queue_route_reflects_snapshot() {
    let state = ServiceState::for_tests();
    state
        .set_task_snapshot(TaskSnapshot {
            queue_len: 1,
            ..TaskSnapshot::default()
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
        .set_task_snapshot(TaskSnapshot {
            running_conversation_key: Some("private:42".to_string()),
            ..TaskSnapshot::default()
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
        .set_task_snapshot(TaskSnapshot {
            running_conversation_key: Some("private:42".to_string()),
            ..TaskSnapshot::default()
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
        .set_task_snapshot(TaskSnapshot {
            last_retryable_conversation_key: Some("private:42".to_string()),
            ..TaskSnapshot::default()
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
