#!/usr/bin/env bash
#
# The corpus as an MCP server, for a repo's `.rigg/front-mcp.json` to name:
#
#   { "mcpServers": { "front": { "command": "/home/aksel/git/rigg/mail/mcp.sh" } } }
#
# RIGG_INSTANCE picks the corpus. A run started from a Slack channel already
# has it; one started by hand falls back to `default`.
set -euo pipefail
cd "$(dirname "$0")/.."

: "${RIGG_INSTANCE:=default}"
export RIGG_INSTANCE

# One directory per instance. rigg exports these itself; this is for the
# standalone case.
inst_dir="$HOME/.rigg/instances/$RIGG_INSTANCE"
for f in "$inst_dir/secrets.env" "$inst_dir/secrets.local.env"; do
  if [ -f "$f" ]; then
    set -a
    # shellcheck disable=SC1090
    . "$f"
    set +a
  fi
done
: "${RIGG_MAIL_DIR:=$inst_dir/corpus}"
export RIGG_MAIL_DIR

# stdout is the protocol here, so uv must not chat on it.
exec uv run --quiet --with "requests~=2.34" --with "mcp~=2.2" mail/mail_mcp.py
