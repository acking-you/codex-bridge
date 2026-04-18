//! Core runtime and bridge for Codex Bridge.

/// Static runtime configuration values.
pub mod config;

/// Admin approval config and pending-approval pool primitives.
pub mod admin_approval;

/// Normalized event types derived from raw NapCat payloads.
pub mod events;

/// Internal NapCat transport helpers.
pub mod napcat;

/// Shared routing decisions and command/request models.
pub mod message_router;

/// Local HTTP/WebSocket API surface.
pub mod api;

/// Structured outbound QQ message definitions.
pub mod outbound;

/// Foreground QQ launcher helpers.
pub mod launcher;

/// Filesystem path helpers for launcher/runtime state.
pub mod runtime;

/// Filesystem permission shaping for codex runtime writes.
pub mod workspace_guard;

/// SQLite-backed runtime state storage.
pub mod state_store;

/// Runtime-owned system prompt file helpers.
pub mod system_prompt;

/// Shared in-memory service state for the bridge runtime.
pub mod service;

/// Lane and runtime snapshot models for multi-conversation scheduling.
pub mod lane_manager;

/// Lane-scoped QQ conversation history query models.
pub mod conversation_history;

/// Runtime-pool primitives for lane-based Codex execution.
pub mod runtime_pool;

/// Single-task scheduling primitives.
pub mod scheduler;

/// Reply formatting helpers for QQ-facing user responses.
pub mod reply_formatter;

/// Active reply-context registry for skill-facing result delivery.
pub mod reply_context;

/// Command approval guard used for codex execution paths.
pub mod approval_guard;

/// Codex runtime interfaces and result extraction helpers.
pub mod codex_runtime;

/// Orchestrator runtime that connects routing and codex execution.
pub mod orchestrator;

/// Abstract model-capability trait used to augment Codex with one-shot
/// calls against other models (Claude, Gemini, ...).
pub mod model_capability;

/// Registered [`model_capability::ModelCapability`] implementations and
/// their TOML loader.
pub mod model_capabilities;
