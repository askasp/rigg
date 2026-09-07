#!/usr/bin/env bash
#
# Put a file into the Slack thread a branch reports in.
#
#   RIGG_BRANCH=<branch> slack/upload.sh screenshot.png ["a caption"]
#
# For a pipeline step that produces artefacts - Playwright screenshots, a
# coverage report - so they arrive where the run is already being watched
# rather than only on disk.
#
# Uses the external-upload flow: files.upload is deprecated and being removed.
set -euo pipefail

API="${SLACK_API_BASE:-https://slack.com/api}"
file="${1:?usage: upload.sh <file> [comment]}"
comment="${2:-}"

[ -f "$file" ] || { echo "no such file: $file" >&2; exit 1; }
[ -n "${SLACK_BOT_TOKEN:-}" ] || { echo "SLACK_BOT_TOKEN is not set" >&2; exit 0; }
[ -n "${RIGG_BRANCH:-}" ] || { echo "RIGG_BRANCH is not set" >&2; exit 0; }

common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || exit 0
state="$common/rigg/slack/${RIGG_BRANCH//\//-}.json"
[ -f "$state" ] || { echo "no slack thread recorded for $RIGG_BRANCH" >&2; exit 0; }

channel=$(jq -r '.channel // empty' "$state")
thread=$(jq -r '.thread_ts // empty' "$state")
[ -n "$channel" ] || exit 0

name=$(basename "$file")
size=$(wc -c < "$file" | tr -d ' ')

# 1. Ask for somewhere to put it.
up=$(curl -sf -G "$API/files.getUploadURLExternal" \
  -H "Authorization: Bearer $SLACK_BOT_TOKEN" \
  --data-urlencode "filename=$name" --data-urlencode "length=$size")
[ "$(jq -r .ok <<<"$up")" = true ] || { echo "getUploadURL failed: $(jq -r .error <<<"$up")" >&2; exit 1; }
url=$(jq -r .upload_url <<<"$up")
id=$(jq -r .file_id <<<"$up")

# 2. Put it there.
curl -sf -X POST "$url" -F "file=@$file" --output /dev/null

# 3. Say where it belongs. Without a channel it uploads but is visible nowhere.
done_args=$(jq -n --arg id "$id" --arg t "$name" --arg c "$channel" \
  --arg th "$thread" --arg m "$comment" \
  '{files: [{id: $id, title: $t}], channel_id: $c}
   + (if $th == "" then {} else {thread_ts: $th} end)
   + (if $m  == "" then {} else {initial_comment: $m} end)')
res=$(curl -sf -X POST "$API/files.completeUploadExternal" \
  -H "Authorization: Bearer $SLACK_BOT_TOKEN" \
  -H 'Content-type: application/json; charset=utf-8' --data "$done_args")
[ "$(jq -r .ok <<<"$res")" = true ] || { echo "upload failed: $(jq -r .error <<<"$res")" >&2; exit 1; }
echo "uploaded $name"
