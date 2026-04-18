# Codex Bridge Runtime Pool And QQ History Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the bridge's shared-runtime / singleton-reply architecture with lane-scoped scheduling, a fixed-size runtime pool, strict reply isolation, and lane-scoped QQ history lookup.

**Architecture:** Introduce explicit `LaneState`, `RuntimeSlotState`, and a dispatcher that schedules lane keys onto a bounded runtime pool. Delete singleton reply-context behavior, add a dedicated current-conversation history API backed by NapCat history actions, and rewrite status / prompt / skill plumbing around the new lane model. Treat this as a breaking refactor: no compatibility shims, no dual-path reply routing, no single-running-task snapshot retained for legacy callers.

**Tech Stack:** Rust, `tokio`, `axum`, `serde`, `anyhow`, existing NapCat websocket bridge, Codex app-server protocol, Python skill scripts, Markdown skill docs.

---

## File Structure

- Create: `crates/codex-bridge-core/src/lane_manager.rs`
  Purpose: define `LaneState`, lane queue operations, and lane runtime snapshot helpers.
- Create: `crates/codex-bridge-core/src/runtime_pool.rs`
  Purpose: define `RuntimePool`, `RuntimeSlot`, slot health / replacement, and shared-Codex-home configuration.
- Create: `crates/codex-bridge-core/src/conversation_history.rs`
  Purpose: encapsulate current-lane QQ history queries and transcript normalization.
- Modify: `crates/codex-bridge-core/src/lib.rs`
  Purpose: export the new modules.
- Modify: `crates/codex-bridge-core/src/orchestrator.rs`
  Purpose: replace `active_tasks + pending_tasks + Scheduler` control flow with lane-aware dispatch on top of `LaneManager` and `RuntimePool`.
- Modify: `crates/codex-bridge-core/src/service.rs`
  Purpose: replace `TaskSnapshot` with runtime snapshot models and add lane-scoped reply/history helpers.
- Modify: `crates/codex-bridge-core/src/api.rs`
  Purpose: replace single-task status endpoints and add `/api/history/query`.
- Modify: `crates/codex-bridge-core/src/reply_context.rs`
  Purpose: delete singleton mirror behavior and keep only per-lane context persistence / token resolution.
- Modify: `crates/codex-bridge-core/src/runtime.rs`
  Purpose: remove `reply_context_file`, add pool runtime directories, and preserve shared `codex_home`.
- Modify: `crates/codex-bridge-core/src/config.rs`
  Purpose: add runtime-pool and history-budget config fields.
- Modify: `crates/codex-bridge-core/src/napcat.rs`
  Purpose: add current-conversation history actions on top of NapCat `get_group_msg_history` / `get_friend_msg_history`.
- Modify: `crates/codex-bridge-core/src/system_prompt.rs`
- Modify: `crates/codex-bridge-core/assets/bridge_protocol.md`
  Purpose: add the new context-first gate and history guidance.
- Modify: `crates/codex-bridge-cli/src/main.rs`
  Purpose: wire new config, remove singleton reply fallback, and update CLI status/history behavior.
- Modify: `skills/reply-current/reply_current.py`
  Purpose: require `--context-file`.
- Create: `skills/qq-current-history/SKILL.md`
- Create: `skills/qq-current-history/query_current_history.py`
  Purpose: document and expose lane-scoped current-conversation history lookup.
- Modify tests:
  - `crates/codex-bridge-core/tests/reply_context_tests.rs`
  - `crates/codex-bridge-core/tests/api_tests.rs`
  - `crates/codex-bridge-core/tests/orchestrator_tests.rs`
  - `crates/codex-bridge-core/tests/config_tests.rs`
  - `crates/codex-bridge-core/tests/codex_runtime_tests.rs`
  - `crates/codex-bridge-core/tests/napcat_transport_tests.rs`

---

## Task 1: Delete Singleton Reply Context Semantics

**Files:**
- Modify: `crates/codex-bridge-core/src/reply_context.rs`
- Modify: `crates/codex-bridge-core/tests/reply_context_tests.rs`
- Modify: `skills/reply-current/reply_current.py`
- Modify: `crates/codex-bridge-cli/src/main.rs`

- [ ] **Step 1: Write the failing tests for no-singleton behavior**

Add or replace tests in `crates/codex-bridge-core/tests/reply_context_tests.rs` with cases shaped like:

```rust
#[test]
fn activate_only_writes_per_conversation_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let contexts_dir = tmp.path().join("contexts");
    let mut registry = ReplyRegistry::new(contexts_dir.clone());
    let context = sample_context("tok-a", "group:123");

    registry.activate(context.clone()).unwrap();

    let lane_file = reply_context_file_for(&contexts_dir, "group:123");
    assert!(lane_file.is_file());
    assert!(!tmp.path().join("reply_context.json").exists());
}

#[test]
fn deactivate_removes_only_lane_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let contexts_dir = tmp.path().join("contexts");
    let mut registry = ReplyRegistry::new(contexts_dir.clone());
    let context = sample_context("tok-a", "group:123");

    registry.activate(context.clone()).unwrap();
    registry.deactivate("tok-a").unwrap();

    let lane_file = reply_context_file_for(&contexts_dir, "group:123");
    assert!(!lane_file.exists());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p codex-bridge-core --test reply_context_tests 2>&1 | tail -40`
Expected: failure because `ReplyRegistry::new` still takes singleton + directory paths and activation still writes the singleton mirror.

- [ ] **Step 3: Rewrite `ReplyRegistry` to be lane-only**

In `crates/codex-bridge-core/src/reply_context.rs`, collapse the struct to:

```rust
pub struct ReplyRegistry {
    contexts_dir: PathBuf,
    active: HashMap<String, ActiveReplyState>,
}
```

Delete:

```rust
context_file: PathBuf,
mirrored_token: Option<String>,
fn persist_singleton(&self) -> Result<()> { ... }
pub fn current(&self) -> Option<ActiveReplyContext> { ... }
```

And make `activate` / `deactivate` only manage `reply_context_file_for(...)`.

- [ ] **Step 4: Make `reply-current` require `--context-file`**

In `skills/reply-current/reply_current.py`, change the argument and loader contract to:

```python
parser.add_argument(
    "--context-file",
    type=Path,
    required=True,
    help="Absolute path to THIS thread's reply-context JSON file.",
)
```

And replace the fallback loader with:

```python
def load_reply_context(root: Path, context_file: Path) -> dict[str, object]:
    context_path = context_file if context_file.is_absolute() else root / context_file
    try:
        return json.loads(context_path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise SystemExit(f"reply context not found: {context_path}") from exc
```

- [ ] **Step 5: Remove CLI singleton reply fallback**

In `crates/codex-bridge-cli/src/main.rs`, replace the current helper with a hard failure until callers provide a lane context explicitly:

```rust
async fn reply_command(...) -> Result<()> {
    anyhow::bail!(
        "codex-bridge reply now requires an explicit per-lane context file; use the reply-current skill"
    );
}
```

- [ ] **Step 6: Run tests again**

Run:

```bash
cargo test -p codex-bridge-core --test reply_context_tests
python skills/reply-current/reply_current.py --help | rg -- '--context-file'
```

Expected:
- Rust reply-context tests pass.
- Help output shows `--context-file` as required.

- [ ] **Step 7: Commit**

```bash
git add crates/codex-bridge-core/src/reply_context.rs \
        crates/codex-bridge-core/tests/reply_context_tests.rs \
        skills/reply-current/reply_current.py \
        crates/codex-bridge-cli/src/main.rs
git commit -m "refactor(reply): remove singleton reply context paths"
```

---

## Task 2: Introduce Lane And Runtime Snapshot Models

**Files:**
- Create: `crates/codex-bridge-core/src/lane_manager.rs`
- Modify: `crates/codex-bridge-core/src/service.rs`
- Modify: `crates/codex-bridge-core/src/api.rs`
- Modify: `crates/codex-bridge-core/tests/api_tests.rs`
- Modify: `crates/codex-bridge-core/src/lib.rs`

- [ ] **Step 1: Write failing API tests for multi-lane status**

Add a test to `crates/codex-bridge-core/tests/api_tests.rs` shaped like:

```rust
#[tokio::test]
async fn status_endpoint_returns_lane_and_slot_snapshots() {
    let state = test_service_state();
    state
        .set_runtime_snapshot(RuntimeSnapshot {
            lanes: vec![
                LaneSnapshot {
                    conversation_key: "group:1".into(),
                    thread_id: Some("thread-a".into()),
                    state: LaneRuntimeState::Running,
                    pending_turn_count: 2,
                    active_task_id: Some("task-a".into()),
                    active_since: Some("2026-04-18T11:00:00Z".into()),
                    last_progress_at: Some("2026-04-18T11:01:00Z".into()),
                    last_terminal_summary: None,
                },
            ],
            runtime_slots: vec![
                RuntimeSlotSnapshot {
                    slot_id: 0,
                    state: RuntimeSlotState::Busy,
                    assigned_conversation_key: Some("group:1".into()),
                },
            ],
            ready_lane_count: 1,
            total_pending_turn_count: 2,
        })
        .await;

    let app = build_router(state);
    let response = app.oneshot(Request::get("/api/status").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["snapshot"]["lanes"][0]["conversation_key"], "group:1");
    assert_eq!(json["snapshot"]["runtime_slots"][0]["state"], "busy");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p codex-bridge-core --test api_tests status_endpoint_returns_lane_and_slot_snapshots 2>&1 | tail -40`
Expected: compile failure because `RuntimeSnapshot`, `LaneSnapshot`, and `RuntimeSlotSnapshot` do not exist yet.

- [ ] **Step 3: Create lane and runtime snapshot types**

Add `crates/codex-bridge-core/src/lane_manager.rs` with a first minimal model:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneRuntimeState {
    Idle,
    Queued,
    Running,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneSnapshot {
    pub conversation_key: String,
    pub thread_id: Option<String>,
    pub state: LaneRuntimeState,
    pub pending_turn_count: usize,
    pub active_task_id: Option<String>,
    pub active_since: Option<String>,
    pub last_progress_at: Option<String>,
    pub last_terminal_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSlotState {
    Idle,
    Busy,
    Broken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSlotSnapshot {
    pub slot_id: usize,
    pub state: RuntimeSlotState,
    pub assigned_conversation_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuntimeSnapshot {
    pub lanes: Vec<LaneSnapshot>,
    pub runtime_slots: Vec<RuntimeSlotSnapshot>,
    pub ready_lane_count: usize,
    pub total_pending_turn_count: usize,
}
```

Export it from `crates/codex-bridge-core/src/lib.rs`.

- [ ] **Step 4: Replace `TaskSnapshot` in `service.rs`**

Replace the `task_snapshot: RwLock<TaskSnapshot>` field and related methods with:

```rust
runtime_snapshot: RwLock<crate::lane_manager::RuntimeSnapshot>,
```

And methods:

```rust
pub async fn set_runtime_snapshot(&self, snapshot: RuntimeSnapshot) {
    *self.inner.runtime_snapshot.write().await = snapshot;
}

pub async fn runtime_snapshot(&self) -> RuntimeSnapshot {
    self.inner.runtime_snapshot.read().await.clone()
}
```

- [ ] **Step 5: Rewrite `/api/status` around the new snapshot**

In `crates/codex-bridge-core/src/api.rs`, replace the text-only status payload with:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct StatusResponse {
    status: &'static str,
    snapshot: crate::lane_manager::RuntimeSnapshot,
}

async fn status_handler(
    State(state): State<ServiceState>,
) -> Result<Json<StatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    Ok(Json(StatusResponse {
        status: "ok",
        snapshot: state.runtime_snapshot().await,
    }))
}
```

- [ ] **Step 6: Run the tests again**

Run:

```bash
cargo test -p codex-bridge-core --test api_tests status_endpoint_returns_lane_and_slot_snapshots
```

Expected: test passes and old single-running-task assertions are removed or updated.

- [ ] **Step 7: Commit**

```bash
git add crates/codex-bridge-core/src/lane_manager.rs \
        crates/codex-bridge-core/src/service.rs \
        crates/codex-bridge-core/src/api.rs \
        crates/codex-bridge-core/src/lib.rs \
        crates/codex-bridge-core/tests/api_tests.rs
git commit -m "refactor(service): replace task snapshot with lane runtime snapshot"
```

---

## Task 3: Add Lane-Scoped QQ History Query

**Files:**
- Create: `crates/codex-bridge-core/src/conversation_history.rs`
- Modify: `crates/codex-bridge-core/src/napcat.rs`
- Modify: `crates/codex-bridge-core/src/service.rs`
- Modify: `crates/codex-bridge-core/src/api.rs`
- Modify: `crates/codex-bridge-core/tests/api_tests.rs`
- Modify: `crates/codex-bridge-core/tests/napcat_transport_tests.rs`

- [ ] **Step 1: Write a failing API test for `/api/history/query`**

Add a test shaped like:

```rust
#[tokio::test]
async fn history_query_uses_lane_scoped_token() {
    let state = test_service_state();
    state
        .activate_reply_context(sample_active_reply_context("tok-a", "group:123"))
        .await
        .unwrap();
    state
        .set_history_stub(vec![HistoryMessage {
            message_id: 11,
            timestamp: "2026-04-18T10:00:00Z".into(),
            sender_id: 42,
            sender_name: "alice".into(),
            text: "部署今天下午做".into(),
        }])
        .await;

    let app = build_router(state);
    let payload = json!({
        "token": "tok-a",
        "query": "找今天下午部署那句",
    });
    let response = app
        .oneshot(
            Request::post("/api/history/query")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p codex-bridge-core --test api_tests history_query_uses_lane_scoped_token 2>&1 | tail -40`
Expected: missing route / missing history service APIs.

- [ ] **Step 3: Add normalized history types**

Create `crates/codex-bridge-core/src/conversation_history.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub message_id: i64,
    pub timestamp: String,
    pub sender_id: i64,
    pub sender_name: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HistoryQueryResult {
    pub messages: Vec<HistoryMessage>,
    pub truncated: bool,
}
```

- [ ] **Step 4: Add NapCat history actions**

In `crates/codex-bridge-core/src/napcat.rs`, add:

```rust
async fn get_group_msg_history(&self, group_id: i64, count: usize) -> Result<Vec<HistoryMessage>> { ... }
async fn get_friend_msg_history(&self, user_id: i64, count: usize) -> Result<Vec<HistoryMessage>> { ... }
```

Each should call:

```rust
self.call_action("get_group_msg_history", json!({ "group_id": ..., "count": count }))
self.call_action("get_friend_msg_history", json!({ "user_id": ..., "count": count }))
```

And normalize the returned `messages` array into `HistoryMessage`.

- [ ] **Step 5: Add service and API plumbing**

In `crates/codex-bridge-core/src/service.rs`, add a lane-scoped helper:

```rust
pub async fn query_current_conversation_history(
    &self,
    token: &str,
    limit: usize,
) -> Result<crate::conversation_history::HistoryQueryResult> { ... }
```

It should:

1. resolve the reply token,
2. infer current lane type from `conversation_key`,
3. call the matching NapCat history method,
4. return normalized history entries.

In `crates/codex-bridge-core/src/api.rs`, add:

```rust
.route("/api/history/query", post(history_query_handler))
```

And a minimal request/response pair.

- [ ] **Step 6: Run tests again**

Run:

```bash
cargo test -p codex-bridge-core --test api_tests history_query_uses_lane_scoped_token
cargo test -p codex-bridge-core --test napcat_transport_tests
```

Expected: the new API test passes and transport tests are updated for the new OneBot actions.

- [ ] **Step 7: Commit**

```bash
git add crates/codex-bridge-core/src/conversation_history.rs \
        crates/codex-bridge-core/src/napcat.rs \
        crates/codex-bridge-core/src/service.rs \
        crates/codex-bridge-core/src/api.rs \
        crates/codex-bridge-core/tests/api_tests.rs \
        crates/codex-bridge-core/tests/napcat_transport_tests.rs
git commit -m "feat(history): add lane-scoped QQ conversation history query"
```

---

## Task 4: Introduce Runtime Pool Primitives

**Files:**
- Create: `crates/codex-bridge-core/src/runtime_pool.rs`
- Modify: `crates/codex-bridge-core/src/config.rs`
- Modify: `crates/codex-bridge-core/src/runtime.rs`
- Modify: `crates/codex-bridge-core/src/lib.rs`
- Modify: `crates/codex-bridge-core/tests/config_tests.rs`

- [ ] **Step 1: Write failing config tests**

Add tests shaped like:

```rust
#[test]
fn runtime_config_defaults_include_runtime_pool_fields() {
    let config = RuntimeConfig::default();
    assert_eq!(config.runtime_pool_size, 2);
    assert_eq!(config.history_page_size, 50);
    assert_eq!(config.history_max_pages, 4);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p codex-bridge-core --test config_tests runtime_config_defaults_include_runtime_pool_fields 2>&1 | tail -40`
Expected: compile failure because the fields do not exist.

- [ ] **Step 3: Add runtime-pool config fields**

In `crates/codex-bridge-core/src/config.rs`:

```rust
pub runtime_pool_size: usize,
pub lane_pending_capacity: usize,
pub history_page_size: usize,
pub history_max_pages: usize,
pub max_turn_wall_time_secs: u64,
pub stalled_turn_timeout_secs: u64,
pub slot_restart_backoff_ms: u64,
```

With defaults:

```rust
runtime_pool_size: 2,
lane_pending_capacity: 5,
history_page_size: 50,
history_max_pages: 4,
max_turn_wall_time_secs: 900,
stalled_turn_timeout_secs: 120,
slot_restart_backoff_ms: 500,
```

- [ ] **Step 4: Create `runtime_pool.rs` skeleton**

Add a first minimal model:

```rust
use crate::lane_manager::RuntimeSlotState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSlot {
    pub slot_id: usize,
    pub state: RuntimeSlotState,
    pub assigned_conversation_key: Option<String>,
}

#[derive(Debug)]
pub struct RuntimePool {
    slots: Vec<RuntimeSlot>,
}

impl RuntimePool {
    pub fn new(size: usize) -> Self {
        Self {
            slots: (0..size)
                .map(|slot_id| RuntimeSlot {
                    slot_id,
                    state: RuntimeSlotState::Idle,
                    assigned_conversation_key: None,
                })
                .collect(),
        }
    }
}
```

- [ ] **Step 5: Extend runtime paths for pool slots**

In `crates/codex-bridge-core/src/runtime.rs`, replace the single-runtime assumption with a slot-root helper:

```rust
pub fn runtime_slot_dir(runtime_root: &Path, slot_id: usize) -> PathBuf {
    runtime_root.join("slots").join(format!("slot-{slot_id}"))
}
```

Keep one shared `codex_home_dir`, but stop treating a single `run_dir` child as the only runtime execution root.

- [ ] **Step 6: Run tests again**

Run:

```bash
cargo test -p codex-bridge-core --test config_tests runtime_config_defaults_include_runtime_pool_fields
```

Expected: config tests pass; new pool skeleton compiles.

- [ ] **Step 7: Commit**

```bash
git add crates/codex-bridge-core/src/runtime_pool.rs \
        crates/codex-bridge-core/src/config.rs \
        crates/codex-bridge-core/src/runtime.rs \
        crates/codex-bridge-core/src/lib.rs \
        crates/codex-bridge-core/tests/config_tests.rs
git commit -m "feat(runtime): add runtime pool primitives and config"
```

---

## Task 5: Replace Orchestrator Scheduling With Lane-Based Dispatch

**Files:**
- Modify: `crates/codex-bridge-core/src/orchestrator.rs`
- Modify: `crates/codex-bridge-core/tests/orchestrator_tests.rs`
- Modify: `crates/codex-bridge-core/src/service.rs`

- [ ] **Step 1: Write failing scheduler-behavior tests**

Add regression tests shaped like:

```rust
#[tokio::test]
async fn different_conversations_can_run_when_two_slots_exist() {
    // Arrange two tasks on two conversation keys, runtime_pool_size = 2.
    // Assert both lanes reach running state concurrently.
}

#[tokio::test]
async fn same_conversation_never_runs_two_turns_at_once() {
    // Arrange two tasks on one conversation key.
    // Assert the second stays queued until the first finishes.
}
```

Use the existing fake executor pattern, but change the assertions to inspect
lane snapshots rather than `running_task_id`.

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
cargo test -p codex-bridge-core --test orchestrator_tests different_conversations_can_run_when_two_slots_exist
cargo test -p codex-bridge-core --test orchestrator_tests same_conversation_never_runs_two_turns_at_once
```

Expected: failure because the current orchestrator still models a single
queue/status flow and has no runtime-pool scheduler.

- [ ] **Step 3: Introduce lane-owned pending queues**

In `crates/codex-bridge-core/src/orchestrator.rs`, replace:

```rust
let mut pending_tasks: HashMap<String, VecDeque<ScheduledRuntimeTask>> = HashMap::new();
let mut active_tasks: HashMap<String, ActiveRuntimeTask> = HashMap::new();
```

with a lane registry shaped like:

```rust
let mut lanes: HashMap<String, LaneState> = HashMap::new();
let mut ready_lanes: VecDeque<String> = VecDeque::new();
let mut runtime_pool = RuntimePool::new(config.runtime_pool_size);
```

- [ ] **Step 4: Add lane dispatch helpers**

Extract small helpers:

```rust
fn enqueue_lane_if_needed(...)
fn start_next_ready_lane(...)
fn finish_running_lane(...)
fn handle_broken_slot(...)
```

Each helper should mutate lane state and refresh the shared runtime snapshot.

- [ ] **Step 5: Remove `Scheduler` single-running-task assumptions**

Delete or bypass any code path that still computes:

```rust
running_task_id
running_conversation_key
recent_output
```

and replace it with `refresh_runtime_snapshot(...)` built from lanes + slots.

- [ ] **Step 6: Run the orchestrator tests again**

Run:

```bash
cargo test -p codex-bridge-core --test orchestrator_tests
```

Expected: multi-lane concurrency tests pass and all old single-running-task expectations are updated or removed.

- [ ] **Step 7: Commit**

```bash
git add crates/codex-bridge-core/src/orchestrator.rs \
        crates/codex-bridge-core/src/service.rs \
        crates/codex-bridge-core/tests/orchestrator_tests.rs
git commit -m "refactor(orchestrator): schedule lanes on runtime pool"
```

---

## Task 6: Rewrite Prompt, Skill, And Capability Routing Rules

**Files:**
- Modify: `crates/codex-bridge-core/assets/bridge_protocol.md`
- Modify: `crates/codex-bridge-core/src/system_prompt.rs`
- Create: `skills/qq-current-history/SKILL.md`
- Create: `skills/qq-current-history/query_current_history.py`
- Modify: `crates/codex-bridge-core/tests/codex_runtime_tests.rs`

- [ ] **Step 1: Write failing prompt-assembly tests**

Add or extend tests in `crates/codex-bridge-core/tests/codex_runtime_tests.rs`:

```rust
#[test]
fn bridge_protocol_contains_context_first_gate() {
    assert!(BRIDGE_PROTOCOL_TEXT.contains("Gate 0"));
    assert!(BRIDGE_PROTOCOL_TEXT.contains("Context first"));
    assert!(BRIDGE_PROTOCOL_TEXT.contains("current-conversation history"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p codex-bridge-core --test codex_runtime_tests bridge_protocol_contains_context_first_gate 2>&1 | tail -40`
Expected: assertion failure because the current protocol starts at Gate 1.

- [ ] **Step 3: Rewrite the bridge protocol**

In `crates/codex-bridge-core/assets/bridge_protocol.md`, add a new leading section:

```md
**Gate 0 — Context first**

If the user asks about earlier QQ messages, quoted content, or a time-bounded
slice of the current conversation, query current-conversation history first.
Only after assembling that local context may you choose whether a model
capability is needed.
```

Also rewrite the heavy-load section so it forbids unbounded scans but explicitly
allows bounded current-conversation history lookup.

- [ ] **Step 4: Add the new project skill**

Create `skills/qq-current-history/SKILL.md` with guidance like:

```md
# qq-current-history

Use this when the user asks what someone said earlier in the current QQ chat,
asks for a time-bounded chat slice, or wants to jump a reply pill to an earlier
message.

- Only query the current conversation.
- Support time, sender, keyword, and quoted-context intents.
- Never fabricate missing history.
- Narrow the query when the scan budget is exhausted.
```

And `skills/qq-current-history/query_current_history.py` as a thin POST wrapper
for `/api/history/query`.

- [ ] **Step 5: Run tests again**

Run:

```bash
cargo test -p codex-bridge-core --test codex_runtime_tests
python skills/qq-current-history/query_current_history.py --help
```

Expected: protocol tests pass and the new skill wrapper script is callable.

- [ ] **Step 6: Commit**

```bash
git add crates/codex-bridge-core/assets/bridge_protocol.md \
        crates/codex-bridge-core/src/system_prompt.rs \
        skills/qq-current-history/SKILL.md \
        skills/qq-current-history/query_current_history.py \
        crates/codex-bridge-core/tests/codex_runtime_tests.rs
git commit -m "feat(prompt): add context-first QQ history guidance"
```

---

## Task 7: Remove Obsolete Paths And Verify End-To-End

**Files:**
- Modify or delete obsolete code in:
  - `crates/codex-bridge-core/src/scheduler.rs`
  - `crates/codex-bridge-core/src/service.rs`
  - `crates/codex-bridge-core/src/api.rs`
  - `crates/codex-bridge-core/tests/scheduler_tests.rs`
  - any helper still preserving `TaskSnapshot` or singleton reply behavior.

- [ ] **Step 1: Delete dead single-slot helpers and tests**

Remove:

```text
running_task_id
running_conversation_key
reply_context_file
singleton mirror helpers
```

Delete or rewrite `scheduler_tests.rs` if the scheduler abstraction is no
longer used after the lane dispatcher lands.

- [ ] **Step 2: Run the full focused test suite**

Run:

```bash
cargo test -p codex-bridge-core --test reply_context_tests
cargo test -p codex-bridge-core --test api_tests
cargo test -p codex-bridge-core --test orchestrator_tests
cargo test -p codex-bridge-core --test codex_runtime_tests
cargo test -p codex-bridge-core --test config_tests
cargo test -p codex-bridge-core --test napcat_transport_tests
```

Expected: all pass.

- [ ] **Step 3: Run one crate-wide verification pass**

Run:

```bash
cargo test -p codex-bridge-core
cargo test -p codex-bridge-cli
```

Expected: all pass; no compile references remain to deleted singleton or
single-running-task paths.

- [ ] **Step 4: Commit**

```bash
git add crates/codex-bridge-core \
        crates/codex-bridge-cli \
        skills/qq-current-history \
        skills/reply-current \
        docs/superpowers/specs/2026-04-18-qq-runtime-pool-history-design.md \
        docs/superpowers/plans/2026-04-18-qq-runtime-pool-history-refactor.md
git commit -m "refactor: adopt lane runtime pool and QQ history isolation"
```

---

## Self-Review

### Spec Coverage

- Lane isolation / runtime pool: Task 4 + Task 5
- Reply isolation: Task 1
- QQ history capability: Task 3 + Task 6
- Context-first prompt + skill: Task 6
- Status / API rewrite: Task 2 + Task 3
- Deletion of obsolete paths: Task 7

### Placeholder Scan

This plan intentionally avoids `TODO` / `TBD` placeholders. Each task names
files, commands, and first-pass code shapes.

### Type Consistency

Core planned names are:

- `RuntimeSnapshot`
- `LaneSnapshot`
- `RuntimeSlotSnapshot`
- `LaneRuntimeState`
- `RuntimeSlotState`
- `RuntimePool`
- `query_current_conversation_history`

Use these exact names throughout implementation unless a failing test exposes a
better boundary.

## Execution Handoff

Plan complete and saved to
`docs/superpowers/plans/2026-04-18-qq-runtime-pool-history-refactor.md`.

User instruction already selected inline execution in this session:

- do not use subagents,
- do not use git worktrees.

So execution should proceed inline from Task 1 with TDD.
