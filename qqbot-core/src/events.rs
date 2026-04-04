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

/// Normalized event variants exposed by the Rust bridge.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum NormalizedEvent {
    /// Incoming private message.
    PrivateMessageReceived(PrivateMessageEvent),
    /// Incoming group message.
    GroupMessageReceived(GroupMessageEvent),
}

impl TryFrom<Value> for NormalizedEvent {
    type Error = NormalizeEventError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let Some(post_type) = value.get("post_type").and_then(Value::as_str) else {
            return Err(NormalizeEventError::Unsupported);
        };
        if post_type != "message" {
            return Err(NormalizeEventError::Unsupported);
        }

        let Some(message_type) = value.get("message_type").and_then(Value::as_str) else {
            return Err(NormalizeEventError::Unsupported);
        };
        let sender_id = extract_i64(&value, "user_id")?;
        let self_id = extract_i64(&value, "self_id")?;
        let mentions = extract_mentions(&value);
        let mentions_self = mentions.contains(&self_id);
        let text = extract_text(&value);

        match message_type {
            "private" => Ok(Self::PrivateMessageReceived(PrivateMessageEvent {
                sender_id,
                self_id,
                text,
                mentions,
                mentions_self,
                raw: value,
            })),
            "group" => Ok(Self::GroupMessageReceived(GroupMessageEvent {
                group_id: extract_i64(&value, "group_id")?,
                sender_id,
                self_id,
                text,
                mentions,
                mentions_self,
                raw: value,
            })),
            _ => Err(NormalizeEventError::Unsupported),
        }
    }
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

fn extract_text(value: &Value) -> String {
    if let Some(text) = value
        .get("message")
        .and_then(Value::as_array)
        .map(|segments| {
            segments
                .iter()
                .filter_map(|segment| {
                    if segment.get("type").and_then(Value::as_str) != Some("text") {
                        return None;
                    }
                    segment
                        .get("data")
                        .and_then(|data| data.get("text"))
                        .and_then(Value::as_str)
                })
                .collect::<String>()
        })
        .filter(|text| !text.trim().is_empty())
    {
        return text.trim().to_string();
    }

    value
        .get("raw_message")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}
