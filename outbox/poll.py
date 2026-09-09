#!/usr/bin/env python3
"""Read what people said in Slack, act on the unambiguous parts.

    poll.py            resolve everything waiting
    poll.py --list     what is waiting, and what it is waiting for

Sends and drops happen here because they need no judgement. A redraft does,
so it is printed for a pipeline step to `capture` and hand to an agent -
and nothing is printed when there is none, so `when` skips that step and a
quiet poll costs no turn.
"""

from __future__ import annotations

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import outbox  # noqa: E402


def resolve_one(item: dict) -> tuple[str, dict]:
    reacts = outbox.reactions(item["channel"], item["ts"])
    msgs = outbox.replies(item["channel"], item["ts"])
    fresh = [
        m for m in msgs
        if m.get("ts", "") > item.get("seen_ts", "")
        and not m.get("bot_id")
        and m.get("subtype") != "bot_message"
    ]
    texts = [m.get("text", "") for m in fresh]
    high = max([m["ts"] for m in msgs], default=item.get("seen_ts", item["ts"]))
    action, payload = outbox.decide(reacts, texts)

    if action == "drop":
        outbox.update(item["id"], status="dropped", seen_ts=high)
        outbox.post("Dropped, nothing sent.", thread_ts=item["ts"], ch=item["channel"])
        return action, item
    if action in ("send", "verbatim"):
        text = payload if action == "verbatim" else item["draft"]
        it = outbox.send(item, text)
        outbox.update(item["id"], seen_ts=high)
        how = "Sent." if not it["mocked"] else "Would have sent this (no send token):"
        outbox.post(f"{how}\n\n{text}", thread_ts=item["ts"], ch=item["channel"])
        return action, it
    if action == "feedback":
        outbox.update(item["id"], status="redraft", feedback=payload, seen_ts=high)
        return action, item
    return "wait", item


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--list", action="store_true", help="show what is waiting")
    args = ap.parse_args()

    items = outbox.load()
    waiting = [i for i in items if i["status"] in ("pending", "redraft")]

    if args.list:
        if not waiting:
            print("nothing waiting")
            return 0
        for i in waiting:
            print(f"{i['id']}  {i['status']:<8} {i['subject'][:60]}  {i['conversation_id']}")
        return 0

    if not waiting:
        return 0

    counts: dict[str, int] = {}
    for item in waiting:
        if item["status"] == "redraft":
            continue
        try:
            action, _ = resolve_one(item)
        except outbox.SlackError as e:
            print(f"{item['id']}: {e}", file=sys.stderr)
            continue
        counts[action] = counts.get(action, 0) + 1
    for action, n in sorted(counts.items()):
        print(f"{n} {action}", file=sys.stderr)

    # Only the redrafts reach stdout: the pipeline captures this, and an empty
    # capture is what stops the agent step from running at all.
    for item in outbox.load():
        if item["status"] != "redraft":
            continue
        print(f"--- {item['id']}  {item['subject']}  ({item['conversation_id']})")
        print(f"they said: {item['feedback']}")
        print("current draft:")
        print(item["draft"])
        print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
