//! Codex runtime primitives tests.

use std::path::PathBuf;

use codex_app_server_protocol::{ApprovalsReviewer, AskForApproval, ReadOnlyAccess, SandboxPolicy};
use qqbot_core::{
    approval_guard::{ApprovalDecision, ApprovalGuard},
    codex_runtime::{build_turn_interrupt_params, build_turn_start_params, extract_final_reply},
};
use serde_json::json;

fn test_guard() -> ApprovalGuard {
    ApprovalGuard::new("/home/ts_user/llm_pro/NapCatQQ")
}

fn runtime_config() -> qqbot_core::codex_runtime::CodexRuntimeConfig {
    qqbot_core::codex_runtime::CodexRuntimeConfig::new(
        "/home/ts_user/rust_pro/codex",
        "/tmp/qqbot-workspace",
    )
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

#[test]
fn runtime_config_builds_expected_paths() {
    let config = runtime_config();
    assert_eq!(config.codex_repo_root, PathBuf::from("/home/ts_user/rust_pro/codex"));
    assert_eq!(config.workspace_root, PathBuf::from("/tmp/qqbot-workspace"));
}

#[test]
fn guard_constructor_uses_explicit_workspace_root() {
    let guard = ApprovalGuard::new("/tmp/qqbot-workspace");
    let decision = guard.review_command("ls -la", "/tmp/other", &[]);
    assert_eq!(decision, ApprovalDecision::Deny("cwd escapes workspace: /tmp/other".to_string()));
}

#[test]
fn turn_start_params_use_workspace_write_and_granular_approvals() {
    let config = runtime_config();
    let params = build_turn_start_params(&config, "thread-1", "hello codex");

    assert_eq!(params.thread_id, "thread-1");
    assert_eq!(params.cwd, Some("/tmp/qqbot-workspace".into()));
    assert_eq!(params.approvals_reviewer, Some(ApprovalsReviewer::User));
    assert_eq!(
        params.approval_policy,
        Some(AskForApproval::Granular {
            sandbox_approval: true,
            rules: false,
            skill_approval: false,
            request_permissions: false,
            mcp_elicitations: false,
        })
    );
    assert_eq!(
        params.sandbox_policy,
        Some(SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            read_only_access: ReadOnlyAccess::FullAccess,
            network_access: true,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        })
    );
}

#[test]
fn turn_interrupt_params_target_the_active_thread_and_turn() {
    let params = build_turn_interrupt_params("thread-1", "turn-9");
    assert_eq!(params.thread_id, "thread-1");
    assert_eq!(params.turn_id, "turn-9");
}
