#!/usr/bin/env python3
"""List and correlate StaticFlow Kiro upstream errors."""

from __future__ import annotations

import argparse
import datetime as dt
import os
import subprocess
import sys
from pathlib import Path


STATICFLOW_ROOT = Path(os.path.expanduser("~/rust_pro/static_flow"))
DEFAULT_LOG_PATH = STATICFLOW_ROOT / "tmp/staticflow-backend.log"
DEFAULT_DB_PATH = Path(
    os.environ.get("STATICFLOW_DB_PATH", "/mnt/wsl/data4tb/static-flow-data/lancedb")
)
DEFAULT_SF_CLI = STATICFLOW_ROOT / "target/release/sf-cli"
ERROR_PATTERNS = (
    "kiro public request failed while calling upstream",
    "failed to read kiro upstream event stream",
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    list_parser = subparsers.add_parser("list")
    list_parser.add_argument("--limit", type=int, default=20)
    list_parser.add_argument("--log-path", type=Path, default=DEFAULT_LOG_PATH)

    usage_parser = subparsers.add_parser("usage")
    usage_parser.add_argument("--timestamp", required=True)
    usage_parser.add_argument("--status", type=int, required=True)
    usage_parser.add_argument("--key-name", default=None)
    usage_parser.add_argument("--window-ms", type=int, default=1_500)
    usage_parser.add_argument("--db-path", type=Path, default=DEFAULT_DB_PATH)
    usage_parser.add_argument("--sf-cli", type=Path, default=DEFAULT_SF_CLI)

    return parser.parse_args()


def list_errors(log_path: Path, limit: int) -> int:
    if not log_path.is_file():
        print(f"log file not found: {log_path}", file=sys.stderr)
        return 1

    matches: list[tuple[int, str]] = []
    with log_path.open("r", encoding="utf-8", errors="replace") as handle:
        for line_number, line in enumerate(handle, start=1):
            if "ERROR" not in line:
                continue
            if "static_flow_backend::kiro_gateway::anthropic" not in line:
                continue
            if not any(pattern in line for pattern in ERROR_PATTERNS):
                continue
            matches.append((line_number, line.rstrip()))

    for line_number, line in matches[-limit:]:
        print(f"{line_number}: {line}")
    return 0


def parse_timestamp_to_millis(raw_timestamp: str) -> int:
    normalized = raw_timestamp.replace("Z", "+00:00")
    instant = dt.datetime.fromisoformat(normalized)
    if instant.tzinfo is None:
        instant = instant.replace(tzinfo=dt.timezone.utc)
    return int(instant.timestamp() * 1000)


def query_usage(
    timestamp: str,
    status: int,
    key_name: str | None,
    window_ms: int,
    db_path: Path,
    sf_cli: Path,
) -> int:
    if not sf_cli.is_file():
        print(f"sf-cli not found: {sf_cli}", file=sys.stderr)
        return 1
    if not db_path.exists():
        print(f"LanceDB path not found: {db_path}", file=sys.stderr)
        return 1

    target_ms = parse_timestamp_to_millis(timestamp)
    lower = target_ms - window_ms
    upper = target_ms + window_ms
    where = (
        "provider_type = 'kiro' "
        f"AND status_code = {status} "
        f"AND created_at >= to_timestamp_millis({lower}) "
        f"AND created_at <= to_timestamp_millis({upper})"
    )
    if key_name:
        where += f" AND key_name = '{key_name}'"

    command = [
        str(sf_cli),
        "db",
        "--db-path",
        str(db_path),
        "query-rows",
        "llm_gateway_usage_events",
        "--where",
        where,
        "--columns",
        "id,key_name,account_name,status_code,model,endpoint,latency_ms,credit_usage_missing,usage_missing,created_at",
        "--limit",
        "5",
        "--format",
        "vertical",
    ]
    completed = subprocess.run(command, check=False)
    return completed.returncode


def main() -> int:
    args = parse_args()
    if args.command == "list":
        return list_errors(args.log_path, args.limit)
    if args.command == "usage":
        return query_usage(
            timestamp=args.timestamp,
            status=args.status,
            key_name=args.key_name,
            window_ms=args.window_ms,
            db_path=args.db_path,
            sf_cli=args.sf_cli,
        )
    print(f"unsupported command: {args.command}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
