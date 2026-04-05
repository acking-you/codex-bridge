# Codex Bridge Admin Approval Design

## Goal

Add an explicit admin-approval layer in front of Codex execution so that the QQ
bot no longer treats any non-admin message as immediately runnable work.

The resulting system must:

- load one configured admin QQ identifier from an operator-editable file,
- allow only admin private messages to bypass approval,
- place every other executable request into a separate pending-approval pool,
- require approval or denial from the admin through bot-admin private chat,
- expire unanswered approval requests after 15 minutes,
- keep pending approvals out of the current Codex execution queue,
- keep the user-visible behavior clear in both private and group contexts.

## Non-Goals

- Add multi-admin support.
- Auto-approve an entire conversation after one approval.
- Approve from group chat or from any non-private context.
- Persist pending approvals across restart and resume them automatically.
- Replace the existing single global Codex execution queue.
- Rework NapCat transport behavior outside what is needed for admin approval.

## Current Problem

Today the bridge has only two admission gates:

- non-friend private messages are rejected,
- group messages require `@bot`.

That is not enough for ambiguous or high-risk group requests. Once a message
passes those existing gates, the bridge may enqueue or start Codex work without
an explicit human approval checkpoint. The result is a blurry trust boundary:
"friend" and "`@bot`" are being treated as execution permission, even though the
operator wants all non-admin work to require approval first.

## Design Principles

### Only Admin Private Chat Is Trusted

The only direct-execution path is a private message from the configured admin QQ
account. No other sender and no group context is inherently trusted.

### Approval Is Per Task, Not Per Conversation

Every executable request outside admin private chat produces one approval
request. Approval never grants a future window for the same private chat or the
same group.

### Approval Wait State Is Separate from Execution Queue

Pending approval is not "queued execution". It is a different lifecycle stage.
Unapproved work must not occupy the current single-task Codex queue.

### Keep the Approval Surface Small

Admin must only see the minimum information needed to decide:

- task id,
- source type,
- source conversation key,
- sender QQ and display name,
- message summary,
- request time,
- approve / deny / inspect commands.

Do not dump full thread history, prompt text, or large internal state into the
approval message.

### Fail Safe

If approval config is missing, invalid, timed out, or lost due to restart, the
bridge must treat the request as not approved.

## Operator Configuration

### Admin Config File

The bridge will own one operator-editable config file:

- `.run/default/config/admin.toml`

The first iteration needs exactly one required field:

```toml
admin_user_id = 123456789
```

### Startup Rules

- If `admin.toml` does not exist, the bridge creates a template and exits with a
  clear error telling the operator to fill in `admin_user_id`.
- If `admin_user_id` is missing, zero, or invalid, startup fails.
- After QQ login, if `admin_user_id == self_id`, startup fails because the admin
  must not be the same account as the bot itself.

This keeps the approval boundary explicit and avoids silently starting in an
insecure mode.

## Approval Lifecycle

## Trigger Classification

### Direct Execution

Only one source may start Codex work without a separate approval step:

- private message from `admin_user_id`

### Approval-Required Requests

Everything else that would normally become a Codex task becomes a pending
approval request instead:

- friend private messages from non-admin users,
- group `@bot` task messages from any sender, including the admin.

### Non-Executable Messages

Safe bridge-only helpers may still remain outside approval when they do not
start Codex work. The first iteration keeps only `/help` outside approval.

All other task-like or control-like actions that would inspect or alter runtime
state should require the admin gate unless the sender is the admin in private
chat.

## Pending Approval Pool

Pending approvals live in a separate in-memory pool and never occupy the Codex
execution queue.

Rules:

- one global pending-approval pool,
- one active pending approval per conversation key at a time,
- global pending-approval capacity is bounded,
- default timeout is 15 minutes,
- timeout removes the request from the pending pool and marks it terminal.

If a second request arrives from the same conversation while one is already
waiting for approval, the bridge does not create a second pending approval.
Instead it replies that the conversation is already waiting for admin approval.

## Admin Commands

Admin approvals are accepted only through private chat with the bot.

The first iteration supports:

- `/approve <task_id>`
- `/deny <task_id>`
- `/status <task_id>`

No group approval path is allowed.

### `/approve <task_id>`

- Valid only in admin private chat.
- If the request is still pending, it leaves the pending pool and enters the
  normal execution path.
- If the Codex queue is idle, the task starts immediately.
- If the Codex queue is busy, the task enters the existing execution queue and
  the original requester receives queue-position feedback.
- The admin receives a short acknowledgement indicating whether the task started
  immediately or was queued.

### `/deny <task_id>`

- Valid only in admin private chat.
- Marks the request denied.
- Removes it from the pending pool.
- Sends a short denial notice back to the original requester.

### `/status <task_id>`

- Valid only in admin private chat.
- Returns the current approval/task state for that task id:
  - pending approval,
  - approved and queued,
  - running,
  - completed,
  - denied,
  - expired,
  - failed,
  - interrupted.

## Requester-Facing Behavior

### When Approval Is Required

The original requester gets immediate feedback instead of silence.

#### Private Message

The bridge replies with a short persona-consistent notice that the request needs
admin confirmation before execution begins.

#### Group Message

The bridge replies in the group by referencing the source message and `@`-ing
the sender, telling them the request is waiting for admin confirmation.

No salute reaction and no "started" acknowledgement is sent before approval.

### When Admin Approves

After approval:

- private requests follow the normal private start flow,
- group requests follow the normal group start flow,
- if execution cannot start immediately, the original requester gets normal
  queue feedback.

### When Admin Denies

The original requester gets a short refusal notice explaining that the admin did
not approve this request.

### When Approval Expires

After 15 minutes with no approval decision:

- the request is marked expired,
- it leaves the pending pool,
- the original requester gets a short timeout notice.

## State Model

## Task Status

Task state must grow beyond the current execution-only model.

The design adds at least these new terminal or pre-execution states:

- `PendingApproval`
- `Denied`
- `Expired`

Existing states remain:

- `Queued`
- `Running`
- `Completed`
- `Failed`
- `Canceled`
- `Interrupted`

### Lifecycle

1. Incoming message becomes an approval-required task.
2. Store task with `PendingApproval`.
3. Send approval request to admin and wait.
4. On `/approve`, transition to `Queued` or `Running`.
5. On `/deny`, transition to `Denied`.
6. On timeout, transition to `Expired`.
7. On restart, any still-pending approvals are not resumed; they are marked
   `Expired`.

## Persistence

The bridge should persist approval-task metadata in SQLite so that:

- task ids are stable,
- admin `/status <task_id>` works across the process lifetime,
- denial/expiry/completion history stays auditable.

The live pending-approval pool itself stays in memory. Restart clears it.

## Integration with Existing Queueing

The current global Codex scheduler remains unchanged after approval:

- one running task globally,
- bounded execution queue,
- same ownership rules for cancellation and retry where still applicable.

The admin approval layer sits strictly before that scheduler.

## Security and Compatibility

### Security

- Non-admin task requests do not reach Codex before approval.
- Group requests never gain direct trust merely because they mentioned the bot.
- Approval is explicit, per-task, and private to the admin chat.
- Missing admin config fails closed.

### Backward Compatibility

This intentionally changes admission behavior:

- non-admin friend private messages no longer start immediately,
- group `@bot` messages no longer start immediately,
- only admin private chat remains direct.

That break is intentional and is the feature.

## Required Code Areas

The implementation should land primarily in:

- `crates/codex-bridge-core/src/config.rs`
- `crates/codex-bridge-core/src/runtime.rs`
- `crates/codex-bridge-core/src/message_router.rs`
- `crates/codex-bridge-core/src/reply_formatter.rs`
- `crates/codex-bridge-core/src/scheduler.rs`
- `crates/codex-bridge-core/src/state_store.rs`
- `crates/codex-bridge-core/src/service.rs`
- `crates/codex-bridge-core/src/api.rs`
- `crates/codex-bridge-core/src/orchestrator.rs`

## Test Requirements

The implementation must add or update tests for:

- admin config file creation and validation,
- direct-admin-private bypass,
- non-admin friend private messages entering pending approval instead of direct
  execution,
- group `@bot` messages entering pending approval instead of direct execution,
- one-pending-request-per-conversation behavior,
- timeout to `Expired`,
- `/approve <task_id>` starting or queueing approved work,
- `/deny <task_id>` rejecting work,
- `/status <task_id>` reporting approval and execution states,
- restart marking pending approvals as expired rather than resuming them.
