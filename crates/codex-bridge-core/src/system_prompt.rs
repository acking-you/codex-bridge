//! Layered system-prompt assembly for Codex Bridge.
//!
//! The prompt fed into Codex's `developer_instructions` is built in four
//! layers so operators can tune the bot's personality without touching
//! the bridge protocol that keeps the system correct:
//!
//! 1. **Persona** (`persona.md`) — operator-editable identity, voice, and
//!    project-specific skill pointers. Seeded from the packaged template
//!    on first boot; subsequent edits are preserved across bridge
//!    upgrades.
//! 2. **Bridge protocol** (embedded) — turn-start checklist, mention /
//!    quote / reply-to / permissions rules. Code-owned; changes with
//!    bridge versions and is NEVER seeded to disk.
//! 3. **Admin context** (runtime-rendered) — a tiny block that tells
//!    Codex the configured admin's QQ id so it can recognise 主人 even
//!    when the `[主人]` inbound marker is not present (e.g. when the
//!    admin is mentioned in a quoted message).
//! 4. **Model capabilities** (hot-reloadable) — the
//!    `# Available model capabilities` block, rendered from the
//!    [`crate::model_capabilities::ModelRegistry`] and sharable across
//!    the service state via an `Arc<RwLock<Option<String>>>`.
//!
//! `build_developer_instructions` in `codex_runtime.rs` concatenates the
//! four layers in order. The persona layer is the only one an operator
//! is expected to edit; tweaking the bridge protocol or admin context
//! requires a code change and a rebuild.

use std::{fs, path::Path};

use anyhow::{Context, Result};

/// Default template copied into the runtime-owned `persona.md` file on
/// first boot.
pub const DEFAULT_PERSONA_TEMPLATE: &str = include_str!("../assets/persona.md");

/// Static bridge protocol text embedded in the binary. Always prepended
/// to every Codex thread's developer instructions after the operator
/// persona. Contains the turn-start checklist that makes capability
/// delegation checks mandatory and explicit.
pub const BRIDGE_PROTOCOL_TEXT: &str = include_str!("../assets/bridge_protocol.md");

/// Create the operator-owned persona file from the default template when
/// it is missing. Matches the pre-existing contract of the previous
/// `ensure_prompt_file`: seed on first boot, never overwrite.
pub fn ensure_persona_file(persona_file: &Path) -> Result<()> {
    if persona_file.exists() {
        return Ok(());
    }

    if let Some(parent) = persona_file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create persona directory {}", parent.display()))?;
    }

    fs::write(persona_file, DEFAULT_PERSONA_TEMPLATE)
        .with_context(|| format!("write default persona file {}", persona_file.display()))?;
    Ok(())
}

/// Load the active operator persona text from the runtime-owned
/// `persona.md` file.
pub fn load_persona(persona_file: &Path) -> Result<String> {
    let persona = fs::read_to_string(persona_file)
        .with_context(|| format!("read persona file {}", persona_file.display()))?;

    if persona.trim().is_empty() {
        anyhow::bail!("persona file is empty: {}", persona_file.display());
    }

    Ok(persona)
}

/// Render the runtime-owned admin context block.
///
/// The block is always tiny — one short paragraph stating the admin's
/// QQ id so Codex can match `@<QQ:X>` placeholders against 主人 even
/// when the inbound `[主人]` marker is not present (for example when
/// someone else is quoting a message 主人 sent earlier).
///
/// When no admin is configured (`admin_user_id <= 0`), an explicit
/// "no admin" block is returned so Codex knows the 主人 register is
/// inactive.
pub fn render_admin_block(admin_user_id: i64) -> String {
    if admin_user_id <= 0 {
        return String::from(
            "# Admin context\n\n\
             No admin is currently configured for this bridge. The 主人 \
             register is inactive; treat every sender as a regular friend \
             and do not use 主人 terminology until an admin is set.\n",
        );
    }
    format!(
        "# Admin context\n\n\
         The admin's QQ id is **{admin_user_id}** — that person is your 主人. The \
         `[主人]` marker on the current message (see bridge protocol) is the primary \
         way to recognise a 主人 turn; this QQ id is the fallback for cases where the \
         admin appears indirectly (e.g. quoted in a `[quote<msg:...>]` preamble or \
         pointed at via `@<QQ:{admin_user_id}>` inside another user's message).\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_persona_template_contains_identity_and_voice() {
        assert!(DEFAULT_PERSONA_TEMPLATE.contains("# Identity"));
        assert!(DEFAULT_PERSONA_TEMPLATE.contains("# Voice and vitality"));
    }

    #[test]
    fn bridge_protocol_contains_turn_checklist_and_permissions() {
        assert!(BRIDGE_PROTOCOL_TEXT.contains("# Turn start checklist"));
        assert!(BRIDGE_PROTOCOL_TEXT.contains("# Permissions"));
        assert!(BRIDGE_PROTOCOL_TEXT.contains("Gate 1"));
        assert!(BRIDGE_PROTOCOL_TEXT.contains("Gate 2"));
        assert!(BRIDGE_PROTOCOL_TEXT.contains("Gate 3"));
    }

    #[test]
    fn render_admin_block_includes_id_when_configured() {
        let block = render_admin_block(2_394_626_220);
        assert!(block.contains("2394626220"));
        assert!(block.contains("主人"));
    }

    #[test]
    fn render_admin_block_signals_unset_when_zero() {
        let block = render_admin_block(0);
        assert!(block.contains("No admin is currently configured"));
    }

    #[test]
    fn ensure_persona_file_seeds_when_missing() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let path = tmp.path().join("persona.md");
        assert!(!path.exists());

        ensure_persona_file(&path).expect("seed");
        let seeded = fs::read_to_string(&path).expect("read");
        assert!(seeded.contains("# Identity"));
    }

    #[test]
    fn ensure_persona_file_preserves_existing() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let path = tmp.path().join("persona.md");
        fs::write(&path, "# Custom persona").expect("seed");

        ensure_persona_file(&path).expect("preserve");
        assert_eq!(fs::read_to_string(&path).expect("read"), "# Custom persona");
    }

    #[test]
    fn load_persona_rejects_empty_file() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let path = tmp.path().join("persona.md");
        fs::write(&path, "   \n\n").expect("seed");
        let err = load_persona(&path).expect_err("empty file should fail");
        assert!(format!("{err:#}").contains("empty"));
    }
}
