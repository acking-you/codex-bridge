//! CLI smoke tests.

use clap::Parser;
use codex_bridge_cli::{
    cli::{Cli, Commands},
    task_exit::background_task_exit_error,
};

#[test]
fn parse_run_command() {
    let cli = Cli::try_parse_from(["codex-bridge", "run"]).expect("parse run");
    assert!(matches!(cli.command, Commands::Run));
}

#[test]
fn parse_status_command() {
    let cli = Cli::try_parse_from(["codex-bridge", "status"]).expect("parse status");
    assert!(matches!(cli.command, Commands::Status));
}

#[test]
fn parse_queue_command() {
    let cli = Cli::try_parse_from(["codex-bridge", "queue"]).expect("parse queue");
    assert!(matches!(cli.command, Commands::Queue));
}

#[test]
fn parse_cancel_command() {
    let cli = Cli::try_parse_from(["codex-bridge", "cancel"]).expect("parse cancel");
    assert!(matches!(cli.command, Commands::Cancel));
}

#[test]
fn parse_retry_last_command() {
    let cli = Cli::try_parse_from(["codex-bridge", "retry-last"]).expect("parse retry-last");
    assert!(matches!(cli.command, Commands::RetryLast));
}

#[test]
fn parse_reply_text_command() {
    let cli = Cli::try_parse_from(["codex-bridge", "reply", "--text", "hello"])
        .expect("parse reply text");
    assert!(matches!(cli.command, Commands::Reply {
        text: Some(_),
        image: None,
        file: None,
    }));
}

#[test]
fn background_task_exit_error_rejects_unexpected_success() {
    let error =
        background_task_exit_error("bridge runtime", Ok(())).expect_err("unexpected success");
    assert!(error
        .to_string()
        .contains("bridge runtime stopped unexpectedly"));
}

#[test]
fn background_task_exit_error_wraps_component_context() {
    let error = background_task_exit_error("bridge runtime", Err(anyhow::anyhow!("token验证失败")))
        .expect_err("bridge failure should surface");
    let rendered = format!("{error:#}");
    assert!(rendered.contains("bridge runtime stopped"));
    assert!(rendered.contains("token验证失败"));
}
