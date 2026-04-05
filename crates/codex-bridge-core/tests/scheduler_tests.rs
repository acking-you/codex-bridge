//! Scheduler and task-state transition tests.

use codex_bridge_core::scheduler::{Scheduler, TaskQueueError, TaskState};

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
#[should_panic(expected = "attempted to start a task while another task was running")]
fn start_task_rejects_when_running_task_exists() {
    let mut scheduler = Scheduler::new(5);
    scheduler.start_task("task-1", "private:1").expect("start");
    let _ = scheduler.start_task("task-2", "private:2");
}

#[test]
fn retry_last_is_scoped_to_the_same_conversation() {
    let mut scheduler = Scheduler::new(5);
    scheduler.record_terminal_state("task-a", "private:1", TaskState::Failed, Some("boom".into()));
    scheduler.record_terminal_state("task-b", "private:2", TaskState::Interrupted, None);

    let retry = scheduler
        .retry_candidate("private:1")
        .expect("retry candidate");
    assert_eq!(retry.task_id, "task-a");
}

#[test]
fn retry_candidate_never_returns_canceled_task() {
    let mut scheduler = Scheduler::new(5);
    scheduler.record_terminal_state("task-a", "private:1", TaskState::Canceled, None);
    scheduler.record_terminal_state("task-b", "private:1", TaskState::Failed, Some("boom".into()));

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
#[should_panic(expected = "attempted to finish with non-terminal state")]
fn finish_running_requires_terminal_state() {
    let mut scheduler = Scheduler::new(2);
    scheduler.start_task("task-1", "private:1").expect("start");

    let _ = scheduler.finish_running(TaskState::Queued, Some("wrong".into()));
}

#[test]
#[should_panic(expected = "attempted to record non-terminal state")]
fn terminal_recording_requires_terminal_state() {
    let mut scheduler = Scheduler::new(2);
    scheduler.record_terminal_state("task-1", "private:1", TaskState::Queued, None);
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
fn finish_running_without_running_task_is_none() {
    let mut scheduler = Scheduler::new(2);
    let finished = scheduler
        .finish_running(TaskState::Completed, Some("done".into()))
        .is_none();
    assert!(finished, "expected no running task to finish");
    assert!(scheduler.cancel_running().is_none());
}

#[test]
fn retry_only_returns_failed_or_interrupted() {
    let mut scheduler = Scheduler::new(2);
    scheduler.record_terminal_state("task-completed", "private:1", TaskState::Completed, None);
    scheduler.record_terminal_state("task-canceled", "private:1", TaskState::Canceled, None);

    let retry = scheduler.retry_candidate("private:1");
    assert!(retry.is_none());
}

#[test]
fn start_can_be_called_after_finish() {
    let mut scheduler = Scheduler::new(2);
    scheduler
        .start_task("task-1", "private:1")
        .expect("start first");
    scheduler
        .finish_running(TaskState::Completed, Some("ok".into()))
        .expect("finished first");

    scheduler
        .start_task("task-2", "private:2")
        .expect("start second after finish");
    assert_eq!(scheduler.running().expect("running").task_id, "task-2");
}
