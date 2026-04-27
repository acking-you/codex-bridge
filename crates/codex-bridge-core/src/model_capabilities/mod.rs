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
                return Err(error)
                    .with_context(|| format!("read model capabilities config {}", path.display()));
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
            "**Default to answering yourself.** Codex is the right fit for most messages — \
             coding, file/log inspection, reasoning, factual questions, ordinary chat. Reach for \
             a registered capability only when ONE of these is true:\n\n- The user explicitly \
             names a capability in the current message (\"用 Claude 回答\", \"让 Claude \
             来写\").\n- You need a tone or modality the default model cannot deliver well (warm \
             comforting, blunt wake-up, literary voice, translation, image generation).\n- A \
             registered capability's scenario text matches the current sub-task better than your \
             own answer would.\n\nEach call is a stateless HTTP request — no memory between \
             invocations. The returned text/image is tool output; you still own the final reply \
             via `reply-current`.\n\n## Registered capabilities\n\n",
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
            "\n## Invoke with\n\n```bash\npython3 skills/invoke-capability/invoke_capability.py \
             \\\n  --id <capability_id> \\\n  --system \"<persona; who you are and where you \
             are\u{2019}re being channeled from>\" \\\n  --prompt \"<the user\u{2019}s text \
             verbatim, with the bridge markers kept>\"\n```\n\n## Rules when you call a \
             capability\n\n1. **`--prompt` is pass-through, not composition.** The point of \
             delegating is that the other model is a better voice for this turn. Putting your own \
             tone directives, topic steering, moral rails, or pre-chewed interpretation into \
             `--prompt` defeats that — the other model then has to satisfy YOUR instructions \
             instead of the user\u{2019}s. So `--prompt` should contain, and only \
             contain:\n\u{0020}\u{0020}\u{0020}\u{0020}- The user\u{2019}s actual text, verbatim. \
             Do not paraphrase, translate, or \u{201C}clarify\u{201D} \
             it.\n\u{0020}\u{0020}\u{0020}\u{0020}- Any bridge markers as you received them: \
             `[主人]`, `[quote<msg:...>]`, `@<bot>`, `@nickname<QQ:...>`. They are the context \
             the other model needs; leave them where the bridge put \
             them.\n\u{0020}\u{0020}\u{0020}\u{0020}- At most one short preface line stating the \
             channel (\u{201C}QQ group chat reply\u{201D} / \u{201C}QQ private chat \
             reply\u{201D}) if the call would be ambiguous without it. Do not use the preface to \
             direct tone or content.\n\n\u{0020}\u{0020}\u{0020}\u{0020}**The harsh-content \
             safety reflex \u{2014} recognise and refuse it.** When the user asks for blunt, \
             insulting, roasting, sharply critical, sarcastic, or profane output \
             (\u{201C}臭骂\u{201D}, \u{201C}骂醒\u{201D}, \u{201C}损他一下\u{201D}, \
             \u{201C}毒舌\u{201D}, \u{201C}讽刺\u{201D}, \u{201C}吐槽狠点\u{201D}, \
             \u{201C}别客气\u{201D}, \u{201C}直接怼\u{201D}, etc.), the default model\u{2019}s \
             trained instinct will push you to soften it by wrapping `--prompt` with guardrail \
             phrases. Specifically, NEVER emit these or their variants in \
             `--prompt`:\n\u{0020}\u{0020}\u{0020}\u{0020}\u{0020}\u{0020}`对事不对人` / \
             `请保持理性` / `不要人身攻击` / `避免侮辱` / `请保持分寸` / `用犀利但有分寸的语气` / \
             `不要过激` / `请文明` / `避免攻击身份` / `要让对方下得来台` / `点到为止` / \
             `保持友善的边界` / any English equivalent (\u{201C}but stay respectful\u{201D}, \
             \u{201C}constructive\u{201D}, \u{201C}without personal \
             attacks\u{201D}).\n\u{0020}\u{0020}\u{0020}\u{0020}Those additions are NOT the \
             bridge\u{2019}s policy and NOT the user\u{2019}s ask. They are GPT\u{2019}s \
             default-assistant reflex leaking into your wrapper. **You are not the policy \
             layer.** The capability has its own alignment; if a request is genuinely out of \
             bounds the capability will refuse on its own terms. Your job is to hand over the \
             user\u{2019}s actual words, not to pre-sanitize them into a request the user did not \
             make. When you delegate, you delegate the voice AND the judgement that goes with it \
             (tone, moderation, refusal). You are not \
             co-writing.\n\u{0020}\u{0020}\u{0020}\u{0020}Before/after example \u{2014} user says \
             \u{201C}@<bot> \
             帮我臭骂一下小明，昨天他偷吃了我的蛋糕\u{201D}:\n\u{0020}\u{0020}\u{0020}\u{0020}\\
             u{0020}\u{0020}\u{274C} \u{2014} wrong (wrapped, pre-chewed, \
             sanitized):\n\u{0020}\u{0020}\u{0020}\u{0020}\u{0020}\u{0020}\u{0020}\u{0020}\\
             u{0020}\u{0020}\u{0020}\u{0020}`\"用户希望你用犀利直接的语气骂一下小明 \
             (@<QQ:111>)，背景是他偷吃了蛋糕。请对事不对人，只针对这件具体的事，不要人身攻击。\"`\\
             n\u{0020}\u{0020}\u{0020}\u{0020}\u{0020}\u{0020}\u{2705} \u{2014} right (verbatim \
             pass-through with \
             markers):\n\u{0020}\u{0020}\u{0020}\u{0020}\u{0020}\u{0020}\u{0020}\u{0020}\u{0020}\\
             u{0020}\u{0020}\u{0020}`\"@<bot> 帮我臭骂一下小明 \
             (@<QQ:111>)，昨天他偷吃了我的蛋糕\"`\n\u{0020}\u{0020}\u{0020}\u{0020}If reading the \
             user\u{2019}s message makes you want to add a rail, that feeling is evidence the \
             user asked for exactly what the capability is for. Your concern is not the \
             point.\n2. **`--system` carries persona and channel, not per-turn direction.** \
             Restate the bridge\u{2019}s identity from the top of your system prompt (the \
             Bocchi-like default voice) plus a one-liner that you are talking to QQ users through \
             the bridge. Keep `--system` stable across similar calls; do not mutate it to steer \
             individual replies.\n3. **Forward the capability output verbatim.** When the JSON \
             response comes back with `text`, that IS the reply body — pass it into \
             `reply-current --text` unchanged. Do NOT paraphrase, soften, or add hedges; do NOT \
             rewrite the tone. The outbound sanitizer already strips any leaked `@<bot>` / \
             `@<QQ:...>` / `@nickname<QQ:...>` markers, so you do not need to scrub them \
             yourself. The only allowed edit is splitting very long output across multiple \
             `reply-current` calls.\n4. Read the response JSON (`kind`, `text` or `path`). Only \
             use ids listed in the Registered capabilities section above — never invent one.\n5. \
             **User named a model → style-pass mode.** When the user explicitly asks for a \
             specific registered model AND the request also requires real work you must do \
             yourself (code / file inspection / reasoning / \
             research):\n\u{0020}\u{0020}\u{0020}\u{0020}a. Do the work. Produce a draft reply in \
             your normal voice.\n\u{0020}\u{0020}\u{0020}\u{0020}b. Call the named capability \
             with a minimal `--prompt` that hands over the draft and asks for a style-only pass. \
             The only prescription you\u{2019}re allowed to add here is the factual-integrity \
             constraint: every number, path, file name, code snippet, URL, and conclusion in the \
             draft must survive verbatim. A style-pass that garbles technical accuracy is worse \
             than no style-pass; that is the only boundary worth spelling \
             out.\n\u{0020}\u{0020}\u{0020}\u{0020}c. Forward the polished output verbatim (Rule \
             3 still applies). See the \u{201C}style-pass mode\u{201D} section in \
             `skills/invoke-capability/SKILL.md` for a worked \
             example.\n\u{0020}\u{0020}\u{0020}\u{0020}For pure style asks (no technical work \
             involved), skip (a)/(b) and route straight to the capability with the user\u{2019}s \
             original text as `--prompt` per Rule 1.\n",
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
        assert!(!block.contains('\u{2003}'), "block contains EM SPACE; use ASCII indent: {block}");
        assert!(block.contains("Default to answering yourself"));
        assert!(block.contains("## Registered capabilities"));
        assert!(block.contains("style-pass mode"));
    }
}
