#!/usr/bin/env python3
"""Checks for the approval loop, against a fake Slack.

    ./outbox/test.sh

What can go wrong here is not subtle, it is expensive: a rule that reads a
question as approval sends a half-written sentence to a customer, and mail
has no undo. So every path through `decide` is asserted, and the send path is
asserted to be unreachable without an explicit signal.
"""

import json
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

TMP = tempfile.mkdtemp(prefix="rigg-outbox-test-")
os.environ["RIGG_HOME"] = TMP
os.environ["RIGG_INSTANCE"] = "test"
os.environ["RIGG_NOTIFY_CHANNEL"] = "C_TEST"
os.environ["SLACK_BOT_TOKEN"] = "xoxb-test"
os.environ.pop("FRONT_SEND", None)

import outbox  # noqa: E402
import poll  # noqa: E402

ok = 0


def check(label, cond, detail=""):
    global ok
    if not cond:
        print(f"FAIL  {label} {detail}", file=sys.stderr)
        sys.exit(1)
    ok += 1
    print(f"ok    {label}")


# --- a fake Slack -----------------------------------------------------------

class FakeSlack:
    def __init__(self):
        self.posts = []
        self.reactions = {}
        self.thread = {}
        self.n = 0

    def __call__(self, method, payload, get=False):
        if method == "chat.postMessage":
            self.n += 1
            ts = f"{1000 + self.n}.0000"
            self.posts.append({"ts": ts, "thread_ts": payload.get("thread_ts"),
                               "text": payload["text"]})
            return {"ok": True, "ts": ts}
        if method == "reactions.get":
            names = self.reactions.get(payload["timestamp"], set())
            return {"ok": True, "message": {"reactions": [{"name": n} for n in names]}}
        if method == "conversations.replies":
            root = {"ts": payload["ts"]}
            return {"ok": True, "messages": [root] + self.thread.get(payload["ts"], [])}
        raise AssertionError(method)

    def human_says(self, ts, text):
        self.n += 1
        self.thread.setdefault(ts, []).append(
            {"ts": f"{1000 + self.n}.0000", "text": text, "user": "U_HUMAN"}
        )

    def bot_says(self, ts, text):
        self.n += 1
        self.thread.setdefault(ts, []).append(
            {"ts": f"{1000 + self.n}.0000", "text": text, "bot_id": "B1"}
        )


fake = FakeSlack()
outbox._slack = fake


# --- the rules --------------------------------------------------------------

check("a bare thumbs up sends the draft",
      outbox.decide({"+1"}, []) == ("send", None))
check("no signal at all sends nothing",
      outbox.decide(set(), []) == ("wait", None))
check("a plain reply is feedback, never a send",
      outbox.decide(set(), ["can you make it shorter?"]) == ("feedback", "can you make it shorter?"))
check("an explicit send: is the words to send",
      outbox.decide(set(), ["send: Hei, det tar 3-4 dager."])
      == ("verbatim", "Hei, det tar 3-4 dager."))
check("send: wins over a thumbs up on the old draft",
      outbox.decide({"+1"}, ["send: my own words"])[0] == "verbatim")
check("an x drops it even with a thumbs up",
      outbox.decide({"+1", "x"}, ["send: anything"]) == ("drop", None))
check("the last word is the one that counts",
      outbox.decide(set(), ["shorter", "and warmer"]) == ("feedback", "and warmer"))
check("SEND: in capitals is still an explicit send",
      outbox.decide(set(), ["SEND: Hei"]) == ("verbatim", "Hei"))

# --- proposing --------------------------------------------------------------

item = outbox.propose("cnv_1", "Blodprøver", "kunde@example.com",
                      "Hei! Det tar 3-4 dager.", "a standard turnaround question")
check("a proposal posts the mail and the draft in its thread",
      len(fake.posts) == 2 and fake.posts[1]["thread_ts"] == item["ts"], str(fake.posts))
check("the thread explains what the reader can do",
      ":+1:" in fake.posts[1]["text"] and "send:" in fake.posts[1]["text"])
check("it is recorded as waiting",
      outbox.load()[0]["status"] == "pending")

try:
    outbox.propose("cnv_1", "Blodprøver", "kunde@example.com", "again")
    check("the same conversation is not proposed twice", False)
except ValueError as e:
    check("the same conversation is not proposed twice", "already waiting" in str(e))

# --- nothing happens until someone says so ----------------------------------

poll.main_argv = []
sys.argv = ["poll.py"]
poll.main()
check("a poll with no signal sends nothing",
      outbox.load()[0]["status"] == "pending" and not outbox.sent_log().exists())

# --- feedback redrafts, and still does not send -----------------------------

fake.human_says(item["ts"], "for langt, kort ned")
sys.argv = ["poll.py"]
poll.main()
after = outbox.load()[0]
check("feedback moves it to redraft", after["status"] == "redraft", after["status"])
check("feedback still sends nothing", not outbox.sent_log().exists())
check("the feedback is kept for the agent", after["feedback"] == "for langt, kort ned")

outbox.redraft(after["id"], "Hei! 3-4 dager.")
check("a redraft posts and waits again",
      outbox.load()[0]["status"] == "pending"
      and outbox.load()[0]["draft"] == "Hei! 3-4 dager.")

# --- the bot's own messages are not feedback --------------------------------

fake.bot_says(item["ts"], "New draft: ...")
sys.argv = ["poll.py"]
poll.main()
check("the bot does not answer itself",
      outbox.load()[0]["status"] == "pending")

# --- a thumbs up sends, and the send is mocked without a token --------------

fake.reactions[item["ts"]] = {"+1"}
sys.argv = ["poll.py"]
poll.main()
done = outbox.load()[0]
check("a thumbs up sends", done["status"] == "sent", done["status"])
check("the send is mocked without a send token", done["mocked"] is True)
check("what would have gone out is written down",
      json.loads(outbox.sent_log().read_text().strip())["text"] == "Hei! 3-4 dager.")
check("the thread says it was a dry run",
      "Would have sent" in fake.posts[-1]["text"], fake.posts[-1]["text"])
check("a sent item stops being polled",
      not [i for i in outbox.load() if i["status"] in ("pending", "redraft")])

# --- verbatim ---------------------------------------------------------------

it2 = outbox.propose("cnv_2", "Faktura", "b@example.com", "draft b")
fake.human_says(it2["ts"], "send: Hei, fakturaen er sendt på nytt.")
sys.argv = ["poll.py"]
poll.main()
sent = json.loads(outbox.sent_log().read_text().strip().splitlines()[-1])
check("send: sends the human's words, not the draft",
      sent["text"] == "Hei, fakturaen er sendt på nytt.", sent["text"])

# --- dropping ---------------------------------------------------------------

it3 = outbox.propose("cnv_3", "Spam", "spam@example.com", "draft c")
fake.reactions[it3["ts"]] = {"x"}
sys.argv = ["poll.py"]
poll.main()
check("an x drops it and sends nothing",
      outbox.load()[2]["status"] == "dropped"
      and len(outbox.sent_log().read_text().strip().splitlines()) == 2)

# --- sending for real needs both a flag and an author -----------------------

check("no send token means no real send", not outbox.sending_for_real())
os.environ["FRONT_SEND"] = "1"
os.environ["FRONT_API_TOKEN"] = "t"
check("a flag alone is not enough - an unsigned send arrives from nobody",
      not outbox.sending_for_real())
os.environ["FRONT_AUTHOR_ID"] = "a"
check("flag, token and author together are", outbox.sending_for_real())
os.environ.pop("FRONT_SEND")

# --- the server actually starts, and offers what it says it does ------------

import asyncio  # noqa: E402
import outbox_mcp as M  # noqa: E402

names = {t.name for t in asyncio.run(M.mcp.list_tools())}
check("the MCP server starts and registers its tools",
      {"propose_reply", "redraft", "post_digest", "waiting"} <= names, str(names))

check("sending is not a tool an agent can reach",
      not any("send" in n for n in names), str(names))

print(f"\n{ok} checks passed")
