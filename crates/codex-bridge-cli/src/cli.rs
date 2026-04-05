//! Command-line argument types.

use clap::{Parser, Subcommand};

/// Top-level CLI definition for the `codex-bridge` binary.
#[derive(Debug, Parser)]
#[command(
    name = "codex-bridge",
    version,
    about = "Codex app-server bridge with the current QQ transport"
)]
pub struct Cli {
    /// Selected subcommand.
    #[command(subcommand)]
    pub command: Commands,
}

/// Supported command-line operations.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Commands {
    /// Run the foreground bridge service.
    Run,
    /// Print the current task snapshot from the local bridge service.
    Status,
    /// Print the current queue snapshot from the local bridge service.
    Queue,
    /// Cancel the currently running task.
    Cancel,
    /// Retry the latest failed or interrupted task for the active conversation.
    RetryLast,
    /// Send a private text message through the local bridge service.
    SendPrivate {
        /// Target QQ user identifier.
        #[arg(long)]
        user_id: i64,
        /// Plain-text message content.
        #[arg(long)]
        text: String,
    },
    /// Send a group text message through the local bridge service.
    SendGroup {
        /// Target QQ group identifier.
        #[arg(long)]
        group_id: i64,
        /// Plain-text message content.
        #[arg(long)]
        text: String,
    },
    /// Print the cached friend list from the local bridge service.
    Friends,
    /// Print the cached group list from the local bridge service.
    Groups,
}
