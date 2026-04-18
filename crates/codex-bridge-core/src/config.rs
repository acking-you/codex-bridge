//! Runtime configuration defaults used by the launcher and local API.

use std::path::PathBuf;

/// Static runtime configuration shared by the launcher and API service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// Local HTTP bind address for the Rust API.
    pub api_bind: String,
    /// WebUI listen host used by the injected NapCat runtime.
    pub webui_host: String,
    /// WebUI listen port used by the injected NapCat runtime.
    pub webui_port: u16,
    /// Optional formal websocket host written into config for compatibility.
    pub websocket_host: String,
    /// Optional formal websocket port written into config for compatibility.
    pub websocket_port: u16,
    /// Maximum number of queued Codex tasks.
    pub queue_capacity: usize,
    /// Maximum number of concurrently running app-server slots.
    pub runtime_pool_size: usize,
    /// Maximum number of queued turns buffered inside one conversation lane.
    pub lane_pending_capacity: usize,
    /// Maximum number of requests waiting for admin approval.
    pub pending_approval_capacity: usize,
    /// Timeout in seconds for pending admin approvals.
    pub approval_timeout_secs: u64,
    /// Number of history messages to request per NapCat page.
    pub history_page_size: usize,
    /// Maximum number of history pages the bridge scans for one query.
    pub history_max_pages: usize,
    /// Hard wall-clock timeout for one running lane turn.
    pub max_turn_wall_time_secs: u64,
    /// Timeout for a running turn that stops emitting progress.
    pub stalled_turn_timeout_secs: u64,
    /// Backoff before respawning a broken runtime slot.
    pub slot_restart_backoff_ms: u64,
    /// Emoji-like identifier used for the group "salute" start reaction.
    pub group_start_reaction_emoji_id: String,
    /// Optional QQ executable override.
    pub qq_executable: Option<PathBuf>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            api_bind: "127.0.0.1:36111".to_string(),
            webui_host: "127.0.0.1".to_string(),
            webui_port: 6099,
            websocket_host: "127.0.0.1".to_string(),
            websocket_port: 3012,
            queue_capacity: 5,
            runtime_pool_size: 2,
            lane_pending_capacity: 5,
            pending_approval_capacity: 32,
            approval_timeout_secs: 900,
            history_page_size: 50,
            history_max_pages: 4,
            max_turn_wall_time_secs: 900,
            stalled_turn_timeout_secs: 120,
            slot_restart_backoff_ms: 500,
            group_start_reaction_emoji_id: "282".to_string(),
            qq_executable: None,
        }
    }
}
