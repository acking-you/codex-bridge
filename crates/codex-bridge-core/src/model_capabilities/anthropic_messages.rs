//! [`ModelCapability`] backed by the Anthropic Messages API.
//!
//! Accepts either the canonical Anthropic endpoint or a compatible
//! gateway (e.g. the local Kiro proxy). The endpoint is derived by
//! appending `/v1/messages` to `base_url`, so `base_url` must point at
//! the API root (not the full path to `messages`).
//!
//! Statelessness: every call opens a fresh HTTP exchange with a single
//! `user` turn containing the caller-supplied prompt. No conversation
//! history, no tool use, no streaming. Codex is expected to own any
//! multi-turn semantics above this layer.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::model_capability::{
    CapabilityError, CapabilityInput, CapabilityKind, CapabilityOutput, ModelCapability,
};

/// Validated configuration for one Anthropic Messages capability.
#[derive(Debug, Clone)]
pub struct AnthropicMessagesConfig {
    /// Stable id used by the registry.
    pub id: String,
    /// Human-facing name (logs, prompt injection).
    pub display_name: String,
    /// Scenario blurb surfaced in the system prompt so Codex knows when
    /// to reach for this capability.
    pub scenario: String,
    /// Optional tag keywords.
    pub tags: Vec<String>,
    /// API root (no trailing `/v1/messages`).
    pub base_url: String,
    /// API key forwarded as the `x-api-key` header.
    pub api_key: String,
    /// Model id sent to the upstream (e.g. `claude-sonnet-4-6`).
    pub model: String,
    /// Clamp on output tokens when the caller does not specify one.
    pub max_tokens: u32,
    /// Value sent as the `anthropic-version` header.
    pub anthropic_version: String,
}

/// Concrete capability implementation.
#[derive(Debug)]
pub struct AnthropicMessages {
    config: AnthropicMessagesConfig,
    client: Client,
    tags: Vec<&'static str>,
}

impl AnthropicMessages {
    /// Validate config and build the capability, reusing a shared
    /// `reqwest` client with sensible defaults.
    pub fn from_config(config: AnthropicMessagesConfig) -> anyhow::Result<Self> {
        if config.id.trim().is_empty() {
            anyhow::bail!("capability id must not be empty");
        }
        if config.api_key.trim().is_empty() {
            anyhow::bail!("capability {} has empty api_key", config.id);
        }
        if config.base_url.trim().is_empty() {
            anyhow::bail!("capability {} has empty base_url", config.id);
        }
        // Leak the tag strings once so the trait's `&[&'static str]`
        // signature stays cheap. The registry lives for the process
        // lifetime so this is a bounded, deterministic leak.
        let tags: Vec<&'static str> = config
            .tags
            .iter()
            .map(|tag| &*Box::leak(tag.clone().into_boxed_str()))
            .collect();
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|error| anyhow::anyhow!("build reqwest client: {error}"))?;
        Ok(Self { config, client, tags })
    }

    fn endpoint(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        format!("{base}/v1/messages")
    }
}

#[async_trait]
impl ModelCapability for AnthropicMessages {
    fn id(&self) -> &str {
        &self.config.id
    }

    fn kind(&self) -> CapabilityKind {
        CapabilityKind::Text
    }

    fn display_name(&self) -> &str {
        &self.config.display_name
    }

    fn scenario(&self) -> &str {
        &self.config.scenario
    }

    fn tags(&self) -> &[&'static str] {
        &self.tags
    }

    async fn invoke(&self, input: &CapabilityInput) -> Result<CapabilityOutput, CapabilityError> {
        let prompt = input.prompt.trim();
        if prompt.is_empty() {
            return Err(CapabilityError::Input("prompt must not be empty".into()));
        }
        let system = input
            .system
            .as_deref()
            .map(str::trim)
            .filter(|system| !system.is_empty());
        let request = AnthropicRequest {
            model: &self.config.model,
            max_tokens: input.max_tokens.unwrap_or(self.config.max_tokens),
            system,
            messages: vec![AnthropicMessage { role: "user", content: prompt }],
        };
        let response = self
            .client
            .post(self.endpoint())
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", &self.config.anthropic_version)
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(CapabilityError::Upstream(format!(
                "{status}: {body}",
                status = status.as_u16()
            )));
        }
        let parsed: AnthropicResponse = response
            .json()
            .await
            .map_err(|error| CapabilityError::Upstream(format!("decode response: {error}")))?;
        let text = parsed.extract_text();
        if text.trim().is_empty() {
            return Err(CapabilityError::Upstream("upstream returned empty text content".into()));
        }
        Ok(CapabilityOutput::Text { text })
    }
}

#[derive(Debug, Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    messages: Vec<AnthropicMessage<'a>>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<AnthropicContentBlock>,
}

impl AnthropicResponse {
    /// Concatenate every `text` block's body. Non-text blocks are
    /// skipped so future tool-use replies do not crash the parser.
    fn extract_text(&self) -> String {
        let mut out = String::new();
        for block in &self.content {
            if block.kind == "text" {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                if let Some(text) = block.text.as_deref() {
                    out.push_str(text);
                }
            }
        }
        out.trim().to_string()
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> AnthropicMessagesConfig {
        AnthropicMessagesConfig {
            id: "claude-kiro".into(),
            display_name: "Claude via Kiro gateway".into(),
            scenario: "human-tone replies".into(),
            tags: vec!["human-tone".into()],
            base_url: "http://127.0.0.1:39180/api/kiro-gateway/".into(),
            api_key: "sf-kiro-test".into(),
            model: "claude-sonnet-4-6".into(),
            max_tokens: 512,
            anthropic_version: "2023-06-01".into(),
        }
    }

    #[test]
    fn endpoint_strips_trailing_slash() {
        let cap = AnthropicMessages::from_config(sample_config()).expect("build");
        assert_eq!(cap.endpoint(), "http://127.0.0.1:39180/api/kiro-gateway/v1/messages");
    }

    #[test]
    fn config_rejects_empty_api_key() {
        let mut config = sample_config();
        config.api_key = "  ".into();
        let err = AnthropicMessages::from_config(config).expect_err("should fail");
        assert!(format!("{err:#}").contains("api_key"));
    }

    #[test]
    fn extract_text_concatenates_text_blocks_and_skips_others() {
        let payload = r#"{
            "content": [
                {"type": "text", "text": "第一段"},
                {"type": "tool_use", "id": "abc", "name": "noop", "input": {}},
                {"type": "text", "text": "第二段"}
            ]
        }"#;
        let parsed: AnthropicResponse = serde_json::from_str(payload).expect("decode");
        assert_eq!(parsed.extract_text(), "第一段\n第二段");
    }

    #[test]
    fn extract_text_returns_empty_when_no_text_blocks() {
        let parsed = AnthropicResponse {
            content: vec![AnthropicContentBlock { kind: "tool_use".into(), text: None }],
        };
        assert_eq!(parsed.extract_text(), "");
    }

    #[tokio::test]
    async fn invoke_rejects_empty_prompt() {
        let cap = AnthropicMessages::from_config(sample_config()).expect("build");
        let err = cap
            .invoke(&CapabilityInput { prompt: "   ".into(), system: None, max_tokens: None })
            .await
            .expect_err("should reject");
        match err {
            CapabilityError::Input(message) => assert!(message.contains("prompt")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn request_includes_system_when_present() {
        let request = AnthropicRequest {
            model: "claude-sonnet-4-6",
            max_tokens: 256,
            system: Some("you are Bocchi, be blunt"),
            messages: vec![AnthropicMessage { role: "user", content: "prompt body" }],
        };
        let payload = serde_json::to_value(&request).expect("serialize");
        assert_eq!(payload["system"], "you are Bocchi, be blunt");
        assert_eq!(payload["messages"][0]["role"], "user");
        assert_eq!(payload["messages"][0]["content"], "prompt body");
    }

    #[test]
    fn request_omits_system_when_absent() {
        let request = AnthropicRequest {
            model: "claude-sonnet-4-6",
            max_tokens: 256,
            system: None,
            messages: vec![AnthropicMessage { role: "user", content: "prompt body" }],
        };
        let payload = serde_json::to_value(&request).expect("serialize");
        assert!(payload.get("system").is_none(), "system field leaked when None: {payload}");
    }
}
