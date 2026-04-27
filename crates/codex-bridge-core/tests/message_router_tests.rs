//! Message router tests.

use codex_bridge_core::{
    events::NormalizedEvent,
    message_router::{
        CommandRequest, ControlCommand, MessageDeduper, MessageRouter, RouteDecision,
    },
};

#[test]
fn private_help_routes_as_command_with_sender_metadata() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "message_id": 9001,
        "user_id": 42,
        "self_id": 99,
        "raw_message": "/help",
        "message": [
            { "type": "text", "data": { "text": "/help" } }
        ],
        "sender": {
            "nickname": "alice"
        }
    });
    let event = NormalizedEvent::try_from(raw).expect("normalize private message");
    let mut router = MessageRouter::new();

    let decision = router.route_event(event).expect("route decision");

    assert_eq!(
        decision,
        RouteDecision::Command(CommandRequest {
            command: ControlCommand::Help,
            conversation_key: "private:42".to_string(),
            reply_target_id: 42,
            is_group: false,
            source_message_id: 9001,
            source_sender_id: 42,
            source_sender_name: "alice".to_string(),
        })
    );
}

#[test]
fn private_message_routes_to_task_by_default() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "message_id": 1001,
        "user_id": 10,
        "self_id": 99,
        "raw_message": "hello",
        "message": [
            { "type": "text", "data": { "text": "hello" } }
        ],
        "sender": {
            "nickname": "alice"
        }
    });
    let event = NormalizedEvent::try_from(raw).expect("normalize private message");
    let mut router = MessageRouter::new();

    let decision = router.route_event(event).expect("route decision");

    assert_eq!(
        decision,
        RouteDecision::Task(codex_bridge_core::message_router::TaskRequest {
            conversation_key: "private:10".to_string(),
            source_message_id: 1001,
            source_sender_id: 10,
            source_sender_name: "alice".to_string(),
            source_text: "hello".to_string(),
            is_group: false,
            reply_target_id: 10,
            self_id: 99,
            quoted_message_id: None,
        })
    );
}

#[test]
fn group_mention_status_routes_to_command() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "group",
        "message_id": 1002,
        "group_id": 777,
        "user_id": 11,
        "self_id": 99,
        "raw_message": "@bot /status",
        "message": [
            { "type": "at", "data": { "qq": "99", "name": "bot" } },
            { "type": "text", "data": { "text": " /status" } }
        ],
        "sender": {
            "nickname": "alice"
        }
    });
    let event = NormalizedEvent::try_from(raw).expect("normalize group message");
    let mut router = MessageRouter::new();

    let decision = router.route_event(event).expect("route decision");

    assert_eq!(
        decision,
        RouteDecision::Command(CommandRequest {
            command: ControlCommand::Status { task_id: None },
            conversation_key: "group:777".to_string(),
            reply_target_id: 777,
            is_group: true,
            source_message_id: 1002,
            source_sender_id: 11,
            source_sender_name: "alice".to_string(),
        })
    );
}

#[test]
fn private_status_command_has_context() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "message_id": 1004,
        "user_id": 11,
        "self_id": 99,
        "raw_message": "/status",
        "message": [
            { "type": "text", "data": { "text": " /status" } }
        ],
        "sender": {
            "nickname": "alice"
        }
    });
    let event = NormalizedEvent::try_from(raw).expect("normalize private message");
    let mut router = MessageRouter::new();

    let decision = router.route_event(event).expect("route decision");

    assert_eq!(
        decision,
        RouteDecision::Command(CommandRequest {
            command: ControlCommand::Status { task_id: None },
            conversation_key: "private:11".to_string(),
            reply_target_id: 11,
            is_group: false,
            source_message_id: 1004,
            source_sender_id: 11,
            source_sender_name: "alice".to_string(),
        })
    );
}

#[test]
fn private_approve_command_keeps_task_id() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "message_id": 1005,
        "user_id": 11,
        "self_id": 99,
        "raw_message": "/approve task-123",
        "message": [
            { "type": "text", "data": { "text": "/approve task-123" } }
        ],
        "sender": {
            "nickname": "alice"
        }
    });
    let event = NormalizedEvent::try_from(raw).expect("normalize private message");
    let mut router = MessageRouter::new();

    let decision = router.route_event(event).expect("route decision");

    assert_eq!(
        decision,
        RouteDecision::Command(CommandRequest {
            command: ControlCommand::Approve { task_id: "task-123".to_string() },
            conversation_key: "private:11".to_string(),
            reply_target_id: 11,
            is_group: false,
            source_message_id: 1005,
            source_sender_id: 11,
            source_sender_name: "alice".to_string(),
        })
    );
}

#[test]
fn clear_command_routes_from_private_chat() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "message_id": 1006,
        "user_id": 11,
        "self_id": 99,
        "message": [{ "type": "text", "data": { "text": "/clear" } }],
        "sender": { "nickname": "alice" }
    });
    let event = NormalizedEvent::try_from(raw).expect("normalize");
    let mut router = MessageRouter::new();

    let decision = router.route_event(event).expect("route decision");
    assert!(matches!(
        decision,
        RouteDecision::Command(CommandRequest { command: ControlCommand::Clear, .. })
    ));
}

#[test]
fn compact_command_routes_from_private_chat() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "message_id": 1007,
        "user_id": 11,
        "self_id": 99,
        "message": [{ "type": "text", "data": { "text": "/compact" } }],
        "sender": { "nickname": "alice" }
    });
    let event = NormalizedEvent::try_from(raw).expect("normalize");
    let mut router = MessageRouter::new();

    let decision = router.route_event(event).expect("route decision");
    assert!(matches!(
        decision,
        RouteDecision::Command(CommandRequest { command: ControlCommand::Compact, .. })
    ));
}

#[test]
fn deduper_returns_false_when_repeated_message_id_seen() {
    let mut deduper = MessageDeduper::default();

    assert!(deduper.is_new(111));
    assert!(!deduper.is_new(111));
}

#[test]
fn group_mention_only_message_is_ignored() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "group",
        "message_id": 1003,
        "group_id": 777,
        "user_id": 11,
        "self_id": 99,
        "raw_message": "@bot",
        "message": [
            { "type": "at", "data": { "qq": "99", "name": "bot" } },
        ]
    });
    let event = NormalizedEvent::try_from(raw).expect("normalize group message");
    let mut router = MessageRouter::new();

    assert!(router.route_event(event).is_none());
}

#[test]
fn router_ignores_group_reaction_events() {
    let raw = serde_json::json!({
        "post_type": "notice",
        "notice_type": "group_msg_emoji_like",
        "group_id": 777,
        "user_id": 2394626220i64,
        "message_id": 5001,
        "self_id": 2993013575i64,
        "likes": [{ "emoji_id": "282", "count": 1 }],
        "is_add": true
    });
    let event = NormalizedEvent::try_from(raw).expect("normalize");
    let mut router = MessageRouter::new();

    assert!(router.route_event(event).is_none());
}
