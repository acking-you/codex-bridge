# Codex Bridge Runtime Pool And QQ History Design

## Goal

Replace the bridge's half-singleton concurrency model with a correct global
architecture that:

- isolates every QQ conversation as its own lane and Codex thread,
- prevents cross-group reply leakage ("串台"),
- avoids one stuck conversation blocking unrelated conversations,
- adds bridge-owned current-conversation QQ history query capability,
- makes local context exploration happen before optional model-capability
  delegation,
- deletes obsolete singleton and single-running-slot paths instead of keeping
  compatibility shims.

## Non-Goals

- Preserve existing local API or CLI compatibility.
- Build a cross-conversation archival search system.
- Introduce a second persistence database for QQ history.
- Migrate a live in-progress turn from one app-server process to another.
- Keep the legacy singleton reply-context file or any fallback that depends on
  it.
- Keep the current `TaskSnapshot` single-running-task mental model.

## Current Problems

The current bridge has four structural problems.

### Shared Runtime Failure Domain

Today the bridge drives one shared Codex app-server process. That process can
serve multiple threads in principle, but its failure domain is global: one
deadlocked process can stall every conversation.

### Half-Converted Orchestrator State

The orchestrator already stores active work in a
`HashMap<conversation_key, ActiveRuntimeTask>`, but queue promotion, status
reporting, and several control flows still carry single-running-task
assumptions. This mismatch is the main reason the bridge behaves unpredictably
 under concurrent traffic.

### Reply Context Singleton Leakage

The bridge added per-conversation reply-context files, but still preserves a
singleton mirror file and a fallback path in `reply-current`. Under concurrent
tasks, "last activation wins", so a skill reply can land in the wrong QQ chat.

### Missing Current-Conversation History Capability

The bridge can resolve a single quoted message through OneBot `get_msg`, but it
cannot yet perform controlled, lane-scoped history lookup over the current QQ
conversation even though NapCat exposes `get_group_msg_history` and
`get_friend_msg_history`.

## Selected Approach

Three runtime-isolation approaches were considered:

1. Keep one app-server process and harden the existing demux.
2. Create one app-server per active conversation.
3. Create a fixed-size runtime pool and lease one runtime slot per active
   conversation turn.

Approach 3 is selected.

It preserves the useful upstream `thread/resume` semantics, shrinks the failure
domain from "all conversations" to "one slot", keeps resource usage bounded,
and maps cleanly onto the desired policy:

- each conversation is logically isolated,
- each conversation runs only one turn at a time,
- unrelated conversations may still run concurrently,
- a wedged conversation only occupies one pool slot.

## Design Principles

### Conversation Identity Belongs To The Lane

The only stable identity for a QQ chat is `conversation_key`. A lane owns the
conversation-scoped state:

- `conversation_key`,
- `thread_id`,
- pending tasks,
- current active task metadata,
- reply context path,
- last terminal state,
- current history-query cursor state.

No runtime slot owns a conversation.

### Thread History Is Shared, Execution State Is Not

Every runtime slot must see the same persistent Codex thread store so that the
same `thread_id` can be resumed across slots. However, an in-progress turn is
runtime-local and must never be "migrated". If a slot dies mid-turn, that turn
is interrupted or failed, and a later retry starts a fresh turn on the same
`thread_id`.

### Global Scheduling Operates On Lanes, Not Raw Tasks

The ready queue must contain at most one entry per lane. This preserves strict
per-conversation serial execution while still letting unrelated conversations
run concurrently.

### Reply Routing Must Be Explicit

Every reply must be authorized by a lane-scoped reply token loaded from that
lane's explicit context file. The bridge must never infer the destination from
"current activity" or "most recently activated task".

### History Query Must Be Lane-Scoped And Budgeted

QQ history lookup must be limited to the current lane. It must use bounded page
budgets and bounded output formatting so the history capability cannot become a
new infinite scan path.

### Context Comes Before Delegation

When the user asks about quoted messages, earlier lines, or time-bounded chat
history, the bridge protocol must require current-conversation history
exploration first. Optional capability delegation happens only after the local
context has been assembled.

## Core Runtime Model

### Lane

Each QQ conversation has one lane keyed by `conversation_key`.

Minimum lane state:

- `conversation_key: String`
- `thread_id: Option<String>`
- `state: Idle | Queued | Running | Blocked`
- `pending_turns: VecDeque<ScheduledTurn>`
- `active_turn: Option<ActiveLaneTurn>`
- `reply_context_file: PathBuf`
- `history_cursor_state: Option<HistoryCursorState>`
- `last_terminal_state: Option<LaneTerminalState>`

### Runtime Slot

Each pool slot wraps one app-server process.

Minimum slot state:

- `slot_id: usize`
- `state: Idle | Busy | Broken`
- `assigned_conversation_key: Option<String>`
- process handle / join handle
- health metadata
- slot-specific runtime directory metadata

### Runtime Pool

The pool owns:

- slot lifecycle,
- slot health checks,
- slot replacement on failure,
- slot lease / release,
- shared Codex-home configuration.

The pool does not own conversation identity or queue order.

### Dispatcher

The dispatcher owns:

- all lanes,
- the ready-lane FIFO,
- task admission into lanes,
- lane-to-slot assignment,
- turn completion handling,
- stale-turn recovery,
- queue fairness.

## Scheduling State Machine

### Lane States

Only four lane states are needed:

- `Idle`
- `Queued`
- `Running`
- `Blocked`

`Blocked` is reserved for lane-local failures that should stop automatic retry
until an operator or a later explicit command unblocks the lane.

### Slot States

Only three slot states are needed:

- `Idle`
- `Busy`
- `Broken`

A broken slot is removed from scheduling immediately and replaced
asynchronously.

### Task Admission

When a new QQ message becomes a runnable task:

- resolve `conversation_key`,
- load or create the lane,
- append the task to that lane's `pending_turns`,
- if the lane was `Idle`, move it to `Queued` and enqueue the lane key in the
  global ready queue,
- if the lane was already `Queued` or `Running`, do not duplicate it in the
  ready queue.

### Dispatch

While both of the following are true:

- at least one lane is in the ready queue,
- at least one runtime slot is idle,

the dispatcher:

1. pops the oldest ready lane,
2. leases one idle slot,
3. pops one scheduled turn from that lane,
4. starts or resumes the lane thread on that slot,
5. marks the lane `Running`,
6. marks the slot `Busy`.

### Turn Completion

When a running turn finishes:

- release the slot,
- clear the lane's active turn,
- revoke the lane reply context,
- persist the terminal state,
- if the lane still has queued turns, move it back to `Queued` and append it to
  the ready FIFO once,
- otherwise move it to `Idle`.

### Slot Failure

When a slot dies or becomes unreadable:

- mark the slot `Broken`,
- mark the attached lane's active turn `Interrupted` or `Failed`,
- keep the lane's `thread_id`,
- if the lane still has work to do, move it back to `Queued`,
- create a replacement slot with a fresh process.

The bridge does not attempt to continue the lost turn mid-stream.

### Stale Turn Recovery

Each running lane turn records:

- `started_at`,
- `last_progress_at`,
- `slot_id`,
- `thread_id`,
- `turn_id`.

If either hard limit is exceeded:

- `max_turn_wall_time_secs`,
- `stalled_turn_timeout_secs`,

the dispatcher interrupts the turn and converts it into a terminal interrupted
state. Depending on policy, the lane is re-queued or marked `Blocked`.

### Fairness

Fairness is lane-based FIFO, not message-based FIFO. A noisy group must not be
able to starve quieter groups by flooding the bridge with more messages while
already queued or running.

## Reply Isolation Design

### Correct Source Of Truth

The only valid reply context is the lane-scoped file at:

`reply_contexts_dir/<conversation_key>.json`

The file contains:

- reply token,
- `conversation_key`,
- `thread_id`,
- QQ reply target,
- source message id,
- source sender metadata,
- repo / artifacts roots.

### Deleted Behavior

The following behavior is removed entirely:

- singleton `reply_context.json`,
- singleton mirror writes,
- any `reply-current` fallback that reads the singleton,
- CLI reply helper behavior that auto-loads the singleton path.

### `reply-current` Contract

`reply-current` must require `--context-file`.

If `--context-file` is omitted, the command fails immediately with a clear
error. There is no "best effort" fallback.

### Reply API Contract

`/api/reply` resolves the token from the supplied context and uses it to load
the exact active lane reply metadata. The API never infers the active
conversation from global bridge state.

## QQ History Query Design

### Upstream Capability

NapCat already exposes the required conversation-local history actions:

- `get_group_msg_history`
- `get_friend_msg_history`

Both support paged fetches using message-sequence anchors and "latest N"
behavior when the anchor is absent.

### Bridge-Owned History Capability

The bridge adds one lane-scoped history query capability:

`query_current_conversation_history`

Inputs:

- lane-scoped token or lane identity,
- optional natural-language query text,
- optional normalized time window,
- optional sender filter,
- optional keyword filter,
- optional anchor message id,
- optional context-window request,
- bounded scan budget.

Outputs:

- normalized transcript entries,
- matched entry ids,
- optional surrounding context,
- scan-truncated flag when the budget is exhausted.

### Scope Rules

The history capability may only access the current lane:

- current group for group lanes,
- current private chat for private lanes.

It must never accept arbitrary `group_id` / `user_id` from the model.

### Time Query Semantics

User-facing skill guidance remains natural-language friendly:

- relative time ("昨天", "前天", "最近两小时"),
- absolute dates,
- fuzzy time windows.

The model interprets the user's phrasing. The bridge receives only normalized,
bounded query instructions.

### Budget Rules

History scans must be bounded by:

- page size,
- maximum pages,
- maximum transcript lines returned to the model.

When the budget is exceeded, the bridge returns partial results plus an
"insufficient budget" signal so the agent narrows the query instead of
retrying unboundedly.

## Prompt And Skill Design

### New Project Skill

Add a project skill dedicated to current-conversation QQ history lookup, for
example `skills/qq-current-history/SKILL.md`.

The skill explains:

- this capability is only for the current lane,
- supported query intent classes: time, sender, keyword, quoted context,
- how to use returned `message_id` values with `reply-current --reply-to`,
- that the agent must not fabricate history when no result is found,
- that bounded scans may require narrowing the search window.

### Bridge Protocol Changes

The embedded bridge protocol gains a new first gate:

- **Gate 0 — Context first**

If the user is asking about earlier lines, quoted content, or a time-bounded
history slice in the current QQ conversation, the agent must query current
conversation history before optional capability delegation.

The existing capability-routing gates remain, but only after current-context
assembly.

### Heavy-Load Policy Rewrite

The current protocol's broad "do not scan history" wording is rewritten to:

- forbid unbounded or cross-conversation scans,
- allow bounded, current-conversation history lookups via the dedicated bridge
  capability,
- require the agent to narrow the query when the budget is exceeded.

## Service And API Model

### Replace Single-Running Snapshot

The service no longer exposes a single `TaskSnapshot`. Instead it publishes a
runtime snapshot containing:

- lane list with per-lane state,
- runtime slot list,
- ready-queue summary,
- aggregate counts,
- recent terminal lane summaries.

### Status API

`/api/status` returns structured multi-lane, multi-slot JSON.

The user-facing CLI and formatter read this structure to answer:

- which conversations are running,
- which conversations are queued,
- which slot is broken,
- whether one lane is blocked or stale.

### Queue API

Queue reporting becomes lane-centric rather than "raw task count only".

### Reply API

Reply stays as `/api/reply` but becomes explicitly lane-token based.

### History API

Add a dedicated local API endpoint for lane-scoped history query, for example:

`POST /api/history/query`

The request must include the lane-scoped token and bounded query options.

## Configuration Changes

The runtime config should describe the new architecture directly.

New fields:

- `runtime_pool_size`
- `lane_pending_capacity`
- `history_page_size`
- `history_max_pages`
- `max_turn_wall_time_secs`
- `stalled_turn_timeout_secs`
- `slot_restart_backoff_ms`

Old names such as `queue_capacity` should be removed when they no longer
describe the active model accurately.

## File And Module Restructure

### New Or Expanded Modules

- `runtime_pool.rs`
  Own slot lifecycle, health, replacement, shared Codex-home wiring.
- `lane_manager.rs`
  Own lane state and lane-scoped queues.
- `dispatcher.rs`
  Own ready-lane FIFO and dispatch loop.
- `history.rs` or `conversation_history.rs`
  Own NapCat history querying and transcript normalization.

### Simplified Existing Modules

- `reply_context.rs`
  Shrinks to lane-scoped context persistence and token resolution only.
- `service.rs`
  Publishes runtime snapshots and lane-scoped reply / history helpers.
- `api.rs`
  Replaces single-task status/reporting with lane/slot JSON endpoints.
- `orchestrator.rs`
  Either becomes the new dispatcher host or is deleted if the dispatcher fully
  replaces it.

## Deleted Legacy Paths

The following obsolete or dangerous paths must be removed, not hidden behind
compatibility branches:

- singleton reply-context path and mirror logic,
- CLI reply fallback to singleton context,
- `TaskSnapshot` single-running-task state,
- status formatters that assume one active task only,
- queue promotion logic that only considers "same conversation next task",
- any helper or test whose only purpose is to preserve the singleton model.

## Migration And Persistence

### Conversation Binding

Existing `conversation_key -> thread_id` persistence remains valid and should be
reused.

### Runtime Recovery

On restart:

- recover lane-thread bindings from persistent state,
- mark any previously running tasks interrupted,
- rebuild empty in-memory lane / slot state,
- do not assume any slot from before the restart still exists.

## Testing Strategy

The implementation must be test-driven.

Required coverage includes:

- one lane never runs two turns concurrently,
- two different lanes can run concurrently when two slots exist,
- one stuck lane does not block another lane when another slot is free,
- broken slot returns its lane to queued or blocked state correctly,
- reply context file is mandatory and wrong-lane fallback is impossible,
- reply tokens cannot route to another lane,
- history query only accesses the current lane,
- history query obeys page and output budgets,
- status API reports multiple running lanes and slot states correctly,
- protocol / skill prompt content includes the new context-first and history
  guidance.

## Risks

### Shared Codex Home Correctness

The design assumes upstream Codex thread persistence is safe when multiple
app-server processes share the same persistent Codex-home state. This must be
verified under concurrent thread resume and turn execution.

### Large Orchestrator Rewrite Surface

This refactor deliberately removes half-old abstractions instead of layering
more compatibility shims on top. That is correct, but it means broad test
updates are unavoidable.

### History Query Budget Tuning

If the initial scan budget is too small, legitimate queries will fail too
often. If it is too large, the bridge risks reintroducing resource spikes. The
budget must therefore be explicit and test-covered.

## Rollout Strategy

Implementation proceeds in phases:

1. Introduce runtime-pool, lane, and dispatcher primitives.
2. Replace execution scheduling with the new lane/slot architecture.
3. Delete singleton reply paths and enforce explicit lane reply context.
4. Add lane-scoped QQ history query and the new project skill.
5. Rewrite bridge protocol and system prompt assembly.
6. Replace status / queue APIs and CLI output.
7. Delete obsolete scheduler, snapshot, and fallback code.

This is intentionally a breaking refactor. No compatibility layer is kept for
legacy reply-context or single-running-task assumptions.
