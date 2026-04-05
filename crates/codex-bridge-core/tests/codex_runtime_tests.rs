//! Codex runtime primitives tests.

use std::path::PathBuf;

use codex_app_server_protocol::{
    ApprovalsReviewer, AskForApproval, CommandExecutionApprovalDecision,
    CommandExecutionRequestApprovalParams, FileChangeApprovalDecision,
    FileChangeRequestApprovalParams, ReadOnlyAccess, SandboxPolicy, Turn, TurnError, TurnStatus,
};
use codex_bridge_core::{
    approval_guard::ApprovalGuard,
    codex_runtime::{
        build_codex_app_server_command, build_command_approval_response,
        build_file_change_approval_response, build_thread_resume_params, build_thread_start_params,
        build_turn_interrupt_params, build_turn_start_params, extract_final_reply,
        summarize_turn_result,
    },
    system_prompt::{SYSTEM_PROMPT_TEXT, SYSTEM_PROMPT_VERSION},
};
use codex_utils_absolute_path::AbsolutePathBuf;
use serde_json::json;

fn test_guard() -> ApprovalGuard {
    ApprovalGuard::new("/home/ts_user/llm_pro/codex-bridge/deps/NapCatQQ")
}

fn runtime_config() -> codex_bridge_core::codex_runtime::CodexRuntimeConfig {
    codex_bridge_core::codex_runtime::CodexRuntimeConfig::new(
        "/home/ts_user/llm_pro/codex-bridge/deps/codex/codex-rs",
        "/tmp/codex-bridge",
    )
}

#[test]
fn system_prompt_mentions_reply_skill_and_artifact_boundary() {
    assert_eq!(SYSTEM_PROMPT_VERSION, "v2.0.0");
    assert!(SYSTEM_PROMPT_TEXT.contains(".run/artifacts/"));
    assert!(SYSTEM_PROMPT_TEXT.contains("reply skill"));
    assert!(SYSTEM_PROMPT_TEXT.contains("inspect the host machine broadly"));
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
    assert_eq!(
        config.codex_repo_root,
        PathBuf::from("/home/ts_user/llm_pro/codex-bridge/deps/codex/codex-rs")
    );
    assert_eq!(config.workspace_root, PathBuf::from("/tmp/codex-bridge"));
}

#[test]
fn codex_app_server_command_uses_explicit_bin_selection() {
    let config = runtime_config();
    let command = build_codex_app_server_command(&config);

    assert_eq!(command, vec![
        "cargo".to_string(),
        "run".to_string(),
        "--manifest-path".to_string(),
        "/home/ts_user/llm_pro/codex-bridge/deps/codex/codex-rs/Cargo.toml".to_string(),
        "--bin".to_string(),
        "codex-app-server".to_string(),
        "--".to_string(),
        "--listen".to_string(),
        "stdio://".to_string(),
    ]);
}

#[test]
fn thread_start_params_include_prompt_and_persisted_history() {
    let config = runtime_config();
    let params = build_thread_start_params(&config, "private:123");

    assert_eq!(params.cwd, Some("/tmp/codex-bridge".to_string()));
    assert_eq!(params.approvals_reviewer, Some(ApprovalsReviewer::User));
    assert_eq!(params.sandbox, Some(codex_app_server_protocol::SandboxMode::WorkspaceWrite));
    assert_eq!(params.service_name.as_deref(), Some("private:123"));
    assert!(params.developer_instructions.is_some());
    assert!(params.persist_extended_history);
}

#[test]
fn thread_resume_params_keep_existing_prompt_version() {
    let config = runtime_config();
    let params = build_thread_resume_params(&config, "thread-1");

    assert_eq!(params.thread_id, "thread-1");
    assert_eq!(params.cwd, Some("/tmp/codex-bridge".to_string()));
    assert_eq!(params.approvals_reviewer, Some(ApprovalsReviewer::User));
    assert_eq!(params.sandbox, Some(codex_app_server_protocol::SandboxMode::WorkspaceWrite));
    assert!(params.developer_instructions.is_none());
    assert!(params.persist_extended_history);
}

#[test]
fn turn_start_params_use_workspace_write_and_granular_approvals() {
    let config = runtime_config();
    let params = build_turn_start_params(&config, "thread-1", "hello codex");

    assert_eq!(params.thread_id, "thread-1");
    assert_eq!(params.cwd, Some("/tmp/codex-bridge".into()));
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
            writable_roots: vec![AbsolutePathBuf::from_absolute_path("/tmp/codex-bridge")
                .expect("absolute workspace root")],
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

#[test]
fn command_approval_declines_extra_permission_requests() {
    let params = CommandExecutionRequestApprovalParams {
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
        item_id: "item-1".to_string(),
        approval_id: None,
        reason: None,
        network_approval_context: None,
        command: Some("ls -la".to_string()),
        cwd: Some(PathBuf::from("/home/ts_user/llm_pro/codex-bridge/deps/NapCatQQ")),
        command_actions: None,
        additional_permissions: Some(codex_app_server_protocol::AdditionalPermissionProfile {
            network: Some(codex_app_server_protocol::AdditionalNetworkPermissions {
                enabled: Some(true),
            }),
            file_system: None,
        }),
        proposed_execpolicy_amendment: None,
        proposed_network_policy_amendments: None,
        available_decisions: None,
    };

    let response = build_command_approval_response(&test_guard(), &params);
    assert_eq!(response.decision, CommandExecutionApprovalDecision::Decline);
}

#[test]
fn file_change_approval_declines_extra_write_root_requests() {
    let params = FileChangeRequestApprovalParams {
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
        item_id: "item-1".to_string(),
        reason: Some("need extra write root".to_string()),
        grant_root: Some(PathBuf::from("/tmp/outside")),
    };

    let response = build_file_change_approval_response(&test_guard(), &params);
    assert_eq!(response.decision, FileChangeApprovalDecision::Decline);
}

#[test]
fn summarize_turn_result_uses_failure_summary_when_no_reply_exists() {
    let turn = Turn {
        id: "turn-1".to_string(),
        items: vec![],
        status: TurnStatus::Failed,
        error: Some(TurnError {
            message: "permission denied".to_string(),
            codex_error_info: None,
            additional_details: None,
        }),
    };

    assert_eq!(
        summarize_turn_result(&turn, &[]),
        Some("执行失败。\n原因：permission denied".to_string())
    );
}

#[test]
fn summarize_turn_result_uses_interrupted_summary_when_needed() {
    let turn = Turn {
        id: "turn-1".to_string(),
        items: vec![],
        status: TurnStatus::Interrupted,
        error: None,
    };

    assert_eq!(
        summarize_turn_result(&turn, &[]),
        Some("任务因服务重启或异常中断。可使用 /retry_last 重试。".to_string())
    );
}
