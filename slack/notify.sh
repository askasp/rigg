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
[ -f "$state" ] || exit 0

channel=$(jq -r '.channel // empty' "$state")
thread=$(jq -r '.thread_ts // empty' "$state")
[ -n "$channel" ] && [ -n "$thread" ] || exit 0

# A failing post must not fail the run, so every error here is swallowed.
curl -sf -X POST "$API" \
  -H "Authorization: Bearer $SLACK_BOT_TOKEN" \
  -H 'Content-type: application/json; charset=utf-8' \
  --data "$(jq -n --arg c "$channel" --arg t "$thread" --arg x "${RIGG_MESSAGE:-}" \
           '{channel:$c, thread_ts:$t, text:$x}')" >/dev/null 2>&1 || true
