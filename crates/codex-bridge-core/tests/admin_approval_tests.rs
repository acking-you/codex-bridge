//! Admin approval pool tests.

use std::time::{Duration, Instant};

use codex_bridge_core::{
    admin_approval::{PendingApproval, PendingApprovalError, PendingApprovalPool},
    message_router::TaskRequest,
};

fn make_task(conversation_key: &str, source_sender_id: i64, source_message_id: i64) -> TaskRequest {
    TaskRequest {
        conversation_key: conversation_key.to_string(),
        source_message_id,
        source_sender_id,
        source_sender_name: "alice".to_string(),
        source_text: "帮我执行".to_string(),
        is_group: conversation_key.starts_with("group:"),
        reply_target_id: if conversation_key.starts_with("group:") {
            777
        } else {
            source_sender_id
        },
    }
}

#[test]
fn pending_pool_rejects_duplicate_conversation_while_waiting() {
    let mut pool = PendingApprovalPool::new(32);
    let now = Instant::now();

    assert!(pool
        .insert(PendingApproval::new(
            "task-1".to_string(),
            make_task("group:9", 42, 9001),
            now,
            Duration::from_secs(900),
        ))
        .is_ok());

    let error = pool
        .insert(PendingApproval::new(
            "task-2".to_string(),
            make_task("group:9", 42, 9002),
            now,
            Duration::from_secs(900),
        ))
        .expect_err("second pending approval for same conversation should fail");

    assert_eq!(error, PendingApprovalError::ConversationAlreadyWaiting);
}

#[test]
fn pending_pool_expires_elapsed_requests() {
    let mut pool = PendingApprovalPool::new(32);
    let now = Instant::now();
    pool.insert(PendingApproval::new(
        "task-1".to_string(),
        make_task("private:42", 42, 9001),
        now,
        Duration::from_secs(1),
    ))
    .expect("insert pending approval");

    let expired = pool.take_expired(now + Duration::from_secs(2));

    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].task_id, "task-1");
    assert!(pool.get("task-1").is_none());
}
