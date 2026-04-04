# My QQ Bot

`my_qq_bot` is a Linux-only Rust bridge around desktop QQ.

It does three things:

- launches Linux QQ in the foreground,
- keeps a local receive/send bridge through the injected NapCat runtime,
- exposes a small local HTTP/WebSocket API so your own code can subscribe to
  messages and send replies.

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
cd my_qq_bot
cargo run -p qqbot-cli -- run
```

What happens:

1. the current repository builds the required NapCat shell assets,
2. `my_qq_bot/.run/default` is prepared,
3. QQ is patched to load the local shell build,
4. QQ starts in the foreground,
5. the terminal prints QQ/NapCat logs and the text QR code,
6. after you scan the QR code, the local API becomes usable.

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
  -d '{"user_id":2394626220,"text":"hello from my_qq_bot"}'
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

## CLI Shortcuts

Start the bridge:

```bash
cargo run -p qqbot-cli -- run
```

Send a private message through the running bridge:

```bash
cargo run -p qqbot-cli -- send-private --user-id 2394626220 --text "hello"
```

Send a group message through the running bridge:

```bash
cargo run -p qqbot-cli -- send-group --group-id 123456 --text "hello group"
```

Query cached contacts:

```bash
cargo run -p qqbot-cli -- friends
cargo run -p qqbot-cli -- groups
```

## Developer Commands

```bash
make fmt
make lint
make test
make run
```
