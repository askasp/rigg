#!/usr/bin/env bash
#
# rigg notify hook: posts a run's progress into the Slack thread that started
# it. Wire it up in the repo's rigg.toml:
#
#   [notify]
#   command = "/home/aksel/git/rigg/slack/notify.sh"
#
# rigg passes everything as RIGG_* environment variables, so nothing here has
# to be quoted. SLACK_BOT_TOKEN is inherited from the bridge process that
# started the run; a run you start by hand has none and this exits quietly.
set -euo pipefail

# Overridable so the hook can be exercised against a local server.
API="${SLACK_API_URL:-https://slack.com/api/chat.postMessage}"

[ -n "${RIGG_BRANCH:-}" ] || exit 0
[ -n "${SLACK_BOT_TOKEN:-}" ] || exit 0

common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || exit 0
state="$common/rigg/slack/${RIGG_BRANCH//\//-}.json"

channel=""
thread=""
if [ -f "$state" ]; then
  channel=$(jq -r '.channel // empty' "$state")
  thread=$(jq -r '.thread_ts // empty' "$state")
fi

# A run nobody started from Slack - a timer, or one started in a terminal - has
# no thread to reply in, and used to report nowhere at all. Name a channel and
# it reports there instead, as a new message.
[ -n "$channel" ] || channel="${RIGG_NOTIFY_CHANNEL:-}"
[ -n "$channel" ] || exit 0

# A run started from Slack has a thread, and progress there is what somebody
# asked for. A run nobody started - a timer - has none, and "main started -
# 1 step(s)" in a channel is noise nobody reads, which is how the one message
# that matters gets missed. So without a thread, only failure is worth saying;
# what a scheduled run has to report, it reports itself.
if [ -z "$thread" ]; then
  if [ "${RIGG_EVENT:-}" != "failed" ] && [ "${RIGG_STATUS:-}" != "failed" ]; then
    exit 0
  fi
  # rigg's own line is written for a run log next to the steps around it -
  # "main [2/2] draft failed" says nothing in a channel, where "main" is
  # whatever branch the box happens to be on. Compose one that stands alone.
  repo=$(basename "${RIGG_REPO:-the repo}")
  step="${RIGG_STEP:-a step}"
  at=""
  [ -n "${RIGG_INDEX:-}" ] && at=" (step $RIGG_INDEX of ${RIGG_TOTAL:-?})"
  took=""
  [ -n "${RIGG_ELAPSED:-}" ] && took=" after $RIGG_ELAPSED"
  RIGG_MESSAGE="⚠️ scheduled run in \`$repo\` failed at \`$step\`$at$took"
  if [ -n "${RIGG_TASK:-}" ]; then
    RIGG_MESSAGE="$RIGG_MESSAGE
task: ${RIGG_TASK}"
  fi
fi

# A failing post must not fail the run, so every error here is swallowed.
curl -sf -X POST "$API" \
  -H "Authorization: Bearer $SLACK_BOT_TOKEN" \
  -H 'Content-type: application/json; charset=utf-8' \
  --data "$(jq -n --arg c "$channel" --arg t "$thread" --arg x "${RIGG_MESSAGE:-}" \
           '{channel:$c, text:$x} + (if $t == "" then {} else {thread_ts:$t} end)')" \
  >/dev/null 2>&1 || true
