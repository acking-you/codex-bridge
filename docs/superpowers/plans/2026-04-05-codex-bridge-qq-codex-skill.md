# Codex Bridge QQ Codex Skill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `codex-bridge` into a QQ bot runtime that admits only valid QQ conversations, lets Codex return normal results through one reply skill, and enforces the new host-read/repo-write/artifacts-create policy.

**Architecture:** Keep NapCat as the QQ transport and Codex app-server as the agent runtime, but move policy ownership into the Rust bridge. The bridge will own admission, command permissions, queueing, reply-token lifecycle, and bridge-generated status/error messages; successful result delivery will move to a unified skill that calls back into the bridge CLI/API. To satisfy "machine-wide readable but repo write constrained", the runtime will stop relying on a narrow filesystem sandbox and instead combine strict shell approval with a workspace permission shaper that makes existing tracked files writable while only `.run/artifacts/` stays directory-writable for new file creation.

**Tech Stack:** Rust (`tokio`, `axum`, `clap`, `rusqlite`), Codex app-server over stdio JSON-RPC, NapCat formal OneBot WebSocket, local skills under `.agents/skills`, repository permission shaping via POSIX file modes.

---

## File Structure

- Modify: `crates/codex-bridge-core/src/config.rs`
  Purpose: add the salute emoji default and expose any runtime knobs needed by the new flow.
- Modify: `crates/codex-bridge-core/src/runtime.rs`
  Purpose: create `.run/artifacts/`, expose skills/symlink paths, and prepare runtime directories.
- Create: `crates/codex-bridge-core/src/workspace_guard.rs`
  Purpose: shape repository permissions so existing tracked files stay writable but new files can only be created under `.run/artifacts/`.
- Modify: `crates/codex-bridge-core/src/system_prompt.rs`
  Purpose: replace the old repo-only prompt with the approved cyber-life + Bocchi-flavored prompt and the new permission contract.
- Modify: `crates/codex-bridge-core/src/approval_guard.rs`
  Purpose: change command approval from "workspace only" to "machine-readable but dangerous or mutating shell commands denied unless clearly allowed".
- Modify: `crates/codex-bridge-core/src/codex_runtime.rs`
  Purpose: switch to the new runtime policy, inject reply-token env/context, and stop treating assistant text as the normal QQ result path.
- Modify: `crates/codex-bridge-core/src/message_router.rs`
  Purpose: add `/help`, preserve sender/message metadata on commands, and keep enough context for ownership and reply-token generation.
- Modify: `crates/codex-bridge-core/src/scheduler.rs`
  Purpose: store task owner metadata so `/cancel` and `/retry_last` can enforce sender ownership.
- Modify: `crates/codex-bridge-core/src/state_store.rs`
  Purpose: persist task owner/source metadata and prompt version state needed for restarts and retry ownership.
- Create: `crates/codex-bridge-core/src/reply_context.rs`
  Purpose: hold active reply tokens and current-conversation reply metadata for the running task.
- Create: `crates/codex-bridge-core/src/outbound.rs`
  Purpose: define structured outbound QQ operations: text, image, file, and group emoji reaction.
- Modify: `crates/codex-bridge-core/src/service.rs`
  Purpose: carry reply context, richer task snapshots, and new structured outbound transport commands.
- Modify: `crates/codex-bridge-core/src/api.rs`
  Purpose: add `/api/reply`, expose richer `/status` and `/help` behavior, and validate reply-token payloads.
- Modify: `crates/codex-bridge-core/src/napcat.rs`
  Purpose: implement formal OneBot actions for group salute reaction and structured message segments with reply/at/image/file support.
- Modify: `crates/codex-bridge-core/src/reply_formatter.rs`
  Purpose: add persona-consistent bridge-generated messages, `/help`, friend-rejection, and failure text.
- Modify: `crates/codex-bridge-core/src/orchestrator.rs`
  Purpose: enforce friend-only private admission, command ownership, reply-token issue/revoke, group salute start feedback, and bridge-vs-skill result split.
- Modify: `crates/codex-bridge-core/src/lib.rs`
  Purpose: export new core modules.
- Modify: `crates/codex-bridge-cli/src/cli.rs`
  Purpose: add the `reply` subcommand and keep manual send commands separate from skill-facing flow.
- Modify: `crates/codex-bridge-cli/src/main.rs`
  Purpose: wire the new `reply` command and initialize workspace guard / skills link before run.
- Create: `crates/codex-bridge-core/tests/workspace_guard_tests.rs`
  Purpose: verify the repo permission shaping rules.
- Create: `crates/codex-bridge-core/tests/reply_context_tests.rs`
  Purpose: verify token issue, expiry, and multi-send behavior.
- Modify: `crates/codex-bridge-core/tests/message_router_tests.rs`
  Purpose: cover `/help`, richer command metadata, and group/private routing.
- Modify: `crates/codex-bridge-core/tests/state_store_tests.rs`
  Purpose: cover task-owner persistence and prompt version state.
- Modify: `crates/codex-bridge-core/tests/api_tests.rs`
  Purpose: cover `/api/reply`, `/help`, and richer status handling.
- Modify: `crates/codex-bridge-core/tests/napcat_transport_tests.rs`
  Purpose: cover `set_msg_emoji_like` and structured send payloads.
- Modify: `crates/codex-bridge-core/tests/orchestrator_tests.rs`
  Purpose: cover friend rejection, group salute, zero-skill fallback, and command ownership.
- Modify: `crates/codex-bridge-core/tests/codex_runtime_tests.rs`
  Purpose: cover the new prompt/runtime policy and stop assuming final assistant text becomes the default QQ reply.
- Modify: `crates/codex-bridge-cli/tests/cli_tests.rs`
  Purpose: cover the new `reply` CLI.
- Create: `skills/reply_current/SKILL.md`
  Purpose: define the single reply skill Codex uses for text/image/file returns.
- Create: `.agents/skills` (symlink to `skills/`)
  Purpose: expose first-party skills to Codex discovery.
- Modify: `README.md`
  Purpose: document the new trigger rules, permissions, and skill-driven result path.

### Task 1: Build the Runtime Permission Substrate

**Files:**
- Create: `crates/codex-bridge-core/src/workspace_guard.rs`
- Modify: `crates/codex-bridge-core/src/runtime.rs`
- Modify: `crates/codex-bridge-core/src/config.rs`
- Modify: `crates/codex-bridge-core/src/lib.rs`
- Test: `crates/codex-bridge-core/tests/workspace_guard_tests.rs`
- Test: `crates/codex-bridge-core/tests/config_tests.rs`

- [ ] **Step 1: Write the failing runtime-path and permission-shaping tests**

```rust
#[test]
fn runtime_state_creates_artifacts_dir() {
    let temp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(temp.path(), Some(temp.path().join("qq")));
    let config = RuntimeConfig::default();

    let _ = prepare_runtime_state(&paths, &config, || "webui".into(), || "ws".into()).unwrap();

    assert!(paths.runtime_root.join("artifacts").is_dir());
}

#[test]
fn workspace_guard_blocks_new_files_outside_artifacts() {
    let fixture = WorkspaceFixture::tracked_tree();
    let guard = WorkspaceGuard::new(fixture.repo_root(), fixture.artifacts_root());
    let lease = guard.apply().unwrap();

    assert!(fixture.can_write_existing("src/lib.rs"));
    assert!(!fixture.can_create("src/new_file.rs"));
    assert!(fixture.can_create(".run/artifacts/output.md"));

    lease.restore().unwrap();
}
```

- [ ] **Step 2: Run the focused tests to verify the new behavior is missing**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core runtime_state_creates_artifacts_dir -- --nocapture
cargo test -p codex-bridge-core workspace_guard_blocks_new_files_outside_artifacts -- --nocapture
```

Expected:

- `runtime_state_creates_artifacts_dir` fails because `RuntimePaths` has no artifacts path yet.
- `workspace_guard_blocks_new_files_outside_artifacts` fails because `WorkspaceGuard` does not exist yet.

- [ ] **Step 3: Implement runtime paths and the workspace permission shaper**

```rust
// crates/codex-bridge-core/src/config.rs
pub struct RuntimeConfig {
    pub group_start_reaction_emoji_id: String,
    // existing fields...
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            group_start_reaction_emoji_id: "282".to_string(),
            // existing defaults...
        }
    }
}

// crates/codex-bridge-core/src/runtime.rs
pub struct RuntimePaths {
    pub artifacts_dir: PathBuf,
    pub reply_context_file: PathBuf,
    pub skills_dir: PathBuf,
    pub agents_dir: PathBuf,
    pub agents_skills_link: PathBuf,
    // existing fields...
}

// crates/codex-bridge-core/src/workspace_guard.rs
pub struct WorkspaceGuard {
    repo_root: PathBuf,
    artifacts_root: PathBuf,
    tracked_files: BTreeSet<PathBuf>,
}

impl WorkspaceGuard {
    pub fn apply(&self) -> Result<WorkspaceLease> {
        // make tracked files writable
        // make directories read/execute only
        // leave .run/artifacts writable for creation
    }
}
```

- [ ] **Step 4: Re-run the focused tests and the existing config tests**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core runtime_state_creates_artifacts_dir -- --nocapture
cargo test -p codex-bridge-core workspace_guard_blocks_new_files_outside_artifacts -- --nocapture
cargo test -p codex-bridge-core --test config_tests -- --nocapture
```

Expected: all three commands pass.

- [ ] **Step 5: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add crates/codex-bridge-core/src/config.rs \
        crates/codex-bridge-core/src/runtime.rs \
        crates/codex-bridge-core/src/workspace_guard.rs \
        crates/codex-bridge-core/src/lib.rs \
        crates/codex-bridge-core/tests/workspace_guard_tests.rs \
        crates/codex-bridge-core/tests/config_tests.rs
git commit -m "feat: add workspace guard for artifact-only file creation"
```

### Task 2: Replace the Old Prompt and Approval Model

**Files:**
- Modify: `crates/codex-bridge-core/src/system_prompt.rs`
- Modify: `crates/codex-bridge-core/src/approval_guard.rs`
- Modify: `crates/codex-bridge-core/src/codex_runtime.rs`
- Test: `crates/codex-bridge-core/tests/codex_runtime_tests.rs`
- Create: `crates/codex-bridge-core/tests/approval_guard_tests.rs`

- [ ] **Step 1: Write the failing prompt/approval tests**

```rust
#[test]
fn system_prompt_mentions_artifacts_only_creation_and_reply_skill() {
    assert!(SYSTEM_PROMPT_TEXT.contains(".run/artifacts/"));
    assert!(SYSTEM_PROMPT_TEXT.contains("reply skill"));
    assert!(SYSTEM_PROMPT_TEXT.contains("machine"));
}

#[test]
fn approval_guard_allows_process_inspection_but_denies_kill_and_systemctl_restart() {
    let guard = ApprovalGuard::new("/repo");

    assert_eq!(
        guard.review_command("ps aux", "/tmp", &[]),
        ApprovalDecision::Allow
    );
    assert!(matches!(
        guard.review_command("kill 123", "/tmp", &[]),
        ApprovalDecision::Deny(_)
    ));
    assert!(matches!(
        guard.review_command("systemctl restart qq", "/tmp", &[]),
        ApprovalDecision::Deny(_)
    ));
}
```

- [ ] **Step 2: Run the focused tests to confirm the current policy is wrong**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core system_prompt_mentions_artifacts_only_creation_and_reply_skill -- --nocapture
cargo test -p codex-bridge-core approval_guard_allows_process_inspection_but_denies_kill_and_systemctl_restart -- --nocapture
```

Expected:

- prompt test fails because the current prompt still says "project only".
- guard test fails because the current guard still treats non-workspace cwd as invalid.

- [ ] **Step 3: Implement the new runtime policy**

```rust
// crates/codex-bridge-core/src/system_prompt.rs
pub const SYSTEM_PROMPT_VERSION: &str = "v2.0.0";
pub const SYSTEM_PROMPT_TEXT: &str = "\
You are a cybernetic assistant with a restrained Bocchi-like personality...
You may inspect the host machine broadly, including process and socket state.
You may modify existing files in this repository.
You may create new files only under .run/artifacts/.
Use the unified reply skill for normal successful results.
Never use thread/shellCommand.
Never run dangerous host-control commands such as kill, pkill, killall,
shutdown, reboot, poweroff, systemctl stop, systemctl restart, or systemctl kill.";

// crates/codex-bridge-core/src/approval_guard.rs
pub fn review_command(&self, command: &str, cwd: &str, writable_roots: &[String]) -> ApprovalDecision {
    if command_is_dangerous(command) {
        return ApprovalDecision::Deny(...);
    }
    if !command_matches_allowlist(command) {
        return ApprovalDecision::Deny("denied non-inspection shell command".into());
    }
    ApprovalDecision::Allow
}

// crates/codex-bridge-core/src/codex_runtime.rs
fn default_sandbox_policy(_workspace_root: &PathBuf) -> SandboxPolicy {
    SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![AbsolutePathBuf::from_absolute_path(_workspace_root)
            .expect("workspace root must be absolute")],
        read_only_access: ReadOnlyAccess::FullAccess,
        network_access: true,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
    }
}
```

- [ ] **Step 4: Re-run the approval/runtime tests**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test approval_guard_tests -- --nocapture
cargo test -p codex-bridge-core --test codex_runtime_tests -- --nocapture
```

Expected: both test binaries pass.

- [ ] **Step 5: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add crates/codex-bridge-core/src/system_prompt.rs \
        crates/codex-bridge-core/src/approval_guard.rs \
        crates/codex-bridge-core/src/codex_runtime.rs \
        crates/codex-bridge-core/tests/approval_guard_tests.rs \
        crates/codex-bridge-core/tests/codex_runtime_tests.rs
git commit -m "feat: enforce host-read and artifact-create runtime policy"
```

### Task 3: Carry Sender Ownership and Friend Admission Metadata Through the Core

**Files:**
- Modify: `crates/codex-bridge-core/src/message_router.rs`
- Modify: `crates/codex-bridge-core/src/scheduler.rs`
- Modify: `crates/codex-bridge-core/src/state_store.rs`
- Modify: `crates/codex-bridge-core/src/service.rs`
- Test: `crates/codex-bridge-core/tests/message_router_tests.rs`
- Test: `crates/codex-bridge-core/tests/scheduler_tests.rs`
- Test: `crates/codex-bridge-core/tests/state_store_tests.rs`

- [ ] **Step 1: Write the failing routing and ownership tests**

```rust
#[test]
fn private_help_routes_as_command_with_sender_metadata() {
    let mut router = MessageRouter::new();
    let event = private_event("/help", 42, 9001);

    let Some(RouteDecision::Command(command)) = router.route_event(event) else {
        panic!("expected command");
    };

    assert_eq!(command.command, ControlCommand::Help);
    assert_eq!(command.source_sender_id, 42);
    assert_eq!(command.source_message_id, 9001);
}

#[test]
fn scheduler_retry_candidate_is_scoped_by_conversation_and_owner() {
    let mut scheduler = Scheduler::new(5);
    scheduler.record_terminal_state("task-1", "group:99", 42, TaskState::Failed, None);
    scheduler.record_terminal_state("task-2", "group:99", 43, TaskState::Failed, None);

    let task = scheduler.retry_candidate("group:99", 42).unwrap();
    assert_eq!(task.task_id, "task-1");
}
```

- [ ] **Step 2: Run the focused tests to verify the metadata is missing**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core private_help_routes_as_command_with_sender_metadata -- --nocapture
cargo test -p codex-bridge-core scheduler_retry_candidate_is_scoped_by_conversation_and_owner -- --nocapture
```

Expected:

- router test fails because `/help` and sender metadata do not exist yet.
- scheduler test fails because `TaskSummary` does not store owner id yet.

- [ ] **Step 3: Implement richer routing, scheduler, and persistence models**

```rust
// crates/codex-bridge-core/src/message_router.rs
pub enum ControlCommand {
    Help,
    Status,
    Queue,
    Cancel,
    RetryLast,
}

pub struct CommandRequest {
    pub command: ControlCommand,
    pub conversation_key: String,
    pub reply_target_id: i64,
    pub is_group: bool,
    pub source_message_id: i64,
    pub source_sender_id: i64,
    pub source_sender_name: String,
}

// crates/codex-bridge-core/src/scheduler.rs
pub struct TaskSummary {
    pub task_id: String,
    pub conversation_key: String,
    pub owner_sender_id: i64,
    pub source_message_id: i64,
    pub state: TaskState,
    pub summary: Option<String>,
}

// crates/codex-bridge-core/src/state_store.rs
ALTER TABLE task_runs ADD COLUMN owner_sender_id INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_runs ADD COLUMN source_message_id INTEGER NOT NULL DEFAULT 0;
```

- [ ] **Step 4: Re-run the focused tests and the full persistence/routing suites**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test message_router_tests -- --nocapture
cargo test -p codex-bridge-core --test scheduler_tests -- --nocapture
cargo test -p codex-bridge-core --test state_store_tests -- --nocapture
```

Expected: all three test binaries pass.

- [ ] **Step 5: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add crates/codex-bridge-core/src/message_router.rs \
        crates/codex-bridge-core/src/scheduler.rs \
        crates/codex-bridge-core/src/state_store.rs \
        crates/codex-bridge-core/src/service.rs \
        crates/codex-bridge-core/tests/message_router_tests.rs \
        crates/codex-bridge-core/tests/scheduler_tests.rs \
        crates/codex-bridge-core/tests/state_store_tests.rs
git commit -m "feat: track sender ownership for qq task routing"
```

### Task 4: Add Structured Outbound Transport and the Unified Reply Surface

**Files:**
- Create: `crates/codex-bridge-core/src/outbound.rs`
- Create: `crates/codex-bridge-core/src/reply_context.rs`
- Modify: `crates/codex-bridge-core/src/service.rs`
- Modify: `crates/codex-bridge-core/src/api.rs`
- Modify: `crates/codex-bridge-core/src/napcat.rs`
- Modify: `crates/codex-bridge-core/src/lib.rs`
- Modify: `crates/codex-bridge-cli/src/cli.rs`
- Modify: `crates/codex-bridge-cli/src/main.rs`
- Test: `crates/codex-bridge-core/tests/reply_context_tests.rs`
- Test: `crates/codex-bridge-core/tests/api_tests.rs`
- Test: `crates/codex-bridge-core/tests/napcat_transport_tests.rs`
- Test: `crates/codex-bridge-cli/tests/cli_tests.rs`

- [ ] **Step 1: Write the failing tests for reply tokens, structured sends, and group salute**

```rust
#[tokio::test]
async fn reply_context_token_can_send_multiple_times_until_revoked() {
    let registry = ReplyRegistry::default();
    let token = registry.issue(ReplyContext {
        task_id: "task-1".into(),
        conversation_key: "group:9".into(),
        is_group: true,
        target_id: 9,
        source_message_id: 77,
        source_sender_id: 42,
    });

    assert!(registry.resolve(&token).is_some());
    assert!(registry.resolve(&token).is_some());
    registry.revoke_task("task-1");
    assert!(registry.resolve(&token).is_none());
}

#[test]
fn build_group_reply_segments_wraps_reply_and_at_before_text() {
    let payload = OutboundPayload::Text("已完成".into());
    let context = ReplyContext::group("task-1", 9, 77, 42);

    let segments = build_group_reply_segments(&context, &payload).unwrap();
    assert_eq!(segments[0]["type"], "reply");
    assert_eq!(segments[1]["type"], "at");
    assert_eq!(segments[2]["type"], "text");
}

#[test]
fn cli_reply_subcommand_requires_exactly_one_payload_mode() {
    let cli = Cli::try_parse_from(["codex-bridge", "reply", "--text", "a", "--file", "b"]);
    assert!(cli.is_err());
}
```

- [ ] **Step 2: Run the focused tests to prove the reply path does not exist yet**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core reply_context_token_can_send_multiple_times_until_revoked -- --nocapture
cargo test -p codex-bridge-core build_group_reply_segments_wraps_reply_and_at_before_text -- --nocapture
cargo test -p codex-bridge-cli cli_reply_subcommand_requires_exactly_one_payload_mode -- --nocapture
```

Expected: all three tests fail because the new modules and CLI command do not exist yet.

- [ ] **Step 3: Implement reply tokens, the local `/api/reply`, and NapCat structured outbound actions**

```rust
// crates/codex-bridge-core/src/outbound.rs
pub enum OutboundPayload {
    Text(String),
    Image(PathBuf),
    File(PathBuf),
}

pub enum OutboundCommand {
    Reply { context: ReplyContext, payload: OutboundPayload },
    GroupReaction { group_id: i64, message_id: i64, emoji_id: String },
}

pub fn build_group_reply_segments(
    context: &ReplyContext,
    payload: &OutboundPayload,
) -> Result<Vec<serde_json::Value>> {
    // prepend reply + at, then append the text/image/file segment
}

// crates/codex-bridge-core/src/reply_context.rs
pub struct ReplyContext {
    pub token: String,
    pub task_id: String,
    pub conversation_key: String,
    pub is_group: bool,
    pub target_id: i64,
    pub source_message_id: i64,
    pub source_sender_id: i64,
}

pub struct StoredReplyContext {
    pub token: String,
    pub task_id: String,
}

// crates/codex-bridge-cli/src/cli.rs
Reply {
    #[arg(long)]
    text: Option<String>,
    #[arg(long)]
    image: Option<PathBuf>,
    #[arg(long)]
    file: Option<PathBuf>,
}
```

```rust
// crates/codex-bridge-core/src/reply_context.rs
impl ReplyRegistry {
    pub fn activate(&self, context: ReplyContext, path: &Path) -> Result<()> {
        self.insert(context.clone());
        fs::write(path, serde_json::to_vec(&StoredReplyContext::from(&context))?)?;
        Ok(())
    }
}

// crates/codex-bridge-cli/src/main.rs
fn current_reply_token(project_root: &Path) -> Result<String> {
    let path = project_root.join(".run/default/run/reply_context.json");
    let stored: StoredReplyContext = serde_json::from_slice(&fs::read(path)?)?;
    Ok(stored.token)
}

// crates/codex-bridge-core/src/service.rs
impl ServiceState {
    pub async fn react_to_group_message(
        &self,
        group_id: i64,
        message_id: i64,
        emoji_id: impl Into<String>,
    ) -> Result<()> {
        self.send_outbound_command(OutboundCommand::GroupReaction {
            group_id,
            message_id,
            emoji_id: emoji_id.into(),
        })
        .await
    }
}
```

- [ ] **Step 4: Re-run the reply, API, transport, and CLI suites**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test reply_context_tests -- --nocapture
cargo test -p codex-bridge-core --test api_tests -- --nocapture
cargo test -p codex-bridge-core --test napcat_transport_tests -- --nocapture
cargo test -p codex-bridge-cli --test cli_tests -- --nocapture
```

Expected: all four test binaries pass.

- [ ] **Step 5: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add crates/codex-bridge-core/src/outbound.rs \
        crates/codex-bridge-core/src/reply_context.rs \
        crates/codex-bridge-core/src/service.rs \
        crates/codex-bridge-core/src/api.rs \
        crates/codex-bridge-core/src/napcat.rs \
        crates/codex-bridge-core/src/lib.rs \
        crates/codex-bridge-cli/src/cli.rs \
        crates/codex-bridge-cli/src/main.rs \
        crates/codex-bridge-core/tests/reply_context_tests.rs \
        crates/codex-bridge-core/tests/api_tests.rs \
        crates/codex-bridge-core/tests/napcat_transport_tests.rs \
        crates/codex-bridge-cli/tests/cli_tests.rs
git commit -m "feat: add unified reply surface for codex results"
```

### Task 5: Rework the Orchestrator Around Admission, Persona, and Skill-Owned Results

**Files:**
- Modify: `crates/codex-bridge-core/src/reply_formatter.rs`
- Modify: `crates/codex-bridge-core/src/orchestrator.rs`
- Modify: `crates/codex-bridge-core/src/service.rs`
- Test: `crates/codex-bridge-core/tests/orchestrator_tests.rs`

- [ ] **Step 1: Write the failing orchestration tests**

```rust
#[tokio::test]
async fn non_friend_private_message_is_rejected_before_queueing() {
    let harness = OrchestratorHarness::with_friends(vec![]);

    harness.push_private_message(PrivateMessageSpec::friendless("你好"));
    harness.run_once().await.unwrap();

    assert_eq!(harness.queue_len(), 0);
    assert_eq!(harness.codex_turn_count(), 0);
    assert!(harness.replies().contains(&"先加好友再来找我".to_string()));
}

#[tokio::test]
async fn group_task_starts_with_salute_reaction_and_no_text_ack() {
    let harness = OrchestratorHarness::with_friends(vec![]);

    harness.push_group_message(GroupMessageSpec::mention("帮我看一下"));
    harness.run_until_start().await.unwrap();

    assert!(harness.outbound().contains_group_reaction(77, "282"));
    assert!(!harness.replies().iter().any(|text| text.contains("开始处理")));
}

#[tokio::test]
async fn successful_turn_without_skill_reply_sends_short_fallback_notice() {
    let harness = OrchestratorHarness::turn_completes_without_reply();
    harness.run_until_idle().await.unwrap();
    assert_eq!(harness.last_reply(), Some("任务完成了，但这次没有生成可发送结果。".to_string()));
}
```

- [ ] **Step 2: Run the orchestrator suite to verify the current behavior is still the old text bridge**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test orchestrator_tests -- --nocapture
```

Expected: failures for friend admission, group salute, `/help`, command ownership, and zero-skill fallback.

- [ ] **Step 3: Implement the orchestrator policy split**

```rust
// crates/codex-bridge-core/src/reply_formatter.rs
pub fn format_started_private() -> String {
    "欸、我看到了……先让我处理一下。".to_string()
}

pub fn format_help() -> String {
    "\
私聊默认触发，群里需要 @我。\n\
非好友私聊不会调用 Codex。\n\
命令：/help /status /queue /cancel /retry_last\n\
权限：全机可读，仅当前仓库可写，新文件只进 .run/artifacts/，危险操作会被拒绝。"
        .to_string()
}

// crates/codex-bridge-core/src/orchestrator.rs
async fn is_friend_sender(state: &ServiceState, sender_id: i64) -> Result<bool> {
    let friends = state.friends().await;
    Ok(friends.iter().any(|friend| friend.user_id == sender_id))
}

if task.is_private() && !is_friend_sender(&state, task.source_sender_id).await? {
    return send_reply(... format_non_friend_rejection()).await;
}

reply_registry.activate(reply_context, &runtime_paths.reply_context_file)?;
codex.start_turn(...).await?;

if task.is_group {
    service.react_to_group_message(task.reply_target_id, task.source_message_id, "282").await?;
} else {
    send_reply(... format_started_private()).await?;
}

if !reply_registry.task_sent_reply(&task_id) {
    send_reply(... "任务完成了，但这次没有生成可发送结果。".to_string()).await?;
}
```

- [ ] **Step 4: Re-run the orchestrator suite**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test orchestrator_tests -- --nocapture
```

Expected: orchestrator tests pass, including `/help`, friend gating, ownership checks, reaction-based start feedback, and zero-skill fallback.

- [ ] **Step 5: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add crates/codex-bridge-core/src/reply_formatter.rs \
        crates/codex-bridge-core/src/orchestrator.rs \
        crates/codex-bridge-core/src/service.rs \
        crates/codex-bridge-core/tests/orchestrator_tests.rs
git commit -m "feat: enforce qq admission and skill-driven result delivery"
```

### Task 6: Add Skill Discovery and User-Facing Documentation

**Files:**
- Create: `skills/reply_current/SKILL.md`
- Create: `.agents/skills`
- Modify: `README.md`
- Test: manual verification commands only

- [ ] **Step 1: Write the skill and symlink with the exact supported command surface**

```markdown
# reply_current

Use this skill to send the final result back to the current QQ conversation.

Rules:
- Never choose a QQ or group target yourself.
- Use exactly one of:
  - `codex-bridge reply --text "..."`
  - `codex-bridge reply --image .run/artifacts/<file>`
  - `codex-bridge reply --file .run/artifacts/<file>`
- Images and files must already exist under `.run/artifacts/`.
- In groups, the bridge will automatically reference the source message and @ the source sender.
```

```bash
cd /home/ts_user/llm_pro/codex-bridge
ln -s ../skills .agents/skills
```

- [ ] **Step 2: Update README to match the new runtime contract**

```markdown
## Trigger Rules

- Private chat: only QQ friends can trigger Codex.
- Group chat: mention the bot with `@`.
- Commands: `/help`, `/status`, `/queue`, `/cancel`, `/retry_last`.

## Result Delivery

- Bridge-generated: start/status/error/policy messages.
- Skill-generated: normal successful text/image/file results.

## Permissions

- Read: machine-wide inspection is allowed.
- Modify: existing repository files may be edited.
- Create: new files may only be created under `.run/artifacts/`.
- Dangerous host-control commands are denied.
```

- [ ] **Step 3: Run the end-to-end verification commands**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo fmt --all --check
cargo test -p codex-bridge-core -- --nocapture
cargo test -p codex-bridge-cli -- --nocapture
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: all commands pass.

- [ ] **Step 4: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add skills/reply_current/SKILL.md \
        .agents/skills \
        README.md
git commit -m "docs: document codex bridge qq skill workflow"
```

## Coverage Check

- Friend-only private-message admission: Task 5.
- Group `@bot` handling and `/help`: Tasks 3 and 5.
- Command ownership for `/cancel` and `/retry_last`: Tasks 3 and 5.
- Persona-aligned private start text and group salute reaction: Tasks 4 and 5.
- Unified reply skill for text/image/file: Tasks 4 and 6.
- `.run/artifacts/` creation rule: Tasks 1, 2, and 4.
- Machine-wide read plus dangerous-command denial: Task 2.
- Zero-reply fallback after successful turns: Task 5.

## Notes for Execution

- Do not ship a fallback where successful assistant text is silently mirrored to QQ. That would violate the new bridge-vs-skill ownership boundary.
- Do not reintroduce workspace-only sandboxing. The point of `WorkspaceGuard` is to preserve machine-wide inspection while narrowing mutation correctly.
- Do not widen the reply CLI to accept arbitrary QQ or group targets. That breaks the single-conversation guarantee.
