#!/usr/bin/env bash
#
# Run a rigg command with an instance's environment.
#
#   cron.sh <instance> tick                 # what a crontab line calls
#   cron.sh <instance> <repo> <rigg args>   # anything else, in that repo
#
# cron hands a process almost nothing: no PATH to uv or the agent, and none of
# the instance's tokens. This supplies both.
set -euo pipefail

usage() { echo "usage: cron.sh <instance> [repo] <rigg args...>" >&2; exit 2; }
[ $# -ge 2 ] || usage
inst="$1"; shift

repo=""
if [ -d "$1" ]; then repo="$1"; shift; fi
[ $# -ge 1 ] || usage

here="$(cd "$(dirname "$0")" && pwd)"
export RIGG_INSTANCE="$inst"
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:/snap/bin:$PATH"

# rigg loads the instance's secrets itself; these are for the shell steps and
# MCP servers it starts before that takes effect.
inst_dir="$HOME/.rigg/instances/$inst"
for f in "$inst_dir/secrets.env" "$inst_dir/secrets.local.env"; do
  if [ -f "$f" ]; then
    set -a
    # shellcheck disable=SC1090
    . "$f"
    set +a
  fi
done

[ -n "$repo" ] && cd "$repo"
exec "${RIGG_BIN:-$here/target/release/rigg}" "$@"
