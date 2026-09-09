#!/usr/bin/env bash
# The outbox as an MCP server, for a repo's .rigg/mcp/*.json to name.
set -euo pipefail
cd "$(dirname "$0")"

: "${RIGG_INSTANCE:=default}"
export RIGG_INSTANCE
inst_dir="$HOME/.rigg/instances/$RIGG_INSTANCE"
for f in "$inst_dir/secrets.env" "$inst_dir/secrets.local.env"; do
  if [ -f "$f" ]; then
    set -a
    # shellcheck disable=SC1090
    . "$f"
    set +a
  fi
done

# stdout is the protocol here, so uv must not chat on it.
exec uv run --quiet --with "mcp~=2.2" outbox_mcp.py
