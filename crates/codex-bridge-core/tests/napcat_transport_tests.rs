//! NapCat transport frame tests.

use std::path::PathBuf;

use codex_bridge_core::{
    config::RuntimeConfig,
    events::NormalizedEvent,
    napcat::{
        build_action_frame, build_outbound_action, build_set_msg_emoji_like_params,
        build_websocket_request, IncomingFrame,
    },
    outbound::{OutboundMessage, OutboundSegment, OutboundTarget},
    runtime::RuntimeTokens,
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

#[test]
fn websocket_request_includes_bearer_token() {
    let config = RuntimeConfig::default();
    let tokens = RuntimeTokens {
        webui_token: "webui-token".to_string(),
        ws_token: "ws-token".to_string(),
    };

    let request = build_websocket_request(&config, &tokens).expect("build websocket request");

    assert_eq!(request.uri().to_string(), "ws://127.0.0.1:3012/");
    assert_eq!(request.headers()["authorization"], "Bearer ws-token");
}

#[test]
fn structured_group_outbound_action_contains_reply_at_and_image_segments() {
    let message = OutboundMessage {
        target: OutboundTarget::Group(777),
        segments: vec![
            OutboundSegment::Reply {
                message_id: 9901,
            },
            OutboundSegment::At {
                user_id: 42,
            },
            OutboundSegment::Image {
                path: PathBuf::from("/tmp/result.png"),
            },
        ],
    };

    let (action, params) = build_outbound_action(&message);

    assert_eq!(action, "send_group_msg");
    assert_eq!(params["group_id"], "777");
    assert_eq!(params["message"][0]["type"], "reply");
    assert_eq!(params["message"][0]["data"]["id"], "9901");
    assert_eq!(params["message"][1]["type"], "at");
    assert_eq!(params["message"][1]["data"]["qq"], "42");
    assert_eq!(params["message"][2]["type"], "image");
    assert_eq!(params["message"][2]["data"]["file"], "/tmp/result.png");
}

#[test]
fn set_msg_emoji_like_params_use_onebot_contract() {
    let params = build_set_msg_emoji_like_params(12345, "282");

    assert_eq!(params["message_id"], "12345");
    assert_eq!(params["emoji_id"], "282");
    assert_eq!(params["set"], true);
}
