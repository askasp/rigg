#!/usr/bin/env bash
#
# Sync this instance's mail corpus.
#
#   ./mail/run.sh                    # default instance, everything new
#   ./mail/run.sh acme --backfill    # acme's first import
#   ./mail/run.sh acme --rebuild     # re-derive pairs, no network
#
# Instances are the ones `slack/run.sh` names: one workspace, one corpus.
set -euo pipefail
cd "$(dirname "$0")/.."

inst=""
if [ $# -gt 0 ] && [ "${1#-}" = "$1" ]; then
  inst="$1"
  shift
fi
export RIGG_INSTANCE="${inst:-default}"

# Tokens for everything that is not Slack live in one file per instance,
# outside the repo, because the MCP server an agent starts reads the same one.
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

exec uv run --quiet --with "requests~=2.34" --with "mail-parser-reply~=1.36" \
  mail/sync.py "$@"
