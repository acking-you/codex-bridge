//! Lane and runtime snapshot models for multi-conversation scheduling.

use serde::{Deserialize, Serialize};

/// Runtime state for one QQ conversation lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LaneRuntimeState {
    /// The lane has no queued or running work.
    #[default]
    Idle,
    /// The lane has queued work waiting for a runtime slot.
    Queued,
    /// The lane is currently executing on one runtime slot.
    Running,
    /// The lane is blocked by a lane-local failure and requires intervention.
    Blocked,
}

/// Public snapshot for one conversation lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LaneSnapshot {
    /// Stable conversation identifier such as `group:123` or `private:456`.
    pub conversation_key: String,
    /// Codex thread id bound to this lane, when one already exists.
    pub thread_id: Option<String>,
    /// Current lane runtime state.
    pub state: LaneRuntimeState,
    /// Number of pending turns buffered behind the currently active turn.
    pub pending_turn_count: usize,
    /// Active task id when the lane is running.
    pub active_task_id: Option<String>,
    /// RFC3339 timestamp for when the active lane turn started.
    pub active_since: Option<String>,
    /// RFC3339 timestamp for the most recent progress observed on this lane.
    pub last_progress_at: Option<String>,
    /// Summary from the most recent terminal turn for this lane.
    pub last_terminal_summary: Option<String>,
}

/// Runtime state for one app-server slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSlotState {
    /// The slot is available for lease.
    #[default]
    Idle,
    /// The slot is currently executing one lane turn.
    Busy,
    /// The slot is unhealthy and should be replaced.
    Broken,
}

/// Public snapshot for one runtime slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuntimeSlotSnapshot {
    /// Stable slot identifier inside the runtime pool.
    pub slot_id: usize,
    /// Current runtime slot state.
    pub state: RuntimeSlotState,
    /// Conversation key currently assigned to the slot, when busy.
    pub assigned_conversation_key: Option<String>,
}

/// Aggregate runtime snapshot exposed by the local API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuntimeSnapshot {
    /// Snapshot of all known conversation lanes.
    pub lanes: Vec<LaneSnapshot>,
    /// Snapshot of all runtime pool slots.
    pub runtime_slots: Vec<RuntimeSlotSnapshot>,
    /// Number of queued lanes currently waiting for execution.
    pub ready_lane_count: usize,
    /// Sum of pending turns across all lanes.
    pub total_pending_turn_count: usize,
    /// Conversation key of the most recent retryable lane, when any.
    pub last_retryable_conversation_key: Option<String>,
    /// Prompt file currently injected into Codex threads.
    pub prompt_file: Option<String>,
}
