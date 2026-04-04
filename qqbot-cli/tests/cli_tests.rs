//! CLI smoke tests.

use clap::Parser;
use qqbot_cli::cli::Cli;

#[test]
fn parse_run_command() {
    let cli = Cli::try_parse_from(["qqbot-cli", "run"]).expect("parse run");
    assert!(matches!(cli.command, qqbot_cli::cli::Commands::Run));
}
