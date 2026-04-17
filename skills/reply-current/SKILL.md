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
- Attachments must already exist under `.run/artifacts/`.
- Prefer text for short answers, image for visual output, and file for markdown/report artifacts.
- If you want line breaks in QQ, put real newline characters in `--text`. The plain `"..."` form in bash does NOT turn `\n` into a newline; use `$'line1\nline2'` (ANSI-C quoting), or split a `"..."` string across two source lines, or call this skill once per line. The bridge defensively decodes a stray `\n`/`\r\n`/`\t` if one slips through, but writing real newlines is cleaner.
- Group messages reach you with mention markers preserved: `@<bot>` is the placeholder for an `@` aimed at the bot itself, and `@nickname<QQ:1234567>` (or `@<QQ:1234567>` when the original at segment carried no name) is the placeholder for an `@` aimed at any other user. Read those markers when relevant; do not echo them back into your reply.

## Commands
Send plain text (default: @-mentions the original sender):

```bash
python3 skills/reply-current/reply_current.py --text "处理完成，结论在这里。"
```

Send plain text and @-mention specific users instead of the sender:

```bash
python3 skills/reply-current/reply_current.py --text "这是你要的结果" --at 1234567
```

```bash
python3 skills/reply-current/reply_current.py --text "你们看看这个" --at 1234567 7654321
```

Send an image artifact:

```bash
python3 skills/reply-current/reply_current.py --image .run/artifacts/result.png
```

Send a file artifact:

```bash
python3 skills/reply-current/reply_current.py --file .run/artifacts/report.md
```

## Choosing who to @

By default the bridge @-mentions the person who sent the original message. Use `--at` to override this when the context makes it clear the sender wants someone else to see the reply:

- **Sender @-mentioned another user alongside the bot** — e.g. `@bot 帮 @小明<QQ:1234567> 看看这个`. The sender wants 小明 to see the answer. Pass `--at 1234567` (or `--at 1234567 <sender_qq>` if you also want to @ the sender).
- **Sender explicitly asked you to reply to someone** — e.g. "把结果发给 @小明<QQ:1234567>". Pass `--at 1234567`.
- **No special mention context** — omit `--at` entirely; the bridge will @ the sender as usual.

Read the QQ id from the `@nickname<QQ:...>` placeholder in the incoming message text. Do not guess QQ ids.

## Common Mistakes
- Do not use `send-private` or `send-group` for normal task results.
- Do not point to files outside `.run/artifacts/`.
- Do not assume bridge will send your final assistant text for you.
