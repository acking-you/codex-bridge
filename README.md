# Codex Bridge

`codex-bridge` is a Linux-first Rust bridge built around `codex app-server`.
The current transport is desktop QQ through the NapCat runtime.

It does three things:

- launches Linux QQ in the foreground,
- keeps a local receive/send bridge through the injected NapCat runtime,
- runs a single Codex task queue over formal OneBot WebSocket messages,
- exposes a small local HTTP/WebSocket API so your own code can subscribe to
  messages, inspect runtime state, and send replies.

## What This Project Is Not

- It does **not** re-implement QQ private protocol.
- It does **not** promise zero risk control or zero ban risk.
- It does **not** expose OneBot concepts as the main user-facing API.

The safety position is simple: keep the official desktop QQ login flow, avoid
private-protocol rewriting, and keep automation explicit.

## Requirements

- Linux
- `node`
- `pnpm`
- `python3`
- `curl`
- `xvfb-run`
- `dpkg` or `rpm2cpio + cpio` if QQ needs auto-install

QQ is reused from `$HOME/Napcat/opt/QQ/qq` by default. If that binary is
missing, the launcher installs Linux QQ automatically.

## Quick Start

```bash
cd codex-bridge
cargo run -p codex-bridge-cli -- run
```

What happens:

1. the current repository builds the required NapCat shell assets,
2. `codex-bridge/.run/default` is prepared,
3. QQ is patched to load the local shell build,
4. QQ starts in the foreground,
5. the terminal prints QQ/NapCat logs and the text QR code,
6. a local `codex app-server` child is started over `stdio`,
7. after you scan the QR code, the local API becomes usable.

## Repository Layout

- `crates/codex-bridge-core`: runtime, orchestrator, state store, and local API
- `crates/codex-bridge-cli`: CLI entrypoint
- `deps/NapCatQQ`: pinned NapCat source fork used to build and launch the QQ transport
- `deps/codex`: pinned Codex source fork used for `codex app-server` dependencies

## Local API

Default bind:

```text
http://127.0.0.1:36111
```

Routes:

- `GET /health`
- `GET /api/session`
- `GET /api/friends`
- `GET /api/groups`
- `GET /api/status`
- `GET /api/queue`
- `POST /api/tasks/cancel`
- `POST /api/tasks/retry-last`
- `POST /api/reply`
- `POST /api/messages/private`
- `POST /api/messages/group`
- `GET /api/events/ws`

### Health Check

```bash
curl http://127.0.0.1:36111/health
```

### List Friends

```bash
curl http://127.0.0.1:36111/api/friends
```

### Send a Private Message

```bash
curl -X POST http://127.0.0.1:36111/api/messages/private \
  -H 'content-type: application/json' \
  -d '{"user_id":2394626220,"text":"hello from codex-bridge"}'
```

### Subscribe to Incoming Events

Use any websocket client against:

```text
ws://127.0.0.1:36111/api/events/ws
```

Events are normalized JSON objects. Group and private messages include:

- sender ID,
- conversation ID,
- plain-text projection,
- mention list,
- whether the bot account was mentioned,
- the original raw JSON payload.

## Persistence

Runtime state is stored under:

```text
codex-bridge/.run/default/
```

Important files:

- `state.sqlite3`: conversation-to-thread bindings, task history, and prompt versions
- `run/launcher.env`: generated WebUI and OneBot tokens
- `logs/launcher.log`: foreground QQ/NapCat launcher log

Restart behavior:

- conversation bindings are kept,
- the currently running task is marked interrupted,
- queued tasks are not auto-resumed after restart.

## CLI Shortcuts

Start the bridge:

```bash
cargo run -p codex-bridge-cli -- run
```

Send a private message through the running bridge:

```bash
cargo run -p codex-bridge-cli -- send-private --user-id 2394626220 --text "hello"
```

Send a group message through the running bridge:

```bash
cargo run -p codex-bridge-cli -- send-group --group-id 123456 --text "hello group"
```

Query cached contacts:

```bash
cargo run -p codex-bridge-cli -- friends
cargo run -p codex-bridge-cli -- groups
```

Query the current orchestrator state:

```bash
cargo run -p codex-bridge-cli -- status
cargo run -p codex-bridge-cli -- queue
```

Send local control commands:

```bash
cargo run -p codex-bridge-cli -- cancel
cargo run -p codex-bridge-cli -- retry-last
```

Send one skill-facing reply to the active conversation:

```bash
cargo run -p codex-bridge-cli -- reply --text "处理完成了"
cargo run -p codex-bridge-cli -- reply --image .run/artifacts/result.png
cargo run -p codex-bridge-cli -- reply --file .run/artifacts/report.md
python3 skills/reply-current/reply_current.py --text "处理完成了"
```

Current behavior:

- `cancel` sends a real `turn/interrupt` request to the active Codex turn and waits for the task to reach `interrupted`.

## Trigger Rules

- Only friends can trigger Codex through private chat.
- Group messages trigger only when they `@` the bot.
- Control commands: `/help`, `/status`, `/queue`, `/cancel`, `/retry_last`.
- Only one Codex task runs at a time, with a bounded waiting queue behind it.
- Private start feedback is a short in-character text reply.
- Group start feedback is a salute emoji reaction on the triggering message.
- Normal successful results are expected to be returned through the `reply-current` skill.

## Safety

- Codex can inspect the host machine broadly, including process and port state.
- Web search is allowed.
- Existing files inside the current repository may be edited.
- New files may only be created under `.run/artifacts/`.
- `kill`, `pkill`, `killall`, `shutdown`, `reboot`, and service-stop commands are denied.
- `thread/shellCommand` is never used.

## Skills

- First-party skills live under `skills/`.
- Runtime startup ensures `.agents/skills` is a symlink to `skills/`.
- The unified QQ result-return skill is `skills/reply-current/SKILL.md`.

## Developer Commands

```bash
make fmt
make lint
make test
make run
```
