//! Local approval policy used for command execution requests.

use std::path::{Path, PathBuf};

/// Result of a command-approval check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Command is allowed to run.
    Allow,
    /// Command is denied with a human-readable reason.
    Deny(String),
}

/// Local guard for hard safety checks before command execution.
#[derive(Debug)]
pub struct ApprovalGuard {
    #[allow(dead_code)]
    workspace_root: PathBuf,
}

impl ApprovalGuard {
    /// Build a new guard for a workspace root.
    pub fn new<P: Into<PathBuf>>(workspace_root: P) -> Self {
        let workspace_root = workspace_root.into();

        Self {
            workspace_root,
        }
    }

    /// Review a command request and return an approval decision.
    pub fn review_command(
        &self,
        command: &str,
        _cwd: &str,
        _writable_roots: &[String],
    ) -> ApprovalDecision {
        if command_is_dangerous(command) {
            return ApprovalDecision::Deny(format!("denied dangerous command: {command}"));
        }
        if !command_is_safe_inspection(command) {
            return ApprovalDecision::Deny(format!(
                "denied non-inspection shell command: {command}"
            ));
        }

        ApprovalDecision::Allow
    }

    /// Review a file-change approval request.
    pub fn review_file_change(&self, grant_root: Option<&Path>) -> ApprovalDecision {
        match grant_root {
            Some(root) => ApprovalDecision::Deny(format!(
                "denied extra write root request: {}",
                root.display()
            )),
            None => ApprovalDecision::Allow,
        }
    }
}

fn command_is_dangerous(command: &str) -> bool {
    let tokens = split_tokens(command);
    if tokens.is_empty() {
        return false;
    }

    let mut idx = 0;
    while idx < tokens.len() {
        let token = tokens[idx].as_str();

        if is_prefix_token(token) {
            idx += 1;
            continue;
        }

        if token == "systemctl"
            && matches!(tokens.get(idx + 1).map(String::as_str), Some("stop" | "restart" | "kill"))
        {
            return true;
        }

        if is_dangerous_simple_command(token) {
            return true;
        }

        idx += 1;
    }

    false
}

fn is_dangerous_simple_command(token: &str) -> bool {
    matches!(token, "kill" | "pkill" | "killall" | "reboot" | "shutdown" | "poweroff")
}

fn command_is_safe_inspection(command: &str) -> bool {
    let tokens = split_tokens(command);
    if tokens.is_empty() {
        return false;
    }

    let mut idx = 0;
    while idx < tokens.len() && is_prefix_token(tokens[idx].as_str()) {
        idx += 1;
    }
    let Some(command_name) = tokens.get(idx).map(String::as_str) else {
        return false;
    };

    match command_name {
        "ls" | "pwd" | "cat" | "rg" | "grep" | "find" | "head" | "tail" | "sed" | "awk"
        | "ps" | "ss" | "lsof" | "top" | "free" | "df" | "du" | "curl" | "wget"
        | "uname" | "whoami" | "id" | "env" | "printenv" | "stat" | "file" | "which"
        | "whereis" | "readlink" | "realpath" | "date" | "netstat" | "journalctl" | "wc"
        | "sort" | "uniq" | "cut" | "tr" => true,
        "git" => git_subcommand_is_safe(tokens.get(idx + 1).map(String::as_str)),
        _ => false,
    }
}

fn git_subcommand_is_safe(subcommand: Option<&str>) -> bool {
    matches!(
        subcommand,
        Some("status" | "diff" | "show" | "log" | "branch" | "remote" | "rev-parse")
    )
}

fn is_prefix_token(token: &str) -> bool {
    matches!(token, "sudo" | "env" | "command")
        || (token.contains('=') && !token.starts_with('-') && !token.contains('/'))
}

fn split_tokens(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .filter_map(|token| {
            let token = token.trim_matches(|ch: char| {
                ch == '('
                    || ch == ')'
                    || ch == ';'
                    || ch == '&'
                    || ch == '`'
                    || ch == '"'
                    || ch == '\''
                    || ch == '|'
            });

            (!token.is_empty()).then(|| token.to_lowercase())
        })
        .collect()
}
