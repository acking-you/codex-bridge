//! Local API tests.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use qqbot_core::{
    api::build_router,
    service::{ServiceState, SessionSnapshot, SessionStatus},
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
