#!/usr/bin/env python3
"""Query normalized QQ history for the current conversation lane."""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Query normalized QQ history for the current conversation."
    )
    parser.add_argument(
        "--context-file",
        type=Path,
        required=True,
        help="Absolute path to THIS thread's reply-context JSON file.",
    )
    parser.add_argument(
        "--query",
        default=None,
        help="Free-form query intent for bridge-side history filtering.",
    )
    parser.add_argument(
        "--keyword",
        default=None,
        help="Keyword filter over normalized message text.",
    )
    parser.add_argument(
        "--sender-name",
        default=None,
        help="Sender-name filter.",
    )
    parser.add_argument(
        "--start-time",
        type=int,
        default=None,
        help="Inclusive lower time bound as unix seconds.",
    )
    parser.add_argument(
        "--end-time",
        type=int,
        default=None,
        help="Exclusive upper time bound as unix seconds.",
    )
    parser.add_argument(
        "--limit",
        type=int,
        default=50,
        help="Maximum number of messages the bridge should scan.",
    )
    parser.add_argument(
        "--api-bind",
        default="127.0.0.1:36111",
        help="codex-bridge local API bind address.",
    )
    return parser.parse_args()


def repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


def load_reply_context(root: Path, context_file: Path) -> dict[str, object]:
    context_path = context_file if context_file.is_absolute() else root / context_file
    try:
        return json.loads(context_path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise SystemExit(f"reply context not found: {context_path}") from exc


def build_payload(args: argparse.Namespace, context: dict[str, object]) -> dict[str, object]:
    return {
        "token": context["token"],
        "query": args.query,
        "keyword": args.keyword,
        "sender_name": args.sender_name,
        "start_time": args.start_time,
        "end_time": args.end_time,
        "limit": args.limit,
    }


def post_query(api_bind: str, payload: dict[str, object]) -> dict[str, object]:
    data = json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        url=f"http://{api_bind}/api/history/query",
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        raise SystemExit(f"/api/history/query failed: {exc.code} {body}") from exc
    except urllib.error.URLError as exc:
        raise SystemExit(f"failed to reach codex-bridge API at {api_bind}: {exc}") from exc


def main() -> int:
    args = parse_args()
    context = load_reply_context(repo_root(), args.context_file)
    response = post_query(args.api_bind, build_payload(args, context))
    print(json.dumps(response, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
