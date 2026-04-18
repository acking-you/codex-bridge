#!/usr/bin/env python3
"""Invoke one registered model capability from inside a Codex turn.

Speaks to the codex-bridge local API (``/api/capability/invoke``) and
prints the raw JSON response to stdout so Codex can parse it
programmatically. This skill is stateless: it does not touch
``.run/artifacts/`` or the reply context — the caller (Codex) decides
what to do with the returned text/image.
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Call a registered model capability (e.g. Claude via Kiro) and "
            "return the raw JSON response."
        )
    )
    parser.add_argument(
        "--id",
        required=True,
        help="Capability id declared in model_capabilities.toml "
             "(e.g. 'claude-kiro').",
    )
    parser.add_argument(
        "--prompt",
        required=True,
        help="Prompt forwarded to the external model verbatim.",
    )
    parser.add_argument(
        "--system",
        default=None,
        help="Optional system prompt forwarded to the external model "
             "(persona, target tone, guardrails). Capabilities that "
             "support a system role pass it natively; otherwise the "
             "backend prepends it to the user prompt.",
    )
    parser.add_argument(
        "--max-tokens",
        type=int,
        default=None,
        help="Optional upper bound on output tokens. Omitting it lets "
             "the capability use its configured default.",
    )
    parser.add_argument(
        "--api-bind",
        default="127.0.0.1:36111",
        help="codex-bridge local API bind address.",
    )
    return parser.parse_args()


def invoke(api_bind: str, payload: dict[str, object]) -> dict[str, object]:
    data = json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        url=f"http://{api_bind}/api/capability/invoke",
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        raise SystemExit(f"/api/capability/invoke failed: {exc.code} {body}") from exc
    except urllib.error.URLError as exc:
        raise SystemExit(
            f"failed to reach codex-bridge API at {api_bind}: {exc}"
        ) from exc


def main() -> int:
    args = parse_args()
    payload: dict[str, object] = {
        "id": args.id,
        "prompt": args.prompt,
    }
    if args.system is not None:
        payload["system"] = args.system
    if args.max_tokens is not None:
        payload["max_tokens"] = args.max_tokens
    response = invoke(args.api_bind, payload)
    print(json.dumps(response, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
