//! Local HTTP and websocket API for the QQ bridge service.

use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::service::{SendMessageReceipt, ServiceState};

/// Build the local API router for the bridge runtime.
pub fn build_router(state: ServiceState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/api/session", get(session_handler))
        .route("/api/friends", get(friends_handler))
        .route("/api/groups", get(groups_handler))
        .route("/api/events/ws", get(events_ws_handler))
        .route("/api/messages/private", post(send_private_handler))
        .route("/api/messages/group", post(send_group_handler))
        .with_state(state)
}

/// Run the local API server until the task is cancelled or the listener fails.
pub async fn serve(bind: &str, state: ServiceState) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, build_router(state)).await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SendMessageResponse {
    status: &'static str,
    receipt: SendMessageReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct SendPrivateRequest {
    user_id: i64,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct SendGroupRequest {
    group_id: i64,
    text: String,
}

async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
    })
}

async fn session_handler(
    State(state): State<ServiceState>,
) -> Json<crate::service::SessionSnapshot> {
    Json(state.session().await)
}

async fn friends_handler(
    State(state): State<ServiceState>,
) -> Json<Vec<crate::service::FriendProfile>> {
    Json(state.friends().await)
}

async fn groups_handler(
    State(state): State<ServiceState>,
) -> Json<Vec<crate::service::GroupProfile>> {
    Json(state.groups().await)
}

async fn send_private_handler(
    State(state): State<ServiceState>,
    Json(payload): Json<SendPrivateRequest>,
) -> Result<Json<SendMessageResponse>, (StatusCode, Json<ErrorResponse>)> {
    let text = payload.text.trim().to_string();
    if text.is_empty() {
        return Err(bad_request("text must not be empty"));
    }

    let receipt = state
        .send_private_message(payload.user_id, text)
        .await
        .map_err(internal_error)?;
    Ok(Json(SendMessageResponse {
        status: "ok",
        receipt,
    }))
}

async fn send_group_handler(
    State(state): State<ServiceState>,
    Json(payload): Json<SendGroupRequest>,
) -> Result<Json<SendMessageResponse>, (StatusCode, Json<ErrorResponse>)> {
    let text = payload.text.trim().to_string();
    if text.is_empty() {
        return Err(bad_request("text must not be empty"));
    }

    let receipt = state
        .send_group_message(payload.group_id, text)
        .await
        .map_err(internal_error)?;
    Ok(Json(SendMessageResponse {
        status: "ok",
        receipt,
    }))
}

async fn events_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<ServiceState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        let mut socket = socket;
        let mut events_rx = state.subscribe_events();
        while let Ok(event) = events_rx.recv().await {
            let Ok(payload) = serde_json::to_string(&event) else {
                debug!("skip event that failed to serialize");
                continue;
            };
            if socket.send(Message::Text(payload.into())).await.is_err() {
                break;
            }
        }
    })
}

fn bad_request(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: message.to_string(),
        }),
    )
}

fn internal_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: error.to_string(),
        }),
    )
}
