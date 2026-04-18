#!/usr/bin/env bash
#
# Trigger a hot-reload of the codex-bridge model-capability registry.
#
# The bridge reads `.run/default/config/model_capabilities.toml` fresh,
# rebuilds the registry, and re-renders the "Available model
# capabilities" block injected into the Codex system prompt. Existing
# Codex threads keep their already-embedded prompt until they next
# resume; new threads see the updated block immediately.
#
# Usage:
#   scripts/reload-capabilities.sh              # hit the default bind
#   CODEX_BRIDGE_API_BIND=host:port scripts/reload-capabilities.sh
#   scripts/reload-capabilities.sh --help
#
# Requires: bash, curl. `jq` is used for pretty-printing when available.
set -euo pipefail

API_BIND="${CODEX_BRIDGE_API_BIND:-127.0.0.1:36111}"

usage() {
  cat <<EOF
Usage: $(basename "$0") [--help]

Re-parses model_capabilities.toml on the running codex-bridge via its
local API. No arguments beyond --help are accepted.

Environment:
  CODEX_BRIDGE_API_BIND   override the API bind (default: 127.0.0.1:36111)
EOF
}

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
  "")
    ;;
  *)
    echo "error: unexpected argument: $1" >&2
    usage >&2
    exit 2
    ;;
esac

url="http://${API_BIND}/api/capability/reload"
response="$(mktemp)"
trap 'rm -f "$response"' EXIT

http_status="$(
  curl --silent --show-error \
       --output "$response" \
       --write-out "%{http_code}" \
       --request POST \
       --header 'content-type: application/json' \
       "$url" \
  || true
)"

if [[ -z "$http_status" || "$http_status" == "000" ]]; then
  echo "error: could not reach codex-bridge at $url" >&2
  exit 1
fi

if command -v jq >/dev/null 2>&1; then
  jq . "$response"
else
  cat "$response"
  echo
fi

if [[ "$http_status" =~ ^2[0-9][0-9]$ ]]; then
  exit 0
fi

echo "error: reload failed with HTTP $http_status" >&2
exit 1
