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
- Use exactly one `codex-bridge reply` command per message you want to send.
- Attachments must already exist under `.run/artifacts/`.
- Prefer text for short answers, image for visual output, and file for markdown/report artifacts.

## Commands
Send plain text:

```bash
codex-bridge reply --text "处理完成，结论在这里。"
```

Send an image artifact:

```bash
codex-bridge reply --image .run/artifacts/result.png
```

Send a file artifact:

```bash
codex-bridge reply --file .run/artifacts/report.md
```

## Common Mistakes
- Do not use `send-private` or `send-group` for normal task results.
- Do not point to files outside `.run/artifacts/`.
- Do not assume bridge will send your final assistant text for you.
