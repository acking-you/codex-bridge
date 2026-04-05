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
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

use crate::{
    config::RuntimeConfig,
    events::NormalizedEvent,
    runtime::RuntimeTokens,
    service::{
        FriendProfile, GroupProfile, SendMessageReceipt, ServiceCommand, ServiceState,
        SessionSnapshot, SessionStatus,
    },
};

#[path = "message_router.rs"]
pub mod message_router;

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
            return Ok(Self::Response {
                echo: echo.to_string(),
                payload: value,
            });
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

/// Logged-in QQ identity returned from the bootstrap action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginIdentity {
    /// Logged-in QQ identifier.
    pub self_id: i64,
    /// Logged-in QQ nickname.
    pub nickname: String,
}

/// Hash a WebUI token with the same rule used by NapCat WebUI login.
pub fn webui_password_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hasher.update(b".napcat");
    format!("{:x}", hasher.finalize())
}

/// Wait for WebUI, authenticate, bootstrap the session, and consume commands.
pub async fn run_bridge_loop(
    config: RuntimeConfig,
    _tokens: RuntimeTokens,
    state: ServiceState,
    mut command_rx: mpsc::Receiver<ServiceCommand>,
) -> Result<()> {
    state
        .set_session(SessionSnapshot {
            status: SessionStatus::WaitingForLogin,
            ..SessionSnapshot::default()
        })
        .await;

    let (client, mut event_rx) = NapCatClient::connect(&config).await?;
    let identity = wait_for_login_identity(&client).await?;
    let previous = state.session().await;
    state
        .set_session(SessionSnapshot {
            status: SessionStatus::Connected,
            self_id: Some(identity.self_id),
            nickname: Some(identity.nickname.clone()),
            qq_pid: previous.qq_pid,
        })
        .await;

    if let Ok(friends) = client.get_friend_list().await {
        state.set_friends(friends).await;
    }
    if let Ok(groups) = client.get_group_list().await {
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
    ) -> Result<(Self, mpsc::Receiver<NormalizedEvent>)> {
        let ws_url = format!("ws://{}:{}/", config.websocket_host, config.websocket_port);
        let stream = loop {
            match connect_async(ws_url.as_str()).await {
                Ok((stream, _)) => break stream,
                Err(_) => sleep(Duration::from_secs(1)).await,
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

        Ok((
            Self {
                sender: action_tx,
                pending,
            },
            event_rx,
        ))
    }

    async fn get_login_info(&self) -> Result<LoginIdentity> {
        let raw: RawLoginInfo = self.call_action("get_login_info", json!({})).await?;
        Ok(LoginIdentity {
            self_id: parse_i64_value(&raw.user_id)?,
            nickname: raw.nickname,
        })
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
        Ok(SendMessageReceipt {
            message_id: parse_i64_value(&raw.message_id)?,
        })
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
        Ok(SendMessageReceipt {
            message_id: parse_i64_value(&raw.message_id)?,
        })
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
            Err(_) => sleep(Duration::from_secs(1)).await,
        }
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
