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
    /// A task is already running.
    AlreadyRunning,
    /// No running task exists to finish or cancel.
    NoRunningTask,
    /// Non-terminal state passed where terminal state is required.
    NonTerminalState,
}

/// Single running-task scheduler with bounded waiting queue.
pub struct Scheduler {
    /// Maximum number of waiting tasks in the queue.
    queue_capacity: usize,
    /// Maximum number of terminal tasks remembered for retry and status.
    terminal_history_capacity: usize,
    /// Currently running task.
    running: Option<TaskSummary>,
    /// FIFO queue of waiting tasks.
    queued: VecDeque<TaskSummary>,
    /// Recent terminal task history, newest at end.
    last_terminal: VecDeque<TaskSummary>,
}

impl Scheduler {
    /// Build a scheduler with an explicit queue capacity.
    pub fn new(queue_capacity: usize) -> Self {
        Self {
            queue_capacity,
            terminal_history_capacity: 16,
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
    ) -> Result<(), TaskQueueError> {
        if self.running.is_some() {
            return Err(TaskQueueError::AlreadyRunning);
        }
        self.running = Some(TaskSummary {
            task_id: task_id.to_string(),
            conversation_key: conversation_key.to_string(),
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
    ) -> Result<usize, TaskQueueError> {
        if self.queued.len() >= self.queue_capacity {
            return Err(TaskQueueError::QueueFull);
        }

        self.queued.push_back(TaskSummary {
            task_id,
            conversation_key,
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
    ) -> Result<TaskSummary, TaskQueueError> {
        if !Self::is_terminal_state(state) {
            return Err(TaskQueueError::NonTerminalState);
        }
        let mut finished = self.running.take().ok_or(TaskQueueError::NoRunningTask)?;
        finished.state = state;
        finished.summary = summary;
        self.push_terminal(finished.clone());

        if let Some(next) = self.queued.pop_front() {
            self.running = Some(TaskSummary {
                state: TaskState::Running,
                ..next
            });
        }

        Ok(finished)
    }

    /// Append a terminal task snapshot for retry and status views.
    pub fn record_terminal_state(
        &mut self,
        task_id: &str,
        conversation_key: &str,
        state: TaskState,
        summary: Option<String>,
    ) -> Result<(), TaskQueueError> {
        if !Self::is_terminal_state(state) {
            return Err(TaskQueueError::NonTerminalState);
        }

        self.push_terminal(TaskSummary {
            task_id: task_id.to_string(),
            conversation_key: conversation_key.to_string(),
            state,
            summary,
        });
        Ok(())
    }

    /// Return the latest terminal task candidate for retry in the same
    /// conversation.
    pub fn retry_candidate(&self, conversation_key: &str) -> Option<TaskSummary> {
        self.last_terminal.iter().rev().find_map(|task| {
            if task.conversation_key == conversation_key
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

    /// Current terminal history capacity.
    pub fn terminal_history_capacity(&self) -> usize {
        self.terminal_history_capacity
    }

    /// Current terminal history length.
    pub fn terminal_history_len(&self) -> usize {
        self.last_terminal.len()
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
    pub fn cancel_running(&mut self) -> Result<TaskSummary, TaskQueueError> {
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
        if self.last_terminal.len() > self.terminal_history_capacity {
            let _ = self.last_terminal.pop_front();
        }
    }
}
