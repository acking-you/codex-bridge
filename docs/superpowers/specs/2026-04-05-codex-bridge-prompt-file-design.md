# Codex Bridge Prompt File Design

## Goal

Replace the current split prompt model with one global Markdown file that acts as the only source of truth for the bot's system prompt.

After this change:

- The active prompt text is loaded from a Markdown file under `.run/`.
- SQLite no longer stores prompt text or prompt versions.
- Thread start and thread resume both read the current file contents directly.
- The operator can edit the prompt file manually at any time without touching code or the database.

## Current Problem

The project currently keeps prompt state in multiple places:

- a compiled Rust constant in `crates/codex-bridge-core/src/system_prompt.rs`
- SQLite bookkeeping in `state_store`
- conversation bindings and task snapshots that still mention prompt versioning

This creates two problems:

1. The real prompt source is unclear.
2. Changing persona or execution guidance requires code changes or state migration logic.

That is the wrong model for a single-identity bot.

## Design

### Single Prompt File

The runtime-owned prompt file will live at:

`.run/default/prompt/system_prompt.md`

This file is the only prompt source used by the runtime.

If the file does not exist, runtime preparation creates it from a bundled default template.

### Runtime Read Path

Prompt loading becomes a plain file read.

- `thread/start` reads the file and passes the contents as `developer_instructions`
- `thread/resume` reads the same file and passes the contents as `developer_instructions`

The file is read at use time, not cached in SQLite.

This means edits to the file take effect on the next thread start or resume without database coordination.

### SQLite Changes

SQLite stops being part of prompt management.

- `system_prompt_versions` becomes unused
- `conversation_bindings.prompt_version` becomes unused
- task/status APIs stop depending on prompt version data

For compatibility and migration safety, existing columns and tables may remain on disk, but runtime logic must stop reading or writing them.

The source of truth changes; the schema does not need a destructive migration.

### Default Template

The project still needs a default prompt template checked into source control so first boot can create the runtime file.

The default template should be stored as a normal text asset in the Rust project, not as a compiled behavior constant that the runtime treats as authoritative.

The runtime may copy this template into `.run/default/prompt/system_prompt.md` only when the runtime file is missing.

Once the runtime file exists, the operator-owned file wins.

## Behavior Changes

### Status Output

`/status` and related snapshots should stop reporting prompt version strings.

If prompt metadata is needed at all, it should be limited to stable operator-facing facts such as the prompt file path.

### Conversation Continuity

Changing the prompt file updates future turn behavior globally.

There is no per-conversation prompt version anymore.

This intentionally trades per-thread prompt history for one global bot identity, which matches the product goal.

## Risks

### Prompt Hot Changes and Old Thread History

Even after the runtime starts sending the new prompt on resume, old thread history still exists inside Codex state. That is acceptable for this design because the user explicitly wants one global editable prompt, not versioned historical prompt semantics.

### Operator Errors

If the operator writes an invalid or empty prompt file, runtime behavior can degrade. The runtime should therefore:

- fail clearly when the prompt file cannot be read
- reject an empty prompt file

It should not silently fall back to SQLite or stale compiled behavior.

## Non-Goals

- No prompt version registry
- No prompt text in SQLite
- No per-conversation prompt migration logic
- No dynamic prompt editing API in the bridge

## Acceptance Criteria

The change is complete when all of the following are true:

1. `.run/default/prompt/system_prompt.md` is created automatically on first run.
2. Editing that file changes what `thread/start` and `thread/resume` send as `developer_instructions`.
3. `state_store` no longer reads or writes prompt text/version state for runtime behavior.
4. `/status` no longer exposes prompt version values.
5. Existing tests are updated to validate file-based prompt loading instead of SQLite-backed prompt metadata.
