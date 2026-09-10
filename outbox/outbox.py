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
import subprocess
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
    items = json.loads(text) if text else []
    # Items written before actions existed were all Front replies.
    for i in items:
        i.setdefault("action", "front-mail")
        i.setdefault("key", i.get("conversation_id", i["id"]))
        i.setdefault("title", i.get("subject", ""))
        i.setdefault("params", {"conversation_id": i.get("conversation_id", "")})
        if i.get("status") == "sent":
            i["status"] = "done"
    return items


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


def digest(arrived: int, items: list[dict], ignored: str = "") -> str:
    """The morning summary, rendered here rather than written by an agent.

    A digest is skimmed on a phone before anything else is opened, so its job
    is to be scannable, not complete. Given free text a model writes prose -
    counts restated in a sentence, a numbered list, a closing paragraph - and
    the two lines that matter get buried. Taking the parts and doing the
    writing here is what keeps it one glance.
    """
    need = [i for i in items if not i.get("drafted")]
    drafted = [i for i in items if i.get("drafted")]
    head = f"*Siste {arrived} inn* · {len(drafted)} utkast · {len(need)} til deg"
    if not items:
        head = f"*Siste {arrived} inn* · ingenting som trenger deg"

    lines = [head, ""]
    for i in drafted + need:
        mark = "✏️" if i.get("drafted") else "🔴"
        who = (i.get("who") or "ukjent").strip()
        what = " ".join((i.get("what") or "").split())[:110]
        note = " ".join((i.get("note") or "").split())[:60]
        lines.append(f"{mark} *{who}* — {what}")
        tail = f"`{i.get('conversation_id', '?')}`"
        if note:
            tail += f" · _{note}_"
        lines.append(f"      {tail}")
    if ignored:
        lines += ["", f"_{' '.join(ignored.split())[:160]}_"]
    return "\n".join(lines).strip()


# ---------------------------------------------------------------- proposing

def propose(action: str, key: str, title: str, draft: str,
            why: str = "", params: dict | None = None,
            subtitle: str = "") -> dict:
    """Post something for a person to approve, and remember we are waiting.

    `action` names an [approvals.*] block in the repo's config - what happens
    on approval. `key` is what makes it idempotent: the same key cannot be
    waiting twice.
    """
    known = approvals()
    if known and action not in known:
        raise ValueError(
            f"no [approvals.{action}] in this repo's config; "
            f"there is {', '.join(sorted(known)) or 'none'}"
        )
    items = load()
    if any(i["key"] == key and i["status"] in ("pending", "redraft") for i in items):
        raise ValueError(f"{key} is already waiting on someone")
    head = f"*{title}*"
    if subtitle:
        head += f"\n{subtitle}"
    head += f"  ·  `{key}`"
    if why:
        head += f"\n_{why}_"
    ch = channel()
    ts = post(head, ch=ch)
    post(f"Suggested {action}:\n\n{draft}\n\n{LEGEND}", thread_ts=ts, ch=ch)
    item = {
        "id": new_id(items),
        "action": action,
        "key": key,
        "title": title,
        "params": params or {},
        "channel": ch,
        "ts": ts,
        "draft": draft,
        "status": "pending",
        "feedback": None,
        "seen_ts": ts,
        "created": time.strftime("%Y-%m-%d %H:%M"),
        "done_at": None,
        "mocked": None,
    }
    items.append(item)
    save(items)
    return item


def redraft(item_id: str, draft: str) -> dict:
    it = update(item_id, draft=draft, status="pending", feedback=None)
    post(f"New draft:\n\n{draft}\n\n{LEGEND}", thread_ts=it["ts"], ch=it["channel"])
    return it


# ---------------------------------------------------------------- executing

def config_path() -> Path | None:
    """The repo's config, found from where the poll is running."""
    for c in (Path(".rigg/rigg.toml"), Path("rigg.toml")):
        if c.is_file():
            return c
    return None


def approvals() -> dict[str, dict]:
    """What this repo says may be approved, and what happens when it is.

    Actions are capability, so they live in the repo and are reviewed. Adding
    one is a config change - no code here knows what any of them do.
    """
    p = config_path()
    if p is None:
        return {}
    try:
        import tomllib
        return tomllib.loads(p.read_text()).get("approvals", {})
    except Exception:
        return {}


def blocked(spec: dict) -> str | None:
    """Why this action cannot run for real, if it cannot."""
    if not spec.get("run"):
        return "it has no `run`"
    missing = [n for n in spec.get("needs", []) if not os.environ.get(n, "").strip()]
    if missing:
        verb = "is" if len(missing) == 1 else "are"
        return f"{', '.join(missing)} {verb} not set for instance `{instance()}`"
    return None


def execute(item: dict, text: str) -> dict:
    """Do the thing, or write down what would have been done.

    Mocked whenever the action's `needs` are not all present, which is the
    same seam `rigg doctor` reads - so an approval that cannot really run says
    so before anyone clicks, rather than after.
    """
    spec = approvals().get(item.get("action", ""), {})
    why = blocked(spec)
    if why is None:
        env = dict(os.environ)
        env["RIGG_APPROVAL_ID"] = item["id"]
        env["RIGG_APPROVAL_ACTION"] = item["action"]
        env["RIGG_APPROVAL_KEY"] = item["key"]
        env["RIGG_APPROVAL_TITLE"] = item.get("title", "")
        for k, v in (item.get("params") or {}).items():
            env[f"RIGG_APPROVAL_{k.upper()}"] = str(v)
        r = subprocess.run(
            spec["run"], shell=True, input=text, text=True, env=env,
            capture_output=True, timeout=120,
        )
        if r.returncode != 0:
            raise RuntimeError(
                f"{item['action']} failed ({r.returncode}): "
                f"{(r.stderr or r.stdout).strip()[:300]}"
            )
        mocked = False
    else:
        p = sent_log()
        p.parent.mkdir(parents=True, exist_ok=True)
        with p.open("a") as fh:
            fh.write(json.dumps({
                "at": time.strftime("%Y-%m-%d %H:%M:%S"),
                "action": item.get("action"),
                "key": item["key"],
                "title": item.get("title"),
                "text": text,
                "why_mocked": why,
            }, ensure_ascii=False) + "\n")
        mocked = why
    return update(
        item["id"],
        status="done",
        draft=text,
        done_at=time.strftime("%Y-%m-%d %H:%M"),
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
