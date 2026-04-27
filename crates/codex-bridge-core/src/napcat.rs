//! Internal NapCat transport helpers.

use std::{collections::HashMap, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{
    sync::{mpsc, oneshot, Mutex},
    time::sleep,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    config::RuntimeConfig,
    conversation_history::{apply_history_query, HistoryMessage, HistoryQuery, HistoryQueryResult},
    events::NormalizedEvent,
    outbound::{OutboundMessage, OutboundSegment, OutboundTarget},
    runtime::RuntimeTokens,
    service::{
        FriendProfile, GroupProfile, SendMessageReceipt, ServiceCommand, ServiceState,
        SessionSnapshot, SessionStatus,
    },
};

/// OneBot websocket frame consumed by the bridge worker.
#[derive(Debug)]
pub enum IncomingFrame {
    /// Incoming event payload produced by OneBot.
    Event(NormalizedEvent),
    /// OneBot action response payload associated with `echo`.
    Response {
        /// Echo value used to match the originating action.
        echo: String,
        /// Full response payload.
        payload: Value,
    },
}

impl IncomingFrame {
    /// Parse a raw websocket payload into an event or response frame.
    pub fn from_value(value: Value) -> Result<Self> {
        if let Some(echo) = value.get("echo").and_then(Value::as_str) {
            return Ok(Self::Response { echo: echo.to_string(), payload: value });
        }

        let event = NormalizedEvent::try_from(value)?;
        Ok(Self::Event(event))
    }
}

/// Build an action request frame for OneBot websocket calls.
pub fn build_action_frame(action: &str, params: Value, echo: &str) -> Value {
    json!({
        "action": action,
        "params": params,
        "echo": echo,
    })
}

/// Build the formal OneBot action and params for one structured outbound
/// message.
pub fn build_outbound_action(message: &OutboundMessage) -> (&'static str, Value) {
    let payload = Value::Array(
        message
            .segments
            .iter()
            .map(build_outbound_segment)
            .collect::<Vec<_>>(),
    );

    match message.target {
        OutboundTarget::Private(user_id) => (
            "send_private_msg",
            json!({
                "user_id": user_id.to_string(),
                "message": payload,
            }),
        ),
        OutboundTarget::Group(group_id) => (
            "send_group_msg",
            json!({
                "group_id": group_id.to_string(),
                "message": payload,
            }),
        ),
    }
}

/// Build `set_msg_emoji_like` params for one source message.
pub fn build_set_msg_emoji_like_params(message_id: i64, emoji_id: &str) -> Value {
    json!({
        "message_id": message_id.to_string(),
        "emoji_id": emoji_id,
        "set": true,
    })
}

/// Logged-in QQ identity returned from the bootstrap action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginIdentity {
    /// Logged-in QQ identifier.
    pub self_id: i64,
    /// Logged-in QQ nickname.
    pub nickname: String,
}

/// Historical QQ message fetched through the OneBot `get_msg` action,
/// used by the orchestrator to present quoted context to the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedMessage {
    /// Original QQ message identifier.
    pub message_id: i64,
    /// QQ identifier of the person who sent the quoted message.
    pub sender_id: i64,
    /// Display name of the sender at fetch time (falls back to `unknown`).
    pub sender_name: String,
    /// Placeholder-preserving text rendering of the quoted message.
    pub text: String,
}

/// Hash a WebUI token with the same rule used by NapCat WebUI login.
pub fn webui_password_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hasher.update(b".napcat");
    format!("{:x}", hasher.finalize())
}

/// Build the authenticated websocket request used for the formal OneBot
/// channel.
pub fn build_websocket_request(
    config: &RuntimeConfig,
    tokens: &RuntimeTokens,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request> {
    let ws_url = format!("ws://{}:{}/", config.websocket_host, config.websocket_port);
    let mut request = ws_url
        .into_client_request()
        .context("build websocket upgrade request")?;
    if !tokens.ws_token.is_empty() {
        request.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(format!("Bearer {}", tokens.ws_token).as_str())
                .context("build websocket authorization header")?,
        );
    }
    Ok(request)
}

/// Wait for WebUI, authenticate, bootstrap the session, and consume commands.
pub async fn run_bridge_loop(
    config: RuntimeConfig,
    tokens: RuntimeTokens,
    state: ServiceState,
    mut command_rx: mpsc::Receiver<ServiceCommand>,
) -> Result<()> {
    info!(
        websocket = %format!("ws://{}:{}/", config.websocket_host, config.websocket_port),
        "starting formal OneBot bridge loop"
    );
    state
        .set_session(SessionSnapshot {
            status: SessionStatus::WaitingForLogin,
            ..SessionSnapshot::default()
        })
        .await;

    let (client, mut event_rx) = NapCatClient::connect(&config, &tokens).await?;
    let identity = match wait_for_login_identity(&client).await {
        Ok(identity) => identity,
        Err(error) if is_disconnected_error(&error) => {
            state
                .set_session(SessionSnapshot {
                    status: SessionStatus::Disconnected,
                    ..state.session().await
                })
                .await;
            return Err(error);
        },
        Err(error) => return Err(error),
    };
    let previous = state.session().await;
    info!(
        self_id = identity.self_id,
        nickname = %identity.nickname,
        "formal OneBot bridge connected"
    );
    state
        .set_session(SessionSnapshot {
            status: SessionStatus::Connected,
            self_id: Some(identity.self_id),
            nickname: Some(identity.nickname.clone()),
            qq_pid: previous.qq_pid,
        })
        .await;

    if let Ok(friends) = client.get_friend_list().await {
        info!(count = friends.len(), "loaded friend list");
        state.set_friends(friends).await;
    }
    if let Ok(groups) = client.get_group_list().await {
        info!(count = groups.len(), "loaded group list");
        state.set_groups(groups).await;
    }

    let event_state = state.clone();
    loop {
        tokio::select! {
            event = event_rx.recv() => {
                match event {
                    Some(event) => {
                        event_state.publish_event(event);
                    },
                    None => {
                        let previous = state.session().await;
                        state.set_session(SessionSnapshot {
                            status: SessionStatus::Disconnected,
                            self_id: previous.self_id,
                            nickname: previous.nickname,
                            qq_pid: previous.qq_pid,
                        }).await;
                        return Ok(());
                    },
                }
            },
            command = command_rx.recv() => {
                match command {
                    Some(command) => match command {
                        ServiceCommand::SendPrivate {
                            user_id,
                            text,
                            respond_to,
                        } => {
                            let _ = respond_to.send(client.send_private_message(user_id, text).await);
                        },
                        ServiceCommand::SendGroup {
                            group_id,
                            text,
                            respond_to,
                        } => {
                            let _ = respond_to.send(client.send_group_message(group_id, text).await);
                        },
                        ServiceCommand::SendOutbound {
                            message,
                            respond_to,
                        } => {
                            let _ = respond_to.send(client.send_outbound_message(message).await);
                        },
                        ServiceCommand::SetMessageReaction {
                            message_id,
                            emoji_id,
                            respond_to,
                        } => {
                            let _ = respond_to.send(client.set_message_reaction(message_id, emoji_id).await);
                        },
                        ServiceCommand::FetchMessage {
                            message_id,
                            self_id,
                            respond_to,
                        } => {
                            let _ = respond_to.send(client.get_msg(message_id, self_id).await);
                        },
                        ServiceCommand::FetchConversationHistory {
                            is_group,
                            target_id,
                            self_id,
                            query,
                            respond_to,
                        } => {
                            let result = if is_group {
                                client.get_group_history(target_id, self_id, query).await
                            } else {
                                client.get_friend_history(target_id, self_id, query).await
                            };
                            let _ = respond_to.send(result);
                        },
                        ServiceCommand::Control { .. } => {},
                    },
                    None => {
                        let previous = state.session().await;
                        state.set_session(SessionSnapshot {
                            status: SessionStatus::Disconnected,
                            self_id: previous.self_id,
                            nickname: previous.nickname,
                            qq_pid: previous.qq_pid,
                        }).await;
                        return Ok(());
                    },
                }
            },
        }
    }
}

#[derive(Debug, Clone)]
struct NapCatClient {
    sender: mpsc::UnboundedSender<Value>,
    pending: std::sync::Arc<tokio::sync::Mutex<HashMap<String, oneshot::Sender<Result<Value>>>>>,
}

impl NapCatClient {
    /// Connect a single OneBot websocket and split message streams for
    /// actions/events.
    pub async fn connect(
        config: &RuntimeConfig,
        tokens: &RuntimeTokens,
    ) -> Result<(Self, mpsc::Receiver<NormalizedEvent>)> {
        let request = build_websocket_request(config, tokens)?;
        let ws_url = format!("ws://{}:{}/", config.websocket_host, config.websocket_port);
        let mut attempt = 0_u64;
        let stream = loop {
            match connect_async(request.clone()).await {
                Ok((stream, _)) => break stream,
                Err(error) => {
                    attempt += 1;
                    if attempt == 1 || attempt.is_multiple_of(10) {
                        warn!(
                            attempt,
                            websocket = %ws_url,
                            error = %error,
                            "waiting for formal OneBot websocket"
                        );
                    }
                    sleep(Duration::from_secs(1)).await;
                },
            }
        };

        let (mut writer, mut reader) = stream.split();
        let (event_tx, event_rx) = mpsc::channel(256);
        let (action_tx, mut action_rx) = mpsc::unbounded_channel::<Value>();
        let pending = std::sync::Arc::new(Mutex::new(HashMap::<
            String,
            oneshot::Sender<Result<Value>>,
        >::new()));

        let reader_pending = pending.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(frame) = action_rx.recv() => {
                        if writer.send(Message::Text(frame.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    frame = reader.next() => {
                        match frame {
                            Some(Ok(Message::Text(text))) => {
                                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                                    continue;
                                };
                                let Ok(frame) = IncomingFrame::from_value(value) else {
                                    continue;
                                };
                                match frame {
                                    IncomingFrame::Event(event) => {
                                        let _ = event_tx.send(event).await;
                                    }
                                    IncomingFrame::Response { echo, payload } => {
                                        let sender = reader_pending.lock().await.remove(&echo);
                                        if let Some(sender) = sender {
                                            let _ = sender.send(Ok(payload));
                                        }
                                    }
                                }
                            }
                            Some(Ok(Message::Close(_))) | None => {
                                break;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) => {
                                break;
                            }
                        }
                    }
                }
            }
            let mut pending = reader_pending.lock().await;
            for (_echo, responder) in pending.drain() {
                let _ = responder.send(Err(anyhow!("websocket disconnected")));
            }
        });

        Ok((Self { sender: action_tx, pending }, event_rx))
    }

    async fn get_login_info(&self) -> Result<LoginIdentity> {
        let raw: RawLoginInfo = self.call_action("get_login_info", json!({})).await?;
        Ok(LoginIdentity { self_id: parse_i64_value(&raw.user_id)?, nickname: raw.nickname })
    }

    async fn get_friend_list(&self) -> Result<Vec<FriendProfile>> {
        let rows: Vec<RawFriendProfile> = self.call_action("get_friend_list", json!({})).await?;
        rows.into_iter()
            .map(|row| {
                Ok(FriendProfile {
                    user_id: parse_i64_value(&row.user_id)?,
                    nickname: row.nickname,
                    remark: row.remark.filter(|value| !value.trim().is_empty()),
                })
            })
            .collect()
    }

    async fn get_group_list(&self) -> Result<Vec<GroupProfile>> {
        let rows: Vec<RawGroupProfile> = self.call_action("get_group_list", json!({})).await?;
        rows.into_iter()
            .map(|row| {
                Ok(GroupProfile {
                    group_id: parse_i64_value(&row.group_id)?,
                    group_name: row.group_name,
                })
            })
            .collect()
    }

    async fn send_private_message(&self, user_id: i64, text: String) -> Result<SendMessageReceipt> {
        let raw: RawSendReceipt = self
            .call_action(
                "send_private_msg",
                json!({
                    "user_id": user_id.to_string(),
                    "message": text,
                }),
            )
            .await?;
        Ok(SendMessageReceipt { message_id: parse_i64_value(&raw.message_id)? })
    }

    async fn send_group_message(&self, group_id: i64, text: String) -> Result<SendMessageReceipt> {
        let raw: RawSendReceipt = self
            .call_action(
                "send_group_msg",
                json!({
                    "group_id": group_id.to_string(),
                    "message": text,
                }),
            )
            .await?;
        Ok(SendMessageReceipt { message_id: parse_i64_value(&raw.message_id)? })
    }

    async fn send_outbound_message(&self, message: OutboundMessage) -> Result<SendMessageReceipt> {
        let (action, params) = build_outbound_action(&message);
        let raw: RawSendReceipt = self.call_action(action, params).await?;
        Ok(SendMessageReceipt { message_id: parse_i64_value(&raw.message_id)? })
    }

    async fn set_message_reaction(&self, message_id: i64, emoji_id: String) -> Result<()> {
        let _: Value = self
            .call_action(
                "set_msg_emoji_like",
                build_set_msg_emoji_like_params(message_id, emoji_id.as_str()),
            )
            .await?;
        Ok(())
    }

    /// Fetch one historical QQ message via OneBot `get_msg` and render it
    /// into the crate's standard placeholder-preserving form.
    async fn get_msg(&self, message_id: i64, self_id: i64) -> Result<FetchedMessage> {
        let raw: Value = self
            .call_action(
                "get_msg",
                json!({
                    "message_id": message_id.to_string(),
                }),
            )
            .await?;
        let sender_id = raw
            .get("sender")
            .and_then(|sender| sender.get("user_id"))
            .and_then(|value| parse_i64_value(value).ok())
            .unwrap_or(0);
        let sender_name = raw
            .get("sender")
            .and_then(|sender| sender.get("nickname"))
            .and_then(|value| value.as_str())
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        let text = crate::events::extract_text(&raw, self_id);
        Ok(FetchedMessage { message_id, sender_id, sender_name, text })
    }

    async fn get_group_history(
        &self,
        group_id: i64,
        self_id: i64,
        query: HistoryQuery,
    ) -> Result<HistoryQueryResult> {
        let raw: Value = self
            .call_action(
                "get_group_msg_history",
                json!({
                    "group_id": group_id.to_string(),
                    "count": query.effective_limit(),
                }),
            )
            .await?;
        normalize_history_result(&raw, self_id, &query)
    }

    async fn get_friend_history(
        &self,
        user_id: i64,
        self_id: i64,
        query: HistoryQuery,
    ) -> Result<HistoryQueryResult> {
        let raw: Value = self
            .call_action(
                "get_friend_msg_history",
                json!({
                    "user_id": user_id.to_string(),
                    "count": query.effective_limit(),
                }),
            )
            .await?;
        normalize_history_result(&raw, self_id, &query)
    }

    async fn call_action<T>(&self, action: &str, params: Value) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let raw = self.dispatch_action(action, params).await?;
        let envelope: OneBotResponse<T> =
            serde_json::from_value(raw).context("decode action response")?;
        if envelope.status != "ok" || envelope.retcode != 0 {
            let detail = if !envelope.message.trim().is_empty() {
                envelope.message
            } else {
                envelope
                    .wording
                    .unwrap_or_else(|| "unknown action error".to_string())
            };
            bail!("{action} failed: {detail}");
        }
        Ok(envelope.data)
    }

    async fn dispatch_action(&self, action: &str, params: Value) -> Result<Value> {
        let echo = Uuid::new_v4().to_string();
        let payload = build_action_frame(action, params, echo.as_str());

        let (respond_to, response_rx) = oneshot::channel::<Result<Value>>();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(echo.clone(), respond_to);
        }

        if self.sender.send(payload).is_err() {
            let mut pending = self.pending.lock().await;
            let _ = pending.remove(&echo);
            bail!("websocket send channel closed");
        }

        match response_rx.await {
            Ok(result) => result,
            Err(_) => bail!("action response dropped for {action}"),
        }
    }
}

fn normalize_history_result(
    raw: &Value,
    self_id: i64,
    query: &HistoryQuery,
) -> Result<HistoryQueryResult> {
    let messages = raw
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|message| normalize_history_message(&message, self_id))
        .collect::<Vec<_>>();
    Ok(apply_history_query(messages, query, query.effective_limit()))
}

fn normalize_history_message(message: &Value, self_id: i64) -> HistoryMessage {
    let message_id = message
        .get("message_id")
        .and_then(parse_i64_value_from_json)
        .or_else(|| message.get("id").and_then(parse_i64_value_from_json))
        .unwrap_or_default();
    let timestamp = message
        .get("time")
        .and_then(parse_i64_value_from_json)
        .unwrap_or_default();
    let sender_id = message
        .get("sender")
        .and_then(|sender| sender.get("user_id"))
        .and_then(parse_i64_value_from_json)
        .unwrap_or_default();
    let sender_name = message
        .get("sender")
        .and_then(|sender| sender.get("nickname"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("unknown")
        .to_string();
    HistoryMessage {
        message_id,
        timestamp,
        sender_id,
        sender_name,
        text: crate::events::extract_text(message, self_id),
    }
}

fn parse_i64_value_from_json(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|raw| raw.parse::<i64>().ok()))
}

#[derive(Debug, Serialize, Deserialize)]
struct OneBotResponse<T> {
    status: String,
    retcode: i32,
    data: T,
    message: String,
    wording: Option<String>,
}

async fn wait_for_login_identity(client: &NapCatClient) -> Result<LoginIdentity> {
    loop {
        match client.get_login_info().await {
            Ok(identity) => return Ok(identity),
            Err(error) if is_disconnected_error(&error) => return Err(error),
            Err(_) => sleep(Duration::from_secs(1)).await,
        }
    }
}

fn is_disconnected_error(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("websocket send channel closed")
        || message.contains("action response dropped for get_login_info")
        || message.contains("websocket disconnected")
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    #[test]
    fn disconnected_error_patterns_are_detected() {
        assert!(super::is_disconnected_error(&anyhow!("websocket send channel closed")));
        assert!(super::is_disconnected_error(&anyhow!(
            "action response dropped for get_login_info"
        )));
        assert!(super::is_disconnected_error(&anyhow!("websocket disconnected")));
        assert!(!super::is_disconnected_error(&anyhow!("temporary login error")));
    }
}

fn parse_i64_value(value: &Value) -> Result<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .ok_or_else(|| anyhow!("numeric identifier is out of range")),
        Value::String(text) => text
            .parse::<i64>()
            .with_context(|| format!("parse numeric identifier from {text}")),
        other => bail!("unsupported identifier value: {other}"),
    }
}

#[derive(Debug, Deserialize)]
struct RawLoginInfo {
    user_id: Value,
    nickname: String,
}

#[derive(Debug, Deserialize)]
struct RawFriendProfile {
    user_id: Value,
    nickname: String,
    remark: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawGroupProfile {
    group_id: Value,
    group_name: String,
}

#[derive(Debug, Deserialize)]
struct RawSendReceipt {
    message_id: Value,
}

fn build_outbound_segment(segment: &OutboundSegment) -> Value {
    match segment {
        OutboundSegment::Reply { message_id } => json!({
            "type": "reply",
            "data": {
                "id": message_id.to_string(),
            },
        }),
        OutboundSegment::At { user_id } => json!({
            "type": "at",
            "data": {
                "qq": user_id.to_string(),
            },
        }),
        OutboundSegment::Text { text } => json!({
            "type": "text",
            "data": {
                "text": text,
            },
        }),
        OutboundSegment::Image { path } => json!({
            "type": "image",
            "data": {
                "file": path.display().to_string(),
            },
        }),
        OutboundSegment::File { path, name } => json!({
            "type": "file",
            "data": {
                "file": path.display().to_string(),
                "name": name,
            },
        }),
    }
}
