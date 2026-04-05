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

use crate::{
    message_router::{CommandRequest, ControlCommand},
    service::{SendMessageReceipt, ServiceState, TaskSnapshot},
};

/// Build the local API router for the bridge runtime.
pub fn build_router(state: ServiceState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/api/session", get(session_handler))
        .route("/api/friends", get(friends_handler))
        .route("/api/groups", get(groups_handler))
        .route("/api/status", get(status_handler))
        .route("/api/queue", get(queue_handler))
        .route("/api/tasks/cancel", post(cancel_handler))
        .route("/api/tasks/retry-last", post(retry_last_handler))
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TextResponse {
    status: &'static str,
    text: String,
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

async fn status_handler(
    State(state): State<ServiceState>,
) -> Result<Json<TextResponse>, (StatusCode, Json<ErrorResponse>)> {
    let snapshot = state.task_snapshot().await;
    let running = snapshot
        .running_task_id
        .clone()
        .unwrap_or_else(|| "无".to_string());
    let running_conversation = snapshot
        .running_conversation_key
        .clone()
        .unwrap_or_else(|| "无".to_string());
    let queue_len = snapshot.queue_len;
    let last = snapshot
        .last_terminal_summary
        .clone()
        .unwrap_or_else(|| "无".to_string());
    let prompt_version = snapshot
        .prompt_version
        .clone()
        .unwrap_or_else(|| "2026-04-05".to_string());
    let text = format!(
        "当前任务：{running}\n会话：{running_conversation}\n排队数量：{queue_len}\n最近结果：\
         {last}\nPrompt version: {prompt_version}"
    );
    Ok(Json(TextResponse {
        status: "ok",
        text,
    }))
}

async fn queue_handler(
    State(state): State<ServiceState>,
) -> Result<Json<TextResponse>, (StatusCode, Json<ErrorResponse>)> {
    let snapshot = state.task_snapshot().await;
    let text = format!("队列中的任务数量：{}", snapshot.queue_len);
    Ok(Json(TextResponse {
        status: "ok",
        text,
    }))
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

async fn cancel_handler(
    State(state): State<ServiceState>,
) -> Result<Json<TextResponse>, (StatusCode, Json<ErrorResponse>)> {
    let snapshot = state.task_snapshot().await;
    let command = command_from_snapshot(&snapshot, ControlCommand::Cancel)?;
    state
        .send_control_command(command)
        .await
        .map_err(internal_error)?;
    Ok(Json(TextResponse {
        status: "ok",
        text: "cancel sent".to_string(),
    }))
}

async fn retry_last_handler(
    State(state): State<ServiceState>,
) -> Result<Json<TextResponse>, (StatusCode, Json<ErrorResponse>)> {
    let snapshot = state.task_snapshot().await;
    let command = retry_command_from_snapshot(&snapshot)?;
    state
        .send_control_command(command)
        .await
        .map_err(internal_error)?;
    Ok(Json(TextResponse {
        status: "ok",
        text: "retry command sent".to_string(),
    }))
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

fn parse_conversation_command_target(conversation_key: &str) -> Option<(bool, i64)> {
    let mut parts = conversation_key.split(':');
    let scope = parts.next()?;
    let target = parts.next()?.parse::<i64>().ok()?;
    Some((scope == "group", target))
}

fn command_from_snapshot(
    snapshot: &TaskSnapshot,
    command: ControlCommand,
) -> Result<CommandRequest, (StatusCode, Json<ErrorResponse>)> {
    let conversation_key = snapshot
        .running_conversation_key
        .as_deref()
        .ok_or_else(|| bad_request("missing conversation context"))?;

    let (is_group, target) = parse_conversation_command_target(conversation_key)
        .ok_or_else(|| bad_request("missing conversation context"))?;

    Ok(CommandRequest {
        command,
        conversation_key: conversation_key.to_string(),
        reply_target_id: target,
        is_group,
    })
}

fn retry_command_from_snapshot(
    snapshot: &TaskSnapshot,
) -> Result<CommandRequest, (StatusCode, Json<ErrorResponse>)> {
    let conversation_key = snapshot
        .running_conversation_key
        .as_deref()
        .or(snapshot.last_retryable_conversation_key.as_deref())
        .ok_or_else(|| bad_request("missing retryable conversation context"))?;

    let (is_group, target) = parse_conversation_command_target(conversation_key)
        .ok_or_else(|| bad_request("missing retryable conversation context"))?;

    Ok(CommandRequest {
        command: ControlCommand::RetryLast,
        conversation_key: conversation_key.to_string(),
        reply_target_id: target,
        is_group,
    })
}
