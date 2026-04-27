//! Event normalization tests.

use codex_bridge_core::events::{GroupMessageEvent, NormalizedEvent, PrivateMessageEvent};

#[test]
fn webui_hash_matches_napcat_rule() {
    assert_eq!(
        codex_bridge_core::napcat::webui_password_hash("abc123"),
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
        "message_id": 100,
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
            message_id,
            sender_name,
            self_id,
            text,
            mentions_self,
            mentions,
            ..
        }) => {
            assert_eq!(group_id, 42);
            assert_eq!(sender_id, 7);
            assert_eq!(message_id, 100);
            assert_eq!(self_id, 99);
            assert_eq!(text, "@<bot> hello");
            assert_eq!(sender_name, "unknown");
            assert!(mentions_self);
            assert_eq!(mentions, vec![99]);
        },
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn group_message_event_preserves_other_user_mentions_with_qq() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "group",
        "group_id": 42,
        "user_id": 7,
        "message_id": 200,
        "self_id": 99,
        "raw_message": "@bot ping @bob",
        "message": [
            { "type": "at", "data": { "qq": "99", "name": "bot" } },
            { "type": "text", "data": { "text": " ping " } },
            { "type": "at", "data": { "qq": "12345", "name": "bob" } }
        ]
    });

    let event = NormalizedEvent::try_from(raw).expect("normalize group message");
    match event {
        NormalizedEvent::GroupMessageReceived(GroupMessageEvent {
            text,
            mentions,
            mentions_self,
            ..
        }) => {
            assert_eq!(text, "@<bot> ping @bob<QQ:12345>");
            assert!(mentions_self);
            assert_eq!(mentions, vec![99, 12345]);
        },
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn group_message_event_falls_back_when_at_segment_has_no_name() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "group",
        "group_id": 42,
        "user_id": 7,
        "message_id": 201,
        "self_id": 99,
        "raw_message": "@bot ping @somebody",
        "message": [
            { "type": "at", "data": { "qq": "99", "name": "bot" } },
            { "type": "text", "data": { "text": " ping " } },
            { "type": "at", "data": { "qq": "67890" } }
        ]
    });

    let event = NormalizedEvent::try_from(raw).expect("normalize group message");
    match event {
        NormalizedEvent::GroupMessageReceived(GroupMessageEvent { text, .. }) => {
            assert_eq!(text, "@<bot> ping @<QQ:67890>");
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
        "message_id": 101,
        "self_id": 99,
        "sender": {
            "nickname": "alice"
        },
        "raw_message": "just text",
        "message": [
            { "type": "text", "data": { "text": "just text" } }
        ]
    });

    let event = NormalizedEvent::try_from(raw).expect("normalize private message");

    match event {
        NormalizedEvent::PrivateMessageReceived(PrivateMessageEvent {
            sender_id,
            message_id,
            sender_name,
            self_id,
            text,
            mentions_self,
            ..
        }) => {
            assert_eq!(sender_id, 1001);
            assert_eq!(message_id, 101);
            assert_eq!(self_id, 99);
            assert_eq!(sender_name, "alice");
            assert_eq!(text, "just text");
            assert!(!mentions_self);
        },
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn group_reaction_notice_extracts_operator_message_and_emoji() {
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

    let event = NormalizedEvent::try_from(raw).expect("normalize reaction notice");

    match event {
        NormalizedEvent::GroupMessageReactionReceived(reaction) => {
            assert_eq!(reaction.group_id, 777);
            assert_eq!(reaction.operator_id, 2394626220);
            assert_eq!(reaction.message_id, 5001);
            assert_eq!(reaction.emoji_id, "282");
            assert!(reaction.is_add);
        },
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn group_message_event_captures_quoted_message_id_from_reply_segment() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "group",
        "group_id": 42,
        "user_id": 7,
        "message_id": 200,
        "self_id": 99,
        "message": [
            { "type": "reply", "data": { "id": "12345" } },
            { "type": "at", "data": { "qq": "99", "name": "bot" } },
            { "type": "text", "data": { "text": " 这句话什么意思？" } }
        ]
    });

    let event = NormalizedEvent::try_from(raw).expect("normalize group message");
    match event {
        NormalizedEvent::GroupMessageReceived(GroupMessageEvent {
            quoted_message_id,
            text,
            ..
        }) => {
            assert_eq!(quoted_message_id, Some(12345));
            assert_eq!(text, "@<bot> 这句话什么意思？");
        },
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn group_message_event_quoted_id_is_none_without_reply_segment() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "group",
        "group_id": 42,
        "user_id": 7,
        "message_id": 201,
        "self_id": 99,
        "message": [
            { "type": "at", "data": { "qq": "99", "name": "bot" } },
            { "type": "text", "data": { "text": " hi" } }
        ]
    });

    let event = NormalizedEvent::try_from(raw).expect("normalize group message");
    match event {
        NormalizedEvent::GroupMessageReceived(GroupMessageEvent { quoted_message_id, .. }) => {
            assert_eq!(quoted_message_id, None);
        },
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn private_message_event_captures_quoted_id_when_id_is_numeric() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 1001,
        "message_id": 102,
        "self_id": 99,
        "sender": { "nickname": "alice" },
        "message": [
            { "type": "reply", "data": { "id": 8888 } },
            { "type": "text", "data": { "text": "follow up" } }
        ]
    });

    let event = NormalizedEvent::try_from(raw).expect("normalize private message");
    match event {
        NormalizedEvent::PrivateMessageReceived(PrivateMessageEvent {
            quoted_message_id, ..
        }) => {
            assert_eq!(quoted_message_id, Some(8888));
        },
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn malformed_reply_segment_id_yields_none() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "group",
        "group_id": 42,
        "user_id": 7,
        "message_id": 202,
        "self_id": 99,
        "message": [
            { "type": "reply", "data": { "id": "not-a-number" } },
            { "type": "text", "data": { "text": "x" } }
        ]
    });

    let event = NormalizedEvent::try_from(raw).expect("normalize group message");
    match event {
        NormalizedEvent::GroupMessageReceived(GroupMessageEvent { quoted_message_id, .. }) => {
            assert_eq!(quoted_message_id, None);
        },
        other => panic!("unexpected event: {other:?}"),
    }
}
