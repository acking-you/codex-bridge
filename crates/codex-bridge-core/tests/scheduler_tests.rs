//! Scheduler and task-state transition tests.

use codex_bridge_core::scheduler::{Scheduler, TaskQueueError, TaskState};

const OWNER_1: i64 = 101;
const OWNER_2: i64 = 202;

#[test]
fn queue_rejects_the_sixth_waiting_task() {
    let mut scheduler = Scheduler::new(5, 5);
    scheduler
        .start_task("task-1", "private:1", OWNER_1, 1001)
        .expect("start");

    for index in 0..5 {
        scheduler
            .enqueue(
                format!("task-{}", index + 2),
                format!("private:{}", index + 2),
                OWNER_1,
                2000 + index as i64,
            )
            .expect("enqueue");
    }

    let error = scheduler
        .enqueue("task-7".to_string(), "private:7".to_string(), OWNER_1, 2007)
        .expect_err("queue full");
    assert_eq!(error, TaskQueueError::QueueFull);
}

#[test]
#[should_panic(expected = "second concurrent task")]
fn start_task_rejects_parallel_start_for_same_conversation() {
    let mut scheduler = Scheduler::new(5, 5);
    scheduler
        .start_task("task-1", "private:1", OWNER_1, 1001)
        .expect("start");
    let _ = scheduler.start_task("task-2", "private:1", OWNER_2, 1002);
}

#[test]
fn retry_last_is_scoped_to_the_same_conversation_and_owner() {
    let mut scheduler = Scheduler::new(5, 5);
    scheduler.record_terminal_state(
        "task-a",
        "private:1",
        OWNER_1,
        3001,
        TaskState::Failed,
        Some("boom".into()),
    );
    scheduler.record_terminal_state(
        "task-b",
        "private:2",
        OWNER_2,
        3002,
        TaskState::Interrupted,
        None,
    );

    let retry = scheduler
        .retry_candidate("private:1", OWNER_1)
        .expect("retry candidate");
    assert_eq!(retry.task_id, "task-a");
}

#[test]
fn retry_candidate_never_returns_canceled_task() {
    let mut scheduler = Scheduler::new(5, 5);
    scheduler.record_terminal_state(
        "task-a",
        "private:1",
        OWNER_1,
        3001,
        TaskState::Canceled,
        None,
    );
    scheduler.record_terminal_state(
        "task-b",
        "private:1",
        OWNER_1,
        3002,
        TaskState::Failed,
        Some("boom".into()),
    );

    let retry = scheduler
        .retry_candidate("private:1", OWNER_1)
        .expect("retry candidate");
    assert_eq!(retry.task_id, "task-b");
}

#[test]
fn finish_running_marks_next_task_ready_within_the_same_conversation() {
    let mut scheduler = Scheduler::new(2, 2);
    scheduler
        .start_task("task-1", "private:1", OWNER_1, 1001)
        .expect("start first task");
    scheduler
        .enqueue("task-2".to_string(), "private:1".to_string(), OWNER_2, 1002)
        .expect("enqueue follow-up on same conversation");

    let finished = scheduler
        .finish_running("private:1", TaskState::Completed, Some("done".into()))
        .expect("finish running");
    assert_eq!(finished.task_id, "task-1");
    assert_eq!(finished.owner_sender_id, OWNER_1);
    assert!(scheduler.running_for("private:1").is_none());
    assert_eq!(scheduler.ready_len(), 1);
    assert_eq!(scheduler.pop_ready_lane().as_deref(), Some("private:1"));
    let running = scheduler
        .promote_queued("private:1")
        .expect("explicit promote");
    assert_eq!(running.task_id, "task-2");
    assert_eq!(running.owner_sender_id, OWNER_2);
    assert_eq!(running.state, TaskState::Running);
    assert_eq!(scheduler.queue_len(), 0);
}

#[test]
#[should_panic(expected = "attempted to finish with non-terminal state")]
fn finish_running_requires_terminal_state() {
    let mut scheduler = Scheduler::new(2, 2);
    scheduler
        .start_task("task-1", "private:1", OWNER_1, 1001)
        .expect("start");

    let _ = scheduler.finish_running("private:1", TaskState::Queued, Some("wrong".into()));
}

#[test]
#[should_panic(expected = "attempted to record non-terminal state")]
fn terminal_recording_requires_terminal_state() {
    let mut scheduler = Scheduler::new(2, 2);
    scheduler.record_terminal_state("task-1", "private:1", OWNER_1, 1001, TaskState::Queued, None);
}

#[test]
fn cancel_running_marks_canceled_and_readies_next_task_within_the_same_conversation() {
    let mut scheduler = Scheduler::new(2, 2);
    scheduler
        .start_task("task-1", "private:1", OWNER_1, 1001)
        .expect("start first");
    scheduler
        .enqueue("task-2".to_string(), "private:1".to_string(), OWNER_2, 1002)
        .expect("enqueue follow-up on same conversation");

    let finished = scheduler
        .cancel_running("private:1")
        .expect("cancel running");

    assert_eq!(finished.task_id, "task-1");
    assert_eq!(finished.state, TaskState::Canceled);
    assert!(scheduler.running_for("private:1").is_none());
    assert_eq!(scheduler.pop_ready_lane().as_deref(), Some("private:1"));
    let running = scheduler.promote_queued("private:1").expect("promoted");
    assert_eq!(running.task_id, "task-2");
    assert_eq!(running.state, TaskState::Running);
}

#[test]
fn finish_running_without_running_task_is_none() {
    let mut scheduler = Scheduler::new(2, 2);
    let finished = scheduler
        .finish_running("private:1", TaskState::Completed, Some("done".into()))
        .is_none();
    assert!(finished, "expected no running task to finish");
    assert!(scheduler.cancel_running("private:1").is_none());
}

#[test]
fn retry_only_returns_failed_or_interrupted() {
    let mut scheduler = Scheduler::new(2, 2);
    scheduler.record_terminal_state(
        "task-completed",
        "private:1",
        OWNER_1,
        1001,
        TaskState::Completed,
        None,
    );
    scheduler.record_terminal_state(
        "task-canceled",
        "private:1",
        OWNER_1,
        1002,
        TaskState::Canceled,
        None,
    );

    let retry = scheduler.retry_candidate("private:1", OWNER_1);
    assert!(retry.is_none());
}

#[test]
fn start_can_be_called_after_finish() {
    let mut scheduler = Scheduler::new(2, 2);
    scheduler
        .start_task("task-1", "private:1", OWNER_1, 1001)
        .expect("start first");
    scheduler
        .finish_running("private:1", TaskState::Completed, Some("ok".into()))
        .expect("finished first");

    scheduler
        .start_task("task-2", "private:2", OWNER_2, 1002)
        .expect("start second after finish");
    assert_eq!(scheduler.running_for("private:2").expect("running").task_id, "task-2");
}

#[test]
fn different_conversations_run_concurrently() {
    let mut scheduler = Scheduler::new(2, 2);
    scheduler
        .start_task("task-a", "private:1", OWNER_1, 1001)
        .expect("start first");
    scheduler
        .start_task("task-b", "group:9", OWNER_2, 1002)
        .expect("start second in distinct conversation");
    assert_eq!(scheduler.running_all().len(), 2);
}

#[test]
fn per_lane_capacity_rejects_second_waiting_turn() {
    let mut scheduler = Scheduler::new(8, 1);
    scheduler
        .enqueue("task-a".to_string(), "private:1".to_string(), OWNER_1, 1001)
        .expect("enqueue first");

    let error = scheduler
        .enqueue("task-b".to_string(), "private:1".to_string(), OWNER_1, 1002)
        .expect_err("lane full");
    assert_eq!(error, TaskQueueError::LaneFull);
}
