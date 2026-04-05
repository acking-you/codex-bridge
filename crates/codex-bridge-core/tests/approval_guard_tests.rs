//! Approval guard policy tests.

use codex_bridge_core::approval_guard::{ApprovalDecision, ApprovalGuard};

#[test]
fn approval_guard_allows_process_inspection_but_denies_kill_and_systemctl_restart() {
    let guard = ApprovalGuard::new("/repo");

    assert_eq!(guard.review_command("ps aux", "/tmp", &[]), ApprovalDecision::Allow);

    match guard.review_command("kill 123", "/tmp", &[]) {
        ApprovalDecision::Deny(reason) => assert!(reason.contains("dangerous")),
        other => panic!("expected dangerous deny, got {other:?}"),
    }

    match guard.review_command("systemctl restart qq", "/tmp", &[]) {
        ApprovalDecision::Deny(reason) => assert!(reason.contains("dangerous")),
        other => panic!("expected dangerous deny, got {other:?}"),
    }
}

#[test]
fn approval_guard_denies_non_inspection_shell_commands() {
    let guard = ApprovalGuard::new("/repo");

    match guard.review_command("mkdir /tmp/new-dir", "/tmp", &[]) {
        ApprovalDecision::Deny(reason) => assert!(reason.contains("non-inspection")),
        other => panic!("expected deny, got {other:?}"),
    }
}

#[test]
fn approval_guard_allows_git_status_outside_workspace() {
    let guard = ApprovalGuard::new("/repo");
    assert_eq!(guard.review_command("git status --short", "/tmp", &[]), ApprovalDecision::Allow);
}
