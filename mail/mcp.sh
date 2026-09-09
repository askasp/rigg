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

secrets="$HOME/.rigg/secrets/$RIGG_INSTANCE.env"
if [ -f "$secrets" ]; then
  set -a
  # shellcheck disable=SC1091
  . "$secrets"
  set +a
fi
# Set from Slack, and not templated by Ansible, so it survives a converge and
# wins on a clash.
if [ -f "${secrets%.env}.local.env" ]; then
  set -a
  # shellcheck disable=SC1090
  . "${secrets%.env}.local.env"
  set +a
fi

# stdout is the protocol here, so uv must not chat on it.
exec uv run --quiet --with "requests~=2.34" --with "mcp~=2.2" mail/mail_mcp.py
