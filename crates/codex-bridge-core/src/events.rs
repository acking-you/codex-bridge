//! Normalized message events produced from raw NapCat websocket payloads.

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

/// Error returned when a raw payload cannot be normalized.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NormalizeEventError {
    /// The payload does not represent a supported event type.
    #[error("unsupported event payload")]
    Unsupported,
}

/// A private-message event normalized for local consumers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrivateMessageEvent {
    /// Sender QQ identifier.
    pub sender_id: i64,
    /// Message identifier from OneBot event.
    pub message_id: i64,
    /// Sender display name.
    pub sender_name: String,
    /// Bot QQ identifier.
    pub self_id: i64,
    /// Plain-text projection of the message.
    pub text: String,
    /// Mentioned QQ identifiers extracted from message segments.
    pub mentions: Vec<i64>,
    /// Whether the message explicitly mentioned the bot account.
    pub mentions_self: bool,
    /// Raw JSON payload for debugging.
    pub raw: Value,
}

/// A group-message event normalized for local consumers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupMessageEvent {
    /// Target group identifier.
    pub group_id: i64,
    /// Sender QQ identifier.
    pub sender_id: i64,
    /// Message identifier from OneBot event.
    pub message_id: i64,
    /// Sender display name.
    pub sender_name: String,
    /// Bot QQ identifier.
    pub self_id: i64,
    /// Plain-text projection of the message.
    pub text: String,
    /// Mentioned QQ identifiers extracted from message segments.
    pub mentions: Vec<i64>,
    /// Whether the message explicitly mentioned the bot account.
    pub mentions_self: bool,
    /// Raw JSON payload for debugging.
    pub raw: Value,
}

/// A group-message reaction event normalized for local consumers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupMessageReactionEvent {
    /// Target group identifier.
    pub group_id: i64,
    /// QQ identifier of the reacting operator.
    pub operator_id: i64,
    /// Target message identifier from OneBot event.
    pub message_id: i64,
    /// Emoji identifier reported by the transport.
    pub emoji_id: String,
    /// Whether the reaction was added rather than removed.
    pub is_add: bool,
    /// Raw JSON payload for debugging.
    pub raw: Value,
}

/// Normalized event variants exposed by the Rust bridge.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum NormalizedEvent {
    /// Incoming private message.
    PrivateMessageReceived(PrivateMessageEvent),
    /// Incoming group message.
    GroupMessageReceived(GroupMessageEvent),
    /// Incoming group message reaction notice.
    GroupMessageReactionReceived(GroupMessageReactionEvent),
}

impl TryFrom<Value> for NormalizedEvent {
    type Error = NormalizeEventError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let Some(post_type) = value.get("post_type").and_then(Value::as_str) else {
            return Err(NormalizeEventError::Unsupported);
        };
        match post_type {
            "message" => normalize_message_event(value),
            "notice" => normalize_notice_event(value),
            _ => Err(NormalizeEventError::Unsupported),
        }
    }
}

fn normalize_message_event(value: Value) -> Result<NormalizedEvent, NormalizeEventError> {
    let Some(message_type) = value.get("message_type").and_then(Value::as_str) else {
        return Err(NormalizeEventError::Unsupported);
    };
    let sender_id = extract_i64(&value, "user_id")?;
    let self_id = extract_i64(&value, "self_id")?;
    let message_id = extract_i64(&value, "message_id")?;
    let mentions = extract_mentions(&value);
    let mentions_self = mentions.contains(&self_id);
    let text = extract_text(&value, self_id);
    let sender_name = extract_sender_name(&value);

    match message_type {
        "private" => Ok(NormalizedEvent::PrivateMessageReceived(PrivateMessageEvent {
            sender_id,
            message_id,
            sender_name,
            self_id,
            text,
            mentions,
            mentions_self,
            raw: value,
        })),
        "group" => Ok(NormalizedEvent::GroupMessageReceived(GroupMessageEvent {
            group_id: extract_i64(&value, "group_id")?,
            sender_id,
            message_id,
            sender_name,
            self_id,
            text,
            mentions,
            mentions_self,
            raw: value,
        })),
        _ => Err(NormalizeEventError::Unsupported),
    }
}

fn normalize_notice_event(value: Value) -> Result<NormalizedEvent, NormalizeEventError> {
    match value.get("notice_type").and_then(Value::as_str) {
        Some("group_msg_emoji_like") => Ok(NormalizedEvent::GroupMessageReactionReceived(
            GroupMessageReactionEvent {
                group_id: extract_i64(&value, "group_id")?,
                operator_id: extract_i64(&value, "user_id")?,
                message_id: extract_i64(&value, "message_id")?,
                emoji_id: extract_notice_emoji_id(&value)?,
                is_add: value.get("is_add").and_then(Value::as_bool).unwrap_or(true),
                raw: value,
            },
        )),
        Some("reaction") => Ok(NormalizedEvent::GroupMessageReactionReceived(
            GroupMessageReactionEvent {
                group_id: extract_i64(&value, "group_id")?,
                operator_id: extract_i64(&value, "operator_id")?,
                message_id: extract_i64(&value, "message_id")?,
                emoji_id: value
                    .get("code")
                    .and_then(Value::as_str)
                    .ok_or(NormalizeEventError::Unsupported)?
                    .to_string(),
                is_add: value.get("sub_type").and_then(Value::as_str) == Some("add"),
                raw: value,
            },
        )),
        _ => Err(NormalizeEventError::Unsupported),
    }
}

fn extract_notice_emoji_id(value: &Value) -> Result<String, NormalizeEventError> {
    value
        .get("likes")
        .and_then(Value::as_array)
        .and_then(|likes| likes.first())
        .and_then(|like| like.get("emoji_id"))
        .and_then(|emoji| {
            emoji
                .as_str()
                .map(ToString::to_string)
                .or_else(|| emoji.as_i64().map(|number| number.to_string()))
        })
        .ok_or(NormalizeEventError::Unsupported)
}

fn extract_i64(value: &Value, key: &str) -> Result<i64, NormalizeEventError> {
    match value.get(key) {
        Some(Value::Number(number)) => number.as_i64().ok_or(NormalizeEventError::Unsupported),
        Some(Value::String(text)) => text
            .parse::<i64>()
            .map_err(|_| NormalizeEventError::Unsupported),
        _ => Err(NormalizeEventError::Unsupported),
    }
}

fn extract_mentions(value: &Value) -> Vec<i64> {
    let mut mentions = Vec::new();
    let Some(segments) = value.get("message").and_then(Value::as_array) else {
        return mentions;
    };

    for segment in segments {
        let is_at = segment.get("type").and_then(Value::as_str) == Some("at");
        if !is_at {
            continue;
        }
        let Some(raw_qq) = segment
            .get("data")
            .and_then(|data| data.get("qq"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if let Ok(qq) = raw_qq.parse::<i64>() {
            mentions.push(qq);
        }
    }

    mentions
}

fn extract_sender_name(value: &Value) -> String {
    let sender = value.get("sender").or_else(|| value.get("sender_id"));
    if let Some(sender) = sender.and_then(Value::as_object) {
        for key in ["nickname", "card", "user_name", "name"] {
            let Some(text) = sender.get(key).and_then(Value::as_str) else {
                continue;
            };
            let normalized = text.trim();
            if !normalized.is_empty() {
                return normalized.to_string();
            }
        }
    }
    if let Some(text) = value.get("sender_name").and_then(Value::as_str) {
        let normalized = text.trim();
        if !normalized.is_empty() {
            return normalized.to_string();
        }
    }

    "unknown".to_string()
}

/// Render the message body as a flat string while preserving every `@`
/// segment in a deterministic, agent-readable form.
///
/// - `@bot` (the bot's own self_id) becomes the literal placeholder
///   `@<bot>` so the agent can recognise that it is being addressed without
///   leaking its raw QQ id.
/// - Any other `@user` becomes `@nickname<QQ:1234>` when the OneBot `at`
///   segment carries a `name` so the agent sees both the displayed name and
///   the underlying QQ id, or `@<QQ:1234>` when no name was supplied.
/// - Unknown segment types are dropped.
fn extract_text(value: &Value, self_id: i64) -> String {
    if let Some(segments) = value.get("message").and_then(Value::as_array) {
        let mut buf = String::new();
        for segment in segments {
            let kind = segment.get("type").and_then(Value::as_str);
            match kind {
                Some("text") => {
                    if let Some(text) = segment
                        .get("data")
                        .and_then(|data| data.get("text"))
                        .and_then(Value::as_str)
                    {
                        buf.push_str(text);
                    }
                },
                Some("at") => {
                    let data = segment.get("data");
                    let qq = data
                        .and_then(|data| data.get("qq"))
                        .and_then(Value::as_str)
                        .and_then(|raw| raw.parse::<i64>().ok());
                    let name = data
                        .and_then(|data| data.get("name"))
                        .and_then(Value::as_str)
                        .map(sanitize_at_name)
                        .filter(|name| !name.is_empty());
                    match qq {
                        Some(id) if id == self_id => buf.push_str("@<bot>"),
                        Some(id) => match name {
                            Some(name) => buf.push_str(&format!("@{name}<QQ:{id}>")),
                            None => buf.push_str(&format!("@<QQ:{id}>")),
                        },
                        None => {},
                    }
                },
                _ => {},
            }
        }
        return buf.trim().to_string();
    }

    let Some(raw_message) = value.get("raw_message").and_then(Value::as_str) else {
        return String::new();
    };
    raw_message.trim().to_string()
}

/// Strip characters that would confuse the `@nickname<QQ:...>` placeholder
/// parser (the angle brackets that delimit the metadata block) and trim any
/// surrounding whitespace.
fn sanitize_at_name(name: &str) -> String {
    name.replace(['<', '>'], "_").trim().to_string()
}
