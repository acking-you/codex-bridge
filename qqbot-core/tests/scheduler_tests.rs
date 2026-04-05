//! Scheduler and task-state transition tests.

use qqbot_core::scheduler::{Scheduler, TaskQueueError, TaskState};

#[test]
fn queue_rejects_the_sixth_waiting_task() {
    let mut scheduler = Scheduler::new(5);
    scheduler.start_task("task-1", "private:1").expect("start");

    for index in 0..5 {
        scheduler
            .enqueue(format!("task-{}", index + 2), format!("private:{}", index + 2))
            .expect("enqueue");
    }

    let error = scheduler
        .enqueue("task-7".to_string(), "private:7".to_string())
        .expect_err("queue full");
    assert_eq!(error, TaskQueueError::QueueFull);
}

#[test]
fn start_task_rejects_when_running_task_exists() {
    let mut scheduler = Scheduler::new(5);
    scheduler.start_task("task-1", "private:1").expect("start");
    let error = scheduler
        .start_task("task-2", "private:2")
        .expect_err("already running");
    assert_eq!(error, TaskQueueError::AlreadyRunning);
}

#[test]
fn retry_last_is_scoped_to_the_same_conversation() {
    let mut scheduler = Scheduler::new(5);
    scheduler
        .record_terminal_state("task-a", "private:1", TaskState::Failed, Some("boom".into()))
        .expect("record terminal");
    scheduler
        .record_terminal_state("task-b", "private:2", TaskState::Interrupted, None)
        .expect("record terminal");

    let retry = scheduler
        .retry_candidate("private:1")
        .expect("retry candidate");
    assert_eq!(retry.task_id, "task-a");
}

#[test]
fn retry_candidate_never_returns_canceled_task() {
    let mut scheduler = Scheduler::new(5);
    scheduler
        .record_terminal_state("task-a", "private:1", TaskState::Canceled, None)
        .expect("record terminal");
    scheduler
        .record_terminal_state("task-b", "private:1", TaskState::Failed, Some("boom".into()))
        .expect("record terminal");

    let retry = scheduler
        .retry_candidate("private:1")
        .expect("retry candidate");
    assert_eq!(retry.task_id, "task-b");
}

#[test]
fn finish_running_moves_next_waiting_task_into_running() {
    let mut scheduler = Scheduler::new(2);
    scheduler
        .start_task("task-1", "private:1")
        .expect("start first task");
    scheduler
        .enqueue("task-2".to_string(), "private:2".to_string())
        .expect("enqueue follow-up");

    let finished = scheduler
        .finish_running(TaskState::Completed, Some("done".into()))
        .expect("finish running");
    let running = scheduler.running().expect("next running");
    assert_eq!(finished.task_id, "task-1");
    assert_eq!(running.task_id, "task-2");
    assert_eq!(running.state, TaskState::Running);
    assert_eq!(scheduler.queue_len(), 0);
}

#[test]
fn finish_running_requires_terminal_state() {
    let mut scheduler = Scheduler::new(2);
    scheduler.start_task("task-1", "private:1").expect("start");

    let error = scheduler
        .finish_running(TaskState::Queued, Some("wrong".into()))
        .expect_err("non-terminal");
    assert_eq!(error, TaskQueueError::NonTerminalState);
}

#[test]
fn terminal_recording_requires_terminal_state() {
    let mut scheduler = Scheduler::new(2);
    let error = scheduler
        .record_terminal_state("task-1", "private:1", TaskState::Queued, None)
        .expect_err("non-terminal");
    assert_eq!(error, TaskQueueError::NonTerminalState);
}

#[test]
fn cancel_running_marks_canceled_and_promotes_next_task() {
    let mut scheduler = Scheduler::new(2);
    scheduler
        .start_task("task-1", "private:1")
        .expect("start first");
    scheduler
        .enqueue("task-2".to_string(), "private:2".to_string())
        .expect("enqueue second");

    let finished = scheduler.cancel_running().expect("cancel running");
    let running = scheduler.running().expect("promoted");

    assert_eq!(finished.task_id, "task-1");
    assert_eq!(finished.state, TaskState::Canceled);
    assert_eq!(running.task_id, "task-2");
    assert_eq!(running.state, TaskState::Running);
}

#[test]
fn finish_running_without_running_task_is_error() {
    let mut scheduler = Scheduler::new(2);
    let error = scheduler
        .finish_running(TaskState::Completed, Some("done".into()))
        .expect_err("no running");
    assert_eq!(error, TaskQueueError::NoRunningTask);
}

#[test]
fn cancel_running_without_running_task_is_error() {
    let mut scheduler = Scheduler::new(2);
    let error = scheduler.cancel_running().expect_err("no running");
    assert_eq!(error, TaskQueueError::NoRunningTask);
}

#[test]
fn terminal_history_is_bounded() {
    let mut scheduler = Scheduler::new(2);
    let capacity = scheduler.terminal_history_capacity();

    for index in 0..(capacity + 5) {
        let state = if index % 2 == 0 { TaskState::Failed } else { TaskState::Interrupted };
        scheduler
            .record_terminal_state(&format!("task-{index}"), "private:1", state, None)
            .expect("record terminal");
    }

    assert_eq!(scheduler.terminal_history_len(), capacity);
    assert!(scheduler.retry_candidate("private:1").is_some());
}
