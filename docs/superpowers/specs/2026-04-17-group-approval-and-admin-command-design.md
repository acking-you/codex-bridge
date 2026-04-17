# Codex Bridge Group Approval And Admin Command Design

## Goal

Refine bridge-side control flow so that group approval and admin operations are
usable in practice without loosening the safety boundary.

The resulting system must:

- approve non-admin group requests only when the configured admin reacts to the
  original group message with the salute emoji,
- keep private approval behavior unchanged,
- allow admin-only runtime commands from both admin private chat and admin
  group chat,
- add `/clear` and `/compact` as precise per-conversation management commands,
- preserve the current single-queue execution model and conversation binding
  model.

## Non-Goals

- Add multi-admin support.
- Approve private requests through reactions.
- Allow bare slash commands in groups without `@bot`.
- Introduce a second approval-emoji configuration field.
- Add parameterized thread-management commands in the first iteration.
- Rework scheduler semantics, persistence schema shape, or restart recovery
  beyond what this feature needs.

## Current Problem

Today the bridge has two operator-friction problems.

First, pending group approvals still require admin private-chat commands. That
is mechanically correct but operationally awkward, because the admin is already
looking at the original group message and wants to approve in place.

Second, runtime management commands such as `/status` are effectively locked to
admin private chat by orchestrator-side permission checks even though the group
router can already parse `@bot /status`.

There is also no conversation-level command to:

- reset the current Codex thread binding for one chat, or
- trigger a manual Codex context compaction for one chat.

## Selected Approach

Three approaches were considered:

1. Keep private-chat approval and only loosen admin commands.
2. Move group approval to in-group salute reactions while keeping command
   syntax conservative.
3. Make both approval and admin commands broadly group-native, including bare
   slash commands.

Approach 2 is selected.

It solves the operator pain directly, preserves the existing safety boundary,
and avoids unnecessary trigger expansion in noisy group chats.

## Design Principles

### Group Approval Must Happen On The Original Message

Approval must map back to one exact pending request. The source of truth is the
original group message id already stored on the pending task row.

### Keep Group Command Triggers Narrow

Admin commands in groups still require `@bot`. This preserves existing group
task trigger rules and avoids accidental command execution from unrelated slash
text.

### Reuse Existing Salute Emoji Configuration

The approval gesture uses the same configured emoji id already stored in
`group_start_reaction_emoji_id`.

This keeps the first iteration simple:

- bot start feedback in groups uses the salute emoji,
- admin approval-by-reaction also uses the salute emoji.

No second emoji setting is introduced.

### `/clear` Resets Future Context, Not History

`/clear` must not delete task records or mutate finished runs. It only removes
the current conversation-to-thread binding so that the next task starts from a
fresh Codex thread.

### `/compact` Must Be Real

`/compact` must call the actual Codex `thread/compact/start` RPC. It must not
be implemented as a fake acknowledgement or as a prompt that asks the model to
"summarize itself".

## Event Model Changes

### New Normalized Group Reaction Event

The bridge currently normalizes only message events. It must also normalize the
group message reaction notice emitted by NapCat.

The normalized event needs exactly the fields required by approval matching:

- `group_id`,
- `message_id`,
- `operator_id`,
- `emoji_id`,
- `is_add`,
- raw payload for debugging.

The normalizer must accept the NapCat group reaction notice shape used for
group emoji responses. Approval logic only cares about add-events, not remove
events.

### Approval Match Rule

A pending group approval is approved if and only if all of the following are
true:

- the pending task is a group task,
- the reaction event targets the pending task's `source_message_id`,
- the reaction operator is `admin_user_id`,
- the reaction emoji id equals the configured salute emoji id,
- the event represents an added reaction.

Any other reaction is ignored.

## Approval Flow

### Private Requests

Private non-admin requests keep the current approval flow:

- requester gets a waiting message,
- admin receives private approval notice plus `/approve`, `/deny`, `/status`
  helper commands,
- approval and denial continue to work through admin private chat only.

### Group Requests

Non-admin group `@bot` requests still enter `PendingApproval`, but the admin
interaction changes:

- requester gets a group-visible waiting message,
- the bridge may still send one minimal admin-facing informational notice so
  the pending task id remains inspectable,
- that informational notice must not include an `/approve <task_id>` helper
  because group approval is reaction-only,
- the bridge waits for the admin to react to the original group message with
  the salute emoji,
- on approval, the task leaves `PendingApproval` and enters the normal
  execution path,
- `/approve <task_id>` is not a valid approval path for group pending tasks,
- denial and inspection may still use explicit admin commands when the admin
  needs them.

This design intentionally replaces only group approval, not private approval.

### Admin Group Requests

Admin-authored group tasks continue to bypass approval entirely and execute
immediately, exactly as they do today.

## Admin Command Permissions

### Permission Rule

All admin-only runtime commands are allowed when:

- sender id equals `admin_user_id`, and
- the request is either:
  - an admin private message command, or
  - an admin group command that mentioned the bot.

Non-admin callers still receive the existing admin-only rejection path.

### Approval Command Rule

`/approve <task_id>` becomes source-type-sensitive:

- private pending tasks may still be approved through `/approve <task_id>`,
- group pending tasks must be approved only by the salute reaction on the
  original source message.

If the admin tries to approve a group pending task through `/approve`, the
bridge returns a short correction telling them to use the salute reaction on the
original group message instead.

### Public Command Rule

`/help` remains public.

No other command is made public in this change.

## New Commands

### `/clear`

`/clear` operates on the current command conversation only.

Conversation resolution:

- admin private chat targets `private:<admin_user_id>`,
- admin group chat targets `group:<group_id>`.

Behavior:

- if no binding exists for that conversation, reply that there is no context to
  clear,
- otherwise delete the conversation binding,
- do not delete task history,
- do not interrupt a running task,
- do not mutate queue entries or pending approvals,
- next task for that conversation creates a fresh Codex thread.

### `/compact`

`/compact` also operates on the current command conversation only.

Behavior:

- if no binding exists, reply that there is no context to compact,
- if the same conversation currently has the active running task, reject the
  command and tell the admin to wait or `/cancel` first,
- otherwise call Codex `thread/compact/start` using the bound thread id.

Important semantic detail:

- `thread/compact/start` returns `{}` immediately and signals real progress via
  normal thread/item notifications,
- therefore bridge command success means "compaction successfully started", not
  "compaction fully completed".

The first iteration returns a concise start acknowledgement after the RPC
accepts the request. It does not block waiting for the later compaction
notification.

## Persistence Changes

No schema migration is required.

The existing `conversation_bindings` table is enough. The state store only
needs one delete operation by `conversation_key` so `/clear` can remove the
binding.

Task history remains untouched.

## Runtime Changes

The Codex runtime trait must grow one explicit compaction entrypoint so bridge
logic does not misuse unrelated APIs.

Required capability:

- `compact_thread(thread_id)`

This call should wrap `thread/compact/start` and return once the app-server has
accepted the request.

## User-Facing Behavior

### Help Text

`/help` must be updated so it no longer claims that admin-only commands are
private-chat-only.

It must mention:

- admin commands can be issued in admin private chat or admin group chat with
  `@bot`,
- `/clear`,
- `/compact`,
- group requests require admin salute-reaction approval.

### Waiting Message For Group Approval

Group waiting copy should explicitly tell users that the request is waiting for
the admin to react to the original message with the salute emoji.

### Admin Command Replies

New reply copy is required for:

- clear success,
- clear no-op,
- compact started,
- compact missing binding,
- compact rejected because the same conversation is currently running.

## Testing Contract

The implementation is complete only when these behaviors are covered.

### Event Tests

- normalize one supported group reaction payload into the new normalized event.

### Router Tests

- parse `/clear`,
- parse `/compact`,
- keep group command routing gated on `@bot`.

### State Store Tests

- deleting a conversation binding removes it without affecting task rows.

### Orchestrator Tests

- non-admin group task waits for approval and is approved by admin salute
  reaction on the source message,
- wrong emoji does not approve,
- non-admin reaction does not approve,
- `/approve <task_id>` on a group pending task is rejected with a correction,
- admin group `@bot /status` works,
- non-admin group `@bot /status` is rejected,
- `/clear` removes the current conversation binding and causes the next task to
  allocate a fresh thread,
- `/compact` reports missing binding when absent,
- `/compact` rejects when the same conversation is currently active,
- `/compact` calls the runtime compaction entrypoint when the conversation is
  idle and bound.

## Risks And Tradeoffs

### Reaction Payload Variants

NapCat exposes group emoji response notices in multiple closely related forms.
The bridge must normalize only the concrete shapes needed for this repository
and ignore unsupported variants safely.

### Shared Salute Emoji

Using one salute emoji id for both "bot started work" and "admin approved this
group request" is intentional for the first iteration, but it does mean one
emoji now has two meanings depending on who applied it and when.

That tradeoff is acceptable because approval matching also checks the operator
id and source message id.

### Compact Is Asynchronous

Because compaction is started asynchronously by the runtime, the bridge cannot
truthfully promise immediate completion from the command response alone. The
user-facing copy must stay precise.

## Acceptance Criteria

The change is accepted when all of the following are true:

- group pending approvals no longer require admin private approval commands for
  approval,
- admin salute reaction on the original group message approves the pending task,
- `/approve <task_id>` no longer approves group pending tasks,
- admin runtime commands work in admin private chat and admin group chat with
  `@bot`,
- `/clear` resets future conversation context by removing the binding,
- `/compact` starts real Codex thread compaction,
- README and help text match the implemented behavior.
