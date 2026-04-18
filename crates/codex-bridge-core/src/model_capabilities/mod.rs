//! Registered [`ModelCapability`] implementations, plus the TOML loader
//! that seeds them at startup.
//!
//! The registry is intentionally small and boring: it is a
//! lookup-by-id map over `Arc<dyn ModelCapability>` so that
//! [`crate::service::ServiceState`] can hand a shared reference to HTTP
//! handlers without cloning the underlying HTTP clients.
//!
//! Config is loaded from `.run/default/config/model_capabilities.toml`
//! (by convention). The file shape is:
//!
//! ```toml
//! [[capabilities]]
//! id = "claude-kiro"
//! kind = "anthropic_messages"
//! display_name = "Claude via Kiro gateway"
//! scenario = "..."
//! base_url = "http://127.0.0.1:39180/api/kiro-gateway"
//! api_key = "sf-kiro-..."
//! model = "claude-sonnet-4-6"
//! max_tokens = 1024
//! ```
//!
//! `kind` selects which concrete implementation is instantiated; unknown
//! kinds are rejected so typos do not silently disable a capability.

use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::model_capability::ModelCapability;

pub mod anthropic_messages;

/// Shared, immutable set of model capabilities available at runtime.
#[derive(Debug, Default, Clone)]
pub struct ModelRegistry {
    entries: HashMap<String, Arc<dyn ModelCapability>>,
    order: Vec<String>,
}

impl ModelRegistry {
    /// Build an empty registry. Used when no config file is present and
    /// for tests.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Load the registry from a TOML file. A missing file is treated as
    /// "no capabilities configured" and returns an empty registry,
    /// matching the bridge's previous behaviour of running with Codex
    /// only.
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let raw = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::empty());
            },
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("read model capabilities config {}", path.display())
                });
            },
        };
        Self::load_from_str(&raw)
            .with_context(|| format!("parse model capabilities config {}", path.display()))
    }

    /// Load the registry from an in-memory TOML string. Exposed so tests
    /// can exercise the loader without touching disk.
    pub fn load_from_str(raw: &str) -> Result<Self> {
        let config: ModelCapabilitiesConfig =
            toml::from_str(raw).context("decode capabilities config")?;
        let mut registry = Self::empty();
        for entry in config.capabilities {
            registry.insert(entry.build()?)?;
        }
        Ok(registry)
    }

    /// Insert one capability. Fails when the id is already taken — the
    /// registry is a flat namespace and silent overrides would make
    /// diagnostics painful.
    pub fn insert(&mut self, capability: Arc<dyn ModelCapability>) -> Result<()> {
        let id = capability.id().to_string();
        if self.entries.contains_key(&id) {
            bail!("duplicate capability id: {id}");
        }
        self.order.push(id.clone());
        self.entries.insert(id, capability);
        Ok(())
    }

    /// Resolve a capability by id.
    pub fn get(&self, id: &str) -> Option<Arc<dyn ModelCapability>> {
        self.entries.get(id).cloned()
    }

    /// Iterate over capabilities in insertion order. Used for prompt
    /// injection where order determines the human-facing presentation.
    pub fn iter(&self) -> impl Iterator<Item = Arc<dyn ModelCapability>> + '_ {
        self.order
            .iter()
            .filter_map(|id| self.entries.get(id).cloned())
    }

    /// Number of registered capabilities.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return whether the registry has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Render a Markdown block describing every registered capability so
    /// Codex knows what is available. Returns `None` when the registry
    /// has no entries so the caller can skip prompt injection.
    pub fn render_prompt_block(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut out = String::from("# Available model capabilities\n\n");
        out.push_str(
            "**Default to answering yourself.** Codex is the right fit for most \
             messages — coding, file/log inspection, reasoning, factual questions, \
             ordinary chat. Reach for a registered capability only when ONE of these \
             is true:\n\n\
             - The user explicitly names a capability in the current message \
             (\"用 Claude 回答\", \"让 Claude 来写\").\n\
             - You need a tone or modality the default model cannot deliver \
             well (warm comforting, blunt wake-up, literary voice, translation, \
             image generation).\n\
             - A registered capability's scenario text matches the current sub-task \
             better than your own answer would.\n\n\
             Each call is a stateless HTTP request — no memory between invocations. \
             The returned text/image is tool output; you still own the final reply \
             via `reply-current`.\n\n\
             ## Registered capabilities\n\n",
        );
        for capability in self.iter() {
            let kind = match capability.kind() {
                crate::model_capability::CapabilityKind::Text => "text",
                crate::model_capability::CapabilityKind::Image => "image",
            };
            out.push_str(&format!(
                "- **{id}** ({kind}) — {display}. Scenario: {scenario}\n",
                id = capability.id(),
                kind = kind,
                display = capability.display_name(),
                scenario = capability.scenario(),
            ));
        }
        out.push_str(
            "\n## Invoke with\n\n```bash\n\
             python3 skills/invoke-capability/invoke_capability.py \\\n  \
             --id <capability_id> \\\n  \
             --system \"<persona + target tone; keep it in-character>\" \\\n  \
             --prompt \"<full context + the specific ask>\"\n\
             ```\n\n\
             ## Rules when you call a capability\n\n\
             1. **Pass enough context in `--prompt`.** The external model has zero \
             memory of this conversation. Include the user's actual text, any \
             `[quote<msg:...>]` preamble the bridge gave you, relevant recent turns, \
             and — critically — what you want out. Fragments like \"回答：你好\" are \
             almost always wrong; \"用户在群里抱怨部署失败，原话是 \\\"...\\\"，请用辛辣\
             但对事不对人的口气骂醒他\" is right.\n\
             2. **Pass your persona via `--system`.** Without it the external model \
             drops into generic-assistant voice — exactly what calling out to it was \
             supposed to avoid. Restate the bridge's identity from the Identity \
             section at the top of this system prompt, plus the per-call tone.\n\
             3. **Forward the capability output verbatim.** When the JSON response \
             comes back with `text`, that IS the reply body — pass it into \
             `reply-current --text` unchanged. Do NOT paraphrase, soften, or add \
             hedges like \"以上内容仅供参考\" / \"希望对你有帮助\"; do NOT rewrite \
             the tone. If you asked for a blunt reply and then polish it smooth, the \
             whole detour was pointless. Only allowed edit: split very long output \
             across multiple `reply-current` calls. You do NOT need to strip leaked \
             `@<bot>` / `@<QQ:...>` / `@nickname<QQ:...>` markers — the outbound \
             sanitizer already handles that.\n\
             4. Read the response JSON (`kind`, `text` or `path`). Only use ids listed \
             in the Registered capabilities section above — never invent one.\n\
             5. **User named a model → style-pass mode.** When the user explicitly \
             asks for a specific registered model, honour the preference even when you \
             already have an answer ready:\n\
             \u{0020}\u{0020}\u{0020}\u{0020}- If the task is style-only (rewrite, \
             translate, comfort, roast, chat), route straight to that capability and \
             skip your own drafting.\n\
             \u{0020}\u{0020}\u{0020}\u{0020}- If the task requires real work first \
             (code, file/log inspection, reasoning, multi-step research), do the work \
             yourself, draft the reply, then run the draft through the named \
             capability as a style-pass before `reply-current`. The `--prompt` MUST \
             explicitly instruct the model to preserve every number, path, file name, \
             code snippet, URL, and factual conclusion verbatim, and only polish the \
             natural-language parts. A style-pass that garbles technical accuracy is \
             worse than no style-pass. See the worked example in \
             `skills/invoke-capability/SKILL.md` (\"style-pass mode\" section).\n\
             \u{0020}\u{0020}\u{0020}\u{0020}- Rule 3 still applies: forward the \
             polished text verbatim.\n",
        );
        Some(out)
    }
}

/// Top-level TOML shape for `model_capabilities.toml`.
#[derive(Debug, Deserialize)]
struct ModelCapabilitiesConfig {
    #[serde(default)]
    capabilities: Vec<CapabilityConfigEntry>,
}

/// One `[[capabilities]]` table in the TOML file. The `kind` field picks
/// the concrete implementation; the remaining fields are dispatched to
/// that implementation's own validator.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityConfigEntry {
    id: String,
    kind: String,
    display_name: String,
    scenario: String,
    #[serde(default)]
    tags: Vec<String>,
    base_url: String,
    api_key: String,
    model: String,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    anthropic_version: Option<String>,
}

impl CapabilityConfigEntry {
    /// Turn one TOML entry into a concrete [`ModelCapability`].
    fn build(self) -> Result<Arc<dyn ModelCapability>> {
        match self.kind.as_str() {
            "anthropic_messages" => {
                let cap = anthropic_messages::AnthropicMessages::from_config(
                    anthropic_messages::AnthropicMessagesConfig {
                        id: self.id,
                        display_name: self.display_name,
                        scenario: self.scenario,
                        tags: self.tags,
                        base_url: self.base_url,
                        api_key: self.api_key,
                        model: self.model,
                        max_tokens: self.max_tokens.unwrap_or(1024),
                        anthropic_version: self
                            .anthropic_version
                            .unwrap_or_else(|| "2023-06-01".to_string()),
                    },
                )?;
                Ok(Arc::new(cap))
            },
            other => bail!("unsupported capability kind: {other}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry_is_empty() {
        let registry = ModelRegistry::empty();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.get("anything").is_none());
    }

    #[test]
    fn missing_file_is_empty_registry() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let path = tmp.path().join("does-not-exist.toml");
        let registry = ModelRegistry::load_from_file(&path).expect("load");
        assert!(registry.is_empty());
    }

    #[test]
    fn loads_anthropic_messages_entry_from_toml() {
        let toml = r#"
            [[capabilities]]
            id = "claude-kiro"
            kind = "anthropic_messages"
            display_name = "Claude via Kiro gateway"
            scenario = "human-tone replies"
            tags = ["human-tone"]
            base_url = "http://127.0.0.1:39180/api/kiro-gateway"
            api_key = "sf-kiro-test"
            model = "claude-sonnet-4-6"
            max_tokens = 512
        "#;
        let registry = ModelRegistry::load_from_str(toml).expect("load");
        assert_eq!(registry.len(), 1);
        let cap = registry.get("claude-kiro").expect("capability present");
        assert_eq!(cap.id(), "claude-kiro");
        assert_eq!(cap.display_name(), "Claude via Kiro gateway");
    }

    #[test]
    fn unknown_kind_is_rejected() {
        let toml = r#"
            [[capabilities]]
            id = "mystery"
            kind = "quantum_oracle"
            display_name = "Mystery"
            scenario = "?"
            base_url = "http://127.0.0.1"
            api_key = "nope"
            model = "m"
        "#;
        let err = ModelRegistry::load_from_str(toml).expect_err("should fail");
        let rendered = format!("{err:#}");
        assert!(rendered.contains("quantum_oracle"), "error: {rendered}");
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let toml = r#"
            [[capabilities]]
            id = "dup"
            kind = "anthropic_messages"
            display_name = "A"
            scenario = "x"
            base_url = "http://127.0.0.1"
            api_key = "k"
            model = "m"

            [[capabilities]]
            id = "dup"
            kind = "anthropic_messages"
            display_name = "B"
            scenario = "y"
            base_url = "http://127.0.0.1"
            api_key = "k"
            model = "m"
        "#;
        let err = ModelRegistry::load_from_str(toml).expect_err("should fail");
        let rendered = format!("{err:#}");
        assert!(rendered.contains("duplicate capability id"), "error: {rendered}");
    }

    #[test]
    fn rendered_prompt_block_is_free_of_known_footguns() {
        let toml = r#"
            [[capabilities]]
            id = "claude-kiro"
            kind = "anthropic_messages"
            display_name = "Claude via Kiro gateway"
            scenario = "warm or blunt rewrites"
            base_url = "http://127.0.0.1:39180/api/kiro-gateway"
            api_key = "sf-kiro-test"
            model = "claude-sonnet-4-6"
            max_tokens = 1024
        "#;
        let block = ModelRegistry::load_from_str(toml)
            .expect("load")
            .render_prompt_block()
            .expect("block rendered");
        assert!(
            !block.contains("\\n"),
            "block contains literal backslash-n pair (conflicts with real-newline rule): {block}"
        );
        assert!(
            !block.contains('\u{2026}'),
            "block contains unicode ellipsis; use ASCII three dots: {block}"
        );
        assert!(
            !block.contains('\u{2003}'),
            "block contains EM SPACE; use ASCII indent: {block}"
        );
        assert!(block.contains("Default to answering yourself"));
        assert!(block.contains("## Registered capabilities"));
        assert!(block.contains("style-pass mode"));
    }
}
