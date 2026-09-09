#!/usr/bin/env python3
"""Front -> the corpus.

Walks Front for (what someone asked, what we sent back) pairs and stores them
in this instance's corpus, so an agent drafting a reply can be shown how
similar ones were answered before.

    mail/run.sh                    # everything new since last time
    mail/run.sh acme --backfill    # the first import, walking inboxes
    mail/run.sh acme --rebuild     # re-derive pairs from stored messages

`--rebuild` touches no network. It exists because the cleaning below is the
part most likely to need improving, and re-deriving is seconds where
re-fetching is hours at 50 requests a minute.
"""

import argparse
import os
import re
from html.parser import HTMLParser
import sys
import time
from datetime import datetime, timezone

import requests
from mailparser_reply import EmailReplyParser
from mailparser_reply.constants import MAIL_LANGUAGES

import corpus

# ---------------------------------------------------------------------------
# cleaning

# mailparser-reply ships Danish and Swedish but not Norwegian. Bokmål is close
# enough to Danish that this is that entry with the words changed: `skrev`
# survives, the Outlook header words and the sign-offs do not. Longer sign-offs
# come first so `Med vennlig hilsen` is not cut short by `Hilsen`.
MAIL_LANGUAGES["no"] = {
    "wrote_header": r"^(?!(?:Den|På)[.\s]*(?:Den|På)\s(.+?)\sskrev:)"
    r"((?:> ?)*(?:Den|På|[Mm]an|[Tt]ir|[Oo]ns|[Tt]or|[Ff]re|[Ll]ør|[Ss]øn|[0-9])"
    r".+?\sskrev\s.+?:)$",
    "from_header": r"((?:(?:^|\n|\n(?:> ?)*)?-----Opprinnelig melding-----\n)?"
    r"(?:(?:^|\n|\n(?:> ?)*)[* ]*"
    r"(?:[Ff]ra|[Ss]endt|[Tt]il|[Ee]mne|[Dd]ato|[Cc]c|[Kk]opi|[Vv]edlegg):"
    r"[ *]*(?:\s{,2}).*){2,}(?:\n.*){,1})",
    "disclaimers": ["Advarsel:", "Merk:", "Konfidensielt:"],
    "signatures": [
        "Med vennlig hilsen",
        "Vennlig hilsen",
        "Med hilsen",
        "Beste hilsen",
        "På forhånd takk",
        "Hilsen",
        "Mvh",
        "Vh",
    ],
    "sent_from": "Sendt fra min|Sendt fra|Hent Outlook for|Få Outlook for",
}

# Norwegian first, then the two it is most often mixed with in the same inbox.
PARSER = EmailReplyParser(languages=["no", "en", "da"], default_language="no")


def clean(text: str) -> str:
    """The message without the thread it was quoting, or the sign-off."""
    if not text:
        return ""
    try:
        replies = PARSER.read(text).replies
        # `.body` is the reply with both the quoted thread and the sign-off
        # taken off; `parse_reply()` would leave the sign-off on, and a corpus
        # of sign-offs matches every query equally well and helps with none.
        return (replies[0].body if replies else text).strip()
    except re.error:
        # A language pattern that trips on one odd mail must not lose the mail.
        # The raw text is stored either way, so a rebuild can try again once the
        # pattern is fixed. Anything other than a bad regex is a real bug and
        # should be seen, not swallowed.
        return text.strip()


class _Flatten(HTMLParser):
    """HTML to text, stopping where the quoted thread starts.

    Front leaves `text` empty on some HTML-only messages, and storing "" for
    those drops the message from the corpus without a word. Quoting is explicit
    in HTML — a <blockquote>, or the wrapper Gmail, Outlook or Apple Mail puts
    round the thread — so cutting there beats any regex over flattened text.
    """

    QUOTE_IDS = {"divRplyFwdMsg", "appendonsend"}
    QUOTE_CLASSES = ("gmail_quote", "moz-cite-prefix", "yahoo_quoted")
    BLOCK = {"p", "div", "br", "tr", "li", "h1", "h2", "h3", "h4", "h5", "h6"}

    def __init__(self) -> None:
        super().__init__()
        self.out: list[str] = []
        self.done = False
        self.hidden = 0

    def handle_starttag(self, tag, attrs):
        if self.done:
            return
        a = dict(attrs)
        klass = a.get("class") or ""
        if (
            tag == "blockquote"
            or a.get("id") in self.QUOTE_IDS
            or a.get("name") == "messageReplySection"
            or any(c in klass for c in self.QUOTE_CLASSES)
        ):
            self.done = True
            return
        if tag in ("style", "script"):
            self.hidden += 1
        elif tag in self.BLOCK:
            self.out.append("\n")

    def handle_endtag(self, tag):
        if tag in ("style", "script") and self.hidden:
            self.hidden -= 1

    def handle_data(self, data):
        if not self.done and not self.hidden:
            self.out.append(data)


def html_to_text(body: str) -> str:
    f = _Flatten()
    try:
        f.feed(body)
    except Exception:
        return ""
    text = re.sub(r"[ \t\xa0]+", " ", "".join(f.out))
    return re.sub(r"\n\s*\n\s*\n+", "\n\n", text).strip()


# An auto-acknowledgement or a bounce arrives as ordinary inbound mail, and
# taken as a question it teaches the corpus to answer machines.
AUTO_SUBJECT = re.compile(
    r"^\s*(re:\s*)?("
    r"auto(matic|matisk)?[\s-]*(reply|svar|response|report)|autosvar|"
    r"out of office|ute av kontoret|frav[æa]r|"
    r"undeliver(ed|able)|mail delivery (failed|subsystem)|"
    r"delivery status notification|returnert e-post"
    r")",
    re.I,
)


# ---------------------------------------------------------------------------
# Front

class Front:
    BASE = "https://api2.frontapp.com"

    def __init__(self, token: str):
        self.s = requests.Session()
        self.s.headers.update(
            {"Authorization": f"Bearer {token}", "Accept": "application/json"}
        )

    def get(self, url: str, params: dict | None = None) -> dict:
        """One request, waiting rather than failing when the window is spent.

        Front allows 50-200 a minute by plan, and says in every response how
        much of that is left. Reading those headers is what lets a backfill run
        unattended instead of dying two thousand conversations in.
        """
        for attempt in range(6):
            try:
                r = self.s.get(url, params=params, timeout=60)
            except (requests.ConnectionError, requests.Timeout) as e:
                # Front drops a connection now and then. A scheduled backfill
                # that dies of it has walked two thousand conversations for
                # nothing, so a dropped socket is retried like a 5xx.
                if attempt == 5:
                    raise SystemExit(f"gave up on {url}: {e}") from e
                time.sleep(2 ** attempt)
                continue
            if r.status_code == 429:
                time.sleep(float(r.headers.get("retry-after", 10)) + 0.5)
                continue
            if r.status_code >= 500:
                time.sleep(2 ** attempt)
                continue
            if not r.ok:
                raise SystemExit(f"Front {r.status_code} on {url}: {r.text[:300]}")
            remaining = r.headers.get("x-ratelimit-remaining")
            if remaining is not None and int(remaining) <= 1:
                reset = float(r.headers.get("x-ratelimit-reset", 0))
                time.sleep(max(0.0, reset - time.time()) + 0.5)
            return r.json()
        raise SystemExit(f"gave up on {url} after repeated retries")

    def paged(self, path: str, params: dict | None = None):
        url = self.BASE + path
        while url:
            data = self.get(url, params)
            yield from data.get("_results", [])
            url = (data.get("_pagination") or {}).get("next")
            params = None  # the next link carries the query already


# ---------------------------------------------------------------------------
# messages -> pairs

def author_email(m: dict) -> str | None:
    a = m.get("author") or {}
    if a.get("email"):
        return a["email"]
    for r in m.get("recipients") or []:
        if r.get("role") == "from":
            return r.get("handle")
    return None


def usable(m: dict) -> bool:
    """Email only, and not a draft.

    An SMS or a WhatsApp message answers a different kind of question in a
    different register, and a draft is something nobody has said yet.
    """
    return m.get("type") == "email" and not m.get("draft_mode")


def pair(messages: list[dict]) -> list[tuple[list[dict], dict]]:
    """A run of inbound messages, and the reply that answered them.

    Outbound with nothing pending is not an answer to anything — an outreach
    that opened the thread, or a second message from the same person before
    anyone replied — so it teaches nothing about how a question gets answered
    and is skipped rather than paired with whatever came before.
    """
    pending: list[dict] = []
    out: list[tuple[list[dict], dict]] = []
    for m in sorted(messages, key=lambda m: m["created_at"]):
        if m["is_inbound"]:
            pending.append(m)
        elif pending:
            out.append((pending, m))
            pending = []
    return out


MIN_REPLY_CHARS = 20  # "Takk!" is not an example of anything

# How far back to re-read on every incremental run. Front's /events is
# documented to sort and filter on different timestamps, so a watermark set to
# the last event seen can step over messages that arrive around the boundary.
# Re-reading a day costs a few requests and every write is an upsert.
WATERMARK_OVERLAP = 86400


def rebuild_pairs(db, conversation_ids: list[str] | None = None) -> int:
    """Re-derive pairs from stored messages, for some conversations or all."""
    if conversation_ids is None:
        conversation_ids = [
            r["conversation_id"]
            for r in db.execute("SELECT DISTINCT conversation_id FROM messages")
        ]
    written = 0
    for cid in conversation_ids:
        rows = db.execute(
            "SELECT * FROM messages WHERE conversation_id = ? ORDER BY created_at",
            (cid,),
        ).fetchall()
        inbox = db.execute(
            "SELECT inbox_id FROM conversation_inbox WHERE conversation_id = ?", (cid,)
        ).fetchone()
        # Explicit delete rather than INSERT OR REPLACE: REPLACE only fires the
        # delete trigger with recursive_triggers on, and the FTS index would
        # quietly keep the old rows.
        db.execute("DELETE FROM pairs WHERE conversation_id = ?", (cid,))
        # Both filters happen before pairing, not after, so that a machine's
        # message cannot stand between a question and the person who answered
        # it: an auto-acknowledgement would otherwise consume the question and
        # leave the real reply paired with nothing.
        msgs = [
            dict(r)
            for r in rows
            if not (r["is_inbound"] and AUTO_SUBJECT.search(r["subject"] or ""))
            and not (not r["is_inbound"] and r["author_id"] is None)
        ]
        for inbound, reply in pair(msgs):
            asked = "\n\n".join(m["clean_text"] for m in inbound if m["clean_text"])
            answered = reply["clean_text"]
            if not asked or len(answered) < MIN_REPLY_CHARS:
                continue
            db.execute(
                "INSERT INTO pairs(reply_id, conversation_id, inbox_id, subject,"
                " inbound_text, reply_text, author_email, sent_at)"
                " VALUES(?,?,?,?,?,?,?,?)",
                (
                    reply["id"],
                    cid,
                    inbox["inbox_id"] if inbox else None,
                    reply["subject"] or inbound[0]["subject"],
                    asked,
                    answered,
                    reply["author_email"],
                    reply["created_at"],
                ),
            )
            written += 1
    return written


def store_messages(db, cid: str, messages: list[dict], subject: str | None) -> None:
    for m in messages:
        if not usable(m):
            continue
        raw = (m.get("text") or "").strip() or html_to_text(m.get("body") or "")
        db.execute(
            "INSERT INTO messages(id, conversation_id, is_inbound, author_id,"
            " author_email, created_at, subject, text, clean_text)"
            " VALUES(?,?,?,?,?,?,?,?,?)"
            " ON CONFLICT(id) DO UPDATE SET clean_text = excluded.clean_text,"
            " text = excluded.text",
            (
                m["id"],
                cid,
                1 if m.get("is_inbound") else 0,
                (m.get("author") or {}).get("id"),
                author_email(m),
                m.get("created_at") or 0,
                m.get("subject") or subject,
                raw,
                clean(raw),
            ),
        )


def note_inbox(db, front: Front, cid: str, inbox: dict | None) -> None:
    """Which inbox a conversation is in, learned once and remembered."""
    if db.execute(
        "SELECT 1 FROM conversation_inbox WHERE conversation_id = ?", (cid,)
    ).fetchone():
        return
    if inbox is None:
        found = list(front.paged(f"/conversations/{cid}/inboxes"))
        inbox = found[0] if found else {}
    db.execute(
        "INSERT OR REPLACE INTO conversation_inbox VALUES(?,?,?)",
        (cid, inbox.get("id"), inbox.get("name")),
    )


def ingest(db, front: Front, cid: str, subject: str | None, inbox: dict | None) -> None:
    note_inbox(db, front, cid, inbox)
    messages = list(front.paged(f"/conversations/{cid}/messages"))
    store_messages(db, cid, messages, subject)
    rebuild_pairs(db, [cid])


def recent(db, hours: float) -> str:
    """Inbound mail from the last `hours`, as text for a prompt.

    A digest wants everything that arrived, answered or not, in the order it
    arrived — which is the opposite of what `search_replies` does, and the
    reason this is a command rather than a tool. Printed rather than returned
    so a pipeline step can `capture` it.
    """
    cutoff = time.time() - hours * 3600
    rows = db.execute(
        "SELECT m.conversation_id, m.created_at, m.author_email, m.subject,"
        "       m.clean_text,"
        "       (SELECT count(*) FROM messages r"
        "         WHERE r.conversation_id = m.conversation_id"
        "           AND r.is_inbound = 0 AND r.created_at > m.created_at) AS replies,"
        "       (SELECT inbox_name FROM conversation_inbox c"
        "         WHERE c.conversation_id = m.conversation_id) AS inbox"
        "  FROM messages m"
        " WHERE m.is_inbound = 1 AND m.created_at > ?"
        " ORDER BY m.created_at",
        (cutoff,),
    ).fetchall()
    if not rows:
        return f"No mail arrived in the last {hours:g} hours."

    out = [f"{len(rows)} message(s) in the last {hours:g} hours.", ""]
    for r in rows:
        when = time.strftime("%Y-%m-%d %H:%M", time.localtime(r["created_at"]))
        state = "answered" if r["replies"] else "UNANSWERED"
        body = (r["clean_text"] or "").strip()
        if len(body) > 800:
            body = body[:800] + " […]"
        out += [
            f"--- {when}  {r['author_email'] or 'unknown'}  [{state}]",
            f"inbox: {r['inbox'] or '?'}   subject: {r['subject'] or '(none)'}",
            f"id: {r['conversation_id']}",
            body,
            "",
        ]
    return "\n".join(out)


# ---------------------------------------------------------------------------
# the two ways in

def backfill(db, front: Front, since: float | None) -> None:
    """The first import: every inbox, newest conversations first.

    `/events` only reaches so far back, so the history comes from walking
    conversations. The watermark is set to when this started, not when it
    finished, so anything that arrived during the walk is picked up next run.
    """
    started = time.time()
    seen = 0
    for inbox in front.paged("/inboxes"):
        print(f"inbox {inbox.get('name')}", file=sys.stderr)
        for conv in front.paged(f"/inboxes/{inbox['id']}/conversations"):
            created = conv.get("created_at") or 0
            if since and created < since:
                # Conversations come back newest first, so the first one older
                # than the cutoff means the rest of this inbox is too.
                break
            ingest(db, front, conv["id"], conv.get("subject"), inbox)
            seen += 1
            if seen % 25 == 0:
                db.commit()
                print(f"  {seen} conversations", file=sys.stderr)
    corpus.set_state(db, "events_after", str(started - WATERMARK_OVERLAP))
    db.commit()
    print(f"backfilled {seen} conversations", file=sys.stderr)


def incremental(db, front: Front) -> None:
    after = float(corpus.get_state(db, "events_after", "0") or 0)
    if not after:
        raise SystemExit("no watermark — run with --backfill first")
    touched: dict[str, dict] = {}
    high = after
    for ev in front.paged(
        "/events", {"q[after]": int(after), "q[types]": ["inbound", "outbound"]}
    ):
        conv = ev.get("conversation") or {}
        if conv.get("id"):
            touched[conv["id"]] = conv
        high = max(high, ev.get("emitted_at") or 0)
    for cid, conv in touched.items():
        ingest(db, front, cid, conv.get("subject"), None)
    # Only after every touched conversation is committed, or a crash halfway
    # would leave a gap no later run goes back for.
    corpus.set_state(db, "events_after", str(high - WATERMARK_OVERLAP))
    db.commit()
    print(f"{len(touched)} conversation(s) updated", file=sys.stderr)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--backfill", action="store_true", help="walk inboxes, not events")
    ap.add_argument("--since", help="backfill only conversations after this ISO date")
    ap.add_argument(
        "--rebuild", action="store_true", help="re-derive pairs from stored messages"
    )
    ap.add_argument(
        "--list", action="store_true",
        help="print recent inbound mail instead of syncing, for a digest to read",
    )
    ap.add_argument("--since-hours", type=float, default=24.0,
                    help="how far back --list reaches. Default 24")
    args = ap.parse_args()

    db = corpus.connect(corpus.db_path())

    if args.list:
        # stdout, because a `capture` step reads it; everything else here
        # reports on stderr.
        print(recent(db, args.since_hours))
        return 0

    if args.rebuild:
        n = rebuild_pairs(db)
        db.commit()
        print(f"rebuilt {n} pairs in {corpus.db_path()}", file=sys.stderr)
        return 0

    token = os.environ.get("FRONT_API_TOKEN")
    if not token:
        raise SystemExit(
            "FRONT_API_TOKEN is not set — put it in "
            f"~/.rigg/instances/{corpus.instance()}/secrets.env"
        )
    front = Front(token)

    if args.backfill:
        since = None
        if args.since:
            since = datetime.fromisoformat(args.since).replace(
                tzinfo=timezone.utc
            ).timestamp()
        backfill(db, front, since)
    else:
        incremental(db, front)

    total = db.execute("SELECT count(*) c FROM pairs").fetchone()["c"]
    print(f"{total} pairs in {corpus.db_path()}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
