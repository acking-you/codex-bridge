//! Message router tests.

use qqbot_core::{
    events::NormalizedEvent,
    napcat::message_router::{ControlCommand, MessageDeduper, MessageRouter, RouteDecision},
};

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
        RouteDecision::Task(qqbot_core::napcat::message_router::TaskRequest {
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

    assert_eq!(decision, RouteDecision::Command(ControlCommand::Status));
}

#[test]
fn deduper_returns_false_when_repeated_message_id_seen() {
    let mut deduper = MessageDeduper::default();

    assert!(deduper.is_new(111));
    assert!(!deduper.is_new(111));
}
