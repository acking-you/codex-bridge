//! NapCat transport frame tests.

use codex_bridge_core::{
    events::NormalizedEvent,
    napcat::{build_action_frame, IncomingFrame},
};

#[test]
fn build_action_frame_includes_action_params_and_echo() {
    let frame = build_action_frame(
        "send_private_msg",
        serde_json::json!({
            "user_id": "1001",
            "message": "hello",
        }),
        "echo-1",
    );

    assert_eq!(frame["action"], "send_private_msg");
    assert_eq!(frame["params"]["user_id"], "1001");
    assert_eq!(frame["params"]["message"], "hello");
    assert_eq!(frame["echo"], "echo-1");
}

#[test]
fn incoming_frame_from_value_distinguishes_event_and_response() {
    let event_payload = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "message_id": 42,
        "user_id": 2,
        "self_id": 1,
        "raw_message": "ping",
        "message": [
            { "type": "text", "data": { "text": "ping" } }
        ],
        "sender": {
            "nickname": "alice"
        }
    });
    let response_payload = serde_json::json!({
        "echo": "abcd",
        "status": "ok",
        "retcode": 0,
        "data": {
            "message_id": 123
        },
        "message": "",
        "wording": null
    });

    let event_frame = IncomingFrame::from_value(event_payload).expect("parse event frame");
    let response_frame = IncomingFrame::from_value(response_payload).expect("parse response frame");

    match event_frame {
        IncomingFrame::Event(NormalizedEvent::PrivateMessageReceived(event)) => {
            assert_eq!(event.message_id, 42);
            assert_eq!(event.sender_name, "alice");
        },
        IncomingFrame::Event(_)
        | IncomingFrame::Response {
            ..
        } => {
            panic!("expected event");
        },
    }
    match response_frame {
        IncomingFrame::Response {
            echo,
            payload,
        } => {
            assert_eq!(echo, "abcd");
            assert_eq!(payload["status"], "ok");
        },
        IncomingFrame::Event(_) => panic!("expected response"),
    }
}
