---
name: reply-current
description: Use when a Codex turn running inside codex-bridge must send normal results back to the active QQ conversation as text, image, or file
---

# Reply Current

## Overview
Use this skill whenever you need to return a normal successful result to QQ.
Do not ask the bridge to mirror your last assistant message automatically. Send the result yourself.

## When to Use
- You finished a task and need to answer the current QQ conversation.
- You generated an image, markdown file, report, or other artifact under `.run/artifacts/`.
- You need to send a short text result directly.

Do not use this skill for failures. Bridge-generated errors are handled by the runtime.

## Rules
- Reply only to the current active conversation. Do not invent QQ IDs or group IDs.
- Use exactly one local `reply_current.py` command per message you want to send.
- **Always pass `--context-file <path>`.** The path is given to you in the "# Reply context" section of your `developer_instructions` and is specific to THIS thread's conversation. The legacy singleton at `.run/default/run/reply_context.json` is shared across all active tasks and may point at the wrong group under concurrency — skill calls that omit `--context-file` can silently deliver the reply into another chat.
- Attachments must already exist under `.run/artifacts/`.
- Prefer text for short answers, image for visual output, and file for markdown/report artifacts.
- If you want line breaks in QQ, put real newline characters in `--text`. The plain `"..."` form in bash does NOT turn `\n` into a newline; use `$'line1\nline2'` (ANSI-C quoting), or split a `"..."` string across two source lines, or call this skill once per line. The bridge defensively decodes a stray `\n`/`\r\n`/`\t` if one slips through, but writing real newlines is cleaner.
- Group messages reach you with mention markers preserved: `@<bot>` is the placeholder for an `@` aimed at the bot itself, and `@nickname<QQ:1234567>` (or `@<QQ:1234567>` when the original at segment carried no name) is the placeholder for an `@` aimed at any other user. Read those markers when relevant; do not echo them back into your reply — a bare QQ id is meaningless to a human reader. The bridge will defensively strip any marker that slips through (and downgrade `@nickname<QQ:...>` to `@nickname`), but write clean text to begin with. Only @ someone when the sender asked for it; do not invent extra pings.
- When the inbound message quoted another message, the bridge prepends a context block in the form `[quote<msg:12345> @nickname<QQ:1111>: 原文]` (or `[quote<msg:12345>]` when the fetch failed). Use it as read-only context. If the quoted content matters and the block is missing the real body — especially for quoted images — recover the original payload first with `qq-quoted-image-recovery` instead of inventing a summary.

## Commands
Send plain text (default: @-mentions the original sender and quotes the triggering message):

```bash
python3 skills/reply-current/reply_current.py \
  --context-file /abs/path/from/developer_instructions/contexts/group_111.json \
  --text "处理完啦～"
```

`--context-file` is mandatory (the absolute path is given in your `developer_instructions` "# Reply context" section); omit it and you risk cross-talk between concurrent groups.

Send plain text and @-mention specific users instead of the sender:

```bash
python3 skills/reply-current/reply_current.py \
  --context-file /abs/path/from/developer_instructions/contexts/group_111.json \
  --text "这是你要的结果" \
  --at 1234567
```

```bash
python3 skills/reply-current/reply_current.py \
  --context-file /abs/path/from/developer_instructions/contexts/group_111.json \
  --text "你们看看这个" \
  --at 1234567 7654321
```

Quote a specific earlier message instead of the triggering one (the reply pill in QQ will land on that message):

```bash
python3 skills/reply-current/reply_current.py \
  --context-file /abs/path/from/developer_instructions/contexts/group_111.json \
  --text "找到了，就是这条" \
  --reply-to 12345
```

Send an image artifact:

```bash
python3 skills/reply-current/reply_current.py \
  --context-file /abs/path/from/developer_instructions/contexts/group_111.json \
  --image .run/artifacts/result.png
```

Send a file artifact:

```bash
python3 skills/reply-current/reply_current.py \
  --context-file /abs/path/from/developer_instructions/contexts/group_111.json \
  --file .run/artifacts/report.md
```

## Choosing who to @

By default the bridge @-mentions the person who sent the original message. Use `--at` to override this when the context makes it clear the sender wants someone else to see the reply:

- **Sender @-mentioned another user alongside the bot** — e.g. `@bot 帮 @小明<QQ:1234567> 看看这个`. The sender wants 小明 to see the answer. Pass `--at 1234567` (or `--at 1234567 <sender_qq>` if you also want to @ the sender).
- **Sender explicitly asked you to reply to someone** — e.g. "把结果发给 @小明<QQ:1234567>". Pass `--at 1234567`.
- **No special mention context** — omit `--at` entirely; the bridge will @ the sender as usual.

Read the QQ id from the `@nickname<QQ:...>` placeholder in the incoming message text. Do not guess QQ ids.

## Choosing which message to quote

The reply pill in QQ defaults to the message that addressed you. Override with `--reply-to <msg_id>` when jumping to a different line reads more naturally:

- **Sender asked you to find an earlier chat record** — e.g. "帮我翻一下昨天小明说过的那句关于部署的话". After you locate the target message id (from `[quote<msg:...>]` in the inbound block, or from history the sender pasted), pass `--reply-to <that_msg_id>` so the pill lands on the actual target.
- **Sender replied-to an earlier message while asking you a follow-up** — if your answer is about that quoted message itself (not the sender's latest sentence), pass `--reply-to <quoted_msg_id>` so the pill rebounds to that line.
- **No special context** — omit `--reply-to`; the default reads naturally.

Never fabricate a `--reply-to` id. Read ids from placeholders in the inbound text; do not invent numbers.

## Common Mistakes
- Do not use `send-private` or `send-group` for normal task results.
- Do not point to files outside `.run/artifacts/`.
- Do not assume bridge will send your final assistant text for you.
