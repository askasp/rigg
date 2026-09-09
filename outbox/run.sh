#!/usr/bin/env bash
# Resolve whatever people have answered in Slack.
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

exec uv run --quiet poll.py "$@"
