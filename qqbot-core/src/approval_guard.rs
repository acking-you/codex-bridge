//! Local approval policy used for command execution requests.

use std::path::{Component, Path, PathBuf};

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
        cwd: &str,
        writable_roots: &[String],
    ) -> ApprovalDecision {
        if command_is_dangerous(command) {
            return ApprovalDecision::Deny(format!("denied dangerous command: {command}"));
        }

        if !is_cwd_within_workspace(cwd, &self.workspace_root, writable_roots) {
            return ApprovalDecision::Deny(format!("cwd escapes workspace: {cwd}"));
        }

        ApprovalDecision::Allow
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

        if token == "systemctl" {
            if matches!(tokens.get(idx + 1).map(String::as_str), Some("stop" | "restart" | "kill"))
            {
                return true;
            }
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

fn is_prefix_token(token: &str) -> bool {
    matches!(token, "sudo" | "env" | "command")
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

fn is_cwd_within_workspace(cwd: &str, workspace_root: &Path, writable_roots: &[String]) -> bool {
    let allowed_roots = gather_allowed_roots(workspace_root, writable_roots);
    let command_cwd = normalize_path_to_absolute(cwd, workspace_root);

    allowed_roots
        .iter()
        .any(|root| command_cwd.starts_with(root))
}

fn gather_allowed_roots(workspace_root: &Path, writable_roots: &[String]) -> Vec<PathBuf> {
    let mut roots = Vec::with_capacity(writable_roots.len() + 1);
    roots.push(normalize_path(workspace_root.to_path_buf()));
    for root in writable_roots {
        if root.is_empty() {
            continue;
        }

        let parsed = PathBuf::from(root);
        if parsed.is_absolute() {
            roots.push(normalize_path(parsed));
        } else {
            roots.push(normalize_path(workspace_root.join(parsed)));
        }
    }

    roots
}

fn normalize_path_to_absolute(path: &str, base: &Path) -> PathBuf {
    let candidate = PathBuf::from(path);
    let candidate = if candidate.is_absolute() { candidate } else { base.join(candidate) };
    normalize_path(candidate)
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            },
            Component::CurDir => {},
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}
