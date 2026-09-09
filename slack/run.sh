#!/usr/bin/env bash
#
# Start the bridge, or check the setup with --check.
#
#   ./slack/run.sh                # slack/.env     + slack/channels.toml
#   ./slack/run.sh acme           # slack/acme.env + slack/acme-channels.toml
#   ./slack/run.sh acme --check
#
# An instance is a Slack workspace: its own app, its own tokens, its own
# channels, its own repos. Run one process per workspace. Tokens live in the
# instance's env file (gitignored) so they do not have to be exported by hand
# every time, and do not end up in shell history.
set -euo pipefail
cd "$(dirname "$0")/.."

# A leading `-` is a flag for the bridge, not a name, so `run.sh --check` still
# checks the unnamed instance.
inst=""
if [ $# -gt 0 ] && [ "${1#-}" = "$1" ]; then
  inst="$1"
  shift
fi

# `default` is what the unnamed instance is called once it needs a name -
# RIGG_INSTANCE, the corpus filename, cron.sh's argument - so accept it here
# too rather than hunting for a slack/default.env nobody wrote.
[ "$inst" = "default" ] && inst=""

env_file="slack/$inst.env"
channels="slack/${inst:+$inst-}channels.toml"

# Tokens for everything that is not Slack - the Front API, say - live in one
# file per instance outside the repo, so a run started from a channel and one
# started by a timer reach the same services. Optional: no file, no bother.
inst_dir="$HOME/.rigg/instances/${inst:-default}"
for f in "$inst_dir/secrets.env" "$inst_dir/secrets.local.env"; do
  if [ -f "$f" ]; then
    set -a
    # shellcheck disable=SC1090
    . "$f"
    set +a
  fi
done

if [ ! -f "$env_file" ]; then
  echo "no $env_file — copy slack/env.example to it and put ${inst:-this workspace}'s tokens in it" >&2
  exit 1
fi

set -a
# shellcheck disable=SC1091
. "$env_file"
set +a

# Which workspace this is, for everything downstream: rigg inherits its whole
# environment, so the agent, its shell steps and any MCP server they start all
# see it. An unnamed instance is `default` rather than empty, so a consumer can
# use it as a filename without a special case.
export RIGG_INSTANCE="${inst:-default}"

exec uv run --with "slack-bolt~=1.30" slack/rigg_slack.py "$channels" "$@"
