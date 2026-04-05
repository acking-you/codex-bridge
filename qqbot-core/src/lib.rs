//! Core runtime and bridge for My QQ Bot.

/// Static runtime configuration values.
pub mod config;

/// Normalized event types derived from raw NapCat payloads.
pub mod events;

/// Internal NapCat transport helpers.
pub mod napcat;

/// Local HTTP/WebSocket API surface.
pub mod api;

/// Foreground QQ launcher helpers.
pub mod launcher;

/// Filesystem path helpers for launcher/runtime state.
pub mod runtime;

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
