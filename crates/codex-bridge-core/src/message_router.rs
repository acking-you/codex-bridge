//! Routing helpers for incoming normalized messages.

use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

use crate::events::{GroupMessageEvent, NormalizedEvent, PrivateMessageEvent};

/// Available local control commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlCommand {
    /// Return command help and trigger rules.
    Help,
    /// Return current bot status.
    Status {
        /// Optional explicit task identifier for admin inspection.
        task_id: Option<String>,
    },
    /// Return current queue status.
    Queue,
    /// Cancel current or specified tasks.
    Cancel,
    /// Retry latest failed task.
    RetryLast,
    /// Approve a pending task by task identifier.
    Approve {
        /// Stable task identifier.
        task_id: String,
    },
    /// Deny a pending task by task identifier.
    Deny {
        /// Stable task identifier.
        task_id: String,
    },
    /// Clear the current conversation binding.
    Clear,
    /// Start compaction for the current conversation thread.
    Compact,
}

/// Request object carrying metadata for task execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRequest {
    /// Stable conversation key for queueing and deduping.
    pub conversation_key: String,
    /// Source message identifier.
    pub source_message_id: i64,
    /// QQ identifier of the sender.
    pub source_sender_id: i64,
    /// Display name of the sender.
    pub source_sender_name: String,
    /// Text content after extraction/removal of mentions.
    pub source_text: String,
    /// Indicates whether the source was a group chat.
    pub is_group: bool,
    /// Conversation id for response routing.
    pub reply_target_id: i64,
}

/// Request object carrying metadata for command execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    /// Control command.
    pub command: ControlCommand,
    /// Stable conversation key for queueing and deduping.
    pub conversation_key: String,
    /// Conversation id for response routing.
    pub reply_target_id: i64,
    /// Indicates whether the source was a group chat.
    pub is_group: bool,
    /// Source message identifier.
    pub source_message_id: i64,
    /// QQ identifier of the sender.
    pub source_sender_id: i64,
    /// Display name of the sender.
    pub source_sender_name: String,
}

/// Routing decision for an incoming message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDecision {
    /// Execute a local control command.
    Command(CommandRequest),
    /// Dispatch message to queue/worker.
    Task(TaskRequest),
}

/// Deduplicates messages within a bounded message-id window.
#[derive(Debug)]
pub struct MessageDeduper {
    max_entries: usize,
    window: Duration,
    seen: HashMap<i64, Instant>,
    order: VecDeque<(i64, Instant)>,
}

impl MessageDeduper {
    /// Build a deduper with explicit window size and retention count.
    pub fn new(max_entries: usize, window: Duration) -> Self {
        Self {
            max_entries,
            window,
            seen: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Return `true` when the message id was not seen in the current window.
    pub fn is_new(&mut self, message_id: i64) -> bool {
        let now = Instant::now();
        self.purge_expired(now);
        if self.seen.contains_key(&message_id) {
            return false;
        }
        self.insert(message_id, now);
        true
    }

    fn purge_expired(&mut self, now: Instant) {
        while let Some((message_id, seen_at)) = self.order.front().cloned() {
            if now.duration_since(seen_at) <= self.window {
                break;
            }
            self.order.pop_front();
            self.seen.remove(&message_id);
        }
    }

    fn insert(&mut self, message_id: i64, now: Instant) {
        self.seen.insert(message_id, now);
        self.order.push_back((message_id, now));
        while self.order.len() > self.max_entries {
            if let Some((message_id, _)) = self.order.pop_front() {
                self.seen.remove(&message_id);
            }
        }
    }
}

impl Default for MessageDeduper {
    fn default() -> Self {
        Self::new(1024, Duration::from_secs(600))
    }
}

/// Router for normalized OneBot messages.
#[derive(Debug, Default)]
pub struct MessageRouter {
    deduper: MessageDeduper,
}

impl MessageRouter {
    /// Create a router with default dedupe settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Decide whether the event is a command, a task, or should be ignored.
    pub fn route_event(&mut self, event: NormalizedEvent) -> Option<RouteDecision> {
        match event {
            NormalizedEvent::PrivateMessageReceived(event) => self.route_private_message(event),
            NormalizedEvent::GroupMessageReceived(event) => self.route_group_message(event),
            NormalizedEvent::GroupMessageReactionReceived(_) => None,
        }
    }

    fn route_private_message(&mut self, event: PrivateMessageEvent) -> Option<RouteDecision> {
        if !self.deduper.is_new(event.message_id) {
            return None;
        }

        let source_text = event.text.trim();
        if source_text.is_empty() {
            return None;
        }

        if let Some(command) = parse_command(source_text) {
            return Some(RouteDecision::Command(CommandRequest {
                command,
                conversation_key: format!("private:{}", event.sender_id),
                reply_target_id: event.sender_id,
                is_group: false,
                source_message_id: event.message_id,
                source_sender_id: event.sender_id,
                source_sender_name: event.sender_name,
            }));
        }

        Some(RouteDecision::Task(TaskRequest {
            conversation_key: format!("private:{}", event.sender_id),
            source_message_id: event.message_id,
            source_sender_id: event.sender_id,
            source_sender_name: event.sender_name,
            source_text: source_text.to_string(),
            is_group: false,
            reply_target_id: event.sender_id,
        }))
    }

    fn route_group_message(&mut self, event: GroupMessageEvent) -> Option<RouteDecision> {
        if !event.mentions_self {
            return None;
        }
        if !self.deduper.is_new(event.message_id) {
            return None;
        }

        let source_text = event.text.trim();
        if source_text.is_empty() {
            return None;
        }

        // Commands may be preceded by one or more `@<bot>` markers (the
        // form extract_text uses for the bot's own mention). Strip them only
        // when checking for a command; preserve the full text in the task
        // payload so the agent can still see who was addressed.
        let command_candidate = strip_leading_bot_mentions(source_text);
        if command_candidate.is_empty() {
            return None;
        }

        if let Some(command) = parse_command(command_candidate) {
            return Some(RouteDecision::Command(CommandRequest {
                command,
                conversation_key: format!("group:{}", event.group_id),
                reply_target_id: event.group_id,
                is_group: true,
                source_message_id: event.message_id,
                source_sender_id: event.sender_id,
                source_sender_name: event.sender_name,
            }));
        }

        Some(RouteDecision::Task(TaskRequest {
            conversation_key: format!("group:{}", event.group_id),
            source_message_id: event.message_id,
            source_sender_id: event.sender_id,
            source_sender_name: event.sender_name,
            source_text: source_text.to_string(),
            is_group: true,
            reply_target_id: event.group_id,
        }))
    }
}

/// Drop one or more leading `@<bot>` markers (with surrounding whitespace) so
/// the remainder can be inspected for a `/command` prefix. The returned slice
/// borrows from the input.
fn strip_leading_bot_mentions(text: &str) -> &str {
    let mut current = text.trim_start();
    while let Some(rest) = current.strip_prefix("@<bot>") {
        current = rest.trim_start();
    }
    current
}

fn parse_command(text: &str) -> Option<ControlCommand> {
    let mut parts = text.split_whitespace();
    match parts.next() {
        Some("/help") => Some(ControlCommand::Help),
        Some("/status") => Some(ControlCommand::Status {
            task_id: parts.next().map(ToString::to_string),
        }),
        Some("/queue") => Some(ControlCommand::Queue),
        Some("/cancel") => Some(ControlCommand::Cancel),
        Some("/retry_last") => Some(ControlCommand::RetryLast),
        Some("/approve") => parts.next().map(|task_id| ControlCommand::Approve {
            task_id: task_id.to_string(),
        }),
        Some("/deny") => parts.next().map(|task_id| ControlCommand::Deny {
            task_id: task_id.to_string(),
        }),
        Some("/clear") => Some(ControlCommand::Clear),
        Some("/compact") => Some(ControlCommand::Compact),
        _ => None,
    }
}
