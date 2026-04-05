//! Core runtime and bridge for Codex Bridge.

/// Static runtime configuration values.
pub mod config;

/// Normalized event types derived from raw NapCat payloads.
pub mod events;

/// Internal NapCat transport helpers.
pub mod napcat;

/// Shared routing decisions and command/request models.
pub mod message_router;

/// Local HTTP/WebSocket API surface.
pub mod api;

/// Foreground QQ launcher helpers.
pub mod launcher;

/// Filesystem path helpers for launcher/runtime state.
pub mod runtime;

/// Filesystem permission shaping for codex runtime writes.
pub mod workspace_guard;

/// SQLite-backed runtime state storage.
pub mod state_store;

/// Versioned system prompt constants.
pub mod system_prompt;

/// Shared in-memory service state for the bridge runtime.
pub mod service;

/// Single-task scheduling primitives.
pub mod scheduler;

/// Reply formatting helpers for QQ-facing user responses.
pub mod reply_formatter;

/// Command approval guard used for codex execution paths.
pub mod approval_guard;

/// Codex runtime interfaces and result extraction helpers.
pub mod codex_runtime;

/// Orchestrator runtime that connects routing and codex execution.
pub mod orchestrator;
