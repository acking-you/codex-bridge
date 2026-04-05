//! CLI smoke tests.

use clap::Parser;
use codex_bridge_cli::cli::Cli;

#[test]
fn parse_run_command() {
    let cli = Cli::try_parse_from(["codex-bridge", "run"]).expect("parse run");
    assert!(matches!(cli.command, codex_bridge_cli::cli::Commands::Run));
}
