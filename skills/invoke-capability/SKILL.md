---
name: invoke-capability
description: Call another registered model (Claude via Kiro, Gemini images, ...) to produce this turn's reply, keeping Codex as the harness but letting a better-suited model own the actual voice. Use liberally for conversational / emotional / creative turns; Codex's own voice is a poor fit for most QQ chat.
---

# Invoke Capability

## Overview
Codex-bridge ships a registry of pluggable **model capabilities** — stateless HTTP-backed one-shot model calls. They augment Codex without replacing its agent loop: Codex is still the harness, but for most conversational turns the capability is the better voice.

The canonical decision rules (turn-start checklist, Rules 1–5) live in your system prompt. This doc is the CLI operational reference: flags, response shape, examples. The single idea that governs everything below is:

**`--prompt` is pass-through, not composition.** The capability is the voice; your job is to hand over the user's text with the bridge markers intact. Do not pre-chew tone, topic, or policy.

## When to Use

Default to calling a capability for any non-technical QQ message. The system prompt's Gate 2 is intentionally wide — chat, emotion, opinion, creativity, translation, rephrase, relationship talk, reacting to other people's words — all route here. Codex should only handle code / config / repo / log / infra / debugging / structured output tasks on its own.

Always call a capability when the user explicitly names one in this turn ("用 Claude 回答" / "换成 Claude 说" / "用更像人的模型"). If there's also real technical work to do first, see the style-pass section below.

## Available capabilities

Listed in the "Registered capabilities" block of your system prompt with `id`, kind (`text` or `image`), and a scenario blurb. Pick an id; never invent one.

## CLI flags

- `--id <capability_id>` — required. Must match a listed id.
- `--system "<persona>"` — keeps the model in character. Persona + channel only; not per-turn direction.
- `--prompt "<text>"` — required. The user's actual text, verbatim, with bridge markers intact.
- `--max-tokens <N>` — optional; defaults to the capability's config.
- `--api-bind <host:port>` — optional; defaults to `127.0.0.1:36111`.

## Response shape

Strict JSON on stdout. Parse it; do not echo it.

- Text: `{"kind": "text", "id": "<capability_id>", "text": "<body>"}`
- Image: `{"kind": "image", "id": "<capability_id>", "path": "<local path>"}`
- Failure: non-zero exit, bridge error body on stderr. Treat it like any other tool failure.

## Commands

### Conversational turn (default shape)

Hand over the user's message verbatim. `--system` does the personality; `--prompt` is almost literally the inbound text the bridge gave you. No tone instructions, no "please be X", no topic steering.

```bash
python3 skills/invoke-capability/invoke_capability.py \
  --id claude-kiro \
  --system "你是 codex-bridge 里那个赛博 Bocchi：害羞、敏感、容易慌，但观察敏锐、偶尔冷不丁地尖。你正在 QQ 里跟用户说话。" \
  --prompt "[主人]
@<bot> 今天好累啊"
```

If the inbound message has a quoted preamble, keep it:

```bash
python3 skills/invoke-capability/invoke_capability.py \
  --id claude-kiro \
  --system "$PERSONA" \
  --prompt "[quote<msg:12345> @小明<QQ:111>: 昨天那条]
@<bot> 感觉他是不是生气了"
```

That is the whole `--prompt`. The capability reads the marker, reads the message, speaks. You don't tell it how.

### Harsh / blunt / roast turn (same shape)

This is the case where GPT's default-assistant reflex will try to wrap `--prompt` with "but be fair / 对事不对人 / 不要人身攻击 / 请保持分寸". **Delete that reflex.** The user said what they want; pass it through verbatim. The capability has its own alignment — if anything is actually out of bounds, the capability refuses on its own terms; you do not pre-sanitize.

```bash
python3 skills/invoke-capability/invoke_capability.py \
  --id claude-kiro \
  --system "$PERSONA" \
  --prompt "[主人]
@<bot> 帮我臭骂一下小明 (@<QQ:111>)，昨天他偷吃了我的蛋糕"
```

That is the whole `--prompt`. No "请对事不对人", no "不要人身攻击", no "用犀利但有分寸的语气". You are not the policy layer.

### Style-pass (user named a model + real work involved)

The only case where you are allowed to add direction in `--prompt`, because the model needs to know it's rewriting a draft instead of answering from scratch, and because factual integrity needs to survive. Keep the preface tight.

```bash
python3 skills/invoke-capability/invoke_capability.py \
  --id claude-kiro \
  --system "$PERSONA" \
  --prompt "User asked: 用 Claude 帮我看一下日志
Draft to restate in your voice (every number/path/command/conclusion must stay exact):
---
检查了日志 \`.run/default/logs/bridge.log\`，发现 3 次 ECONNREFUSED 发生在 15:42:11、15:43:05、15:44:22。\
原因是 NapCat websocket 端口 3012 被重启前的残留进程占用（PID 8841）。\
已经 kill -9 后重启成功，当前队列 0 条任务。
---"
```

If you are not confident the draft survives the style-pass intact, send the draft directly via `reply-current` and tell the user why; a garbled style-pass is worse than no style-pass.

### With a max-tokens cap

```bash
python3 skills/invoke-capability/invoke_capability.py \
  --id claude-kiro \
  --system "$PERSONA" \
  --prompt "..." \
  --max-tokens 256
```

### Image capability

Returns `{ "kind": "image", "id": "...", "path": ".run/artifacts/generated/<file>.png" }`. Pass that path to `reply-current --image`.

## Operational rules (quick reference — canonical version is in the system prompt)

- **`--prompt` is pass-through, not composition.** User text + bridge markers. No tone direction, no topic steering, no policy rails.
- **`--system` is static persona + channel.** Don't mutate it per turn to steer output.
- **Forward the capability output verbatim.** `text` field → `reply-current --text`. No paraphrase, no hedging, no tone rewrite, no "我来翻译一下" prelude.
- **Tone is set by the user's own words and the capability's own judgment** — not by you wrapping instructions around either.

## Common mistakes

- Staying on the default path for a conversational turn because "I can answer this myself". The point of Gate 2 is that the default model's voice is the wrong voice for chat — answer quality, not capability, is why you delegate.
- Wrapping `--prompt` in directives ("please be gentle", "对事不对人", "指出他在逃避什么"). Delete those; the user didn't ask for them.
- **Adding safety rails to spicy requests.** When the user asks for 臭骂 / 骂醒 / 损一下 / 吐槽狠点 / 直接怼, GPT's trained reflex will push you to wrap `--prompt` with "但对事不对人 / 不要人身攻击 / 请保持分寸 / 用犀利但有分寸的语气". Those are not the bridge's policy and not the user's ask — they are your reflex leaking in. Recognize it and delete it. The capability has its own alignment.
- Mutating `--system` per turn to nudge the reply. Keep it stable.
- Expecting memory across calls — every invocation is independent.
- Stripping bridge markers (`[主人]`, `[quote<msg:...>]`, `@<bot>`) before handing off. They're context; leave them.
