---
name: gpt2api-image-generator
description: Use when a QQ user asks Codex to draw, paint, create, generate, or render an image through GPT2API, including Chinese requests such as 画图, 画一张, 生成图片, 出图, 做张图, or 帮我画
---

# GPT2API Image Generator

## Overview
Use this skill for direct image-generation and direct image-edit requests. It
calls the configured GPT2API public image endpoints, stores generated images
under `.run/artifacts/generated/`, and returns local artifact paths that can be
sent with `reply-current --image`.

Do not answer image-generation requests with text only unless generation fails.
Do not use GPT2API session APIs for this skill.

## Config
The private config lives at:

```text
.bridge-private/gpt2api-image.json
```

That directory is gitignored. The required fields are:

- `base_url`: GPT2API public API root, for example `https://ackingliu.top/api/gpt2api`
- `api_key`: bearer token for GPT2API

Optional fields:

- `model`: defaults to `gpt-image-2`
- `size`: defaults to `1024x1024`
- `n`: defaults to `1`
- `response_format`: defaults to `b64_json`
- `timeout_seconds`: defaults to `300`
- `endpoint_path`: defaults to `/images/generations`
- `edit_endpoint_path`: defaults to `/images/edits`

## Workflow
### Direct Generation
1. Treat the user's visual request as the prompt. Keep concrete style, subject,
   text, ratio, and size requirements if they were given.
2. Generate the image:

```bash
python3 skills/gpt2api-image-generator/generate_gpt2api_image.py \
  --prompt "一只穿宇航服的猫，月球表面，电影感光照"
```

### Direct Edit
1. Recover or use the input image file path from the current request context.
2. Treat the user's requested change as the prompt. Keep size requirements if
   they were given.
3. Edit the image:

```bash
python3 skills/gpt2api-image-generator/generate_gpt2api_image.py \
  --image .run/artifacts/input.png \
  --prompt "保留人物姿势，把背景改成雨夜霓虹街道" \
  --size 1024x1536
```

### Reply
1. Parse the JSON output. For one image, use `path`; for multiple images, use
   each value in `paths`.
2. Send each generated image with `reply-current`:

```bash
python3 skills/reply-current/reply_current.py \
  --context-file /abs/path/from/developer_instructions/contexts/group_111.json \
  --image .run/artifacts/generated/gpt2api_20260428T120000Z_1_abcd1234.png
```

## Rules
- This skill exposes only two bot-level concepts: direct generation
  (`prompt` + optional `size`/`n`) and direct edit (`image` + `prompt` +
  optional `size`/`n`).
- Use `/images/generations` for direct generation and `/images/edits` for
  direct edit. Do not use `/sessions`, `/messages`, or product session APIs.
- Always send the generated image artifact itself. A text-only "done" reply is
  not enough.
- If the user asks for more than one image, pass `--n`, capped by the script.
- If the user specifies a square or exact size, pass `--size WIDTHxHEIGHT`.
- If the user asks to edit an existing image, pass that image with `--image`.
- Keep the prompt focused on the requested image. Do not prepend chat markers,
  routing notes, or commentary.
- Do not expose the API key in replies, logs, or copied command output.
