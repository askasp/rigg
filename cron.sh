#!/usr/bin/env bash
#
# Run a rigg command on a schedule, with an instance's environment.
#
#   cron.sh <instance> <repo> <rigg args...>
#
# A crontab line then reads as the command you would have typed:
#
#   0 7 * * * /home/aksel/git/rigg/cron.sh amino ~/git/amino-monorepo \
#             new morning-mail "draft replies to anything unanswered overnight"
#
# cron hands a process almost nothing: no PATH to uv or the agent, none of the
# instance's tokens, and no Slack thread to report progress in. This supplies
# all three. Set RIGG_NOTIFY_CHANNEL in the instance's env file and the run
# reports there, since there is no thread for the notify hook to reply in.
#
# A pipeline with a `confirm` step cannot run unattended — rigg says so rather
# than skipping the step — so schedule one that has none.
#
# Under a systemd timer, prefer `run --pipeline <name>` (foreground) or add
# `--fg` to `new`. `new` detaches under setsid, and a Type=oneshot unit takes
# its whole cgroup down the moment this script exits — killing the run it just
# started. Plain crontab does not do that, but the flag is harmless there.
set -euo pipefail

usage() { echo "usage: cron.sh <instance> <repo> <rigg args...>" >&2; exit 2; }
[ $# -ge 3 ] || usage
inst="$1"; repo="$2"; shift 2

here="$(cd "$(dirname "$0")" && pwd)"
export RIGG_INSTANCE="$inst"

# cron's PATH is /usr/bin:/bin and nothing else.
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:/snap/bin:$PATH"

# The unnamed instance is `default` here but `slack/.env` on disk, the same
# spelling `slack/run.sh` uses.
slack_env="$here/slack/$inst.env"
[ "$inst" = "default" ] && slack_env="$here/slack/.env"

for f in "$HOME/.rigg/secrets/$inst.env" \
         "$HOME/.rigg/secrets/$inst.local.env" \
         "$slack_env"; do
  if [ -f "$f" ]; then
    set -a
    # shellcheck disable=SC1090
    . "$f"
    set +a
  fi
done

cd "$repo"
exec "${RIGG_BIN:-$here/target/release/rigg}" "$@"
