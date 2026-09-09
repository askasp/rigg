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
cd "$(dirname "$0")"

inst=""
if [ $# -gt 0 ] && [ "${1#-}" = "$1" ]; then
  inst="$1"
  shift
fi
export RIGG_INSTANCE="${inst:-default}"

# Tokens for everything that is not Slack live in one file per instance,
# outside the repo, because the MCP server an agent starts reads the same one.
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

exec uv run --quiet --with "requests~=2.34" --with "mail-parser-reply~=1.36" \
  sync.py "$@"
