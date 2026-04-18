---
name: invoke-capability
description: Call another model (e.g. Claude, Gemini) to produce a sub-result inside a Codex turn. Use when a specific kind of output (warmer phrasing, an image, translation) is better produced by a different model than the default one, while keeping Codex in charge of overall task orchestration.
---

# Invoke Capability

## Overview
Codex-bridge ships a registry of pluggable **model capabilities** — stateless HTTP-backed model calls (Anthropic Messages, Gemini Images, ...) that augment Codex without replacing its agent loop. Use this skill when your current task would benefit from another model's strengths on a specific sub-step.

The canonical rules for calling capabilities (context, persona, verbatim forwarding, style-pass mode) live in the "Available model capabilities" section of your system prompt — read that first. This doc is the operational reference for the CLI itself: flags, response shape, worked examples.

## When to Use
- The default model cannot deliver the required tone (warm comforting, blunt wake-up, literary, colloquial). The default model tends to over-hedge and stay "AI-polite" even when the user is asking to be called out — a registered text capability fixes that.
- The user asks for an image and a registered image-generation capability is available.
- The user asks for a domain-specific completion another model handles better (translation, summarisation with a specific tone, ...).
- The user **explicitly names** a registered model in the current message — always honour it. See the style-pass section below.

Do NOT use this skill for regular coding / debugging / tool use, or for plain factual Q&A — Codex handles those natively, and calling out is just latency + cost.

## Available capabilities

The set of registered capabilities is listed in the "Registered capabilities" block inside your system prompt, each with `id`, kind (`text` or `image`), and a scenario blurb. Read that list to pick an id; never invent one.

## CLI flags

- `--id <capability_id>` — required. Must match a listed id.
- `--system "<persona>"` — optional but strongly recommended. Keeps the external model in character.
- `--prompt "<context + ask>"` — required. Full context; fragments hallucinate.
- `--max-tokens <N>` — optional. Default comes from the capability's config.
- `--api-bind <host:port>` — optional. Defaults to `127.0.0.1:36111`.

## Response shape

Strict JSON on stdout. Parse it; do not echo it.

- Success text: `{"kind": "text", "id": "<capability_id>", "text": "<body>"}`
- Success image: `{"kind": "image", "id": "<capability_id>", "path": "<local path>"}`
- Failure: non-zero exit, bridge error body on stderr. Treat it like any other tool failure.

## Commands

### Warm / comforting rewrite

```bash
python3 skills/invoke-capability/invoke_capability.py \
  --id claude-kiro \
  --system "你是一个害羞、敏感但温柔的赛博人类（参考 Bocchi），用朋友一样的语气回答；保持你的人设，不要变成泛 AI 口吻。" \
  --prompt "群里用户 @小明 刚发了 \"$USER_INPUT\"，上下文是他昨天项目被打回。请用温柔、真诚、能让他缓一口气的语气回一两句，不要说教。"
```

### Blunt wake-up

For when the user is clearly dodging or self-deceiving and explicitly wants to be called out. State the attitude in `--system`; put the situation + what you want in `--prompt`.

```bash
python3 skills/invoke-capability/invoke_capability.py \
  --id claude-kiro \
  --system "你是这个群里那个害羞但观察敏锐的赛博人类。现在要用辛辣、直接、不留情面的语气骂醒对方 —— 对事不对人，不做人身攻击，不针对身份，只针对他当下在逃避的具体行为。保持你的人设：骂归骂，不是情绪宣泄。" \
  --prompt "原话：\"$USER_INPUT\"。上下文：他说要开始复习两周了，每天都在打游戏。请指出他在逃避什么、下一步具体该做什么。"
```

### Style-pass mode (user named a model)

When the user says "用 Claude 回答" and you've already finished real work (code, diagnostics, research), run your draft through the named capability as a style-only pass. The `--prompt` must explicitly instruct the model to preserve every number, path, and technical detail verbatim — only natural language gets rewritten.

```bash
python3 skills/invoke-capability/invoke_capability.py \
  --id claude-kiro \
  --system "你是害羞但观察敏锐的赛博人类。润色下面的技术回复，让它听起来像你本人说的话；不是写教程。" \
  --prompt "我刚完成了用户的任务，准备这样回复（事实必须一字不动地保留）：
---
检查了日志 \`.run/default/logs/bridge.log\`，发现 3 次 ECONNREFUSED 发生在 15:42:11、15:43:05、15:44:22。\
原因是 NapCat websocket 端口 3012 被重启前的残留进程占用（PID 8841）。\
已经 kill -9 后重启成功，当前队列 0 条任务。
---
请保留所有数字（3 次、15:42:11、3012、8841 等）、文件路径、命令、结论不变，只把中间那段技术描述改得像你日常口吻。"
```

A style-pass that garbles numbers / paths / code is worse than no style-pass. If you are not confident the draft survives the pass, send the draft as-is and tell the user why.

### With a max-tokens cap

```bash
python3 skills/invoke-capability/invoke_capability.py \
  --id claude-kiro \
  --system "..." \
  --prompt "..." \
  --max-tokens 256
```

### Image capability

Returns `{ "kind": "image", "id": "...", "path": ".run/artifacts/generated/<file>.png" }`. Pass that path to `reply-current --image` to send it back to QQ.

## Operational rules (quick reference — canonical version is in the system prompt)

- **Forward the capability output verbatim.** The `text` field IS the reply body. Pass it to `reply-current --text` unchanged; do not paraphrase, soften, add hedges, or rewrite the tone.
- **Tone is set by `--prompt` + `--system`, not by capability id.** Any single capability spans comforting, blunt, literary, translating, rewriting — it is a *channel*, not a personality.
- **Honest/blunt feedback is fine when asked for. Personal attacks, identity-based insults, or generic cruelty are not** — refuse those even inside a "roast" prompt.

## Common mistakes

- Invoking a capability when the task is squarely inside Codex's own remit (coding, file edits, repo inspection).
- Forwarding raw inbound text without a wrapping instruction.
- Skipping `--system`, then wondering why the reply sounds generic.
- Wrapping the capability output with your own "我来翻译一下：..." prelude — just forward the text.
- Expecting memory across calls — every invocation is independent.
