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
            command: ControlCommand::Status,
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
            command: ControlCommand::Status,
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
