---
name: staticflow-kiro-log-diagnoser
description: Use when a Codex turn needs to diagnose StaticFlow Kiro upstream failures by reading the backend error log and correlating real usage events through sf-cli
---

# StaticFlow Kiro Log Diagnoser

## Overview
Use this skill when someone asks you to troubleshoot current StaticFlow Kiro failures.
This skill is only for Kiro upstream errors. Ignore unrelated backend noise, including non-Kiro gateway failures.

## Scope
Focus only on these StaticFlow log errors under `~/rust_pro/static_flow/tmp/staticflow-backend.log`:

- `kiro public request failed while calling upstream`
- `failed to read kiro upstream event stream`

If the error is not a Kiro upstream failure, say it is out of scope for this skill and stop.

## Required Evidence
Do not guess from one log line alone. Always collect both:

1. the actual Kiro error line from `staticflow-backend.log`
2. the matching `llm_gateway_usage_events` row from StaticFlow's LanceDB via `sf-cli`

Use:

```bash
python3 skills/staticflow-kiro-log-diagnoser/inspect_kiro_errors.py list --limit 20
```

Then query the matching usage event:

```bash
python3 skills/staticflow-kiro-log-diagnoser/inspect_kiro_errors.py usage \
  --timestamp 2026-04-05T10:37:17.913736Z \
  --status 502 \
  --key-name admin
```

or:

```bash
python3 skills/staticflow-kiro-log-diagnoser/inspect_kiro_errors.py usage \
  --timestamp 2026-04-05T14:43:20.119261Z \
  --status 599 \
  --key-name for-external
```

## Diagnosis Rules

### 1. `kiro public request failed while calling upstream`
Treat this as a malformed request class if the upstream error is:

- `400 Bad Request`
- `Improperly formed request`

For this class, inspect nearby log lines for:

- `request_validation_enabled`
- `normalized_tool_description_count`
- `empty_tool_description`
- `fill_tool_description`

The default high-confidence explanation is:

- local Kiro request normalization or validation did not catch malformed tool metadata before the upstream call
- the most likely cause is an empty tool description escaping local normalization
- `anyOf` alone is not the main cause unless you find stronger evidence

When you explain this class, ground it with:

- `backend/src/kiro_gateway/anthropic/mod.rs`
- `backend/src/kiro_gateway/anthropic/converter.rs`
- `docs/superpowers/specs/2026-04-05-kiro-tool-validation-design.md`

### 2. `failed to read kiro upstream event stream`
Treat this as a streaming timeout class if you see:

- `is_timeout=true`
- `is_connect=false`
- usage event `status_code=599`
- latency near `720000ms`

The default high-confidence explanation is:

- the upstream request was accepted and started streaming
- reading the response body later timed out or stalled midstream
- this is not the same failure class as a malformed request

When you explain this class, ground it with:

- `backend/src/kiro_gateway/anthropic/mod.rs`
- `backend/src/kiro_gateway/provider.rs`

## Working Commands
Show the last Kiro upstream error lines:

```bash
python3 skills/staticflow-kiro-log-diagnoser/inspect_kiro_errors.py list --limit 20
```

Show a small log window around one line number:

```bash
sed -n '5373,5379p' ~/rust_pro/static_flow/tmp/staticflow-backend.log
```

Read the Kiro tool validation design:

```bash
sed -n '1,160p' ~/rust_pro/static_flow/docs/superpowers/specs/2026-04-05-kiro-tool-validation-design.md
```

## Output Contract
Your final answer should include:

- which Kiro error class it is
- the exact evidence from log lines
- the matching usage event evidence from `sf-cli`
- the most likely real cause
- whether the failure is a deterministic local malformed request or an upstream streaming timeout

Do not mix in unrelated Codex/OpenAI/other gateway errors.
