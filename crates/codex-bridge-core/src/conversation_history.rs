//! Lane-scoped QQ conversation history query models.

use serde::{Deserialize, Serialize};

/// One normalized QQ history message returned by the bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMessage {
    /// QQ message id.
    pub message_id: i64,
    /// Unix timestamp in seconds.
    pub timestamp: i64,
    /// QQ sender id.
    pub sender_id: i64,
    /// Sender display name.
    pub sender_name: String,
    /// Placeholder-preserving rendered text body.
    pub text: String,
}

/// Lane-scoped history query options accepted by the bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryQuery {
    /// Free-form user intent carried through for simple text matching.
    pub query: Option<String>,
    /// Explicit keyword filter over message text.
    pub keyword: Option<String>,
    /// Optional sender-name filter.
    pub sender_name: Option<String>,
    /// Inclusive lower time bound as unix seconds.
    pub start_time: Option<i64>,
    /// Exclusive upper time bound as unix seconds.
    pub end_time: Option<i64>,
    /// Maximum number of messages the bridge should scan.
    pub limit: usize,
}

impl Default for HistoryQuery {
    fn default() -> Self {
        Self {
            query: None,
            keyword: None,
            sender_name: None,
            start_time: None,
            end_time: None,
            limit: 50,
        }
    }
}

/// Normalized history-query result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HistoryQueryResult {
    /// Matched messages in normalized bridge format.
    pub messages: Vec<HistoryMessage>,
    /// Whether the bridge hit the configured scan budget while querying.
    pub truncated: bool,
}

impl HistoryQuery {
    /// Return the effective scan limit, clamped to at least one message.
    pub fn effective_limit(&self) -> usize {
        self.limit.max(1)
    }
}

/// Filter one normalized history slice according to the query options.
pub fn apply_history_query(
    messages: Vec<HistoryMessage>,
    query: &HistoryQuery,
    scanned_limit: usize,
) -> HistoryQueryResult {
    let freeform = query
        .query
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let keyword = query
        .keyword
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .or(freeform);
    let sender_name = query
        .sender_name
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| text.to_ascii_lowercase());

    let messages = messages
        .into_iter()
        .filter(|message| {
            if let Some(start) = query.start_time {
                if message.timestamp < start {
                    return false;
                }
            }
            if let Some(end) = query.end_time {
                if message.timestamp >= end {
                    return false;
                }
            }
            if let Some(sender) = sender_name.as_deref() {
                if !message.sender_name.to_ascii_lowercase().contains(sender) {
                    return false;
                }
            }
            if let Some(keyword) = keyword {
                if !message.text.contains(keyword) && !message.sender_name.contains(keyword) {
                    return false;
                }
            }
            true
        })
        .collect::<Vec<_>>();

    HistoryQueryResult {
        truncated: messages.len() >= scanned_limit,
        messages,
    }
}
