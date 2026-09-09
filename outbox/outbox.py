"""Proposals waiting on a human, and what a human's answer means.

An agent drafts; a person in Slack decides. The state that outlives both is
instance state, so it lives beside the schedules and the tokens:

    ~/.rigg/instances/<instance>/outbox.json

Nothing here sends without an explicit signal. A plain reply is feedback, not
an instruction to send - getting that the wrong way round sends a half-written
sentence to a customer, and there is no undo on mail.
"""

from __future__ import annotations

import json
import os
import time
import urllib.parse
import urllib.request
from pathlib import Path

SEND_REACTIONS = {"+1", "thumbsup", "white_check_mark", "heavy_check_mark"}
DROP_REACTIONS = {"x", "no_entry", "no_entry_sign", "wastebasket"}
VERBATIM_PREFIX = "send:"

LEGEND = (
    ":+1: send this draft   ·   reply with feedback and I redraft   ·   "
    "reply `send: <text>` to send your own words   ·   :x: drop it"
)


def instance() -> str:
    return os.environ.get("RIGG_INSTANCE") or "default"


def _dir() -> Path:
    root = Path(os.environ.get("RIGG_HOME") or (Path.home() / ".rigg"))
    return root / "instances" / instance()


def store_path() -> Path:
    return _dir() / "outbox.json"


def sent_log() -> Path:
    return _dir() / "outbox-sent.log"


def load() -> list[dict]:
    p = store_path()
    if not p.exists():
        return []
    text = p.read_text().strip()
    return json.loads(text) if text else []


def save(items: list[dict]) -> None:
    p = store_path()
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(items, indent=2, ensure_ascii=False) + "\n")
    tmp.replace(p)


def find(items: list[dict], item_id: str) -> dict | None:
    return next((i for i in items if i["id"] == item_id), None)


def update(item_id: str, **fields) -> dict:
    items = load()
    it = find(items, item_id)
    if it is None:
        raise KeyError(f"no outbox item {item_id}")
    it.update(fields)
    save(items)
    return it


def new_id(items: list[dict]) -> str:
    n = 1
    while any(i["id"] == f"ob{n:03d}" for i in items):
        n += 1
    return f"ob{n:03d}"


# --------------------------------------------------------------------- Slack

class SlackError(RuntimeError):
    pass


def _slack(method: str, payload: dict, get: bool = False) -> dict:
    token = os.environ.get("SLACK_BOT_TOKEN", "").strip()
    if not token:
        raise SlackError(
            "SLACK_BOT_TOKEN is not set for instance "
            f"`{instance()}` - rigg secret set SLACK_BOT_TOKEN <value>"
        )
    url = f"https://slack.com/api/{method}"
    if get:
        url += "?" + urllib.parse.urlencode(payload)
        req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    else:
        req = urllib.request.Request(
            url,
            data=json.dumps(payload).encode(),
            headers={
                "Authorization": f"Bearer {token}",
                "Content-Type": "application/json; charset=utf-8",
            },
        )
    with urllib.request.urlopen(req, timeout=30) as r:
        out = json.load(r)
    if not out.get("ok"):
        raise SlackError(_explain(method, out.get("error", "")))
    return out


NEEDED = {
    "reactions.get": "reactions:read",
    "conversations.replies": "groups:history (private channels) or channels:history",
    "chat.postMessage": "chat:write",
}


def _explain(method: str, error: str) -> str:
    """A missing scope is a five-minute fix in the Slack app, but only if the
    message names the scope rather than saying missing_scope."""
    if error == "missing_scope" and method in NEEDED:
        return (
            f"{method}: the bot token is missing {NEEDED[method]}. Add it under "
            "OAuth & Permissions, reinstall the app, then "
            "`rigg secret set SLACK_BOT_TOKEN <new token>`"
        )
    if error == "not_in_channel":
        return f"{method}: invite the bot to the channel first"
    return f"{method}: {error}"


def channel() -> str:
    ch = os.environ.get("RIGG_NOTIFY_CHANNEL", "").strip()
    if not ch:
        raise SlackError(
            "RIGG_NOTIFY_CHANNEL is not set - rigg secret set RIGG_NOTIFY_CHANNEL <id>"
        )
    return ch


def post(text: str, thread_ts: str | None = None, ch: str | None = None) -> str:
    payload = {"channel": ch or channel(), "text": text, "unfurl_links": False}
    if thread_ts:
        payload["thread_ts"] = thread_ts
    return _slack("chat.postMessage", payload)["ts"]


def reactions(ch: str, ts: str) -> set[str]:
    try:
        out = _slack("reactions.get", {"channel": ch, "timestamp": ts}, get=True)
    except SlackError as e:
        if "no_reaction" in str(e):
            return set()
        raise
    msg = out.get("message") or {}
    return {r["name"] for r in msg.get("reactions", [])}


def replies(ch: str, ts: str) -> list[dict]:
    out = _slack("conversations.replies", {"channel": ch, "ts": ts, "limit": 100}, get=True)
    return out.get("messages", [])[1:]


# ---------------------------------------------------------------- proposing

def propose(conversation_id: str, subject: str, sender: str, draft: str,
            why: str = "") -> dict:
    """Post a mail and its suggested reply, and remember we are waiting."""
    items = load()
    if any(i["conversation_id"] == conversation_id and i["status"] in ("pending", "redraft")
           for i in items):
        raise ValueError(f"{conversation_id} is already waiting on someone")
    head = f"*{subject}*\nfrom {sender}  ·  `{conversation_id}`"
    if why:
        head += f"\n_{why}_"
    ch = channel()
    ts = post(head, ch=ch)
    post(f"Suggested reply:\n\n{draft}\n\n{LEGEND}", thread_ts=ts, ch=ch)
    item = {
        "id": new_id(items),
        "conversation_id": conversation_id,
        "subject": subject,
        "sender": sender,
        "channel": ch,
        "ts": ts,
        "draft": draft,
        "status": "pending",
        "feedback": None,
        "seen_ts": ts,
        "created": time.strftime("%Y-%m-%d %H:%M"),
        "sent_at": None,
        "mocked": None,
    }
    items.append(item)
    save(items)
    return item


def redraft(item_id: str, draft: str) -> dict:
    it = update(item_id, draft=draft, status="pending", feedback=None)
    post(f"New draft:\n\n{draft}\n\n{LEGEND}", thread_ts=it["ts"], ch=it["channel"])
    return it


# ------------------------------------------------------------------ sending

def sending_for_real() -> bool:
    return (
        os.environ.get("FRONT_SEND") == "1"
        and bool(os.environ.get("FRONT_API_TOKEN", "").strip())
        and bool(os.environ.get("FRONT_AUTHOR_ID", "").strip())
    )


def send(item: dict, text: str) -> dict:
    """Send a reply, or record what would have been sent.

    Mocked unless FRONT_SEND=1 with a token and an author. A send that is
    attributed to the API token rather than a person arrives unsigned, which
    is why the author is required rather than defaulted.
    """
    if sending_for_real():
        url = f"https://api2.frontapp.com/conversations/{item['conversation_id']}/messages"
        body = json.dumps({
            "body": text,
            "author_id": os.environ["FRONT_AUTHOR_ID"],
        }).encode()
        req = urllib.request.Request(
            url,
            data=body,
            headers={
                "Authorization": f"Bearer {os.environ['FRONT_API_TOKEN']}",
                "Content-Type": "application/json",
            },
        )
        with urllib.request.urlopen(req, timeout=30):
            pass
        mocked = False
    else:
        p = sent_log()
        p.parent.mkdir(parents=True, exist_ok=True)
        with p.open("a") as fh:
            fh.write(json.dumps({
                "at": time.strftime("%Y-%m-%d %H:%M:%S"),
                "conversation_id": item["conversation_id"],
                "subject": item["subject"],
                "text": text,
            }, ensure_ascii=False) + "\n")
        mocked = True
    return update(
        item["id"],
        status="sent",
        draft=text,
        sent_at=time.strftime("%Y-%m-%d %H:%M"),
        mocked=mocked,
    )


# ----------------------------------------------------------------- resolving

def decide(reacts: set[str], new_replies: list[str]) -> tuple[str, str | None]:
    """What a human's signals mean. Pure, so the rules can be asserted.

    Returns (action, payload) where action is drop, send, verbatim, feedback
    or wait. Nothing sends on a plain reply: that is feedback, and the draft
    it produces still has to be approved.
    """
    if reacts & DROP_REACTIONS:
        return "drop", None
    verbatim = [r for r in new_replies if r.strip().lower().startswith(VERBATIM_PREFIX)]
    if verbatim:
        return "verbatim", verbatim[-1].strip()[len(VERBATIM_PREFIX):].strip()
    if reacts & SEND_REACTIONS:
        return "send", None
    if new_replies:
        return "feedback", new_replies[-1].strip()
    return "wait", None
