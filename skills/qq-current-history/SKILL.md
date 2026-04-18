---
name: qq-current-history
description: Query normalized QQ message history for the current conversation only, with optional time, sender, keyword, and free-form filters
---

# QQ Current History

## Overview
Use this skill when the user asks about earlier messages in the current QQ
conversation and the quoted preamble is not enough.

This skill is lane-scoped:
- it only queries the current group or private chat,
- it uses the current lane's reply-context token,
- it never accepts arbitrary `group_id` or `user_id`.

## When to Use
- The user asks "刚才谁说了什么", "昨天群里那句", "上周三聊部署那条".
- The user wants the context around a quoted message.
- The user wants you to quote an earlier message back with `reply-current --reply-to`.
- You need to verify who said something before drafting a reply.

If the quoted message was probably an image or screenshot and you need the
pixels, switch to `qq-quoted-image-recovery` instead of treating this as plain
text history.

## Rules
- **Always pass `--context-file <path>`.** The path is given in your developer
  instructions for THIS thread.
- Query only the current conversation. Never invent other chat ids.
- Support time, sender, keyword, and free-form query intent.
- The bridge scan budget is bounded. If the result says `truncated: true` and
  you still need more precision, narrow the query instead of broadening it.
- If no result is found, say so plainly. Do not fabricate QQ history.
- Returned `message_id` values are safe to use with `reply-current --reply-to`.

## Commands

Free-form lookup:

```bash
python3 skills/qq-current-history/query_current_history.py \
  --context-file /abs/path/from/developer_instructions/contexts/group_111.json \
  --query "找今天下午提部署那句"
```

Keyword lookup:

```bash
python3 skills/qq-current-history/query_current_history.py \
  --context-file /abs/path/from/developer_instructions/contexts/group_111.json \
  --keyword "部署"
```

Sender-scoped lookup:

```bash
python3 skills/qq-current-history/query_current_history.py \
  --context-file /abs/path/from/developer_instructions/contexts/group_111.json \
  --sender-name "小明" \
  --keyword "部署"
```

Time-bounded lookup:

```bash
python3 skills/qq-current-history/query_current_history.py \
  --context-file /abs/path/from/developer_instructions/contexts/group_111.json \
  --start-time 1744970400 \
  --end-time 1744977600
```

## Reading Results
- `messages` is the filtered normalized transcript slice.
- `message_id` is the QQ message id you can reuse with `--reply-to`.
- `timestamp` is unix seconds.
- `sender_name` and `text` are already normalized into bridge-friendly text.
- `truncated: true` means the bridge hit its scan budget.

## Common Mistakes
- Do not use this skill to search other groups or other private chats.
- Do not retry with a broader and broader query if the first attempt is
  truncated.
- Do not treat `query` as magic semantic search. It is just a bounded bridge
  query input, so tighter filters are better.
