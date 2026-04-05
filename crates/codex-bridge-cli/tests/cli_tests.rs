//! CLI smoke tests.

use clap::Parser;
use codex_bridge_cli::cli::{Cli, Commands};

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
