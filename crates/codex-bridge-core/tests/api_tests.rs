//! Local API tests.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use codex_bridge_core::{
    api::build_router,
    service::{ServiceState, SessionSnapshot, SessionStatus, TaskSnapshot},
};
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
async fn status_route_returns_running_snapshot_and_prompt_version() {
    let state = ServiceState::for_tests();
    state
        .set_task_snapshot(TaskSnapshot {
            running_task_id: Some("task-1".to_string()),
            running_conversation_key: Some("private:42".to_string()),
            running_summary: Some("正在执行".to_string()),
            queue_len: 2,
            last_terminal_summary: Some("已完成".to_string()),
            last_retryable_conversation_key: None,
            prompt_version: Some("2026-04-05".to_string()),
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
