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
