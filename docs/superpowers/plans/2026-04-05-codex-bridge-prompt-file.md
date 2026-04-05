# Codex Bridge Prompt File Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans or superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the current compiled/SQLite-backed prompt model with one runtime-owned Markdown file under `.run/default/prompt/system_prompt.md`, then route `thread/start` and `thread/resume` through that file as the only source of truth.

**Architecture:** Runtime preparation will create a prompt directory under `.run/default/` and seed a default Markdown template only when the runtime file is missing. Prompt loading moves to a small file-loader module or helper in `system_prompt.rs`; `codex_runtime` reads the file contents at use time for both new and resumed threads. SQLite keeps its existing schema for compatibility, but prompt text/version logic becomes dead data: no runtime reads, writes, or status surfaces depend on it anymore.

**Tech Stack:** Rust (`std::fs`, `anyhow`, `tokio`, `rusqlite`), runtime-owned files under `.run/default`, existing `RuntimePaths`, `StateStore`, and Axum status API.

---

## File Structure

- Modify: `crates/codex-bridge-core/src/system_prompt.rs`
  Purpose: replace compiled prompt text/version constants with the default prompt template asset and file-loading helpers.
- Modify: `crates/codex-bridge-core/src/runtime.rs`
  Purpose: add runtime prompt directory/file paths and create/seed `.run/default/prompt/system_prompt.md`.
- Modify: `crates/codex-bridge-core/src/codex_runtime.rs`
  Purpose: load prompt file contents for `thread/start` and `thread/resume` instead of using compiled constants.
- Modify: `crates/codex-bridge-core/src/state_store.rs`
  Purpose: stop prompt seeding/version refresh logic and stop reading/writing prompt metadata for runtime behavior while leaving schema compatibility intact.
- Modify: `crates/codex-bridge-core/src/orchestrator.rs`
  Purpose: remove prompt-version refresh/update flow from conversation binding handling and snapshots.
- Modify: `crates/codex-bridge-core/src/service.rs`
  Purpose: remove prompt-version from task snapshots.
- Modify: `crates/codex-bridge-core/src/api.rs`
  Purpose: stop exposing prompt version in `/status`; optionally expose prompt file path if needed.
- Modify: `crates/codex-bridge-core/tests/config_tests.rs`
  Purpose: verify prompt directory/file creation.
- Modify: `crates/codex-bridge-core/tests/codex_runtime_tests.rs`
  Purpose: verify runtime reads prompt file contents for start/resume and rejects empty prompt files.
- Modify: `crates/codex-bridge-core/tests/state_store_tests.rs`
  Purpose: remove prompt version assumptions and verify store opens without prompt bookkeeping.
- Modify: `crates/codex-bridge-core/tests/orchestrator_tests.rs`
  Purpose: remove prompt-version assertions from binding/task flows.
- Modify: `crates/codex-bridge-core/tests/api_tests.rs`
  Purpose: stop expecting prompt version in `/status`.

---

### Task 1: Add Runtime Prompt Paths And Seed File

**Files:**
- Modify: `crates/codex-bridge-core/src/runtime.rs`
- Modify: `crates/codex-bridge-core/src/system_prompt.rs`
- Test: `crates/codex-bridge-core/tests/config_tests.rs`

- [ ] **Step 1: Write the failing runtime prompt-file test**

```rust
#[test]
fn prepare_runtime_state_creates_prompt_file_from_default_template() {
    let temp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(temp.path(), Some(temp.path().join("qq")));
    let config = RuntimeConfig::default();

    prepare_runtime_state(&paths, &config, || "webui".into(), || "ws".into()).unwrap();

    let prompt_file = paths.runtime_root.join("prompt/system_prompt.md");
    assert!(prompt_file.is_file());
    let contents = std::fs::read_to_string(prompt_file).unwrap();
    assert!(contents.contains("reply-current"));
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core prepare_runtime_state_creates_prompt_file_from_default_template -- --nocapture
```

Expected: failure because `RuntimePaths` has no prompt path and runtime preparation does not seed a prompt file.

- [ ] **Step 3: Implement runtime prompt paths and default template seeding**

```rust
// runtime.rs
pub struct RuntimePaths {
    pub prompt_dir: PathBuf,
    pub prompt_file: PathBuf,
    // ...
}

pub fn prepare_runtime_state(...) -> Result<RuntimeTokens> {
    std::fs::create_dir_all(&paths.prompt_dir)?;
    ensure_prompt_file(paths)?;
    // existing setup...
}

// system_prompt.rs
pub const DEFAULT_SYSTEM_PROMPT_TEMPLATE: &str = include_str!("../assets/default_system_prompt.md");

pub fn ensure_prompt_file(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::write(path, DEFAULT_SYSTEM_PROMPT_TEMPLATE)?;
    }
    Ok(())
}
```

- [ ] **Step 4: Re-run the focused config tests**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core prepare_runtime_state_creates_prompt_file_from_default_template -- --nocapture
cargo test -p codex-bridge-core --test config_tests -- --nocapture
```

Expected: both commands pass.

---

### Task 2: Read Prompt File At Thread Start/Resume

**Files:**
- Modify: `crates/codex-bridge-core/src/system_prompt.rs`
- Modify: `crates/codex-bridge-core/src/codex_runtime.rs`
- Test: `crates/codex-bridge-core/tests/codex_runtime_tests.rs`

- [ ] **Step 1: Write failing prompt-loader tests**

```rust
#[test]
fn thread_start_params_read_prompt_from_runtime_file() {
    let dir = tempfile::tempdir().unwrap();
    let prompt_file = dir.path().join("system_prompt.md");
    std::fs::write(&prompt_file, "prompt from file").unwrap();

    let params = build_thread_start_params(..., &prompt_file).unwrap();

    assert_eq!(params.developer_instructions.as_deref(), Some("prompt from file"));
}

#[test]
fn thread_resume_params_reject_empty_prompt_file() {
    let dir = tempfile::tempdir().unwrap();
    let prompt_file = dir.path().join("system_prompt.md");
    std::fs::write(&prompt_file, "   \n").unwrap();

    let err = build_thread_resume_params(..., &prompt_file).unwrap_err();

    assert!(err.to_string().contains("empty"));
}
```

- [ ] **Step 2: Run the focused codex-runtime tests and verify they fail**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core thread_start_params_read_prompt_from_runtime_file -- --nocapture
cargo test -p codex-bridge-core thread_resume_params_reject_empty_prompt_file -- --nocapture
```

Expected: failure because prompt building still uses compiled constants.

- [ ] **Step 3: Implement file-based prompt loading**

```rust
pub fn load_system_prompt(path: &Path) -> Result<String> {
    let contents = std::fs::read_to_string(path)?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        anyhow::bail!("system prompt file is empty");
    }
    Ok(contents)
}

fn build_thread_start_params(config: &RuntimeConfig, prompt_file: &Path, ...) -> Result<...> {
    let prompt = load_system_prompt(prompt_file)?;
    // use prompt as developer_instructions
}
```

- [ ] **Step 4: Re-run the codex-runtime tests**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test codex_runtime_tests -- --nocapture
```

Expected: all prompt-related runtime tests pass.

---

### Task 3: Remove Prompt Versioning From Runtime State And Status

**Files:**
- Modify: `crates/codex-bridge-core/src/state_store.rs`
- Modify: `crates/codex-bridge-core/src/orchestrator.rs`
- Modify: `crates/codex-bridge-core/src/service.rs`
- Modify: `crates/codex-bridge-core/src/api.rs`
- Test: `crates/codex-bridge-core/tests/state_store_tests.rs`
- Test: `crates/codex-bridge-core/tests/orchestrator_tests.rs`
- Test: `crates/codex-bridge-core/tests/api_tests.rs`

- [ ] **Step 1: Write/adjust failing tests that still depend on prompt version**

Key expectations to change:

```rust
assert!(snapshot.prompt_version.is_none());
assert!(!status_body.contains("Prompt version"));
assert_eq!(binding.thread_id, expected_thread_id);
// no prompt_version assertions
```

- [ ] **Step 2: Run affected test targets and verify the old prompt-version assumptions fail**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test state_store_tests -- --nocapture
cargo test -p codex-bridge-core --test orchestrator_tests -- --nocapture
cargo test -p codex-bridge-core --test api_tests -- --nocapture
```

Expected: failures on prompt-version references.

- [ ] **Step 3: Remove runtime prompt bookkeeping**

```rust
// state_store.rs
pub struct ConversationBinding {
    pub conversation_key: String,
    pub thread_id: String,
}

// service.rs
pub struct TaskSnapshot {
    pub prompt_version: Option<String>, // removed
}

// api.rs
// stop formatting prompt version into /status
```

Keep schema compatibility by leaving unused columns/tables in SQLite, but stop reading/writing them in runtime code.

- [ ] **Step 4: Re-run the affected tests**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test state_store_tests -- --nocapture
cargo test -p codex-bridge-core --test orchestrator_tests -- --nocapture
cargo test -p codex-bridge-core --test api_tests -- --nocapture
```

Expected: all pass without prompt-version logic.

---

### Task 4: Full Verification

**Files:**
- Modify: `README.md` if prompt-file operator workflow or `/status` output references need updating.

- [ ] **Step 1: Run full core verification**

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo fmt --all --check
cargo test -p codex-bridge-core -- --nocapture
cargo test -p codex-bridge-cli -- --nocapture
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: all commands pass.

- [ ] **Step 2: Manual runtime sanity check**

Verify:

1. `.run/default/prompt/system_prompt.md` is created automatically.
2. Editing that file changes the next `thread/start`/`thread/resume` prompt payload.
3. `/status` no longer shows a prompt version string.
4. SQLite no longer controls prompt behavior.

- [ ] **Step 3: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add docs/superpowers/plans/2026-04-05-codex-bridge-prompt-file.md \
        crates/codex-bridge-core/src/system_prompt.rs \
        crates/codex-bridge-core/src/runtime.rs \
        crates/codex-bridge-core/src/codex_runtime.rs \
        crates/codex-bridge-core/src/state_store.rs \
        crates/codex-bridge-core/src/orchestrator.rs \
        crates/codex-bridge-core/src/service.rs \
        crates/codex-bridge-core/src/api.rs \
        crates/codex-bridge-core/tests/config_tests.rs \
        crates/codex-bridge-core/tests/codex_runtime_tests.rs \
        crates/codex-bridge-core/tests/state_store_tests.rs \
        crates/codex-bridge-core/tests/orchestrator_tests.rs \
        crates/codex-bridge-core/tests/api_tests.rs \
        README.md
git commit -m "feat: load codex bridge prompt from runtime markdown file"
```
