//! Codex runtime primitives tests.

use std::{fs, path::PathBuf};

use codex_app_server_protocol::{
    ApprovalsReviewer, AskForApproval, CommandExecutionApprovalDecision,
    CommandExecutionRequestApprovalParams, FileChangeApprovalDecision,
    FileChangeRequestApprovalParams, ItemStartedNotification, ReadOnlyAccess,
    ReasoningTextDeltaNotification, SandboxPolicy, ServerNotification, Turn, TurnError,
    TurnStartedNotification, TurnStatus, UserInput,
};
use codex_bridge_core::{
    approval_guard::ApprovalGuard,
    codex_runtime::{
        build_codex_app_server_command, build_codex_app_server_env,
        build_command_approval_response, build_file_change_approval_response,
        build_thread_resume_params, build_thread_start_params, build_turn_interrupt_params,
        build_turn_start_params, codex_app_server_workdir, describe_server_notification,
        extract_final_reply, is_missing_thread_rollout_error, summarize_turn_result,
    },
    system_prompt::{load_persona, BRIDGE_PROTOCOL_TEXT, DEFAULT_PERSONA_TEMPLATE},
};
use codex_utils_absolute_path::AbsolutePathBuf;
use serde_json::json;
use tempfile::tempdir;

fn test_guard() -> ApprovalGuard {
    ApprovalGuard::new("/home/ts_user/llm_pro/codex-bridge/deps/NapCatQQ")
}

fn runtime_config() -> codex_bridge_core::codex_runtime::CodexRuntimeConfig {
    codex_bridge_core::codex_runtime::CodexRuntimeConfig::new(
        "/home/ts_user/llm_pro/codex-bridge/deps/codex/codex-rs",
        "/tmp/codex-bridge",
        "/tmp/codex-bridge/.run/default/prompt/persona.md",
        "/tmp/codex-bridge-home",
        "/tmp/codex-bridge-codex-home",
    )
}

fn write_skill(workspace_root: &std::path::Path, skill_name: &str) {
    let skill_dir = workspace_root.join("skills").join(skill_name);
    fs::create_dir_all(&skill_dir).expect("create skill directory");
    fs::write(
        skill_dir.join("SKILL.md"),
        format!(
            "---\nname: {skill_name}\ndescription: Use when testing skill injection\n---\n\n# \
             {skill_name}\n"
        ),
    )
    .expect("write skill file");
}

#[test]
fn persona_template_covers_identity_voice_and_project_skills() {
    assert!(DEFAULT_PERSONA_TEMPLATE.contains("You are a cyber-human lifeform"));
    assert!(DEFAULT_PERSONA_TEMPLATE.contains("If someone asks who you are"));
    assert!(DEFAULT_PERSONA_TEMPLATE.contains("# Voice and vitality"));
    assert!(DEFAULT_PERSONA_TEMPLATE.contains("staticflow-kiro-log-diagnoser"));
    assert!(DEFAULT_PERSONA_TEMPLATE.contains("StaticFlow"));
}

#[test]
fn bridge_protocol_covers_turn_checklist_reply_and_permissions() {
    assert!(BRIDGE_PROTOCOL_TEXT.contains("# Turn start checklist"));
    assert!(BRIDGE_PROTOCOL_TEXT.contains("Gate 0"));
    assert!(BRIDGE_PROTOCOL_TEXT.contains("Context first"));
    assert!(BRIDGE_PROTOCOL_TEXT.contains("qq-current-history"));
    assert!(BRIDGE_PROTOCOL_TEXT.contains("qq-quoted-image-recovery"));
    assert!(BRIDGE_PROTOCOL_TEXT.contains("Recovering quoted images"));
    assert!(BRIDGE_PROTOCOL_TEXT.contains("Gate 1"));
    assert!(BRIDGE_PROTOCOL_TEXT.contains("reply-current"));
    assert!(BRIDGE_PROTOCOL_TEXT.contains(".run/artifacts/"));
    assert!(BRIDGE_PROTOCOL_TEXT.contains("inspect the host machine broadly"));
    assert!(BRIDGE_PROTOCOL_TEXT.contains("must never delete files"));
    assert!(BRIDGE_PROTOCOL_TEXT.contains(
        "Never write the literal two-character sequence \\n when you want a line break"
    ));
}

#[test]
fn load_persona_rejects_empty_file() {
    let dir = tempdir().expect("tempdir");
    let persona_file = dir.path().join("persona.md");
    fs::write(&persona_file, "   \n").expect("write empty persona");

    let error = load_persona(&persona_file).expect_err("empty persona should fail");
    assert!(error.to_string().contains("empty"));
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
    assert_eq!(
        config.prompt_file,
        PathBuf::from("/tmp/codex-bridge/.run/default/prompt/persona.md")
    );
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
fn codex_app_server_runs_from_workspace_root() {
    let config = runtime_config();

    assert_eq!(codex_app_server_workdir(&config), PathBuf::from("/tmp/codex-bridge"));
}

#[test]
fn codex_app_server_env_isolates_home_and_codex_home() {
    let config = runtime_config();
    let env = build_codex_app_server_env(&config);

    assert!(env.contains(&("HOME".to_string(), "/tmp/codex-bridge-home".to_string())));
    assert!(env.contains(&("CODEX_HOME".to_string(), "/tmp/codex-bridge-codex-home".to_string())));
}

#[test]
fn missing_thread_rollout_errors_are_detected() {
    let error = anyhow::anyhow!(
        "thread/resume failed: no rollout found for thread id 019d5d8f-c920-7093-a1ff-40dcfcca8c39"
    );
    assert!(is_missing_thread_rollout_error(&error));

    let other = anyhow::anyhow!("thread/resume failed: permission denied");
    assert!(!is_missing_thread_rollout_error(&other));
}

#[test]
fn thread_start_params_include_prompt_and_persisted_history() {
    let dir = tempdir().expect("tempdir");
    let persona_file = dir.path().join("persona.md");
    fs::write(&persona_file, "# Persona\n\nprompt from file").expect("write persona");
    let mut config = runtime_config();
    config.prompt_file = persona_file;
    config.admin_user_id = 42;
    let params = build_thread_start_params(&config, "private:123").expect("build start params");

    assert_eq!(params.cwd, Some("/tmp/codex-bridge".to_string()));
    assert_eq!(params.approvals_reviewer, Some(ApprovalsReviewer::User));
    assert_eq!(params.sandbox, Some(codex_app_server_protocol::SandboxMode::WorkspaceWrite));
    assert_eq!(params.service_name.as_deref(), Some("private:123"));
    assert!(params.persist_extended_history);

    let instructions = params
        .developer_instructions
        .as_deref()
        .expect("developer instructions present");
    // Layer 1: persona from file
    assert!(instructions.contains("prompt from file"));
    // Layer 2: bridge protocol (turn checklist is the canonical anchor)
    assert!(instructions.contains("# Turn start checklist"));
    assert!(instructions.contains("reply-current"));
    // Layer 3: admin context with the configured id
    assert!(instructions.contains("# Admin context"));
    assert!(instructions.contains("42"));
}

#[test]
fn thread_resume_params_reapply_current_system_prompt() {
    let dir = tempdir().expect("tempdir");
    let persona_file = dir.path().join("persona.md");
    fs::write(&persona_file, "# Persona\n\nprompt from runtime file").expect("write persona");
    let mut config = runtime_config();
    config.prompt_file = persona_file;
    let params = build_thread_resume_params(&config, "thread-1", "group:42")
        .expect("build resume params");

    assert_eq!(params.thread_id, "thread-1");
    assert_eq!(params.cwd, Some("/tmp/codex-bridge".to_string()));
    assert_eq!(params.approvals_reviewer, Some(ApprovalsReviewer::User));
    assert_eq!(params.sandbox, Some(codex_app_server_protocol::SandboxMode::WorkspaceWrite));
    let instructions = params
        .developer_instructions
        .as_deref()
        .expect("developer instructions present on resume");
    assert!(instructions.contains("prompt from runtime file"));
    assert!(instructions.contains("# Turn start checklist"));
    assert!(params.persist_extended_history);
}

#[test]
fn thread_compact_start_params_target_the_bound_thread() {
    let params =
        codex_bridge_core::codex_runtime::build_thread_compact_start_params("thread-compact-1");
    assert_eq!(params.thread_id, "thread-compact-1");
}

#[test]
fn turn_start_params_use_workspace_write_and_granular_approvals() {
    let dir = tempdir().expect("tempdir");
    let workspace_root = dir.path().join("workspace");
    fs::create_dir_all(&workspace_root).expect("create workspace");
    write_skill(&workspace_root, "reply-current");
    write_skill(&workspace_root, "staticflow-kiro-log-diagnoser");

    let mut config = runtime_config();
    config.workspace_root = workspace_root.clone();
    let params =
        build_turn_start_params(&config, "thread-1", "hello codex").expect("build turn params");

    assert_eq!(params.thread_id, "thread-1");
    assert_eq!(params.cwd, Some(workspace_root.clone()));
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
            writable_roots: vec![AbsolutePathBuf::from_absolute_path(&workspace_root)
                .expect("absolute workspace root")],
            read_only_access: ReadOnlyAccess::FullAccess,
            network_access: true,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        })
    );
    assert_eq!(params.input.len(), 3);
    assert_eq!(params.input[1], UserInput::Skill {
        name: "reply-current".to_string(),
        path: workspace_root.join("skills/reply-current/SKILL.md"),
    });
    assert_eq!(params.input[2], UserInput::Skill {
        name: "staticflow-kiro-log-diagnoser".to_string(),
        path: workspace_root.join("skills/staticflow-kiro-log-diagnoser/SKILL.md"),
    });
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
        cwd: Some(
            AbsolutePathBuf::from_absolute_path(
                "/home/ts_user/llm_pro/codex-bridge/deps/NapCatQQ",
            )
            .expect("absolute path"),
        ),
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
        started_at: None,
        completed_at: None,
        duration_ms: None,
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
        started_at: None,
        completed_at: None,
        duration_ms: None,
    };

    assert_eq!(
        summarize_turn_result(&turn, &[]),
        Some("任务因服务重启或异常中断。可使用 /retry_last 重试。".to_string())
    );
}

#[test]
fn describe_server_notification_summarizes_turn_start_and_item_start() {
    let turn_started = ServerNotification::TurnStarted(TurnStartedNotification {
        thread_id: "thread-1".to_string(),
        turn: Turn {
            id: "turn-1".to_string(),
            items: vec![],
            status: TurnStatus::InProgress,
            error: None,
            started_at: None,
            completed_at: None,
            duration_ms: None,
        },
    });
    let item_started = ServerNotification::ItemStarted(ItemStartedNotification {
        item: serde_json::from_value(json!({
            "type": "agentMessage",
            "id": "item-1",
            "text": "hello"
        }))
        .expect("thread item"),
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
    });

    assert_eq!(
        describe_server_notification(&turn_started),
        Some("turn started: thread=thread-1 turn=turn-1 status=in_progress".to_string())
    );
    assert_eq!(
        describe_server_notification(&item_started),
        Some("item started: thread=thread-1 turn=turn-1 item=item-1 type=agentMessage".to_string())
    );
}

#[test]
fn describe_server_notification_redacts_reasoning_deltas() {
    let reasoning = ServerNotification::ReasoningTextDelta(ReasoningTextDeltaNotification {
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
        item_id: "item-9".to_string(),
        delta: "private reasoning".to_string(),
        content_index: 0,
    });

    assert_eq!(
        describe_server_notification(&reasoning),
        Some("reasoning delta received (hidden)".to_string())
    );
}
