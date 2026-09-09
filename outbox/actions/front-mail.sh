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
here="$(cd "$(dirname "$0")/../.." && pwd)"

cid="${RIGG_APPROVAL_CONVERSATION_ID:?no conversation id on the approval}"
body="$(cat)"

# Refuse to reply outside the inbox this run was scoped to. The draft was
# written from what that scope could see, so sending it elsewhere is a reply
# in the wrong voice at best and to the wrong customer at worst.
if [ -n "${RIGG_VAR_INBOX:-}" ]; then
  CID="$cid" MAILDIR="$here/mail" uv run --quiet python -c '
import os, sys
sys.path.insert(0, os.environ["MAILDIR"])
import corpus
cid = os.environ["CID"]
if not corpus.in_scope(corpus.connect(corpus.db_path()), cid):
    sys.exit(cid + " is not in the scoped inbox " + str(corpus.scope()))
'
fi

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
