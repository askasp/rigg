#!/usr/bin/env bash
#
# Start the bridge, or check the setup with --check.
#
# Tokens live in slack/.env (gitignored) so they do not have to be exported by
# hand every time, and do not end up in shell history.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -f slack/.env ]; then
  set -a
  # shellcheck disable=SC1091
  . slack/.env
  set +a
else
  echo "no slack/.env — copy slack/env.example to slack/.env and put your tokens in it" >&2
  exit 1
fi

exec uv run --with slack-bolt slack/rigg_slack.py "$@"
