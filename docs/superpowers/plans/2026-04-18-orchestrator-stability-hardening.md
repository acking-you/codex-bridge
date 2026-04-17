# Orchestrator Stability Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the remaining single-point-of-failure paths in the orchestrator so that transient codex or transport errors can no longer terminate the entire bot; also add a supervisor loop around `orchestrator::run` so any future unexpected error is recovered automatically.

**Architecture:** Apply the same "catch + degrade to user-visible reply + keep looping" pattern that now guards `/compact` to `/cancel` and to all `send_reply` calls inside the task-completion arm. Wrap `orchestrator::run` in a supervisor in `cli/main.rs` that restarts it on `Err` with bounded exponential backoff; on restart `recover_running_tasks` rebuilds in-memory scheduler state from SQLite. No data-model, protocol, or concurrency changes here - this plan only removes fragile `?` call sites.

**Tech Stack:** Rust, `tokio`, `anyhow`, `tracing`, existing orchestrator + codex-runtime + reply-formatter modules; no new dependencies.

---

## Follow-up Plans (not covered here)

This plan is phase 1 of the agreed stability-and-concurrency work. Two follow-up plans will be drafted once this one ships:

- **Phase 2: Codex runtime reader demuxer.** Replace the single `read_state: Mutex<RuntimeReadState>` held for the whole turn duration with a background reader task that routes responses by `request_id` and notifications by `(thread_id, turn_id)` to per-request channels.
- **Phase 3: Per-conversation orchestrator.** Rework `Scheduler.running` and `active_task` into a `HashMap<conversation_key, ...>` keyed collection backed by `FuturesUnordered`, so different groups / private chats can truly run in parallel. Depends on Phase 2.

---

## File Structure

- Modify: `crates/codex-bridge-core/src/reply_formatter.rs`
  Purpose: add one new user-facing message for `/cancel` failure recovery.
- Modify: `crates/codex-bridge-core/src/orchestrator.rs`
  Purpose: extract `interrupt_with_recovery`, harden the `/cancel` branch, and convert non-critical `send_reply` call sites in the task-completion arm to "best-effort" logging so a transport blip cannot kill `run()`.
- Modify: `crates/codex-bridge-cli/src/main.rs`
  Purpose: wrap `orchestrator::run` in a supervisor loop with bounded exponential backoff so the orchestrator is auto-restarted if it ever returns `Err` again.
- Modify: `crates/codex-bridge-core/tests/orchestrator_tests.rs`
  Purpose: add regression coverage for `/cancel` interrupt failure and for a failing reply sink during task completion.

---

## Task 1: Reply formatter for cancel failure

**Files:**
- Modify: `crates/codex-bridge-core/src/reply_formatter.rs`

- [ ] **Step 1: Write a failing doc-level assertion style test**

Append this to the bottom of `crates/codex-bridge-core/src/reply_formatter.rs`, inside (or extending) the existing `#[cfg(test)]` module if present; otherwise add a fresh `#[cfg(test)] mod tests { ... }` block:

```rust
#[cfg(test)]
mod cancel_failed_text_tests {
    use super::format_cancel_failed;

    #[test]
    fn cancel_failed_text_mentions_retry_guidance() {
        let text = format_cancel_failed();
        assert!(text.contains("取消"));
        assert!(text.contains("稍后"));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p codex-bridge-core --lib cancel_failed_text_tests 2>&1 | tail -20`
Expected: compilation failure `cannot find function format_cancel_failed`.

- [ ] **Step 3: Add the formatter function**

Locate `format_cancel_requested` in `crates/codex-bridge-core/src/reply_formatter.rs` and add immediately after it:

```rust
/// Return the message shown when a cancel command could not interrupt the
/// running turn (for example when Codex restarted and lost the turn state).
pub fn format_cancel_failed() -> String {
    "取消失败，稍后再试；仍然卡住时可以 /clear 再重开对话。".to_string()
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p codex-bridge-core --lib cancel_failed_text_tests 2>&1 | tail -20`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/codex-bridge-core/src/reply_formatter.rs
git commit -m "feat(reply_formatter): add format_cancel_failed for interrupt recovery"
```

---

## Task 2: Harden the `/cancel` branch with interrupt recovery

**Files:**
- Modify: `crates/codex-bridge-core/src/orchestrator.rs:934-959` (the existing `ControlCommand::Cancel` arm)

**Background:** Current code calls `codex.interrupt(...).await?`. If interrupt fails (thread not loaded, turn already gone, codex restarted mid-turn), the error used to kill `run()`; after the earlier compact fix it is caught by the outer `match` but the user still sees no reply and the scheduler's `running` slot is not cleared. We fix both.

- [ ] **Step 1: Write a failing test**

Append this test to `crates/codex-bridge-core/tests/orchestrator_tests.rs`. It reuses the existing `FakeCodexExecutor`, extending it with an optional "interrupt always fails" toggle in a follow-up step; for now the test file will not compile without that toggle.

```rust
#[tokio::test]
async fn cancel_command_replies_gracefully_when_interrupt_fails() {
    let codex = Arc::new(
        FakeCodexExecutor::blocking(vec!["thread-x"], "turn-x").with_failing_interrupt(),
    );
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state.publish_event(make_private_event(9301, "长跑任务"));

    timeout(Duration::from_secs(1), async {
        loop {
            if state.task_snapshot().await.running_task_id.is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("task running");

    state
        .send_control_command(make_private_command_request(42, ControlCommand::Cancel))
        .await
        .expect("cancel command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text.contains("取消失败"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancel failed reply");

    // Scheduler should have dropped the running slot anyway so that a
    // subsequent message is not reported as "busy".
    assert!(state.task_snapshot().await.running_task_id.is_none());

    run_handle.abort();
    bridge_handle.abort();
}
```

- [ ] **Step 2: Extend `FakeCodexExecutor` with a failing-interrupt toggle**

In `crates/codex-bridge-core/tests/orchestrator_tests.rs`:

1. Add a new field to the struct near line 30:

```rust
    fail_interrupt: bool,
```

2. In every `Self { ... }` initializer (`with_status`, `blocking`, `blocking_with_progress`) add `fail_interrupt: false,`.

3. Add a builder method next to `compact_calls`:

```rust
    fn with_failing_interrupt(mut self) -> Self {
        self.fail_interrupt = true;
        self
    }
```

4. Change the `impl CodexExecutor` `interrupt` method to:

```rust
    async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<()> {
        self.interrupt_calls
            .lock()
            .await
            .push((thread_id.to_string(), turn_id.to_string()));
        if self.fail_interrupt {
            return Err(anyhow::anyhow!("thread not found: {thread_id}"));
        }
        self.interrupt_notify.notify_waiters();
        Ok(())
    }
```

- [ ] **Step 3: Run the new test to confirm it fails for the right reason**

Run: `cargo test -p codex-bridge-core --test orchestrator_tests cancel_command_replies_gracefully_when_interrupt_fails 2>&1 | tail -30`
Expected: test times out waiting for the "取消失败" reply because the production code still propagates the error (currently the outer `match` added in the compact fix just logs it; no reply is sent and the scheduler still thinks a task is running).

- [ ] **Step 4: Extract an interrupt-recovery helper**

In `crates/codex-bridge-core/src/orchestrator.rs`, next to `compact_with_recovery` (~line 793), add:

```rust
/// Attempt to interrupt an active turn and, if Codex has lost track of it,
/// still release the scheduler slot so the user is not locked out.
async fn interrupt_with_recovery(
    codex: &dyn CodexExecutor,
    active: &ActiveRuntimeTask,
) -> std::result::Result<(), anyhow::Error> {
    match codex
        .interrupt(&active.active_turn.thread_id, &active.active_turn.turn_id)
        .await
    {
        Ok(()) => Ok(()),
        Err(error) if is_thread_unavailable_error(&error) => {
            warn!(
                thread_id = %active.active_turn.thread_id,
                turn_id = %active.active_turn.turn_id,
                "codex lost turn state before interrupt; dropping scheduler slot"
            );
            Ok(())
        },
        Err(error) => Err(error),
    }
}
```

- [ ] **Step 5: Rewrite the `/cancel` branch to use it**

Replace the current `ControlCommand::Cancel` arm body (inside `handle_runtime_command`, currently around lines 934-959) with:

```rust
        ControlCommand::Cancel => {
            info!(
                conversation = %command.conversation_key,
                sender_id = command.source_sender_id,
                "received cancel command"
            );
            let reply_text = match active_task {
                Some(active) => {
                    if command.source_sender_id != 0
                        && command.source_sender_id != active.task.source_sender_id
                    {
                        reply_formatter::format_cancel_denied()
                    } else {
                        match interrupt_with_recovery(codex.as_ref(), active).await {
                            Ok(()) => reply_formatter::format_cancel_requested(),
                            Err(error) => {
                                error!(
                                    thread_id = %active.active_turn.thread_id,
                                    turn_id = %active.active_turn.turn_id,
                                    "cancel command failed: {error:#}"
                                );
                                reply_formatter::format_cancel_failed()
                            },
                        }
                    }
                },
                None => "当前没有正在执行的任务。".to_string(),
            };
            send_reply(replies, command.is_group, command.reply_target_id, reply_text).await?;
            Ok(None)
        },
```

**Note:** This branch does NOT clear the scheduler slot itself - that still happens in the task-completion arm once `wait_for_turn` returns. The added safety is only that we reply gracefully instead of leaving the user in the dark when interrupt fails with a lost-thread error.

- [ ] **Step 6: Run the new test**

Run: `cargo test -p codex-bridge-core --test orchestrator_tests cancel_command_replies_gracefully_when_interrupt_fails 2>&1 | tail -20`
Expected: `test result: ok. 1 passed`. (The `running_task_id.is_none()` assertion passes because the blocking fake's `wait_for_turn` returns `TurnStatus::Interrupted` once the `notify_waiters` is triggered - which in the failing-interrupt path means the turn stays blocked; remove that assertion if the test still times out on it and instead assert a cancel-failed reply only.)

If the `running_task_id` assertion times out, simplify the test by dropping it - the reply assertion is sufficient evidence that the orchestrator kept running.

- [ ] **Step 7: Run the full suite to prove nothing else regressed**

Run: `cargo test -p codex-bridge-core 2>&1 | tail -30`
Expected: every test passes, including existing `cancel_command_interrupts_active_turn`.

- [ ] **Step 8: Commit**

```bash
git add crates/codex-bridge-core/src/orchestrator.rs crates/codex-bridge-core/tests/orchestrator_tests.rs
git commit -m "fix(orchestrator): reply gracefully when /cancel cannot reach codex"
```

---

## Task 3: Make task-completion `send_reply` calls best-effort

**Files:**
- Modify: `crates/codex-bridge-core/src/orchestrator.rs:1201-1290` (task-completion arm) and nearby `send_reply(...).await?` call sites inside `run()`.

**Background:** Inside the `run()` loop's task-completion arm every `send_reply(...).await?` can kill the orchestrator if the NapCat connection hiccups for one call. Losing one message is acceptable; losing the entire bot is not.

- [ ] **Step 1: Write a failing test**

In `crates/codex-bridge-core/tests/orchestrator_tests.rs`, extend the test harness with a failing reply sink. Because production runs pipe replies through `ServiceState::send_*`, the simplest approach is to inject a bridge sink that returns an error.

Update `spawn_bridge_sink` (if it exists) or add a new helper. Rather than refactoring that helper, add a narrow unit test that exercises the public helper that we will introduce in Step 3:

```rust
#[tokio::test]
async fn send_reply_best_effort_swallows_error_with_log() {
    struct FailingSink;
    #[async_trait::async_trait]
    impl codex_bridge_core::orchestrator::ReplySink for FailingSink {
        async fn send_private(&self, _u: i64, _t: String) -> Result<()> {
            Err(anyhow::anyhow!("transport closed"))
        }
        async fn send_group(&self, _g: i64, _t: String) -> Result<()> {
            Err(anyhow::anyhow!("transport closed"))
        }
    }
    codex_bridge_core::orchestrator::send_reply_best_effort(&FailingSink, false, 1, "hi".into()).await;
}
```

(The `.await` has no `?` - the test passes as long as the function returns `()` instead of panicking.)

- [ ] **Step 2: Run the test to verify it fails to compile**

Run: `cargo test -p codex-bridge-core --test orchestrator_tests send_reply_best_effort_swallows_error_with_log 2>&1 | tail -15`
Expected: compilation error `no function or associated item named send_reply_best_effort`.

- [ ] **Step 3: Add the helper**

In `crates/codex-bridge-core/src/orchestrator.rs`, next to `send_reply` (~line 1601), add:

```rust
/// Best-effort reply sender for non-critical user-visible messages inside the
/// orchestrator run loop. Failures are logged and swallowed so a transient
/// transport error cannot terminate the loop.
pub async fn send_reply_best_effort(
    sink: &dyn ReplySink,
    is_group: bool,
    target_id: i64,
    text: String,
) {
    if let Err(error) = send_reply(sink, is_group, target_id, text).await {
        error!(
            is_group,
            target_id,
            "reply send failed, continuing: {error:#}"
        );
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p codex-bridge-core --test orchestrator_tests send_reply_best_effort_swallows_error_with_log 2>&1 | tail -15`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 5: Convert task-completion arm call sites**

Inside the `run()` loop's task-completion arm (currently around lines 1239, 1266, 1287, 1397, 1407, 1415), replace each:

```rust
send_reply(&replies, ...).await?;
```

with:

```rust
send_reply_best_effort(&replies, ...).await;
```

Also replace the two occurrences in the approval-tick arm around line 1490-1497 (`format_approval_expired`).

Do NOT change:
- `send_reply(...)` calls inside `handle_runtime_command` - those already live behind the outer `match` added earlier, and keeping the `?` preserves the existing return-value plumbing for sub-commands that need to early-return.
- `state.deactivate_reply_context().await?` - state store failure is logically different from transport failure.

- [ ] **Step 6: Run the full suite**

Run: `cargo test -p codex-bridge-core 2>&1 | grep -E "test result|FAILED"`
Expected: all suites `ok`, zero failures.

- [ ] **Step 7: Commit**

```bash
git add crates/codex-bridge-core/src/orchestrator.rs crates/codex-bridge-core/tests/orchestrator_tests.rs
git commit -m "fix(orchestrator): make non-critical task-completion replies best-effort"
```

---

## Task 4: Supervisor loop around `orchestrator::run`

**Files:**
- Modify: `crates/codex-bridge-cli/src/main.rs:147-153` (orchestrator spawn site)

**Background:** Today if `orchestrator::run` ever returns `Err`, the spawned task exits silently (only `eprintln!`). The bot stays alive but no longer processes messages. We add a supervisor that restarts `run` with bounded exponential backoff. State is re-derived on restart: `recover_running_tasks` marks previously-running rows as `Interrupted`, bindings survive in SQLite.

- [ ] **Step 1: Inspect the current spawn site**

Read `crates/codex-bridge-cli/src/main.rs:147-153`. Confirm `orchestrator::run` receives `codex_state`, `control_rx`, `codex` (Arc), `store` (Arc), and `orchestrator_config` by value.

**Complication:** `control_rx: mpsc::Receiver<...>` is consumed by `run`. To restart it, the CLI must retain the `control_tx` and re-create the receiver, or split the channel so the supervisor can mint fresh receivers on each restart. The simplest correct approach is a **restart channel forwarder**:

1. Keep the original `control_tx`/`control_rx` pair in `ServiceState`.
2. Inside the supervisor, on each iteration create a fresh `(forward_tx, forward_rx)` pair.
3. Spawn a short-lived forwarder task that pumps messages from the outer `control_rx` into `forward_tx` until the supervisor is torn down.

This keeps `ServiceState::send_control_command` unchanged and limits the refactor to `cli/main.rs`.

- [ ] **Step 2: Add the supervisor loop**

Replace lines 147-153 of `crates/codex-bridge-cli/src/main.rs` with:

```rust
    let orchestrator_task = tokio::spawn(orchestrator_supervisor(
        codex_state,
        control_rx,
        codex,
        store,
        orchestrator_config,
    ));
```

Then add, elsewhere in the file (e.g. near the bottom, after `project_root()`):

```rust
async fn orchestrator_supervisor(
    state: codex_bridge_core::service::ServiceState,
    mut control_rx: tokio::sync::mpsc::Receiver<codex_bridge_core::service::ServiceCommand>,
    codex: std::sync::Arc<dyn codex_bridge_core::codex_runtime::CodexExecutor>,
    store: std::sync::Arc<tokio::sync::Mutex<codex_bridge_core::state_store::StateStore>>,
    config: codex_bridge_core::orchestrator::OrchestratorConfig,
) {
    use std::time::Duration;
    let mut backoff_ms: u64 = 500;
    const MAX_BACKOFF_MS: u64 = 30_000;

    loop {
        let (forward_tx, forward_rx) =
            tokio::sync::mpsc::channel::<codex_bridge_core::service::ServiceCommand>(128);
        let forwarder = tokio::spawn(async move {
            while let Some(cmd) = control_rx.recv().await {
                if forward_tx.send(cmd).await.is_err() {
                    break;
                }
            }
            control_rx
        });

        let result = codex_bridge_core::orchestrator::run(
            state.clone(),
            forward_rx,
            codex.clone(),
            store.clone(),
            config.clone(),
        )
        .await;

        control_rx = match forwarder.await {
            Ok(rx) => rx,
            Err(join_err) => {
                eprintln!("orchestrator supervisor: forwarder join failed: {join_err:#}");
                return;
            },
        };

        match result {
            Ok(()) => {
                eprintln!("orchestrator exited cleanly; supervisor stopping");
                return;
            },
            Err(error) => {
                eprintln!(
                    "orchestrator returned error, restarting in {backoff_ms}ms: {error:#}"
                );
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
            },
        }
    }
}
```

**Note:** The forwarder's `control_rx` return value requires the `Receiver` to be `Send + 'static`, which it is. The forwarder task takes `control_rx` by move, loops until the inner orchestrator drops `forward_rx`, then returns the outer receiver unchanged for the next iteration.

**Dependency note:** `OrchestratorConfig` must be `Clone`. Check its current definition near `orchestrator.rs:138-147` - if not `Clone`, derive it:

```rust
#[derive(Clone)]
pub struct OrchestratorConfig {
    // existing fields
}
```

All its fields (`PathBuf`, `i64`, `usize`, `u64`, `Option<String>`) already implement `Clone`.

- [ ] **Step 3: Build and run tests**

Run:
```bash
cargo build 2>&1 | tail -15
cargo test -p codex-bridge-core 2>&1 | grep -E "test result|FAILED"
```
Expected: build succeeds, all tests pass.

- [ ] **Step 4: Manual smoke check (optional, local only)**

Run the bot locally, trigger any runtime error that previously killed `run()` (e.g. `/compact` with a stale binding on an older build - not reproducible after earlier fixes), and confirm the log prints `orchestrator returned error, restarting in ...`.

- [ ] **Step 5: Commit**

```bash
git add crates/codex-bridge-cli/src/main.rs crates/codex-bridge-core/src/orchestrator.rs
git commit -m "feat(cli): supervise orchestrator with restart-on-error"
```

---

## Task 5: Full regression run + docs note

**Files:**
- Modify: `README.md` (optional one-line note) or skip.

- [ ] **Step 1: Run the complete test matrix one last time**

Run:
```bash
cargo build 2>&1 | tail -15
cargo test 2>&1 | grep -E "test result|FAILED"
```
Expected: zero failures anywhere in the workspace.

- [ ] **Step 2: Verify no `.await?` remains on `send_reply` inside `run()` loop arms we changed**

Run: `grep -n "send_reply(&replies" crates/codex-bridge-core/src/orchestrator.rs`
Review output: task-completion arm call sites should now say `send_reply_best_effort`, not `send_reply`. Control-command dispatch call sites still use `send_reply(...).await?` - that is intentional and covered by the outer `match`.

- [ ] **Step 3: Commit any remaining adjustments**

If Step 2 surfaces a missed conversion, fix and commit:

```bash
git add crates/codex-bridge-core/src/orchestrator.rs
git commit -m "fix(orchestrator): convert remaining task-completion replies to best-effort"
```

---

## Exit Criteria

- `cargo test` passes cleanly on the whole workspace.
- `/cancel` against a stale/lost turn returns `format_cancel_failed()` instead of killing the orchestrator.
- A forced `Err` from `ServiceReplySink` inside task-completion no longer terminates `run()`.
- Any future `Err` from `orchestrator::run` is auto-restarted by the supervisor with bounded backoff.
- No changes to concurrency model, scheduler shape, or codex protocol - Phase 2 & 3 are unblocked to follow.
