//! Codex runtime primitives tests.

use qqbot_core::{
    approval_guard::{ApprovalDecision, ApprovalGuard},
    codex_runtime::extract_final_reply,
};
use serde_json::json;

fn test_guard() -> ApprovalGuard {
    ApprovalGuard::new("/home/ts_user/llm_pro/NapCatQQ")
}

#[test]
fn review_command_denies_dangerous_kill_command() {
    let guard = test_guard();
    let decision = guard.review_command("kill -9 1", "/home/ts_user/llm_pro/NapCatQQ", &[]);

    match decision {
        ApprovalDecision::Deny(reason) => {
            assert!(reason.contains("dangerous"));
        },
        other => panic!("expected deny, got {other:?}"),
    }
}

#[test]
fn review_command_allows_safe_local_inspection() {
    let guard = test_guard();
    let decision =
        guard
            .review_command("ls -la --color=never .", "/home/ts_user/llm_pro/NapCatQQ/subdir", &[]);

    assert_eq!(decision, ApprovalDecision::Allow);
}

#[test]
fn review_command_denies_cwd_outside_workspace_root() {
    let guard = test_guard();
    let decision = guard.review_command("ls", "/tmp", &[]);

    match decision {
        ApprovalDecision::Deny(reason) => {
            assert!(reason.contains("workspace"));
        },
        other => panic!("expected deny, got {other:?}"),
    }
}

#[test]
fn extract_final_reply_prefers_last_agent_message() {
    let items = vec![
        json!({"type":"assistantMessage","text":"first"}),
        json!({"item":{"type":"assistant","text":"ignored"}}),
        json!({"type":"assistantMessage","text":""}),
        json!({"type":"reasoning","text":"not final"}),
        json!({"item":{"type":"agentMessage","text":"last-valid"}}),
    ];

    assert_eq!(extract_final_reply(&items), Some("last-valid".to_string()));
}

#[test]
fn extract_final_reply_ignores_non_text_or_empty_items() {
    let items = vec![
        json!({"type":"assistantMessage","text":""}),
        json!({"type":"assistantMessage"}),
        json!({"item":{"type":"assistant","text":"finalized"}}),
        json!({"type":"assistant","text":""}),
        json!({"type":"assistantMessage","text":"  "}),
    ];

    assert_eq!(extract_final_reply(&items), Some("finalized".to_string()));
}
