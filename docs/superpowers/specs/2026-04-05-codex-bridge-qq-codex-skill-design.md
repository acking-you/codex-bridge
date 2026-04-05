# Codex Bridge QQ Codex Skill Design

## Goal

Evolve `codex-bridge` from a QQ-to-Codex text bridge into a QQ bot runtime with
clear admission control, skill-driven result delivery, and a real operating
boundary between "what Codex may observe" and "what Codex may change".

The resulting system must:

- launch NapCat as a child process and communicate over the formal OneBot
  WebSocket,
- route friend private messages and group `@bot` mentions into long-lived
  Codex threads,
- keep bridge-owned control flow (`/help`, `/status`, queueing, failure
  handling) separate from Codex-owned result delivery,
- let Codex send text, images, and files back through one unified bridge skill,
- allow machine-wide read access and system inspection,
- restrict writes to the current repository and restrict new-file creation to
  `.run/artifacts/`,
- block dangerous host actions such as process termination and service control.

## Non-Goals

- Reimplement NapCat or Codex app-server behavior inside `codex-bridge`.
- Support non-QQ transports in this iteration.
- Add image understanding, voice handling, or arbitrary rich-media parsing.
- Introduce multi-task parallel execution.
- Allow Codex or skills to choose arbitrary outbound QQ or group targets.
- Allow new files to be created anywhere outside `.run/artifacts/`.
- Allow any dangerous host-control action such as `kill`, `pkill`, `killall`,
  `shutdown`, `reboot`, `poweroff`, or `systemctl stop/restart/kill`.

## Current Problem

The current bridge is still too narrow and too trusting:

- normal final replies are still treated like plain text emitted by the bridge,
- there is no `/help`,
- group start feedback is plain text rather than a native message reaction,
- private messages from non-friends are not blocked before queue admission,
- the current system prompt still describes a repository-only assistant,
- the current approval layer only models "workspace only" semantics rather than
  "machine-readable, repo-writable, artifacts-only for new files",
- there is no unified skill path for Codex to send text, images, and files back
  to the current QQ conversation.

That leaves the user with the wrong runtime contract: the bridge looks like a
generic text relay instead of a controlled Codex-powered bot runtime.

## Design Principles

### Bridge Owns Policy, Codex Owns Normal Results

The bridge owns admission control, queueing, permissions, failure handling, and
system messages. Codex owns successful result delivery through a single approved
skill.

### One Reply Skill, One Active Conversation

Codex must never choose arbitrary outbound targets. The bridge will expose only
one "reply to the current conversation" capability, backed by an expiring
context token for the active task.

### Full Visibility, Narrow Mutation

Codex may inspect the machine broadly enough to answer real operational
questions, including process and port state. Mutation is much narrower:

- existing repository files may be modified,
- new files may only be created under `.run/artifacts/`,
- outbound attachments must also come from `.run/artifacts/`.

### KISS/YAGNI

This design keeps one queue, one reply skill, one artifact root, and one
transport. Do not add multiple output roots, per-skill target selection, or a
second command system in the first implementation.

## User-Facing Behavior

## Trigger Rules

### Private Messages

- Only QQ friends may trigger Codex.
- If the sender is not a friend, the bridge must:
  - reject the message before thread creation,
  - reject it before queue admission,
  - reject it before any Codex call,
  - send a short persona-consistent reply telling the sender to add the bot as a
    friend first.
- If the sender is a friend, normal text messages trigger Codex by default.

### Group Messages

- Only messages that `@` the bot may trigger the runtime.
- The `@bot` mention is removed before message text is sent to Codex.
- Group control commands use the same entry point: if the bot is mentioned and
  the remaining text is `/status`, `/queue`, and so on, the command executes.

### Control Commands

The first iteration supports exactly:

- `/help`
- `/status`
- `/queue`
- `/cancel`
- `/retry_last`

No other bridge-side commands are introduced in this design.

Control commands are handled entirely by the bridge and must not be forwarded to
Codex as normal turn input.

## Persona

The bot persona is a controlled blend:

- 80% competent, all-purpose, high-tech cyber lifeform,
- 20% Bocchi-like restraint: slightly shy, slightly awkward, lightly self-aware.

The persona applies mainly to bridge-generated short messages and routine
conversation. Technical output, error descriptions, and state summaries must
remain clear and direct.

## Start Feedback

### Private Messages

When a friend private message starts execution, the bridge sends a short,
persona-aligned text acknowledgement. It must not use the current cold phrasing
equivalent to "收到，开始处理。".

### Group Messages

When a group `@bot` message starts execution, the bridge does not send a text
acknowledgement. Instead, it applies a native "salute" emoji-like reaction to
the original message.

## Final Results

Normal successful results are not emitted by the bridge as a plain text summary.
They are delivered by Codex through the unified reply skill.

If a turn completes successfully but no reply skill invocation happened, the
bridge sends one short fallback notice indicating that the task finished without
generating a sendable result. This prevents a silent black hole.

If a turn fails, is interrupted, or is rejected by policy, the bridge sends the
error or status message itself.

## Command Behavior

### `/help`

Returns one short but complete help message that includes:

- private-message and group-mention trigger rules,
- the non-friend private-message restriction,
- available commands,
- a short permission summary.

### `/status`

Returns:

- whether a task is running,
- which conversation owns the running task,
- elapsed runtime,
- queue length,
- the latest task outcome summary,
- a short description of the latest active phase when a task is running.

### `/queue`

Returns a compact queue view.

### `/cancel`

Only the user who started the currently running task may cancel it.

### `/retry_last`

Only the user who owns the current conversation's latest failed or interrupted
task may retry it.

### Visibility Rules

- `/help`, `/status`, and `/queue` are available to anyone who can trigger the
  bot in that conversation.
- `/cancel` and `/retry_last` are ownership-restricted as defined above.

## Conversation and Queue Model

## Long-Lived Conversation Bindings

Keep the existing long-lived thread model:

- `private:<user_id>` maps to one Codex thread,
- `group:<group_id>` maps to one Codex thread.

This remains persistent across restarts.

## Single Global Queue

Keep the existing conservative execution model:

- one running task globally,
- queue length limit of five,
- overflow is rejected immediately,
- restart does not auto-resume queued work.

## Admission Order

Admission must happen in this order:

1. Parse the inbound QQ event.
2. Check whether it is a supported trigger.
3. For private messages, verify the sender is a friend.
4. For control commands, check command ownership rules.
5. Only then create or resume a Codex thread and enqueue task work.

This keeps invalid senders and invalid commands out of the scheduler entirely.

## Codex Runtime Contract

## System Prompt

The new prompt version must describe the bot as:

- a cybernetic assistant with the approved persona,
- allowed to inspect the host machine broadly,
- allowed to use web search,
- forbidden from dangerous host actions,
- allowed to modify existing repository files,
- allowed to create new files only under `.run/artifacts/`,
- expected to use the unified reply skill for normal result delivery,
- expected to avoid `thread/shellCommand`.

The prompt must clearly explain that group and private contexts differ, and that
the reply skill automatically routes results back to the current conversation.

## Permission Boundary

Bridge-side enforcement, not just prompt text, must match the same rules:

- machine-wide read access is allowed,
- process, socket, and service inspection is allowed,
- writes are restricted to the current repository,
- modification of existing repository files is allowed,
- creation of new files is restricted to `.run/artifacts/`,
- outbound attachment files must be under `.run/artifacts/`,
- dangerous commands are denied.

Dangerous commands include at least:

- `kill`
- `pkill`
- `killall`
- `reboot`
- `shutdown`
- `poweroff`
- `systemctl stop`
- `systemctl restart`
- `systemctl kill`

This boundary is both the Codex runtime policy and the skill policy.

## Unified Reply Skill

## Skill Layout

The repository root gains:

- `skills/`
- `.agents/skills` as a symlink to `skills/`

This keeps skill source first-party, versioned, and easy for Codex to discover.

## Single Skill Surface

Expose one unified reply skill rather than three separate skills. That skill may
send:

- text,
- image,
- file.

It must route through the `codex-bridge` CLI rather than reimplementing NapCat
or OneBot calls directly.

## CLI Contract

The skill-facing CLI surface becomes:

```text
codex-bridge reply --text "..."
codex-bridge reply --image .run/artifacts/result.png
codex-bridge reply --file .run/artifacts/report.md
```

The skill-facing command must not accept arbitrary `user_id`, `group_id`, or
`message_id` parameters.

The reply token is supplied by the bridge runtime, not typed manually by the
skill author or chat user. The implementation may pass it through environment
variables, stdin context, or another bridge-controlled mechanism, but it must
not become a user-chosen targeting parameter.

## Reply Token

Each running task gets a temporary reply token bound to:

- `task_id`
- conversation identity
- conversation type
- source message id
- source sender id

The reply token:

- is valid only while that turn is active,
- may be used multiple times during the active turn,
- expires immediately when the turn reaches a terminal state,
- only authorizes replies to the current conversation.

## Private and Group Reply Rules

### Private Replies

`codex-bridge reply` sends directly to the private conversation.

### Group Replies

`codex-bridge reply` automatically:

- references the original source message,
- `@` mentions the original sender,
- sends the requested text, image, or file as the message body or attachment.

Codex does not build reply segments manually; the bridge owns that formatting.

## Attachment Constraints

Image and file replies must validate that:

- the path exists,
- the path is within the repository,
- the path is under `.run/artifacts/`,
- the file type matches the mode requested.

If validation fails, the send is rejected.

## Bridge-Owned Messages

Bridge-owned messages are limited to:

- non-friend private-message rejection,
- `/help`,
- `/status`,
- `/queue`,
- `/cancel`,
- `/retry_last`,
- start feedback,
- queue-full feedback,
- enqueue-position feedback,
- policy rejection,
- task failure,
- task interruption,
- "completed with no sendable result" fallback.

Normal successful result content is not bridge-owned.

## Persistence

The existing SQLite-backed state remains the source of truth for:

- conversation-to-thread bindings,
- task run records,
- last-known task outcomes,
- prompt version tracking.

This design adds the requirement that reply-token state and current outbound
reply context are tracked in runtime memory for the active task, with enough
information to enforce reply ownership and expiry.

## Testing

Implementation must cover at least:

- friend versus non-friend private-message admission,
- group `@bot` command parsing including `/help`,
- `/cancel` and `/retry_last` ownership rules,
- start-feedback behavior differences between private and group contexts,
- reply-token lifetime and multi-send validity during a running turn,
- `.run/artifacts/` path enforcement for outbound files,
- approval denial for dangerous commands,
- approval acceptance for host inspection commands such as process and socket
  listing,
- fallback behavior when a turn completes without any reply skill invocation.

## Migration Impact

This design intentionally changes several user-visible behaviors:

- private messages from non-friends no longer reach Codex,
- normal final replies are no longer automatically mirrored from the last
  assistant text,
- group start feedback becomes a message reaction instead of a text response,
- skill infrastructure becomes a first-class part of the repository layout.

These are intentional behavior changes in service of a clearer and safer
runtime, not compatibility shims layered onto the current bridge.
