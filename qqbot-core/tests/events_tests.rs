//! Event normalization tests.

use qqbot_core::events::{GroupMessageEvent, NormalizedEvent, PrivateMessageEvent};

#[test]
fn webui_hash_matches_napcat_rule() {
    assert_eq!(
        qqbot_core::napcat::webui_password_hash("abc123"),
        "3c2641c2bcd5417250a192a97ce6a65b897746023c37e9d22c7a80dd76030746"
    );
}

#[test]
fn group_message_event_extracts_mentions_and_text() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "group",
        "group_id": 42,
        "user_id": 7,
        "self_id": 99,
        "raw_message": "@bot hello",
        "message": [
            { "type": "at", "data": { "qq": "99", "name": "bot" } },
            { "type": "text", "data": { "text": " hello" } }
        ]
    });

    let event = NormalizedEvent::try_from(raw).expect("normalize group message");

    match event {
        NormalizedEvent::GroupMessageReceived(GroupMessageEvent {
            group_id,
            sender_id,
            self_id,
            text,
            mentions_self,
            mentions,
            ..
        }) => {
            assert_eq!(group_id, 42);
            assert_eq!(sender_id, 7);
            assert_eq!(self_id, 99);
            assert_eq!(text, "hello");
            assert!(mentions_self);
            assert_eq!(mentions, vec![99]);
        },
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn private_message_event_preserves_raw_text() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 1001,
        "self_id": 99,
        "raw_message": "just text",
        "message": [
            { "type": "text", "data": { "text": "just text" } }
        ]
    });

    let event = NormalizedEvent::try_from(raw).expect("normalize private message");

    match event {
        NormalizedEvent::PrivateMessageReceived(PrivateMessageEvent {
            sender_id,
            self_id,
            text,
            mentions_self,
            ..
        }) => {
            assert_eq!(sender_id, 1001);
            assert_eq!(self_id, 99);
            assert_eq!(text, "just text");
            assert!(!mentions_self);
        },
        other => panic!("unexpected event: {other:?}"),
    }
}
