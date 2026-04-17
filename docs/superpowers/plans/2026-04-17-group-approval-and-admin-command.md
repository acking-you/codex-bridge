# Group Approval And Admin Commands Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace group-task approval with admin salute reactions on the original group message, allow admin runtime commands in admin private chat and admin group chat with `@bot`, and add per-conversation `/clear` and `/compact`.

**Architecture:** Extend the normalized event model with one group-reaction notice variant, keep the message router conservative by continuing to require `@bot` for group slash commands, and update the orchestrator to consume both router commands and approval reactions. Add one small persistence helper for deleting conversation bindings and one Codex runtime primitive for `thread/compact/start`, then layer `/clear`, `/compact`, and group-reaction approval on top of the existing pending-approval pool and single execution queue.

**Tech Stack:** Rust (`tokio`, `rusqlite`, `async_trait`), existing NapCat OneBot WebSocket transport, existing Codex app-server RPC wrapper, SQLite runtime state, repository docs in `README.md`.

---

## File Structure

- Modify: `crates/codex-bridge-core/src/events.rs`
  Purpose: normalize NapCat group message reaction notices into one bridge event type.
- Modify: `crates/codex-bridge-core/src/message_router.rs`
  Purpose: add `/clear` and `/compact`, and ignore non-message reaction events.
- Modify: `crates/codex-bridge-core/src/admin_approval.rs`
  Purpose: add one lookup/remove helper for pending group approvals by `(group_id, source_message_id)`.
- Modify: `crates/codex-bridge-core/src/state_store.rs`
  Purpose: add one binding-deletion helper used by `/clear`.
- Modify: `crates/codex-bridge-core/src/codex_runtime.rs`
  Purpose: add a `compact_thread()` primitive backed by `thread/compact/start`.
- Modify: `crates/codex-bridge-core/src/reply_formatter.rs`
  Purpose: add new admin/group approval text, `/clear`/`/compact` text, and updated help/admin-only wording.
- Modify: `crates/codex-bridge-core/src/orchestrator.rs`
  Purpose: open admin commands to admin group chat, implement `/clear` and `/compact`, make `/approve` private-task-only, and approve group pending tasks from salute reactions.
- Modify: `README.md`
  Purpose: document group salute approval, admin group commands, `/clear`, and `/compact`.
- Modify: `crates/codex-bridge-core/tests/events_tests.rs`
  Purpose: cover reaction-notice normalization.
- Modify: `crates/codex-bridge-core/tests/message_router_tests.rs`
  Purpose: cover new commands and reaction-event ignore behavior.
- Modify: `crates/codex-bridge-core/tests/state_store_tests.rs`
  Purpose: cover binding deletion.
- Modify: `crates/codex-bridge-core/tests/codex_runtime_tests.rs`
  Purpose: cover `thread/compact/start` request params.
- Modify: `crates/codex-bridge-core/tests/orchestrator_tests.rs`
  Purpose: cover admin group commands, `/clear`, `/compact`, and group approval by salute reaction.

### Task 1: Normalize Group Reaction Notices And Parse New Commands

**Files:**
- Modify: `crates/codex-bridge-core/src/events.rs`
- Modify: `crates/codex-bridge-core/src/message_router.rs`
- Modify: `crates/codex-bridge-core/tests/events_tests.rs`
- Modify: `crates/codex-bridge-core/tests/message_router_tests.rs`

- [ ] **Step 1: Write the failing event and router tests**

```rust
#[test]
fn group_reaction_notice_extracts_operator_message_and_emoji() {
    let raw = serde_json::json!({
        "post_type": "notice",
        "notice_type": "group_msg_emoji_like",
        "group_id": 777,
        "user_id": 2394626220i64,
        "message_id": 5001,
        "self_id": 2993013575i64,
        "likes": [{ "emoji_id": "282", "count": 1 }],
        "is_add": true
    });

    let event = codex_bridge_core::events::NormalizedEvent::try_from(raw)
        .expect("normalize reaction notice");

    match event {
        codex_bridge_core::events::NormalizedEvent::GroupMessageReactionReceived(reaction) => {
            assert_eq!(reaction.group_id, 777);
            assert_eq!(reaction.operator_id, 2394626220);
            assert_eq!(reaction.message_id, 5001);
            assert_eq!(reaction.emoji_id, "282");
            assert!(reaction.is_add);
        },
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn clear_command_routes_from_private_chat() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "message_id": 1006,
        "user_id": 11,
        "self_id": 99,
        "message": [{ "type": "text", "data": { "text": "/clear" } }],
        "sender": { "nickname": "alice" }
    });
    let event = codex_bridge_core::events::NormalizedEvent::try_from(raw).expect("normalize");
    let mut router = codex_bridge_core::message_router::MessageRouter::new();

    let decision = router.route_event(event).expect("route decision");
    assert!(matches!(
        decision,
        codex_bridge_core::message_router::RouteDecision::Command(
            codex_bridge_core::message_router::CommandRequest {
                command: codex_bridge_core::message_router::ControlCommand::Clear,
                ..
            }
        )
    ));
}

#[test]
fn compact_command_routes_from_private_chat() {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "message_id": 1007,
        "user_id": 11,
        "self_id": 99,
        "message": [{ "type": "text", "data": { "text": "/compact" } }],
        "sender": { "nickname": "alice" }
    });
    let event = codex_bridge_core::events::NormalizedEvent::try_from(raw).expect("normalize");
    let mut router = codex_bridge_core::message_router::MessageRouter::new();

    let decision = router.route_event(event).expect("route decision");
    assert!(matches!(
        decision,
        codex_bridge_core::message_router::RouteDecision::Command(
            codex_bridge_core::message_router::CommandRequest {
                command: codex_bridge_core::message_router::ControlCommand::Compact,
                ..
            }
        )
    ));
}

#[test]
fn router_ignores_group_reaction_events() {
    let raw = serde_json::json!({
        "post_type": "notice",
        "notice_type": "group_msg_emoji_like",
        "group_id": 777,
        "user_id": 2394626220i64,
        "message_id": 5001,
        "self_id": 2993013575i64,
        "likes": [{ "emoji_id": "282", "count": 1 }],
        "is_add": true
    });
    let event = codex_bridge_core::events::NormalizedEvent::try_from(raw).expect("normalize");
    let mut router = codex_bridge_core::message_router::MessageRouter::new();

    assert!(router.route_event(event).is_none());
}
```

- [ ] **Step 2: Run the focused tests to prove the feature is missing**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test events_tests group_reaction_notice_extracts_operator_message_and_emoji -- --exact
cargo test -p codex-bridge-core --test message_router_tests clear_command_routes_from_private_chat -- --exact
cargo test -p codex-bridge-core --test message_router_tests compact_command_routes_from_private_chat -- --exact
cargo test -p codex-bridge-core --test message_router_tests router_ignores_group_reaction_events -- --exact
```

Expected:

- the event test fails because `NormalizedEvent` only supports private/group message events,
- the router command test fails because `ControlCommand::Clear` does not exist,
- the compact-router test fails because `ControlCommand::Compact` does not exist,
- the ignore test fails because the reaction notice cannot be normalized yet.

- [ ] **Step 3: Implement the new reaction event and command parsing**

```rust
// crates/codex-bridge-core/src/events.rs
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupMessageReactionEvent {
    pub group_id: i64,
    pub operator_id: i64,
    pub message_id: i64,
    pub emoji_id: String,
    pub is_add: bool,
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum NormalizedEvent {
    PrivateMessageReceived(PrivateMessageEvent),
    GroupMessageReceived(GroupMessageEvent),
    GroupMessageReactionReceived(GroupMessageReactionEvent),
}

impl TryFrom<Value> for NormalizedEvent {
    type Error = NormalizeEventError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value.get("post_type").and_then(Value::as_str) {
            Some("message") => normalize_message_event(value),
            Some("notice") => normalize_notice_event(value),
            _ => Err(NormalizeEventError::Unsupported),
        }
    }
}

fn normalize_message_event(value: Value) -> Result<NormalizedEvent, NormalizeEventError> {
    let Some(message_type) = value.get("message_type").and_then(Value::as_str) else {
        return Err(NormalizeEventError::Unsupported);
    };
    let sender_id = extract_i64(&value, "user_id")?;
    let self_id = extract_i64(&value, "self_id")?;
    let message_id = extract_i64(&value, "message_id")?;
    let mentions = extract_mentions(&value);
    let mentions_self = mentions.contains(&self_id);
    let text = extract_text(&value);
    let sender_name = extract_sender_name(&value);

    match message_type {
        "private" => Ok(NormalizedEvent::PrivateMessageReceived(PrivateMessageEvent {
            sender_id,
            message_id,
            sender_name,
            self_id,
            text,
            mentions,
            mentions_self,
            raw: value,
        })),
        "group" => Ok(NormalizedEvent::GroupMessageReceived(GroupMessageEvent {
            group_id: extract_i64(&value, "group_id")?,
            sender_id,
            message_id,
            sender_name,
            self_id,
            text,
            mentions,
            mentions_self,
            raw: value,
        })),
        _ => Err(NormalizeEventError::Unsupported),
    }
}

fn normalize_notice_event(value: Value) -> Result<NormalizedEvent, NormalizeEventError> {
    match value.get("notice_type").and_then(Value::as_str) {
        Some("group_msg_emoji_like") => {
            let emoji_id = extract_notice_emoji_id(&value)?;
            Ok(NormalizedEvent::GroupMessageReactionReceived(GroupMessageReactionEvent {
                group_id: extract_i64(&value, "group_id")?,
                operator_id: extract_i64(&value, "user_id")?,
                message_id: extract_i64(&value, "message_id")?,
                emoji_id,
                is_add: value.get("is_add").and_then(Value::as_bool).unwrap_or(true),
                raw: value,
            }))
        },
        Some("reaction") => Ok(NormalizedEvent::GroupMessageReactionReceived(GroupMessageReactionEvent {
            group_id: extract_i64(&value, "group_id")?,
            operator_id: extract_i64(&value, "operator_id")?,
            message_id: extract_i64(&value, "message_id")?,
            emoji_id: value.get("code").and_then(Value::as_str).ok_or(NormalizeEventError::Unsupported)?.to_string(),
            is_add: value.get("sub_type").and_then(Value::as_str) == Some("add"),
            raw: value,
        })),
        _ => Err(NormalizeEventError::Unsupported),
    }
}

fn extract_notice_emoji_id(value: &Value) -> Result<String, NormalizeEventError> {
    value
        .get("likes")
        .and_then(Value::as_array)
        .and_then(|likes| likes.first())
        .and_then(|like| like.get("emoji_id"))
        .and_then(|emoji| {
            emoji
                .as_str()
                .map(ToString::to_string)
                .or_else(|| emoji.as_i64().map(|number| number.to_string()))
        })
        .ok_or(NormalizeEventError::Unsupported)
}

// crates/codex-bridge-core/src/message_router.rs
pub enum ControlCommand {
    Help,
    Status { task_id: Option<String> },
    Queue,
    Cancel,
    RetryLast,
    Approve { task_id: String },
    Deny { task_id: String },
    Clear,
    Compact,
}

pub fn route_event(&mut self, event: NormalizedEvent) -> Option<RouteDecision> {
    match event {
        NormalizedEvent::PrivateMessageReceived(event) => self.route_private_message(event),
        NormalizedEvent::GroupMessageReceived(event) => self.route_group_message(event),
        NormalizedEvent::GroupMessageReactionReceived(_) => None,
    }
}

fn parse_command(text: &str) -> Option<ControlCommand> {
    let mut parts = text.split_whitespace();
    match parts.next() {
        Some("/help") => Some(ControlCommand::Help),
        Some("/status") => Some(ControlCommand::Status {
            task_id: parts.next().map(ToString::to_string),
        }),
        Some("/queue") => Some(ControlCommand::Queue),
        Some("/cancel") => Some(ControlCommand::Cancel),
        Some("/retry_last") => Some(ControlCommand::RetryLast),
        Some("/approve") => parts.next().map(|task_id| ControlCommand::Approve {
            task_id: task_id.to_string(),
        }),
        Some("/deny") => parts.next().map(|task_id| ControlCommand::Deny {
            task_id: task_id.to_string(),
        }),
        Some("/clear") => Some(ControlCommand::Clear),
        Some("/compact") => Some(ControlCommand::Compact),
        _ => None,
    }
}
```

- [ ] **Step 4: Re-run the focused tests and the full event/router suites**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test events_tests group_reaction_notice_extracts_operator_message_and_emoji -- --exact
cargo test -p codex-bridge-core --test message_router_tests clear_command_routes_from_private_chat -- --exact
cargo test -p codex-bridge-core --test message_router_tests compact_command_routes_from_private_chat -- --exact
cargo test -p codex-bridge-core --test message_router_tests router_ignores_group_reaction_events -- --exact
cargo test -p codex-bridge-core --test events_tests -- --nocapture
cargo test -p codex-bridge-core --test message_router_tests -- --nocapture
```

Expected: all tests pass, and current group `@bot` routing behavior continues to work.

- [ ] **Step 5: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add crates/codex-bridge-core/src/events.rs \
        crates/codex-bridge-core/src/message_router.rs \
        crates/codex-bridge-core/tests/events_tests.rs \
        crates/codex-bridge-core/tests/message_router_tests.rs
git commit -m "feat: parse group reaction notices and admin commands"
```

### Task 2: Add Binding Deletion And Codex Thread Compaction Primitives

**Files:**
- Modify: `crates/codex-bridge-core/src/state_store.rs`
- Modify: `crates/codex-bridge-core/src/codex_runtime.rs`
- Modify: `crates/codex-bridge-core/tests/state_store_tests.rs`
- Modify: `crates/codex-bridge-core/tests/codex_runtime_tests.rs`
- Modify: `crates/codex-bridge-core/tests/orchestrator_tests.rs`

- [ ] **Step 1: Write the failing persistence/runtime tests**

```rust
#[test]
fn delete_binding_removes_only_requested_conversation() {
    let store = StateStore::open_in_memory().expect("open in-memory store");
    let first = ConversationBinding {
        conversation_key: "group:777".to_string(),
        thread_id: "thr-777".to_string(),
    };
    let second = ConversationBinding {
        conversation_key: "group:888".to_string(),
        thread_id: "thr-888".to_string(),
    };

    store.upsert_binding(&first).expect("upsert first");
    store.upsert_binding(&second).expect("upsert second");

    assert!(store.delete_binding("group:777").expect("delete binding"));
    assert!(store.binding("group:777").expect("query deleted").is_none());
    assert_eq!(store.binding("group:888").expect("query survivor"), Some(second));
}

#[test]
fn thread_compact_start_params_target_the_bound_thread() {
    let params = codex_bridge_core::codex_runtime::build_thread_compact_start_params("thread-compact-1");
    assert_eq!(params.thread_id, "thread-compact-1");
}
```

- [ ] **Step 2: Run the focused tests to confirm the helpers are missing**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test state_store_tests delete_binding_removes_only_requested_conversation -- --exact
cargo test -p codex-bridge-core --test codex_runtime_tests thread_compact_start_params_target_the_bound_thread -- --exact
```

Expected:

- the state-store test fails because no delete-binding API exists,
- the runtime test fails because no compact-start helper exists.

- [ ] **Step 3: Implement the store delete helper and runtime compaction primitive**

```rust
// crates/codex-bridge-core/src/state_store.rs
pub fn delete_binding(&self, conversation_key: &str) -> Result<bool> {
    let deleted = self
        .conn
        .execute(
            "DELETE FROM conversation_bindings WHERE conversation_key = ?1",
            params![conversation_key],
        )
        .context("delete conversation binding")?;
    Ok(deleted == 1)
}

// crates/codex-bridge-core/src/codex_runtime.rs
use codex_app_server_protocol::{ThreadCompactStartParams, ThreadCompactStartResponse};

// add this method to the `CodexExecutor` trait
async fn compact_thread(&self, thread_id: &str) -> Result<()>;

pub fn build_thread_compact_start_params(thread_id: &str) -> ThreadCompactStartParams {
    ThreadCompactStartParams {
        thread_id: thread_id.to_string(),
    }
}

#[async_trait]
impl CodexExecutor for CodexRuntime {
    async fn compact_thread(&self, thread_id: &str) -> Result<()> {
        let request_id = self.next_request_id().await;
        let request = ClientRequest::ThreadCompactStart {
            request_id: request_id.clone(),
            params: build_thread_compact_start_params(thread_id),
        };
        let _: ThreadCompactStartResponse = self
            .send_request(request, request_id, "thread/compact/start")
            .await?;
        Ok(())
    }
}

// crates/codex-bridge-core/tests/orchestrator_tests.rs
#[derive(Debug)]
struct FakeCodexExecutor {
    thread_ids: AsyncMutex<VecDeque<String>>,
    ensure_thread_calls: AsyncMutex<Vec<(String, Option<String>)>>,
    interrupt_calls: AsyncMutex<Vec<(String, String)>>,
    compact_calls: AsyncMutex<Vec<String>>,
    interrupt_notify: Notify,
    turn_id: String,
    reply_text: String,
    progress_updates: Vec<String>,
    status: TurnStatus,
    wait_for_interrupt: bool,
}

impl FakeCodexExecutor {
    fn with_status(
        thread_ids: Vec<&str>,
        turn_id: &str,
        status: TurnStatus,
        reply_text: &str,
    ) -> Self {
        Self {
            thread_ids: AsyncMutex::new(
                thread_ids
                    .into_iter()
                    .map(|thread_id| thread_id.to_string())
                    .collect(),
            ),
            ensure_thread_calls: AsyncMutex::new(Vec::new()),
            interrupt_calls: AsyncMutex::new(Vec::new()),
            compact_calls: AsyncMutex::new(Vec::new()),
            interrupt_notify: Notify::new(),
            turn_id: turn_id.to_string(),
            reply_text: reply_text.to_string(),
            progress_updates: Vec::new(),
            status,
            wait_for_interrupt: false,
        }
    }

    fn blocking(thread_ids: Vec<&str>, turn_id: &str) -> Self {
        Self {
            thread_ids: AsyncMutex::new(
                thread_ids
                    .into_iter()
                    .map(|thread_id| thread_id.to_string())
                    .collect(),
            ),
            ensure_thread_calls: AsyncMutex::new(Vec::new()),
            interrupt_calls: AsyncMutex::new(Vec::new()),
            compact_calls: AsyncMutex::new(Vec::new()),
            interrupt_notify: Notify::new(),
            turn_id: turn_id.to_string(),
            reply_text: String::new(),
            progress_updates: Vec::new(),
            status: TurnStatus::Interrupted,
            wait_for_interrupt: true,
        }
    }

    fn blocking_with_progress(thread_ids: Vec<&str>, turn_id: &str, progress_updates: Vec<&str>) -> Self {
        Self {
            thread_ids: AsyncMutex::new(
                thread_ids
                    .into_iter()
                    .map(|thread_id| thread_id.to_string())
                    .collect(),
            ),
            ensure_thread_calls: AsyncMutex::new(Vec::new()),
            interrupt_calls: AsyncMutex::new(Vec::new()),
            compact_calls: AsyncMutex::new(Vec::new()),
            interrupt_notify: Notify::new(),
            turn_id: turn_id.to_string(),
            reply_text: String::new(),
            progress_updates: progress_updates.into_iter().map(str::to_string).collect(),
            status: TurnStatus::Interrupted,
            wait_for_interrupt: true,
        }
    }

    async fn compact_calls(&self) -> Vec<String> {
        self.compact_calls.lock().await.clone()
    }
}

// in the current `impl CodexExecutor for FakeCodexExecutor`, add:
async fn compact_thread(&self, thread_id: &str) -> Result<()> {
    self.compact_calls.lock().await.push(thread_id.to_string());
    Ok(())
}
```

- [ ] **Step 4: Re-run the focused tests and one orchestrator compile/behavior test**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test state_store_tests delete_binding_removes_only_requested_conversation -- --exact
cargo test -p codex-bridge-core --test codex_runtime_tests thread_compact_start_params_target_the_bound_thread -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests admin_group_message_bypasses_approval -- --exact
```

Expected: all three commands pass, proving the new trait method compiles through the orchestrator test fake.

- [ ] **Step 5: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add crates/codex-bridge-core/src/state_store.rs \
        crates/codex-bridge-core/src/codex_runtime.rs \
        crates/codex-bridge-core/tests/state_store_tests.rs \
        crates/codex-bridge-core/tests/codex_runtime_tests.rs \
        crates/codex-bridge-core/tests/orchestrator_tests.rs
git commit -m "feat: add binding reset and thread compaction primitives"
```

### Task 3: Open Admin Commands In Groups And Implement `/clear` `/compact`

**Files:**
- Modify: `crates/codex-bridge-core/src/reply_formatter.rs`
- Modify: `crates/codex-bridge-core/src/orchestrator.rs`
- Modify: `crates/codex-bridge-core/tests/orchestrator_tests.rs`

- [ ] **Step 1: Write the failing orchestrator tests for admin group commands**

```rust
fn make_group_command_request(sender_id: i64, group_id: i64, command: ControlCommand) -> CommandRequest {
    CommandRequest {
        command,
        conversation_key: format!("group:{group_id}"),
        reply_target_id: group_id,
        is_group: true,
        source_message_id: 9100,
        source_sender_id: sender_id,
        source_sender_name: if sender_id == 2_394_626_220 { "admin".to_string() } else { "LB".to_string() },
    }
}

#[tokio::test]
async fn admin_group_status_command_is_allowed() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-1"], "turn-1", "ok"));
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
        codex,
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state
        .send_control_command(make_group_command_request(
            2_394_626_220,
            777,
            ControlCommand::Status { task_id: None },
        ))
        .await
        .expect("status command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages.lock().expect("messages").iter().any(|text| text.contains("当前任务")) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("admin group status reply");

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn non_admin_group_status_command_is_rejected() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-unused"], "turn-1", "ok"));
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

    state
        .send_control_command(make_group_command_request(
            42,
            777,
            ControlCommand::Status { task_id: None },
        ))
        .await
        .expect("status command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text == "这个命令只开放给管理员。")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("non-admin group status denied");
    assert!(codex.ensure_thread_calls().await.is_empty());

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn compact_command_without_binding_reports_missing_context() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-unused"], "turn-1", "ok"));
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

    state
        .send_control_command(make_group_command_request(
            2_394_626_220,
            777,
            ControlCommand::Compact,
        ))
        .await
        .expect("compact command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages.lock().expect("messages").iter().any(|text| text.contains("没有可压缩的上下文")) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("compact missing-context reply");
    assert!(codex.compact_calls().await.is_empty());

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn clear_command_removes_current_conversation_binding_and_next_turn_uses_new_thread() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-reset"], "turn-reset", "ok"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    store
        .lock()
        .await
        .upsert_binding(&ConversationBinding {
            conversation_key: "group:777".to_string(),
            thread_id: "thread-old".to_string(),
        })
        .expect("seed binding");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store.clone(),
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state
        .send_control_command(make_group_command_request(
            2_394_626_220,
            777,
            ControlCommand::Clear,
        ))
        .await
        .expect("clear command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text.contains("上下文已清空"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("clear reply");

    assert!(
        store
            .lock()
            .await
            .binding("group:777")
            .expect("query cleared binding")
            .is_none()
    );

    state.publish_event(make_group_event_from(
        2_394_626_220,
        "admin",
        777,
        9101,
        "清空后重新开始",
    ));

    timeout(Duration::from_secs(1), async {
        loop {
            if !codex.ensure_thread_calls().await.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("next task should start");

    let calls = codex.ensure_thread_calls().await;
    assert_eq!(calls[0], ("group:777".to_string(), None));

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn compact_command_starts_thread_compaction_when_idle() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-unused"], "turn-1", "ok"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    store
        .lock()
        .await
        .upsert_binding(&ConversationBinding {
            conversation_key: "group:777".to_string(),
            thread_id: "thread-compact".to_string(),
        })
        .expect("seed binding");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state
        .send_control_command(make_group_command_request(
            2_394_626_220,
            777,
            ControlCommand::Compact,
        ))
        .await
        .expect("compact command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text.contains("已发起当前会话的上下文压缩"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("compact started reply");
    assert_eq!(codex.compact_calls().await, vec!["thread-compact".to_string()]);

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn compact_command_for_running_conversation_reports_busy() {
    let codex = Arc::new(FakeCodexExecutor::blocking(vec!["thread-busy"], "turn-busy"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    store
        .lock()
        .await
        .upsert_binding(&ConversationBinding {
            conversation_key: "group:777".to_string(),
            thread_id: "thread-busy".to_string(),
        })
        .expect("seed binding");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state.publish_event(make_group_event_from(
        2_394_626_220,
        "admin",
        777,
        9201,
        "执行中的群任务",
    ));

    timeout(Duration::from_secs(1), async {
        loop {
            if state
                .task_snapshot()
                .await
                .running_conversation_key
                .as_deref()
                == Some("group:777")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("group task running");

    state
        .send_control_command(make_group_command_request(
            2_394_626_220,
            777,
            ControlCommand::Compact,
        ))
        .await
        .expect("compact command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text.contains("当前会话正在执行任务"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("compact busy reply");
    assert!(codex.compact_calls().await.is_empty());

    state
        .send_control_command(make_group_command_request(
            2_394_626_220,
            777,
            ControlCommand::Cancel,
        ))
        .await
        .expect("cancel running task");

    run_handle.abort();
    bridge_handle.abort();
}
```

- [ ] **Step 2: Run the focused tests to confirm the permission and command logic are missing**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test orchestrator_tests admin_group_status_command_is_allowed -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests non_admin_group_status_command_is_rejected -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests compact_command_without_binding_reports_missing_context -- --exact
```

Expected:

- the status test fails because `handle_runtime_command()` currently requires admin private chat,
- the non-admin status test fails because group commands are still treated like ordinary `@bot` tasks,
- the compact test fails because `ControlCommand::Compact` is parsed but not handled.

- [ ] **Step 3: Implement group-capable admin permissions and `/clear` `/compact`**

```rust
// crates/codex-bridge-core/src/reply_formatter.rs
pub fn format_help() -> String {
    "触发方式：私聊默认触发，但非好友不会接入；群里需要先 @我。\
管理员可在私聊或群里 @我 使用管理命令。群聊非管理员任务需要管理员对原消息点敬礼表情批准。\
命令：/help /status /queue /cancel /retry_last /clear /compact /approve <task_id> /deny <task_id>"
        .to_string()
}

pub fn format_admin_only_command() -> String {
    "这个命令只开放给管理员。".to_string()
}

pub fn format_clear_success() -> String {
    "当前会话上下文已清空；下次会从新线程开始。".to_string()
}

pub fn format_clear_missing() -> String {
    "当前会话没有可清空的上下文。".to_string()
}

pub fn format_compact_started() -> String {
    "已发起当前会话的上下文压缩。".to_string()
}

pub fn format_compact_missing() -> String {
    "当前会话还没有可压缩的上下文。".to_string()
}

pub fn format_compact_busy() -> String {
    "当前会话正在执行任务；先等它结束，或先 /cancel。".to_string()
}

// crates/codex-bridge-core/src/orchestrator.rs
let is_admin = command.source_sender_id == config.admin_user_id;
```

Replace the old admin gate:

```rust
let is_admin_private = !command.is_group && command.source_sender_id == config.admin_user_id;
```

with:

```rust
let is_admin = command.source_sender_id == config.admin_user_id;
```

Replace the old rejection arm:

```rust
_ if !is_admin => {
    send_reply(
        replies,
        command.is_group,
        command.reply_target_id,
        reply_formatter::format_admin_only_command(),
    )
    .await?;
    Ok(None)
}
```

Insert these two new arms into `handle_runtime_command()`:

```rust
ControlCommand::Clear => {
    let deleted = {
        let store = state_store.lock().await;
        store.delete_binding(&command.conversation_key)?
    };
    let text = if deleted {
        reply_formatter::format_clear_success()
    } else {
        reply_formatter::format_clear_missing()
    };
    send_reply(replies, command.is_group, command.reply_target_id, text).await?;
    Ok(None)
},
ControlCommand::Compact => {
    let binding = {
        let store = state_store.lock().await;
        store.binding(&command.conversation_key)?
    };
    let Some(binding) = binding else {
        send_reply(
            replies,
            command.is_group,
            command.reply_target_id,
            reply_formatter::format_compact_missing(),
        )
        .await?;
        return Ok(None);
    };
    if active_task
        .map(|task| task.task.conversation_key == command.conversation_key)
        .unwrap_or(false)
    {
        send_reply(
            replies,
            command.is_group,
            command.reply_target_id,
            reply_formatter::format_compact_busy(),
        )
        .await?;
        return Ok(None);
    }
    codex.compact_thread(&binding.thread_id).await?;
    send_reply(
        replies,
        command.is_group,
        command.reply_target_id,
        reply_formatter::format_compact_started(),
    )
    .await?;
    Ok(None)
},
```

- [ ] **Step 4: Extend the orchestrator tests for `/clear` and successful `/compact`**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test orchestrator_tests admin_group_status_command_is_allowed -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests non_admin_group_status_command_is_rejected -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests compact_command_without_binding_reports_missing_context -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests clear_command_removes_current_conversation_binding_and_next_turn_uses_new_thread -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests compact_command_starts_thread_compaction_when_idle -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests compact_command_for_running_conversation_reports_busy -- --exact
```

Expected:

- admin group status passes,
- non-admin group status is rejected with the admin-only reply,
- compact-missing-context passes,
- clear removes the seeded binding and the next task resolves with `existing_thread_id == None`,
- compact on an idle bound conversation records one `compact_thread()` call,
- compact on an active conversation returns the busy reply and does not call `compact_thread()`.

- [ ] **Step 5: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add crates/codex-bridge-core/src/reply_formatter.rs \
        crates/codex-bridge-core/src/orchestrator.rs \
        crates/codex-bridge-core/tests/orchestrator_tests.rs
git commit -m "feat: add admin group commands and context management"
```

### Task 4: Replace Group Approval Commands With Salute-Reaction Approval

**Files:**
- Modify: `crates/codex-bridge-core/src/admin_approval.rs`
- Modify: `crates/codex-bridge-core/src/reply_formatter.rs`
- Modify: `crates/codex-bridge-core/src/orchestrator.rs`
- Modify: `crates/codex-bridge-core/tests/orchestrator_tests.rs`

- [ ] **Step 1: Write the failing group-approval orchestrator tests**

```rust
fn make_group_reaction_event(
    operator_id: i64,
    group_id: i64,
    message_id: i64,
    emoji_id: &str,
    is_add: bool,
) -> NormalizedEvent {
    serde_json::json!({
        "post_type": "notice",
        "notice_type": "group_msg_emoji_like",
        "group_id": group_id,
        "user_id": operator_id,
        "message_id": message_id,
        "self_id": 2993013575i64,
        "likes": [{ "emoji_id": emoji_id, "count": 1 }],
        "is_add": is_add
    })
    .try_into()
    .expect("normalize reaction event")
}

#[tokio::test]
async fn non_admin_group_message_is_approved_by_admin_salute_reaction() {
    let codex = Arc::new(FakeCodexExecutor::with_status(
        vec!["thread-reaction"],
        "turn-reaction",
        TurnStatus::Completed,
        "",
    ));
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
        store.clone(),
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state.publish_event(make_group_event_from(42, "LB", 777, 5001, "帮我跑一下"));

    let pending = timeout(Duration::from_secs(1), async {
        loop {
            if let Some(task) = store.lock().await.latest_task_for_conversation("group:777").expect("query latest") {
                break task;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pending task appears");
    assert_eq!(pending.status, TaskStatus::PendingApproval);

    state.publish_event(make_group_reaction_event(2_394_626_220, 777, 5001, "282", true));

    timeout(Duration::from_secs(1), async {
        loop {
            let messages = sent_messages.lock().expect("messages").clone();
            if messages.iter().any(|text| text == "REACTION:5001:282")
                && !codex.ensure_thread_calls().await.is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("reaction approval starts task");

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn approve_command_rejects_group_pending_task() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-unused"], "turn-unused", "ok"));
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
        store.clone(),
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state.publish_event(make_group_event_from(42, "LB", 777, 5101, "帮我跑一下"));
    let pending = timeout(Duration::from_secs(1), async {
        loop {
            if let Some(task) = store.lock().await.latest_task_for_conversation("group:777").expect("query latest") {
                break task;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pending task appears");

    state
        .send_control_command(make_command_request_from(
            2_394_626_220,
            ControlCommand::Approve { task_id: pending.task_id.clone() },
        ))
        .await
        .expect("approve command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages.lock().expect("messages").iter().any(|text| text.contains("请对原群消息点敬礼表情")) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approve correction reply");
    assert!(codex.ensure_thread_calls().await.is_empty());

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn non_admin_or_wrong_emoji_reaction_does_not_approve_group_task() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-unused"], "turn-unused", "ok"));
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
        store.clone(),
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state.publish_event(make_group_event_from(42, "LB", 777, 5201, "还在等审批"));
    timeout(Duration::from_secs(1), async {
        loop {
            if let Some(task) = store
                .lock()
                .await
                .latest_task_for_conversation("group:777")
                .expect("query latest")
            {
                if task.status == TaskStatus::PendingApproval {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pending task appears");

    state.publish_event(make_group_reaction_event(42, 777, 5201, "282", true));
    state.publish_event(make_group_reaction_event(2_394_626_220, 777, 5201, "13", true));

    tokio::time::sleep(Duration::from_millis(100)).await;

    let task = store
        .lock()
        .await
        .latest_task_for_conversation("group:777")
        .expect("query latest task")
        .expect("pending task exists");
    assert_eq!(task.status, TaskStatus::PendingApproval);
    assert!(codex.ensure_thread_calls().await.is_empty());
    assert!(
        sent_messages
            .lock()
            .expect("messages")
            .iter()
            .all(|text| text != "REACTION:5201:282")
    );

    run_handle.abort();
    bridge_handle.abort();
}
```

- [ ] **Step 2: Run the focused tests to confirm group approval still uses `/approve`**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test orchestrator_tests non_admin_group_message_is_approved_by_admin_salute_reaction -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests approve_command_rejects_group_pending_task -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests non_admin_or_wrong_emoji_reaction_does_not_approve_group_task -- --exact
```

Expected:

- the salute-reaction test fails because reaction notices are ignored by the orchestrator,
- the approve-correction test fails because `/approve` still works for every pending task id,
- the invalid-reaction test fails because there is no reaction-based approval filter yet.

- [ ] **Step 3: Implement pending-approval lookup by source message and reaction approval**

```rust
// crates/codex-bridge-core/src/admin_approval.rs
impl PendingApprovalPool {
    pub fn take_group_by_source_message(
        &mut self,
        group_id: i64,
        source_message_id: i64,
    ) -> Option<PendingApproval> {
        let task_id = self
            .by_task_id
            .iter()
            .find_map(|(task_id, pending)| {
                (pending.task.is_group
                    && pending.task.reply_target_id == group_id
                    && pending.task.source_message_id == source_message_id)
                    .then(|| task_id.clone())
            })?;
        self.take(&task_id)
    }
}

// crates/codex-bridge-core/src/reply_formatter.rs
pub fn format_waiting_for_admin_group_approval() -> String {
    "这件事要先等管理员点头……请他对原消息点个敬礼表情。".to_string()
}

pub fn format_admin_group_approval_notice(pending: &PendingApproval) -> String {
    format!(
        "群待审批任务：{}\n群号：{}\n发起人：{} ({})\n消息：{}\n批准方式：请对原群消息点敬礼表情。\n可选管理：/status {} /deny {}",
        pending.task_id,
        pending.task.reply_target_id,
        pending.task.source_sender_name,
        pending.task.source_sender_id,
        pending.task.source_text,
        pending.task_id,
        pending.task_id,
    )
}

pub fn format_group_approval_use_reaction() -> String {
    "群聊待审批任务不能用 /approve；请对原群消息点敬礼表情。".to_string()
}

// crates/codex-bridge-core/src/orchestrator.rs
async fn approve_pending_task(
    pending: PendingApproval,
    replies: &dyn ReplySink,
    scheduler: &mut Scheduler,
    pending_tasks: &mut VecDeque<ScheduledRuntimeTask>,
    state_store: &Arc<Mutex<StateStore>>,
    active_task: Option<&ActiveRuntimeTask>,
) -> Result<Option<ScheduledRuntimeTask>> {
    let scheduled = ScheduledRuntimeTask::persisted(pending.task_id.clone(), pending.task.clone());
    if active_task.is_some() {
        {
            let store = state_store.lock().await;
            store.update_task_status(&pending.task_id, TaskStatus::Queued)?;
        }
        enqueue_runtime_task(scheduled, replies, scheduler, pending_tasks).await?;
        Ok(None)
    } else {
        Ok(Some(scheduled))
    }
}

async fn handle_group_reaction_approval(
    reaction: crate::events::GroupMessageReactionEvent,
    replies: &dyn ReplySink,
    scheduler: &mut Scheduler,
    pending_tasks: &mut VecDeque<ScheduledRuntimeTask>,
    pending_approvals: &mut PendingApprovalPool,
    active_task: Option<&ActiveRuntimeTask>,
    state_store: &Arc<Mutex<StateStore>>,
    config: &OrchestratorConfig,
) -> Result<Option<ScheduledRuntimeTask>> {
    if !reaction.is_add
        || reaction.operator_id != config.admin_user_id
        || reaction.emoji_id != config.group_start_reaction_emoji_id
    {
        return Ok(None);
    }
    let Some(pending) = pending_approvals.take_group_by_source_message(
        reaction.group_id,
        reaction.message_id,
    ) else {
        return Ok(None);
    };
    approve_pending_task(
        pending,
        replies,
        scheduler,
        pending_tasks,
        state_store,
        active_task,
    )
    .await
}

async fn register_pending_approval(
    task: TaskRequest,
    replies: &dyn ReplySink,
    pending_approvals: &mut PendingApprovalPool,
    state_store: &Arc<Mutex<StateStore>>,
    approval_timeout_secs: u64,
    admin_user_id: i64,
) -> Result<()> {
    let task_id = Uuid::new_v4().to_string();
    let pending = PendingApproval::new(
        task_id.clone(),
        task.clone(),
        Instant::now(),
        Duration::from_secs(approval_timeout_secs),
    );
    match pending_approvals.insert(pending.clone()) {
        Ok(()) => {},
        Err(PendingApprovalError::ConversationAlreadyWaiting) => {
            send_reply(
                replies,
                task.is_group,
                task.reply_target_id,
                reply_formatter::format_waiting_for_admin_approval_duplicate(),
            )
            .await?;
            return Ok(());
        },
        Err(PendingApprovalError::PoolFull) => {
            send_reply(
                replies,
                task.is_group,
                task.reply_target_id,
                reply_formatter::format_queue_full(),
            )
            .await?;
            return Ok(());
        },
    }

    {
        let store = state_store.lock().await;
        if let Err(error) = store.insert_task_pending_approval_with_id(
            &task_id,
            &task.conversation_key,
            task.source_sender_id,
            task.source_message_id,
        ) {
            let _ = pending_approvals.take(&task_id);
            return Err(error);
        }
    }

    if task.is_group {
        send_reply(
            replies,
            true,
            task.reply_target_id,
            reply_formatter::format_waiting_for_admin_group_approval(),
        )
        .await?;
        replies
            .send_private(
                admin_user_id,
                reply_formatter::format_admin_group_approval_notice(&pending),
            )
            .await?;
        return Ok(());
    }

    send_reply(
        replies,
        task.is_group,
        task.reply_target_id,
        reply_formatter::format_waiting_for_admin_approval(),
    )
    .await?;
    replies
        .send_private(admin_user_id, reply_formatter::format_admin_approval_notice(&pending))
        .await?;
    replies
        .send_private(admin_user_id, reply_formatter::format_admin_approve_command(&pending.task_id))
        .await?;
    replies
        .send_private(admin_user_id, reply_formatter::format_admin_deny_command(&pending.task_id))
        .await?;
    replies
        .send_private(admin_user_id, reply_formatter::format_admin_status_command(&pending.task_id))
        .await
}

// inside handle_runtime_command()
ControlCommand::Approve { task_id } => {
    let Some(pending) = pending_approvals.get(&task_id).cloned() else {
        send_reply(replies, command.is_group, command.reply_target_id, reply_formatter::format_admin_task_not_found(&task_id)).await?;
        return Ok(None);
    };
    if pending.task.is_group {
        send_reply(replies, command.is_group, command.reply_target_id, reply_formatter::format_group_approval_use_reaction()).await?;
        return Ok(None);
    }
    let pending = pending_approvals.take(&task_id).expect("pending task still present");
    if let Some(task) = approve_pending_task(
        pending,
        replies,
        scheduler,
        pending_tasks,
        state_store,
        active_task,
    )
    .await? {
        send_reply(replies, command.is_group, command.reply_target_id, reply_formatter::format_admin_approved(&task_id)).await?;
        return Ok(Some(task));
    }
    send_reply(replies, command.is_group, command.reply_target_id, reply_formatter::format_admin_approved(&task_id)).await?;
    Ok(None)
}

// inside the `Ok(event)` arm in `orchestrator::run()`, before `router.route_event(event)`
if let NormalizedEvent::GroupMessageReactionReceived(reaction) = event.clone() {
    if let Some(task) = handle_group_reaction_approval(
        reaction,
        &replies,
        &mut scheduler,
        &mut pending_tasks,
        &mut pending_approvals,
        active_task.as_ref(),
        &state_store,
        &config,
    )
    .await? {
        active_task = Some(
            start_runtime_task(
                task,
                &mut scheduler,
                RuntimeTaskDeps {
                    replies: &replies,
                    state: &state,
                    codex: codex.clone(),
                    state_store: state_store.clone(),
                    config: &config,
                },
                false,
            )
            .await?,
        );
    }
    continue;
}

// the router-based command/task flow stays immediately after this block
if let Some(decision) = router.route_event(event) {
    match decision {
        RouteDecision::Command(command_request) => {
            if let Some(task) = handle_runtime_command(
                command_request,
                RuntimeCommandDeps {
                    state: &state,
                    replies: &replies,
                    scheduler: &mut scheduler,
                    pending_tasks: &mut pending_tasks,
                    retryable_tasks: &mut retryable_tasks,
                    pending_approvals: &mut pending_approvals,
                    active_task: active_task.as_ref(),
                    codex: &codex,
                    state_store: &state_store,
                    config: &config,
                },
            )
            .await? {
                active_task = Some(
                    start_runtime_task(
                        task,
                        &mut scheduler,
                        RuntimeTaskDeps {
                            replies: &replies,
                            state: &state,
                            codex: codex.clone(),
                            state_store: state_store.clone(),
                            config: &config,
                        },
                        false,
                    )
                    .await?,
                );
            }
        },
        RouteDecision::Task(task) => {
            if !task.is_group
                && !is_admin_task(&task, config.admin_user_id)
                && !private_sender_is_friend(&state, &task).await
            {
                send_reply(
                    &replies,
                    false,
                    task.reply_target_id,
                    reply_formatter::format_friend_gate(),
                )
                .await?;
            } else if !is_admin_task(&task, config.admin_user_id) {
                register_pending_approval(
                    task,
                    &replies,
                    &mut pending_approvals,
                    &state_store,
                    config.approval_timeout_secs,
                    config.admin_user_id,
                )
                .await?;
            } else if active_task.is_some() {
                enqueue_runtime_task(
                    ScheduledRuntimeTask::fresh(task),
                    &replies,
                    &mut scheduler,
                    &mut pending_tasks,
                )
                .await?;
            } else {
                active_task = Some(
                    start_runtime_task(
                        ScheduledRuntimeTask::fresh(task),
                        &mut scheduler,
                        RuntimeTaskDeps {
                            replies: &replies,
                            state: &state,
                            codex: codex.clone(),
                            state_store: state_store.clone(),
                            config: &config,
                        },
                        false,
                    )
                    .await?,
                );
            }
        }
    }
}
```

- [ ] **Step 4: Re-run the focused tests and the approval regression suite**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test orchestrator_tests non_admin_group_message_is_approved_by_admin_salute_reaction -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests approve_command_rejects_group_pending_task -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests non_admin_or_wrong_emoji_reaction_does_not_approve_group_task -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests non_admin_friend_private_message_waits_for_admin_approval -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests admin_group_message_bypasses_approval -- --exact
cargo test -p codex-bridge-core --test orchestrator_tests -- --nocapture
```

Expected:

- group pending tasks start only after the admin salute reaction,
- `/approve` no longer starts a group pending task,
- non-admin reactions and wrong emojis leave the task in `PendingApproval`,
- private pending approval still works through `/approve`,
- admin-authored group tasks still bypass approval.

- [ ] **Step 5: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add crates/codex-bridge-core/src/admin_approval.rs \
        crates/codex-bridge-core/src/reply_formatter.rs \
        crates/codex-bridge-core/src/orchestrator.rs \
        crates/codex-bridge-core/tests/orchestrator_tests.rs
git commit -m "feat: approve group tasks by admin salute reaction"
```

### Task 5: Update README And Run Final Regression

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the user-facing command and approval docs**

```md
## 运行时命令

- `/help`
- `/status`
- `/status <task_id>`
- `/queue`
- `/cancel`
- `/retry_last`
- `/clear`
- `/compact`
- `/approve <task_id>`
- `/deny <task_id>`

权限规则：

- `/help` 公开可用
- 管理命令支持 admin 私聊，或 admin 在群里 `@bot` 后触发
- 群聊非管理员任务不会再通过 admin 私聊 `/approve` 批准；需要管理员对原群消息点敬礼表情

审批流：

1. 非管理员群聊 `@bot` 请求进入 `pending approval`
2. bot 在群里回复“等待管理员点头”
3. 管理员对原群消息点敬礼表情
4. bridge 将该任务转入正常执行或排队
```

- [ ] **Step 2: Run the full regression suite for the touched behavior**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
cargo test -p codex-bridge-core --test events_tests -- --nocapture
cargo test -p codex-bridge-core --test message_router_tests -- --nocapture
cargo test -p codex-bridge-core --test state_store_tests -- --nocapture
cargo test -p codex-bridge-core --test codex_runtime_tests -- --nocapture
cargo test -p codex-bridge-core --test orchestrator_tests -- --nocapture
```

Expected:

- every touched test suite passes,
- no existing approval, queue, interrupt, or runtime tests regress.

- [ ] **Step 3: Commit**

```bash
cd /home/ts_user/llm_pro/codex-bridge
git add README.md
git commit -m "docs: document group approval and admin commands"
```

## Self-Review

- Spec coverage:
  - group salute approval is implemented in Task 4,
  - admin group commands and `/clear` `/compact` are implemented in Task 3,
  - state binding reset and real thread compaction support are implemented in Task 2,
  - README/help updates are covered in Tasks 3 and 5.
- Placeholder scan:
  - no `TODO`, `TBD`, “implement later”, or omitted command placeholders remain in the task steps,
  - every code-changing step contains an explicit code block,
  - every verification step contains an exact command and expected outcome.
- Type consistency:
  - `GroupMessageReactionReceived`, `ControlCommand::Clear`, `ControlCommand::Compact`, `delete_binding`, and `compact_thread` are named consistently across tasks,
  - `/approve` remains source-type-sensitive rather than being renamed mid-plan.
