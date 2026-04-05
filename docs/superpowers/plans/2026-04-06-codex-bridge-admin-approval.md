# Codex Bridge Admin Approval Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a file-configured admin approval gate so only admin private chat executes Codex immediately, while every other executable QQ request must be approved or denied from admin private chat first.

**Architecture:** Keep the existing NapCat transport and single global Codex execution queue, but insert a separate pending-approval layer before queue admission. Persist approval-task ids and states in SQLite, keep the live pending-approval pool in memory with timeout handling, and route admin-only `/approve`, `/deny`, and `/status <task_id>` commands through the bridge rather than Codex.

**Tech Stack:** Rust (`tokio`, `rusqlite`, `axum`, `clap`), existing NapCat OneBot WebSocket transport, SQLite runtime store, operator-owned `.run/default/config/admin.toml`.

---

## File Structure

- Create: `crates/codex-bridge-core/src/admin_approval.rs`
  Purpose: define `AdminConfig`, `PendingApproval`, and the in-memory pending-approval pool with timeout/de-duplication behavior.
- Modify: `crates/codex-bridge-core/src/runtime.rs`
  Purpose: add `admin.toml` runtime path helpers, create template config, and fail clearly when admin config is missing or invalid.
- Modify: `crates/codex-bridge-core/src/config.rs`
  Purpose: add approval timeout and pending-approval capacity defaults.
- Modify: `crates/codex-bridge-core/src/message_router.rs`
  Purpose: parse admin approval commands with task ids and distinguish direct `/help` from approval-required commands.
- Modify: `crates/codex-bridge-core/src/reply_formatter.rs`
  Purpose: add requester/admin-facing approval notices, denial/timeout text, and "already waiting approval" feedback.
- Modify: `crates/codex-bridge-core/src/state_store.rs`
  Purpose: add approval-aware task states (`PendingApproval`, `Denied`, `Expired`), task lookup by id, and startup cleanup for stale pending approvals.
- Modify: `crates/codex-bridge-core/src/service.rs`
  Purpose: carry admin approval config/state into runtime-facing shared state if needed and expose richer task snapshot helpers.
- Modify: `crates/codex-bridge-core/src/api.rs`
  Purpose: keep `/status` consistent with approval-aware task states where needed.
- Modify: `crates/codex-bridge-core/src/orchestrator.rs`
  Purpose: insert the pending-approval gate before Codex execution, process admin commands, handle timeout expiry, and send requester/admin feedback.
- Modify: `crates/codex-bridge-core/src/lib.rs`
  Purpose: export the new `admin_approval` module.
- Modify: `README.md`
  Purpose: document the admin config file, approval flow, and new admin commands.
- Create: `crates/codex-bridge-core/tests/admin_approval_tests.rs`
  Purpose: verify pool behavior, duplicate suppression, timeout expiry, and admin config parsing.
- Modify: `crates/codex-bridge-core/tests/config_tests.rs`
  Purpose: cover `admin.toml` creation and validation.
- Modify: `crates/codex-bridge-core/tests/message_router_tests.rs`
  Purpose: cover `/approve <task_id>`, `/deny <task_id>`, `/status <task_id>`, and `/help` behavior.
- Modify: `crates/codex-bridge-core/tests/state_store_tests.rs`
  Purpose: cover new task statuses and stale pending-approval cleanup.
- Modify: `crates/codex-bridge-core/tests/orchestrator_tests.rs`
  Purpose: cover admin-private direct bypass, non-admin pending approval, group approval flow, denial, timeout, and duplicate pending request behavior.

### Task 1: Add Runtime Admin Config and Approval Pool Types

**Files:**
- Create: `crates/codex-bridge-core/src/admin_approval.rs`
- Modify: `crates/codex-bridge-core/src/runtime.rs`
- Modify: `crates/codex-bridge-core/src/config.rs`
- Modify: `crates/codex-bridge-core/src/lib.rs`
- Create: `crates/codex-bridge-core/tests/admin_approval_tests.rs`
- Modify: `crates/codex-bridge-core/tests/config_tests.rs`

- [ ] **Step 1: Write the failing config/pool tests**

```rust
#[test]
fn prepare_runtime_state_creates_admin_template_when_missing() {
    let temp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(temp.path(), Some(temp.path().join("qq")));
    let config = RuntimeConfig::default();

    let error = prepare_runtime_state(&paths, &config, || "webui".into(), || "ws".into())
        .expect_err("missing admin config should fail closed");

    assert!(paths.config_dir.join("admin.toml").is_file());
    assert!(error.to_string().contains("admin_user_id"));
}

#[test]
fn pending_pool_rejects_duplicate_conversation_while_waiting() {
    let mut pool = PendingApprovalPool::new(32);
    let request = PendingApproval::new("task-1", "group:9", 42, 9001, false, "帮我执行".into());

    assert!(pool.insert(request.clone()).is_ok());
    assert!(matches!(
        pool.insert(PendingApproval::new("task-2", "group:9", 42, 9002, false, "第二条".into())),
        Err(PendingApprovalError::ConversationAlreadyWaiting)
    ));
}
```

- [ ] **Step 2: Run the focused tests to confirm the feature is missing**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core prepare_runtime_state_creates_admin_template_when_missing -- --nocapture
cargo test -p codex-bridge-core pending_pool_rejects_duplicate_conversation_while_waiting -- --nocapture
```

Expected:

- runtime test fails because `admin.toml` does not exist and no fail-closed path is implemented,
- pool test fails because `admin_approval` module does not exist yet.

- [ ] **Step 3: Implement admin config parsing and the in-memory pool**

```rust
// crates/codex-bridge-core/src/config.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub approval_timeout_secs: u64,
    pub pending_approval_capacity: usize,
    // existing fields...
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            approval_timeout_secs: 900,
            pending_approval_capacity: 32,
            // existing defaults...
        }
    }
}

// crates/codex-bridge-core/src/admin_approval.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminConfig {
    pub admin_user_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    pub task_id: String,
    pub conversation_key: String,
    pub source_sender_id: i64,
    pub source_message_id: i64,
    pub is_group: bool,
    pub source_text: String,
    pub created_at: std::time::Instant,
    pub expires_at: std::time::Instant,
}

pub struct PendingApprovalPool {
    by_task_id: std::collections::HashMap<String, PendingApproval>,
    by_conversation: std::collections::HashMap<String, String>,
    capacity: usize,
}
```

- [ ] **Step 4: Re-run the focused tests and the existing config tests**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core prepare_runtime_state_creates_admin_template_when_missing -- --nocapture
cargo test -p codex-bridge-core pending_pool_rejects_duplicate_conversation_while_waiting -- --nocapture
cargo test -p codex-bridge-core --test config_tests -- --nocapture
```

Expected: all three commands pass.

- [ ] **Step 5: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add crates/codex-bridge-core/src/admin_approval.rs \
        crates/codex-bridge-core/src/runtime.rs \
        crates/codex-bridge-core/src/config.rs \
        crates/codex-bridge-core/src/lib.rs \
        crates/codex-bridge-core/tests/admin_approval_tests.rs \
        crates/codex-bridge-core/tests/config_tests.rs
git commit -m "feat: add admin approval runtime config"
```

### Task 2: Extend Command Parsing for Admin Approval Commands

**Files:**
- Modify: `crates/codex-bridge-core/src/message_router.rs`
- Modify: `crates/codex-bridge-core/tests/message_router_tests.rs`

- [ ] **Step 1: Write the failing router tests**

```rust
#[test]
fn private_approve_command_keeps_task_id() {
    let mut router = MessageRouter::new();
    let event = private_event("/approve task-123", 42, 9001);

    let decision = router.route_event(event).expect("decision");
    let RouteDecision::Command(command) = decision else { panic!("expected command"); };
    assert_eq!(command.command, ControlCommand::Approve { task_id: "task-123".into() });
}

#[test]
fn group_help_still_routes_without_task_id() {
    let mut router = MessageRouter::new();
    let event = group_event("@bot /help", 9, 42, 7001);

    let decision = router.route_event(event).expect("decision");
    let RouteDecision::Command(command) = decision else { panic!("expected command"); };
    assert_eq!(command.command, ControlCommand::Help);
}
```

- [ ] **Step 2: Run the focused tests to verify parsing is missing**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core private_approve_command_keeps_task_id -- --nocapture
cargo test -p codex-bridge-core group_help_still_routes_without_task_id -- --nocapture
```

Expected:

- `/approve <task_id>` fails because `ControlCommand` has no approval variants,
- existing command parsing ignores task-id arguments.

- [ ] **Step 3: Implement approval-aware command parsing**

```rust
// crates/codex-bridge-core/src/message_router.rs
pub enum ControlCommand {
    Help,
    Status { task_id: Option<String> },
    Queue,
    Cancel,
    RetryLast,
    Approve { task_id: String },
    Deny { task_id: String },
}

fn parse_command(text: &str) -> Option<ControlCommand> {
    let mut parts = text.split_whitespace();
    match parts.next()? {
        "/help" => Some(ControlCommand::Help),
        "/status" => Some(ControlCommand::Status {
            task_id: parts.next().map(ToString::to_string),
        }),
        "/queue" => Some(ControlCommand::Queue),
        "/cancel" => Some(ControlCommand::Cancel),
        "/retry_last" => Some(ControlCommand::RetryLast),
        "/approve" => parts.next().map(|id| ControlCommand::Approve { task_id: id.to_string() }),
        "/deny" => parts.next().map(|id| ControlCommand::Deny { task_id: id.to_string() }),
        _ => None,
    }
}
```

- [ ] **Step 4: Re-run the focused router tests**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core private_approve_command_keeps_task_id -- --nocapture
cargo test -p codex-bridge-core group_help_still_routes_without_task_id -- --nocapture
cargo test -p codex-bridge-core --test message_router_tests -- --nocapture
```

Expected: all commands pass.

- [ ] **Step 5: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add crates/codex-bridge-core/src/message_router.rs \
        crates/codex-bridge-core/tests/message_router_tests.rs
git commit -m "feat: parse admin approval commands"
```

### Task 3: Persist Approval States and Task Lookup

**Files:**
- Modify: `crates/codex-bridge-core/src/state_store.rs`
- Modify: `crates/codex-bridge-core/tests/state_store_tests.rs`

- [ ] **Step 1: Write the failing store tests**

```rust
#[test]
fn task_status_supports_pending_denied_and_expired() {
    let store = StateStore::open_in_memory().unwrap();
    let binding = ConversationBinding {
        conversation_key: "group:9".into(),
        thread_id: "thread-1".into(),
    };
    store.upsert_binding(&binding).unwrap();
    let task_id = store.insert_task_with_source(&binding, TaskStatus::PendingApproval, 42, 7001).unwrap();

    store.update_task_status(&task_id, TaskStatus::Denied).unwrap();
    let latest = store.latest_task_for_conversation("group:9").unwrap().unwrap();
    assert_eq!(latest.status, TaskStatus::Denied);
}

#[test]
fn opening_store_marks_stale_pending_approvals_expired() {
    let store = StateStore::open_in_memory().unwrap();
    let binding = ConversationBinding {
        conversation_key: "private:42".into(),
        thread_id: "thread-1".into(),
    };
    store.upsert_binding(&binding).unwrap();
    let task_id = store.insert_task_with_source(&binding, TaskStatus::PendingApproval, 42, 9001).unwrap();

    store.mark_pending_approvals_expired().unwrap();
    let latest = store.latest_task_for_conversation("private:42").unwrap().unwrap();
    assert_eq!(latest.task_id, task_id);
    assert_eq!(latest.status, TaskStatus::Expired);
}
```

- [ ] **Step 2: Run the focused tests to confirm the schema is incomplete**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core task_status_supports_pending_denied_and_expired -- --nocapture
cargo test -p codex-bridge-core opening_store_marks_stale_pending_approvals_expired -- --nocapture
```

Expected: tests fail because `TaskStatus` lacks approval states and no stale-pending cleanup exists.

- [ ] **Step 3: Extend the SQLite-backed task model**

```rust
// crates/codex-bridge-core/src/state_store.rs
pub enum TaskStatus {
    PendingApproval,
    Queued,
    Running,
    Completed,
    Failed,
    Denied,
    Expired,
    Canceled,
    Interrupted,
}

impl StateStore {
    pub fn task_by_id(&self, task_id: &str) -> Result<Option<TaskRecord>> { /* query by id */ }

    pub fn mark_pending_approvals_expired(&self) -> Result<usize> {
        self.conn.execute(
            "UPDATE task_runs SET status = ?1 WHERE status = ?2",
            params![TaskStatus::Expired.as_str(), TaskStatus::PendingApproval.as_str()],
        )?;
        Ok(updated)
    }
}
```

- [ ] **Step 4: Re-run the focused store tests and the full store suite**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core task_status_supports_pending_denied_and_expired -- --nocapture
cargo test -p codex-bridge-core opening_store_marks_stale_pending_approvals_expired -- --nocapture
cargo test -p codex-bridge-core --test state_store_tests -- --nocapture
```

Expected: all store tests pass.

- [ ] **Step 5: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add crates/codex-bridge-core/src/state_store.rs \
        crates/codex-bridge-core/tests/state_store_tests.rs
git commit -m "feat: persist admin approval task states"
```

### Task 4: Insert Admin Approval into the Orchestrator

**Files:**
- Modify: `crates/codex-bridge-core/src/orchestrator.rs`
- Modify: `crates/codex-bridge-core/src/reply_formatter.rs`
- Modify: `crates/codex-bridge-core/src/service.rs`
- Modify: `crates/codex-bridge-core/src/api.rs`
- Modify: `crates/codex-bridge-core/tests/orchestrator_tests.rs`

- [ ] **Step 1: Write the failing orchestrator tests**

```rust
#[tokio::test]
async fn non_admin_private_message_enters_pending_approval_and_not_codex() {
    let harness = OrchestratorHarness::with_admin(10001).with_friends(vec![42]);
    harness.push_private_message(PrivateMessageSpec::from_user(42, "帮我看一下"));

    harness.run_until_idle().await;

    assert_eq!(harness.codex_turns_started(), 0);
    assert!(harness.pending_approval_exists_for("private:42"));
    assert!(harness.outbound().contains_private_text(42, "需要先得到管理员确认"));
    assert!(harness.outbound().contains_private_text(10001, "待审批任务"));
}

#[tokio::test]
async fn admin_approve_moves_pending_task_into_runtime_queue() {
    let harness = OrchestratorHarness::with_admin(10001).with_friends(vec![42]);
    let task_id = harness.seed_pending_private_request(42, "帮我执行");

    harness.push_admin_private_message(&format!("/approve {task_id}"));
    harness.run_until_idle().await;

    assert_eq!(harness.codex_turns_started(), 1);
    assert!(!harness.pending_approval_exists_for("private:42"));
}
```

- [ ] **Step 2: Run the focused tests to verify the approval layer is missing**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core non_admin_private_message_enters_pending_approval_and_not_codex -- --nocapture
cargo test -p codex-bridge-core admin_approve_moves_pending_task_into_runtime_queue -- --nocapture
```

Expected: both tests fail because all friend private messages still execute or queue directly.

- [ ] **Step 3: Implement the approval gate and admin command handling**

```rust
// crates/codex-bridge-core/src/orchestrator.rs
if is_admin_private_task(&task, admin_config.admin_user_id) {
    start_or_enqueue_runtime_task(...).await?;
} else {
    let approval = PendingApproval::from_task(task.clone(), config.approval_timeout_secs);
    match pending_pool.insert(approval.clone()) {
        Ok(()) => {
            store.update_task_status(&approval.task_id, TaskStatus::PendingApproval)?;
            notify_requester_waiting_approval(...).await?;
            notify_admin_of_pending_request(...).await?;
        }
        Err(PendingApprovalError::ConversationAlreadyWaiting) => {
            notify_requester_already_waiting(...).await?;
        }
        Err(PendingApprovalError::CapacityFull) => {
            notify_requester_queue_full(...).await?;
        }
    }
}

match command.command {
    ControlCommand::Approve { task_id } => approve_pending_request(...).await?,
    ControlCommand::Deny { task_id } => deny_pending_request(...).await?,
    ControlCommand::Status { task_id: Some(task_id) } if is_admin_private_command(...) => {
        send_admin_task_status(...).await?;
    }
    // existing help/cancel/retry logic...
}
```

- [ ] **Step 4: Add timeout expiry and restart cleanup**

```rust
// crates/codex-bridge-core/src/orchestrator.rs
let mut approval_tick = tokio::time::interval(std::time::Duration::from_secs(1));

loop {
    tokio::select! {
        _ = approval_tick.tick() => {
            for expired in pending_pool.take_expired() {
                state_store.update_task_status(&expired.task_id, TaskStatus::Expired)?;
                notify_requester_approval_expired(...).await?;
            }
        }
        // existing task/event/control branches...
    }
}
```

- [ ] **Step 5: Re-run the focused orchestrator tests and the full orchestrator suite**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core non_admin_private_message_enters_pending_approval_and_not_codex -- --nocapture
cargo test -p codex-bridge-core admin_approve_moves_pending_task_into_runtime_queue -- --nocapture
cargo test -p codex-bridge-core --test orchestrator_tests -- --nocapture
```

Expected: approval flow, denial, timeout, and duplicate-pending behaviors all pass.

- [ ] **Step 6: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add crates/codex-bridge-core/src/orchestrator.rs \
        crates/codex-bridge-core/src/reply_formatter.rs \
        crates/codex-bridge-core/src/service.rs \
        crates/codex-bridge-core/src/api.rs \
        crates/codex-bridge-core/tests/orchestrator_tests.rs
git commit -m "feat: gate codex execution behind admin approval"
```

### Task 5: Document the Approval Contract and Run Full Verification

**Files:**
- Modify: `README.md`
- Modify: `crates/codex-bridge-core/tests/api_tests.rs`
- Modify: `crates/codex-bridge-core/tests/message_router_tests.rs`
- Modify: `crates/codex-bridge-core/tests/orchestrator_tests.rs`

- [ ] **Step 1: Update README and help/command expectations**

```md
## Admin Approval

- Configure `.run/default/config/admin.toml` with `admin_user_id = ...`
- Only admin private chat runs directly
- All other executable requests wait for admin approval
- Admin commands:
  - `/approve <task_id>`
  - `/deny <task_id>`
  - `/status <task_id>`
```

- [ ] **Step 2: Add API/help regression coverage**

```rust
#[test]
fn help_text_mentions_admin_approval_gate() {
    let text = reply_formatter::format_help();
    assert!(text.contains("管理员"));
    assert!(text.contains("/approve"));
}
```

- [ ] **Step 3: Run the full verification suite**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo fmt --all --check
cargo test -p codex-bridge-core -- --nocapture
cargo test -p codex-bridge-cli -- --nocapture
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected:

- formatting check passes,
- core tests pass,
- CLI tests still pass,
- clippy passes with no warnings.

- [ ] **Step 4: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add README.md \
        crates/codex-bridge-core/tests/api_tests.rs \
        crates/codex-bridge-core/tests/message_router_tests.rs \
        crates/codex-bridge-core/tests/orchestrator_tests.rs
git commit -m "docs: describe admin approval workflow"
```

## Self-Review

- Spec coverage:
  - file-configured single admin QQ: Task 1
  - only admin private chat direct-exec: Task 4
  - every other executable request enters pending approval: Tasks 2 and 4
  - separate pending-approval pool outside execution queue: Tasks 1 and 4
  - admin-only `/approve`, `/deny`, `/status <task_id>`: Tasks 2 and 4
  - 15-minute timeout with requester feedback: Task 4
  - persistence of approval task ids/states: Task 3
  - restart does not resume pending approvals: Task 3 and Task 4
- Placeholder scan: no `TODO`, `TBD`, or undefined task references remain.
- Type consistency:
  - approval command variants are defined in Task 2 before use in Task 4,
  - `PendingApproval` pool is defined in Task 1 before orchestrator integration,
  - new `TaskStatus` values are defined in Task 3 before `/status <task_id>` uses them.
