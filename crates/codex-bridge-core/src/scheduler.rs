use std::collections::VecDeque;

/// Task lifecycle state persisted by the in-memory scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Task has been queued and is waiting to run.
    Queued,
    /// Task is currently running.
    Running,
    /// Task completed successfully.
    Completed,
    /// Task failed during execution.
    Failed,
    /// Task was canceled by the user.
    Canceled,
    /// Task was interrupted by runtime interruption.
    Interrupted,
}

/// Snapshot used by status and retry flows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSummary {
    /// Stable task identifier.
    pub task_id: String,
    /// Stable conversation key the task belongs to.
    pub conversation_key: String,
    /// QQ identifier of the user that initiated the task.
    pub owner_sender_id: i64,
    /// Source QQ message identifier.
    pub source_message_id: i64,
    /// Current lifecycle state.
    pub state: TaskState,
    /// Optional short summary for terminal states.
    pub summary: Option<String>,
}

/// Queue-level scheduling errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskQueueError {
    /// No more queued slots are available.
    QueueFull,
}

/// Single running-task scheduler with bounded waiting queue.
pub struct Scheduler {
    /// Maximum number of waiting tasks in the queue.
    queue_capacity: usize,
    /// Currently running task.
    running: Option<TaskSummary>,
    /// FIFO queue of waiting tasks.
    queued: VecDeque<TaskSummary>,
    /// Recent terminal task history, newest at end.
    last_terminal: VecDeque<TaskSummary>,
}

impl Scheduler {
    /// Default capacity for terminal task history snapshots.
    const DEFAULT_TERMINAL_HISTORY_CAPACITY: usize = 16;

    /// Build a scheduler with an explicit queue capacity.
    pub fn new(queue_capacity: usize) -> Self {
        Self {
            queue_capacity,
            running: None,
            queued: VecDeque::new(),
            last_terminal: VecDeque::new(),
        }
    }

    /// Set the scheduler into running state for the given task.
    pub fn start_task(
        &mut self,
        task_id: &str,
        conversation_key: &str,
        owner_sender_id: i64,
        source_message_id: i64,
    ) -> Result<(), TaskQueueError> {
        assert!(self.running.is_none(), "attempted to start a task while another task was running");
        self.running = Some(TaskSummary {
            task_id: task_id.to_string(),
            conversation_key: conversation_key.to_string(),
            owner_sender_id,
            source_message_id,
            state: TaskState::Running,
            summary: None,
        });
        Ok(())
    }

    /// Enqueue a task and return the new queue length, or error if full.
    pub fn enqueue(
        &mut self,
        task_id: String,
        conversation_key: String,
        owner_sender_id: i64,
        source_message_id: i64,
    ) -> Result<usize, TaskQueueError> {
        if self.queued.len() >= self.queue_capacity {
            return Err(TaskQueueError::QueueFull);
        }

        self.queued.push_back(TaskSummary {
            task_id,
            conversation_key,
            owner_sender_id,
            source_message_id,
            state: TaskState::Queued,
            summary: None,
        });
        Ok(self.queued.len())
    }

    /// Finish current running task and promote the next queued task if present.
    pub fn finish_running(
        &mut self,
        state: TaskState,
        summary: Option<String>,
    ) -> Option<TaskSummary> {
        assert!(
            Self::is_terminal_state(state),
            "attempted to finish with non-terminal state: {state:?}"
        );
        let mut finished = self.running.take()?;
        finished.state = state;
        finished.summary = summary;
        self.push_terminal(finished.clone());

        if let Some(next) = self.queued.pop_front() {
            self.running = Some(TaskSummary {
                state: TaskState::Running,
                ..next
            });
        }

        Some(finished)
    }

    /// Append a terminal task snapshot for retry and status views.
    pub fn record_terminal_state(
        &mut self,
        task_id: &str,
        conversation_key: &str,
        owner_sender_id: i64,
        source_message_id: i64,
        state: TaskState,
        summary: Option<String>,
    ) {
        assert!(
            Self::is_terminal_state(state),
            "attempted to record non-terminal state: {state:?}"
        );

        self.push_terminal(TaskSummary {
            task_id: task_id.to_string(),
            conversation_key: conversation_key.to_string(),
            owner_sender_id,
            source_message_id,
            state,
            summary,
        });
    }

    /// Return the latest terminal task candidate for retry in the same
    /// conversation.
    pub fn retry_candidate(
        &self,
        conversation_key: &str,
        owner_sender_id: i64,
    ) -> Option<TaskSummary> {
        self.last_terminal.iter().rev().find_map(|task| {
            if task.conversation_key == conversation_key
                && task.owner_sender_id == owner_sender_id
                && matches!(task.state, TaskState::Failed | TaskState::Interrupted)
            {
                Some(task.clone())
            } else {
                None
            }
        })
    }

    /// Current running task, if any.
    pub fn running(&self) -> Option<&TaskSummary> {
        self.running.as_ref()
    }

    /// Number of waiting tasks.
    pub fn queue_len(&self) -> usize {
        self.queued.len()
    }

    /// Most recent terminal task snapshot.
    pub fn last_terminal(&self) -> Option<&TaskSummary> {
        self.last_terminal.back()
    }

    /// Human-readable preview of queued tasks.
    pub fn queue_preview(&self) -> String {
        if self.queued.is_empty() {
            return "队列为空。".to_string();
        }

        self.queued
            .iter()
            .enumerate()
            .map(|(index, task)| {
                format!("{}. {} ({})", index + 1, task.task_id, task.conversation_key)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Mark the running task as canceled and promote the next queued task.
    pub fn cancel_running(&mut self) -> Option<TaskSummary> {
        self.finish_running(TaskState::Canceled, Some("用户取消".to_string()))
    }

    fn is_terminal_state(state: TaskState) -> bool {
        matches!(
            state,
            TaskState::Completed | TaskState::Failed | TaskState::Canceled | TaskState::Interrupted
        )
    }

    fn push_terminal(&mut self, task: TaskSummary) {
        self.last_terminal.push_back(task);
        if self.last_terminal.len() > Self::DEFAULT_TERMINAL_HISTORY_CAPACITY {
            let _ = self.last_terminal.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_history_is_bounded() {
        let mut scheduler = Scheduler::new(2);
        for index in 0..25 {
            let state = if index % 2 == 0 { TaskState::Failed } else { TaskState::Interrupted };
            scheduler.record_terminal_state(
                &format!("task-{index}"),
                "private:1",
                42,
                1000 + index,
                state,
                None,
            );
        }

        assert_eq!(scheduler.last_terminal.len(), Scheduler::DEFAULT_TERMINAL_HISTORY_CAPACITY);
    }
}
