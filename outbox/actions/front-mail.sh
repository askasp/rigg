#!/usr/bin/env bash
# Reply to a Front conversation. The draft arrives on stdin.
#
#   [approvals.front-mail]
#   run = ".../outbox/actions/front-mail.sh"
#   needs = ["FRONT_API_TOKEN", "FRONT_AUTHOR_ID"]
#
# FRONT_AUTHOR_ID is in `needs` rather than defaulted: a message attributed to
# the API token arrives from nobody.
set -euo pipefail

cid="${RIGG_APPROVAL_CONVERSATION_ID:?no conversation id on the approval}"
body="$(cat)"

payload=$(BODY="$body" AUTHOR="$FRONT_AUTHOR_ID" python3 -c '
import json, os
print(json.dumps({"body": os.environ["BODY"], "author_id": os.environ["AUTHOR"]}))')

code=$(curl -sS -o /tmp/front-send.$$ -w '%{http_code}' \
  -X POST "https://api2.frontapp.com/conversations/$cid/messages" \
  -H "Authorization: Bearer $FRONT_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d "$payload")
if [ "$code" -ge 300 ]; then
  echo "Front $code: $(head -c 300 /tmp/front-send.$$)" >&2
  rm -f /tmp/front-send.$$
  exit 1
fi
rm -f /tmp/front-send.$$
echo "sent to $cid"
