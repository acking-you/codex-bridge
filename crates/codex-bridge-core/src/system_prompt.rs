//! Runtime-owned system prompt helpers.

use std::{fs, path::Path};

use anyhow::{Context, Result};

/// Default prompt template copied into the runtime-owned prompt file on first
/// boot.
pub const DEFAULT_SYSTEM_PROMPT_TEMPLATE: &str = include_str!("../assets/default_system_prompt.md");

/// Create the operator-owned prompt file from the default template when it is
/// missing.
pub fn ensure_prompt_file(prompt_file: &Path) -> Result<()> {
    if prompt_file.exists() {
        return Ok(());
    }

    if let Some(parent) = prompt_file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create prompt directory {}", parent.display()))?;
    }

    fs::write(prompt_file, DEFAULT_SYSTEM_PROMPT_TEMPLATE)
        .with_context(|| format!("write default prompt file {}", prompt_file.display()))?;
    Ok(())
}

/// Load the active system prompt from the runtime-owned Markdown file.
pub fn load_system_prompt(prompt_file: &Path) -> Result<String> {
    let prompt = fs::read_to_string(prompt_file)
        .with_context(|| format!("read system prompt file {}", prompt_file.display()))?;

    if prompt.trim().is_empty() {
        anyhow::bail!("system prompt file is empty: {}", prompt_file.display());
    }

    Ok(prompt)
}
