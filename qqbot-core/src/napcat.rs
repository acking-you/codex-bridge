//! Internal NapCat transport helpers.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{sync::mpsc, time::sleep};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

use crate::{
    config::RuntimeConfig,
    events::NormalizedEvent,
    runtime::RuntimeTokens,
    service::{
        FriendProfile, GroupProfile, SendMessageReceipt, ServiceCommand, ServiceState,
        SessionSnapshot, SessionStatus,
    },
};

/// Logged-in QQ identity returned from the bridge bootstrap.
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
    tokens: RuntimeTokens,
    state: ServiceState,
    mut command_rx: mpsc::Receiver<ServiceCommand>,
) -> Result<()> {
    state
        .set_session(SessionSnapshot {
            status: SessionStatus::WaitingForLogin,
            ..SessionSnapshot::default()
        })
        .await;

    let client = NapCatClient::connect(&config, tokens.webui_token.as_str()).await?;
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

    let event_client = client.clone();
    let event_state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = event_client.event_listener_loop(event_state).await {
            eprintln!("event listener stopped: {error:#}");
        }
    });

    while let Some(command) = command_rx.recv().await {
        match command {
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
        }
    }

    Ok(())
}

async fn wait_for_login_identity(client: &NapCatClient) -> Result<LoginIdentity> {
    loop {
        match client.get_login_info().await {
            Ok(identity) => return Ok(identity),
            Err(_) => sleep(Duration::from_secs(1)).await,
        }
    }
}

#[derive(Debug, Clone)]
struct NapCatClient {
    http: Client,
    base_http_url: String,
    base_ws_url: String,
    credential: String,
}

impl NapCatClient {
    async fn connect(config: &RuntimeConfig, webui_token: &str) -> Result<Self> {
        let base_http_url = format!("http://{}:{}", config.webui_host, config.webui_port);
        let base_ws_url = format!("ws://{}:{}", config.webui_host, config.webui_port);
        let http = Client::builder().build()?;
        wait_for_webui_ready(&http, base_http_url.as_str()).await?;
        let credential = login(http.clone(), base_http_url.as_str(), webui_token).await?;
        Ok(Self {
            http,
            base_http_url,
            base_ws_url,
            credential,
        })
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
            message_id: raw.message_id,
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
            message_id: raw.message_id,
        })
    }

    async fn event_listener_loop(&self, state: ServiceState) -> Result<()> {
        loop {
            let session = self.create_debug_session().await?;
            let ws_url =
                Url::parse_with_params(format!("{}/api/Debug/ws", self.base_ws_url).as_str(), [
                    ("adapterName", session.adapter_name.as_str()),
                    ("token", session.token.as_str()),
                ])?;
            let (mut stream, _) = connect_async(ws_url.as_str())
                .await
                .context("connect debug websocket")?;
            while let Some(frame) = stream.next().await {
                let frame = frame.context("read debug websocket frame")?;
                let Message::Text(text) = frame else {
                    if matches!(frame, Message::Close(_)) {
                        break;
                    }
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                let Ok(event) = NormalizedEvent::try_from(value) else {
                    continue;
                };
                state.publish_event(event);
            }
            sleep(Duration::from_secs(1)).await;
        }
    }

    async fn create_debug_session(&self) -> Result<DebugSession> {
        let envelope: ApiEnvelope<DebugSession> = self
            .http
            .post(format!("{}/api/Debug/create", self.base_http_url))
            .header("Authorization", format!("Bearer {}", self.credential))
            .json(&json!({}))
            .send()
            .await
            .context("create debug session request")?
            .json()
            .await
            .context("decode debug session response")?;
        if envelope.code != 0 {
            bail!("create debug session failed: {}", envelope.message);
        }
        Ok(envelope.data)
    }

    async fn call_action<T>(&self, action: &str, params: Value) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let envelope: ApiEnvelope<OneBotResponse<T>> = self
            .http
            .post(format!("{}/api/Debug/call", self.base_http_url))
            .header("Authorization", format!("Bearer {}", self.credential))
            .json(&json!({
                "action": action,
                "params": params,
            }))
            .send()
            .await
            .with_context(|| format!("call action {action}"))?
            .json()
            .await
            .with_context(|| format!("decode action {action} response"))?;
        if envelope.code != 0 {
            bail!("{action} failed: {}", envelope.message);
        }
        if envelope.data.retcode != 0 || envelope.data.status != "ok" {
            let detail = if !envelope.data.message.is_empty() {
                envelope.data.message
            } else {
                envelope
                    .data
                    .wording
                    .unwrap_or_else(|| "unknown action error".to_string())
            };
            bail!("{action} failed: {detail}");
        }
        Ok(envelope.data.data)
    }
}

async fn wait_for_webui_ready(http: &Client, base_http_url: &str) -> Result<()> {
    loop {
        match http.get(format!("{base_http_url}/")).send().await {
            Ok(_) => return Ok(()),
            Err(_) => sleep(Duration::from_secs(1)).await,
        }
    }
}

async fn login(http: Client, base_http_url: &str, webui_token: &str) -> Result<String> {
    let envelope: ApiEnvelope<LoginEnvelope> = http
        .post(format!("{base_http_url}/api/auth/login"))
        .json(&json!({
            "hash": webui_password_hash(webui_token),
        }))
        .send()
        .await
        .context("login to WebUI")?
        .json()
        .await
        .context("decode login response")?;
    if envelope.code != 0 {
        bail!("webui login failed: {}", envelope.message);
    }
    Ok(envelope.data.credential)
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
struct ApiEnvelope<T> {
    code: i32,
    message: String,
    data: T,
}

#[derive(Debug, Deserialize)]
struct LoginEnvelope {
    #[serde(rename = "Credential")]
    credential: String,
}

#[derive(Debug, Deserialize)]
struct DebugSession {
    #[serde(rename = "adapterName")]
    adapter_name: String,
    token: String,
}

#[derive(Debug, Deserialize)]
struct OneBotResponse<T> {
    status: String,
    retcode: i32,
    data: T,
    message: String,
    wording: Option<String>,
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
    message_id: i64,
}
