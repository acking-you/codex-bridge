---
name: qq-quoted-image-recovery
description: Use when a quoted QQ message likely contained an image or screenshot but the bridge only exposed a quote marker or flattened text
---

# QQ Quoted Image Recovery

## Overview
Use this skill when a QQ reply chain points at an earlier image message but the
bridge did not hand you the image itself.

The core rule is simple:
- a quote marker is only a handle,
- the flattened quote text is not the image,
- you have not "seen the image" until you recover the original media payload.

## When to Use
- The inbound text contains `[quote<msg:12345> ...]` or `[quote<msg:12345>]`.
- The user is asking what a quoted screenshot/image says or means.
- The quote block is empty or nearly empty, but the surrounding context suggests
  the original message was visual.
- You need the quoted image pixels before answering.

Do not use this skill for ordinary text-only quote context. Use
`qq-current-history` for that.

## Workflow
1. Read the quoted `message_id` from `[quote<msg:...>]`.
2. Recover the raw quoted message via OneBot `get_msg`.
3. Inspect the raw message JSON for `image` segments.
4. Recover the actual image URL or local path from that raw payload.
5. Download the image into `.run/artifacts/`.
6. Inspect locally first.
7. Only if local inspection is insufficient, send the recovered artifact to a
   vision-capable model.

## Rules
- Stay inside the current conversation and the quoted `message_id`.
- Do not pretend the bridge already showed you the image when it only showed a
  quote marker.
- Do not hallucinate missing text from a screenshot you have not recovered.
- Prefer a bounded, deterministic recovery path over broad history scanning.
- If recovery fails, say that clearly and explain that only the quote marker was
  available.

## Practical Notes
- Today the bridge flattens quoted message text with `extract_text`, so image
  segments can disappear from the visible quote preamble.
- The raw `get_msg` payload is the source of truth when you need to recover
  quoted media.
- If the raw payload contains multiple segments, do not assume the first one is
  text; inspect the segment list directly.
- Save recovered media under `.run/artifacts/quoted_<message_id>.<ext>`.

## Local-First Inspection
- Check file type, dimensions, and whether it is obviously a screenshot, photo,
  or meme.
- Use local OCR / preview / terminal-friendly inspection first when available.
- Escalate to a vision model only when the answer really depends on pixels that
  local inspection cannot reliably recover.

## Common Mistakes
- Treating `[quote<msg:...>]` as if it already contained the image body.
- Summarizing a screenshot before recovering the actual image payload.
- Jumping straight to a vision model without first reconstructing the quoted
  artifact.
- Broadly scraping conversation history instead of targeting the quoted
  `message_id`.
