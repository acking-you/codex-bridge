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
use tracing::{debug, info};

use crate::{
    conversation_history::{HistoryQuery, HistoryQueryResult},
    lane_manager::{LaneRuntimeState, RuntimeSnapshot},
    message_router::{CommandRequest, ControlCommand},
    model_capability::{CapabilityInput, CapabilityOutput},
    outbound::{build_outbound_message, ReplyRequest},
    service::{SendMessageReceipt, ServiceState},
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
        .route("/api/reply", post(reply_handler))
        .route("/api/history/query", post(history_query_handler))
        .route("/api/capability/invoke", post(capability_invoke_handler))
        .route("/api/capability/reload", post(capability_reload_handler))
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct StatusResponse {
    status: &'static str,
    snapshot: RuntimeSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct HistoryQueryRequest {
    token: String,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    keyword: Option<String>,
    #[serde(default)]
    sender_name: Option<String>,
    #[serde(default)]
    start_time: Option<i64>,
    #[serde(default)]
    end_time: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct HistoryQueryResponse {
    status: &'static str,
    result: HistoryQueryResult,
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
) -> Result<Json<StatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    Ok(Json(StatusResponse {
        status: "ok",
        snapshot: state.runtime_snapshot().await,
    }))
}

async fn queue_handler(
    State(state): State<ServiceState>,
) -> Result<Json<TextResponse>, (StatusCode, Json<ErrorResponse>)> {
    let snapshot = state.runtime_snapshot().await;
    let text = format!(
        "等待中的会话：{}，待处理 turn：{}",
        snapshot.ready_lane_count, snapshot.total_pending_turn_count
    );
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
    let snapshot = state.runtime_snapshot().await;
    let command = command_from_runtime_snapshot(&snapshot, ControlCommand::Cancel)?;
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
    let snapshot = state.runtime_snapshot().await;
    let command = retry_command_from_runtime_snapshot(&snapshot)?;
    state
        .send_control_command(command)
        .await
        .map_err(internal_error)?;
    Ok(Json(TextResponse {
        status: "ok",
        text: "retry command sent".to_string(),
    }))
}

async fn reply_handler(
    State(state): State<ServiceState>,
    Json(payload): Json<ReplyRequest>,
) -> Result<Json<SendMessageResponse>, (StatusCode, Json<ErrorResponse>)> {
    let token = payload.token.clone();
    let context = state
        .reply_context(token.as_str())
        .await
        .map_err(|error| bad_request(error.to_string().as_str()))?;
    let (reply_payload, at_targets, reply_to) = payload
        .into_payload(&context)
        .map_err(|error| bad_request(error.to_string().as_str()))?;
    let outbound = build_outbound_message(&context, reply_payload, &at_targets, reply_to);
    info!(
        conversation = %context.conversation_key,
        token = %token,
        is_group = context.is_group,
        segment_count = outbound.segments.len(),
        "received skill reply callback"
    );
    let receipt = state
        .send_outbound_message(outbound)
        .await
        .map_err(internal_error)?;
    state
        .mark_reply_sent(token.as_str())
        .await
        .map_err(internal_error)?;
    Ok(Json(SendMessageResponse {
        status: "ok",
        receipt,
    }))
}

async fn history_query_handler(
    State(state): State<ServiceState>,
    Json(payload): Json<HistoryQueryRequest>,
) -> Result<Json<HistoryQueryResponse>, (StatusCode, Json<ErrorResponse>)> {
    if payload.token.trim().is_empty() {
        return Err(bad_request("token must not be empty"));
    }
    let result = state
        .query_current_conversation_history(payload.token.as_str(), HistoryQuery {
            query: payload.query,
            keyword: payload.keyword,
            sender_name: payload.sender_name,
            start_time: payload.start_time,
            end_time: payload.end_time,
            limit: payload.limit.unwrap_or(50),
        })
        .await
        .map_err(internal_error)?;
    Ok(Json(HistoryQueryResponse {
        status: "ok",
        result,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct CapabilityInvokeRequest {
    id: String,
    prompt: String,
    #[serde(default)]
    system: Option<String>,
    #[serde(default)]
    max_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CapabilityInvokeResponse {
    Text { id: String, text: String },
    Image { id: String, path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CapabilityReloadResponse {
    status: &'static str,
    capability_count: usize,
}

async fn capability_invoke_handler(
    State(state): State<ServiceState>,
    Json(payload): Json<CapabilityInvokeRequest>,
) -> Result<Json<CapabilityInvokeResponse>, (StatusCode, Json<ErrorResponse>)> {
    if payload.id.trim().is_empty() {
        return Err(bad_request("capability id must not be empty"));
    }
    let registry = state.capabilities().await;
    let Some(capability) = registry.get(payload.id.as_str()) else {
        return Err(bad_request(format!("unknown capability id: {}", payload.id).as_str()));
    };
    let input = CapabilityInput {
        prompt: payload.prompt,
        system: payload.system,
        max_tokens: payload.max_tokens,
    };
    debug!(
        capability = capability.id(),
        prompt_len = input.prompt.len(),
        system_len = input.system.as_deref().map(str::len).unwrap_or(0),
        "invoking model capability"
    );
    match capability.invoke(&input).await {
        Ok(CapabilityOutput::Text {
            text,
        }) => Ok(Json(CapabilityInvokeResponse::Text {
            id: capability.id().to_string(),
            text,
        })),
        Ok(CapabilityOutput::Image {
            path,
        }) => Ok(Json(CapabilityInvokeResponse::Image {
            id: capability.id().to_string(),
            path: path.to_string_lossy().into_owned(),
        })),
        Err(error) => {
            info!(
                capability = capability.id(),
                %error,
                "model capability returned an error"
            );
            Err((
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: error.to_string(),
                }),
            ))
        },
    }
}

async fn capability_reload_handler(
    State(state): State<ServiceState>,
) -> Result<Json<CapabilityReloadResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.reload_capabilities().await {
        Ok(capability_count) => {
            info!(capability_count, "model capabilities reloaded");
            Ok(Json(CapabilityReloadResponse {
                status: "ok",
                capability_count,
            }))
        },
        Err(error) => {
            info!(%error, "model capabilities reload failed");
            Err(bad_request(error.to_string().as_str()))
        },
    }
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

fn command_from_runtime_snapshot(
    snapshot: &RuntimeSnapshot,
    command: ControlCommand,
) -> Result<CommandRequest, (StatusCode, Json<ErrorResponse>)> {
    let running = snapshot
        .lanes
        .iter()
        .filter(|lane| lane.state == LaneRuntimeState::Running)
        .collect::<Vec<_>>();
    let [lane] = running.as_slice() else {
        return Err(bad_request(
            "cancel requires exactly one running lane; use the in-chat /cancel command for \
             lane-scoped control",
        ));
    };
    let conversation_key = lane.conversation_key.as_str();

    let (is_group, target) = parse_conversation_command_target(conversation_key)
        .ok_or_else(|| bad_request("missing conversation context"))?;

    Ok(CommandRequest {
        command,
        conversation_key: conversation_key.to_string(),
        reply_target_id: target,
        is_group,
        source_message_id: 0,
        source_sender_id: 0,
        source_sender_name: "local-cli".to_string(),
    })
}

fn retry_command_from_runtime_snapshot(
    snapshot: &RuntimeSnapshot,
) -> Result<CommandRequest, (StatusCode, Json<ErrorResponse>)> {
    let running = snapshot
        .lanes
        .iter()
        .filter(|lane| lane.state == LaneRuntimeState::Running)
        .collect::<Vec<_>>();
    let conversation_key = match running.as_slice() {
        [lane] => lane.conversation_key.as_str(),
        [] => snapshot
            .last_retryable_conversation_key
            .as_deref()
            .ok_or_else(|| bad_request("missing retryable conversation context"))?,
        _ => {
            return Err(bad_request(
                "retry-last requires one running lane or one recorded retry candidate",
            ));
        },
    };

    let (is_group, target) = parse_conversation_command_target(conversation_key)
        .ok_or_else(|| bad_request("missing retryable conversation context"))?;

    Ok(CommandRequest {
        command: ControlCommand::RetryLast,
        conversation_key: conversation_key.to_string(),
        reply_target_id: target,
        is_group,
        source_message_id: 0,
        source_sender_id: 0,
        source_sender_name: "local-cli".to_string(),
    })
}
