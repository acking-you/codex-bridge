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
            group_start_reaction_emoji_id: "282".to_string(),
            qq_executable: None,
        }
    }
}
