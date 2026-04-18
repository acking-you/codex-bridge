#!/usr/bin/env python3
"""Send one skill-driven reply to the active codex-bridge conversation."""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Send one reply to the active codex-bridge conversation."
    )
    parser.add_argument(
        "--text",
        help="Plain-text reply body.",
    )
    parser.add_argument(
        "--image",
        type=Path,
        help="Image artifact path under .run/artifacts/.",
    )
    parser.add_argument(
        "--file",
        type=Path,
        help="File artifact path under .run/artifacts/.",
    )
    parser.add_argument(
        "--at",
        type=int,
        nargs="+",
        default=[],
        help="QQ id(s) to @-mention in the group reply instead of the original sender. "
             "Pass one or more numeric QQ ids. Ignored for private chats.",
    )
    parser.add_argument(
        "--reply-to",
        type=int,
        default=None,
        help="QQ message id to quote with the outbound reply instead of the inbound "
             "triggering message. Useful when the user asked you to locate an earlier "
             "chat record and you want the reply pill to jump straight to it. "
             "Omitting this preserves the default (quote the triggering message). "
             "Ignored for private chats.",
    )
    parser.add_argument(
        "--context-file",
        type=Path,
        required=True,
        help="Absolute path to THIS thread's reply-context JSON file. "
             "Provided in your developer_instructions (section 'Reply "
             "context'). This is required because replies are lane-scoped "
             "and the bridge never falls back to a singleton context file.",
    )
    parser.add_argument(
        "--api-bind",
        default="127.0.0.1:36111",
        help="codex-bridge local API bind address.",
    )
    args = parser.parse_args()
    variants = sum(
        value is not None for value in (args.text, args.image, args.file)
    )
    if variants != 1:
        parser.error("exactly one of --text, --image, or --file is required")
    return args


def repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


def load_reply_context(root: Path, context_file: Path) -> dict[str, object]:
    context_path = context_file if context_file.is_absolute() else root / context_file
    try:
        return json.loads(context_path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise SystemExit(f"reply context not found: {context_path}") from exc


def build_payload(args: argparse.Namespace, context: dict[str, object], root: Path) -> dict[str, object]:
    payload: dict[str, object] = {
        "token": context["token"],
        "text": None,
        "image": None,
        "file": None,
        "at": args.at if args.at else [],
        "reply_to": args.reply_to,
    }
    if args.text is not None:
        payload["text"] = normalize_escape_sequences(args.text)
    elif args.image is not None:
        payload["image"] = str(resolve_user_path(args.image, root))
    else:
        payload["file"] = str(resolve_user_path(args.file, root))
    return payload


def normalize_escape_sequences(text: str) -> str:
    """Defensive decode of literal escape pairs that show up when the agent
    quotes the --text argument with single quotes (or any shell form that does
    not interpret backslash escapes).

    Without this normalisation, text like 'line1\\nline2' would reach QQ as the
    seven literal characters ``line1\\nline2`` instead of two real lines. The
    bridge's system prompt asks the agent to use real newlines, but we also
    decode here so a single shell-quoting slip does not produce a visibly
    broken message in the chat.

    The transformations are intentionally narrow and do not run a full
    unicode_escape decode (which would also try to interpret things like
    ``\\u00xx`` and could mangle legitimate backslashes in user content).
    """
    if not text:
        return text
    return (
        text
        .replace("\\r\\n", "\n")
        .replace("\\n", "\n")
        .replace("\\r", "\n")
        .replace("\\t", "\t")
    )


def resolve_user_path(path: Path, root: Path) -> Path:
    return path if path.is_absolute() else root / path


def post_reply(api_bind: str, payload: dict[str, object]) -> dict[str, object]:
    data = json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        url=f"http://{api_bind}/api/reply",
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        raise SystemExit(f"/api/reply failed: {exc.code} {body}") from exc
    except urllib.error.URLError as exc:
        raise SystemExit(f"failed to reach codex-bridge API at {api_bind}: {exc}") from exc


def main() -> int:
    args = parse_args()
    root = repo_root()
    context = load_reply_context(root, args.context_file)
    payload = build_payload(args, context, root)
    response = post_reply(args.api_bind, payload)
    print(json.dumps(response, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
