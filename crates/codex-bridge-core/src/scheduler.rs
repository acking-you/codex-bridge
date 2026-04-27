use std::collections::{HashMap, HashSet, VecDeque};

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
    /// This conversation already buffered the maximum pending turns.
    LaneFull,
}

/// Per-conversation scheduler: each conversation_key may have at most one
/// running task and keeps its own FIFO wait list. Queue capacity is enforced
/// across the union of all per-conversation wait lists.
pub struct Scheduler {
    /// Total capacity of waiting tasks summed across all conversations.
    queue_capacity: usize,
    /// Maximum number of waiting tasks buffered inside one conversation.
    lane_pending_capacity: usize,
    /// Running tasks keyed by conversation_key.
    running: HashMap<String, TaskSummary>,
    /// Per-conversation FIFO wait lists.
    queued: HashMap<String, VecDeque<TaskSummary>>,
    /// FIFO of conversation lanes that currently have queued work and are
    /// eligible to take the next runtime slot.
    ready_lanes: VecDeque<String>,
    /// Deduplication set for [`Self::ready_lanes`].
    ready_lane_keys: HashSet<String>,
    /// Recent terminal task history, newest at end.
    last_terminal: VecDeque<TaskSummary>,
}

impl Scheduler {
    /// Default capacity for terminal task history snapshots.
    const DEFAULT_TERMINAL_HISTORY_CAPACITY: usize = 16;

    /// Build a scheduler with an explicit queue capacity.
    pub fn new(queue_capacity: usize, lane_pending_capacity: usize) -> Self {
        Self {
            queue_capacity,
            lane_pending_capacity,
            running: HashMap::new(),
            queued: HashMap::new(),
            ready_lanes: VecDeque::new(),
            ready_lane_keys: HashSet::new(),
            last_terminal: VecDeque::new(),
        }
    }

    /// Start a task as running for its conversation.
    pub fn start_task(
        &mut self,
        task_id: &str,
        conversation_key: &str,
        owner_sender_id: i64,
        source_message_id: i64,
    ) -> Result<(), TaskQueueError> {
        assert!(
            !self.running.contains_key(conversation_key),
            "attempted to start a second concurrent task on conversation {conversation_key}"
        );
        self.ready_lane_keys.remove(conversation_key);
        self.running.insert(
            conversation_key.to_string(),
            TaskSummary {
                task_id: task_id.to_string(),
                conversation_key: conversation_key.to_string(),
                owner_sender_id,
                source_message_id,
                state: TaskState::Running,
                summary: None,
            },
        );
        Ok(())
    }

    /// Enqueue a task onto its conversation's wait list and return the new
    /// length of that list, or an error if the global capacity is exhausted.
    pub fn enqueue(
        &mut self,
        task_id: String,
        conversation_key: String,
        owner_sender_id: i64,
        source_message_id: i64,
    ) -> Result<usize, TaskQueueError> {
        if self.queue_len() >= self.queue_capacity {
            return Err(TaskQueueError::QueueFull);
        }

        let list = self.queued.entry(conversation_key.clone()).or_default();
        if list.len() >= self.lane_pending_capacity {
            return Err(TaskQueueError::LaneFull);
        }
        list.push_back(TaskSummary {
            task_id,
            conversation_key: conversation_key.clone(),
            owner_sender_id,
            source_message_id,
            state: TaskState::Queued,
            summary: None,
        });
        if !self.running.contains_key(&conversation_key)
            && !self.ready_lane_keys.contains(&conversation_key)
        {
            self.ready_lanes.push_back(conversation_key.clone());
            self.ready_lane_keys.insert(conversation_key);
        }
        Ok(list.len())
    }

    /// Mark the running task for a conversation as terminal.
    ///
    /// When the same lane still has queued work, it is re-added to the global
    /// ready-lane FIFO instead of being auto-promoted. This preserves
    /// cross-lane fairness once a runtime slot becomes free.
    pub fn finish_running(
        &mut self,
        conversation_key: &str,
        state: TaskState,
        summary: Option<String>,
    ) -> Option<TaskSummary> {
        assert!(
            Self::is_terminal_state(state),
            "attempted to finish with non-terminal state: {state:?}"
        );
        let mut finished = self.running.remove(conversation_key)?;
        finished.state = state;
        finished.summary = summary;
        self.push_terminal(finished.clone());

        if self
            .queued
            .get(conversation_key)
            .is_some_and(|list| !list.is_empty())
        {
            self.mark_lane_ready(conversation_key);
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
    /// conversation, filtered by owner.
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

    /// Return the latest terminal retry candidate for a conversation regardless
    /// of task owner.
    pub fn retry_candidate_any_owner(&self, conversation_key: &str) -> Option<TaskSummary> {
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

    /// Return the running task for a specific conversation.
    pub fn running_for(&self, conversation_key: &str) -> Option<&TaskSummary> {
        self.running.get(conversation_key)
    }

    /// Return one representative running task (an arbitrary one, for legacy
    /// single-running-slot views). Returns the lexicographically first
    /// conversation's running task so the value is stable across calls.
    pub fn running(&self) -> Option<&TaskSummary> {
        self.running
            .iter()
            .min_by_key(|(key, _)| key.as_str())
            .map(|(_, summary)| summary)
    }

    /// Return all currently running tasks.
    pub fn running_all(&self) -> Vec<&TaskSummary> {
        self.running.values().collect()
    }

    /// Total number of queued tasks across all conversations.
    pub fn queue_len(&self) -> usize {
        self.queued.values().map(|list| list.len()).sum()
    }

    /// Number of conversation lanes currently waiting for a runtime slot.
    pub fn ready_len(&self) -> usize {
        self.ready_lane_keys.len()
    }

    /// Remove and return the oldest ready lane that still has queued work.
    pub fn pop_ready_lane(&mut self) -> Option<String> {
        while let Some(conversation_key) = self.ready_lanes.pop_front() {
            self.ready_lane_keys.remove(&conversation_key);
            if self.running.contains_key(&conversation_key) {
                continue;
            }
            if self
                .queued
                .get(&conversation_key)
                .is_some_and(|list| !list.is_empty())
            {
                return Some(conversation_key);
            }
        }
        None
    }

    /// Promote the next queued task for one lane into running state.
    pub fn promote_queued(&mut self, conversation_key: &str) -> Option<TaskSummary> {
        assert!(
            !self.running.contains_key(conversation_key),
            "attempted to promote a queued task while conversation {conversation_key} is already \
             running"
        );
        self.ready_lane_keys.remove(conversation_key);
        let list = self.queued.get_mut(conversation_key)?;
        let next = list.pop_front()?;
        let running = TaskSummary { state: TaskState::Running, ..next };
        self.running
            .insert(conversation_key.to_string(), running.clone());
        if list.is_empty() {
            self.queued.remove(conversation_key);
        }
        Some(running)
    }

    /// Return pending queue counts grouped by conversation.
    pub fn queued_counts(&self) -> Vec<(String, usize)> {
        let mut counts = self
            .queued
            .iter()
            .filter_map(|(conversation_key, list)| {
                if list.is_empty() {
                    None
                } else {
                    Some((conversation_key.clone(), list.len()))
                }
            })
            .collect::<Vec<_>>();
        counts.sort_by(|left, right| left.0.cmp(&right.0));
        counts
    }

    /// Most recent terminal task snapshot.
    pub fn last_terminal(&self) -> Option<&TaskSummary> {
        self.last_terminal.back()
    }

    /// Human-readable preview of queued tasks, grouped by conversation.
    pub fn queue_preview(&self) -> String {
        if self.queue_len() == 0 {
            return "队列为空。".to_string();
        }

        let mut lines = Vec::new();
        let mut keys: Vec<&String> = self.queued.keys().collect();
        keys.sort();
        for key in keys {
            if let Some(list) = self.queued.get(key) {
                if list.is_empty() {
                    continue;
                }
                lines.push(format!("[{key}]"));
                for (index, task) in list.iter().enumerate() {
                    lines.push(format!("  {}. {}", index + 1, task.task_id));
                }
            }
        }
        lines.join("\n")
    }

    /// Mark the running task of one conversation as canceled and promote its
    /// next queued task, if any.
    pub fn cancel_running(&mut self, conversation_key: &str) -> Option<TaskSummary> {
        self.finish_running(conversation_key, TaskState::Canceled, Some("用户取消".to_string()))
    }

    fn is_terminal_state(state: TaskState) -> bool {
        matches!(
            state,
            TaskState::Completed | TaskState::Failed | TaskState::Canceled | TaskState::Interrupted
        )
    }

    fn mark_lane_ready(&mut self, conversation_key: &str) {
        if self.running.contains_key(conversation_key)
            || self.ready_lane_keys.contains(conversation_key)
        {
            return;
        }
        self.ready_lanes.push_back(conversation_key.to_string());
        self.ready_lane_keys.insert(conversation_key.to_string());
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
        let mut scheduler = Scheduler::new(2, 2);
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

    #[test]
    fn distinct_conversations_can_run_concurrently() {
        let mut scheduler = Scheduler::new(8, 8);
        scheduler
            .start_task("a1", "private:1", 1, 100)
            .expect("start a1");
        scheduler
            .start_task("b1", "group:2", 2, 200)
            .expect("start b1");
        assert_eq!(scheduler.running_all().len(), 2);
        assert_eq!(scheduler.queue_len(), 0);
        assert!(scheduler.running_for("private:1").is_some());
        assert!(scheduler.running_for("group:2").is_some());
    }

    #[test]
    #[should_panic(expected = "second concurrent task")]
    fn same_conversation_rejects_parallel_start() {
        let mut scheduler = Scheduler::new(8, 8);
        scheduler
            .start_task("a1", "private:1", 1, 100)
            .expect("start a1");
        let _ = scheduler.start_task("a2", "private:1", 1, 101);
    }

    #[test]
    fn queue_respects_global_capacity() {
        let mut scheduler = Scheduler::new(1, 8);
        scheduler
            .enqueue("a1".to_string(), "private:1".to_string(), 1, 100)
            .expect("enqueue a1");
        let err = scheduler
            .enqueue("a2".to_string(), "group:2".to_string(), 2, 200)
            .unwrap_err();
        assert_eq!(err, TaskQueueError::QueueFull);
    }

    #[test]
    fn finish_marks_same_conversation_ready_without_auto_promotion() {
        let mut scheduler = Scheduler::new(8, 8);
        scheduler
            .start_task("a1", "private:1", 1, 100)
            .expect("start a1");
        scheduler
            .enqueue("a2".to_string(), "private:1".to_string(), 1, 101)
            .expect("enqueue a2");
        let finished = scheduler
            .finish_running("private:1", TaskState::Completed, None)
            .expect("finish a1");
        assert_eq!(finished.task_id, "a1");
        assert!(scheduler.running_for("private:1").is_none());
        assert_eq!(scheduler.ready_len(), 1);
        assert_eq!(scheduler.pop_ready_lane().as_deref(), Some("private:1"));
        let running = scheduler
            .promote_queued("private:1")
            .expect("a2 promoted explicitly");
        assert_eq!(running.task_id, "a2");
        assert_eq!(scheduler.queue_len(), 0);
    }

    #[test]
    fn cancel_respects_conversation_scope() {
        let mut scheduler = Scheduler::new(8, 8);
        scheduler
            .start_task("a1", "private:1", 1, 100)
            .expect("start a1");
        scheduler
            .start_task("b1", "group:2", 2, 200)
            .expect("start b1");
        scheduler.cancel_running("private:1");
        assert!(scheduler.running_for("private:1").is_none());
        assert!(scheduler.running_for("group:2").is_some());
    }

    #[test]
    fn lane_capacity_is_enforced_per_conversation() {
        let mut scheduler = Scheduler::new(8, 1);
        scheduler
            .enqueue("a1".to_string(), "private:1".to_string(), 1, 100)
            .expect("enqueue first");
        let err = scheduler
            .enqueue("a2".to_string(), "private:1".to_string(), 1, 101)
            .expect_err("lane full");
        assert_eq!(err, TaskQueueError::LaneFull);
    }
}
