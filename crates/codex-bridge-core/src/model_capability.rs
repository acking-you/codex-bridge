//! Abstract "model capability" layer for codex-bridge.
//!
//! A *capability* is a stateless, one-shot call against an external model
//! endpoint — not a full agent harness. Codex remains the only harness
//! that runs the full tool/planning loop; capabilities are *tools* that
//! Codex invokes from inside its own turn when a sub-task is better
//! handled by another model (e.g. Claude for warmer phrasing, Gemini for
//! image generation).
//!
//! Because every invocation is stateless, a capability never owns a
//! conversation thread. Context continuity stays with Codex: the
//! capability's response is returned to Codex as tool output, becomes
//! part of the Codex thread history, and informs subsequent turns.
//!
//! Adding a new model is a matter of:
//! 1. Implementing [`ModelCapability`] for the new provider.
//! 2. Declaring an entry in `model_capabilities.toml`.
//! 3. Letting the [`crate::model_capabilities::registry`] load it on boot.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The kind of artifact a capability returns to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// Text/chat completion. Returns a single plain-text body.
    Text,
    /// Image generation. Returns a file path on local disk.
    Image,
}

/// Arguments passed into a single capability invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityInput {
    /// The prompt sent verbatim to the external model.
    pub prompt: String,
    /// Optional system prompt forwarded to the external model so it
    /// speaks in-character (persona, target tone, safety guardrails).
    /// Capabilities that do not natively support a system role should
    /// prepend it to the user prompt instead of silently dropping it.
    pub system: Option<String>,
    /// Optional upper bound on the number of output tokens. Capabilities
    /// that do not honour token limits should clamp to their own safe
    /// default.
    pub max_tokens: Option<u32>,
}

/// Result produced by a single capability invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityOutput {
    /// Plain-text completion body.
    Text {
        /// Response body produced by the external model.
        text: String,
    },
    /// Locally stored image artifact. The path must live under
    /// `.run/artifacts/` so it can be forwarded by the existing reply
    /// pipeline.
    Image {
        /// Canonical path to the generated image on local disk.
        path: PathBuf,
    },
}

/// Error returned when a capability invocation fails.
#[derive(Debug, Error)]
pub enum CapabilityError {
    /// The capability configuration is invalid (missing api_key, bad URL, ...).
    #[error("capability config invalid: {0}")]
    Config(String),
    /// The upstream HTTP call failed before a response was parsed.
    #[error("capability transport error: {0}")]
    Transport(#[from] reqwest::Error),
    /// The upstream returned a non-success status or a payload that
    /// could not be parsed into the expected shape.
    #[error("capability upstream error: {0}")]
    Upstream(String),
    /// The caller supplied an input the capability cannot satisfy
    /// (empty prompt, unsupported modality, ...).
    #[error("capability input error: {0}")]
    Input(String),
}

/// One pluggable model backend. Implementations MUST be stateless between
/// [`ModelCapability::invoke`] calls.
#[async_trait]
pub trait ModelCapability: std::fmt::Debug + Send + Sync {
    /// Stable id used by the registry, config, and Codex-facing skill.
    fn id(&self) -> &str;

    /// The kind of output this capability produces.
    fn kind(&self) -> CapabilityKind;

    /// Human-friendly display name used in logs and prompt injection.
    fn display_name(&self) -> &str;

    /// Free-text description of "when should Codex reach for this
    /// capability". Surfaced into the system prompt so Codex can choose
    /// wisely without hard-coded rules.
    fn scenario(&self) -> &str;

    /// Capability tags — short keywords consumers (future routers,
    /// metrics) can match against. Examples: `human-tone`, `image`,
    /// `translation`.
    fn tags(&self) -> &[&'static str];

    /// Execute the capability. Must be stateless across calls.
    async fn invoke(
        &self,
        input: &CapabilityInput,
    ) -> Result<CapabilityOutput, CapabilityError>;
}
