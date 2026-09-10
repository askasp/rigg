#!/usr/bin/env python3
"""Checks for the corpus, on a fixture rather than on Front.

    ./mail/test.sh

Most of what can go wrong here is silent: cleaning that leaves the quoted
thread in, pairing that invents pairs, an FTS index that drifts out of step
with the table it indexes. None of that raises anything at the time — it just
makes every search slightly worse. So each is asserted.
"""

import os
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

TMP = tempfile.mkdtemp(prefix="rigg-mail-test-")
os.environ["RIGG_MAIL_DIR"] = TMP
os.environ["RIGG_INSTANCE"] = "test"

import corpus  # noqa: E402
import sync  # noqa: E402

NOW = time.time()
DAY = 86400
ok = 0


def check(label: str, cond: bool, detail: str = "") -> None:
    global ok
    if not cond:
        print(f"FAIL  {label} {detail}", file=sys.stderr)
        sys.exit(1)
    ok += 1
    print(f"ok    {label}")


# --- cleaning ---------------------------------------------------------------

NORWEGIAN_REPLY = """Hei Kari,

Blodprøven koster 1 990 kroner, og da er legesamtalen inkludert.

Med vennlig hilsen
Aksel Stadler
Amino AS
+47 900 00 000

Den 3. mars 2026 skrev Kari Nordmann <kari@example.com>:
> Hei!
> Hva koster en blodprøve hos dere?
> Mvh Kari
"""

cleaned = sync.clean(NORWEGIAN_REPLY)
check("the answer survives cleaning", "1 990 kroner" in cleaned, repr(cleaned))
check("the quoted thread is gone", "kari@example.com" not in cleaned, repr(cleaned))
check("the quote header is gone", "skrev" not in cleaned.lower(), repr(cleaned))
check("the sign-off is gone", "Med vennlig hilsen" not in cleaned, repr(cleaned))

OUTLOOK_NO = """Takk, det er notert.

-----Opprinnelig melding-----
Fra: Ola <ola@example.com>
Sendt: 1. mars 2026 09:00
Til: support@amino.no
Emne: Timebestilling

Hei, jeg vil gjerne bestille time.
"""
check(
    "Norwegian Outlook headers are stripped",
    "Opprinnelig melding" not in sync.clean(OUTLOOK_NO),
    repr(sync.clean(OUTLOOK_NO)),
)
check("English still works", "wrote:" not in sync.clean(
    "Sure, that works.\n\nOn 3 Mar 2026, Kari <k@example.com> wrote:\n> Does it?"
))

# --- pairing and the index --------------------------------------------------

db = corpus.connect(corpus.db_path())


def msg(mid, cid, inbound, text, when, author, subject, author_id="tea_1"):
    # author_id None on an outbound row is how Front marks a message a rule or
    # the API token sent, rather than a person.
    db.execute(
        "INSERT OR REPLACE INTO messages(id,conversation_id,is_inbound,author_id,"
        "author_email,created_at,subject,text,clean_text) VALUES(?,?,?,?,?,?,?,?,?)",
        (mid, cid, 1 if inbound else 0, None if inbound else author_id, author,
         when, subject, text, sync.clean(text)),
    )


# the same question, answered years apart at different prices
msg("m1", "c1", True, "Hei! Hva koster en blodprøve hos dere?", NOW - 900 * DAY,
    "kari@example.com", "Pris")
msg("m2", "c1", False, "Hei Kari,\n\nBlodprøven koster 1 990 kroner.\n\nMvh\nAksel",
    NOW - 900 * DAY + 60, "aksel@stadler.no", "Pris")
msg("m3", "c2", True, "Hei! Hva koster en blodprøve hos dere?", NOW - 10 * DAY,
    "per@example.com", "Pris")
msg("m4", "c2", False, "Hei Per,\n\nPakken koster 2 490 kroner i 2026.\n\nMvh\nJulie",
    NOW - 10 * DAY + 60, "julie@amino.no", "Hva koster pakken")
# an unrelated thread
msg("m5", "c3", True, "Jeg får ikke logget inn med BankID", NOW - 5 * DAY,
    "ola@example.com", "Innlogging")
msg("m6", "c3", False, "Hei Ola,\n\nProv a avinstallere appen.\n\nMvh\nJulie",
    NOW - 5 * DAY + 60, "julie@amino.no", "Innlogging")
# outbound that answers nothing
msg("m7", "c4", False, "Vi har kampanje på blodprøver i mars!", NOW - 3 * DAY,
    "julie@amino.no", "Kampanje")
# a reply that teaches nothing
msg("m8", "c5", True, "Takk for hjelpen!", NOW - 2 * DAY, "nn@example.com", "Takk")
msg("m9", "c5", False, "Bare hyggelig!", NOW - 2 * DAY + 60, "julie@amino.no", "Takk")

# an auto-reply landing between the question and the answer
msg("m10", "c7", True, "Hva koster en time hos legen?", NOW - 4 * DAY,
    "liv@example.com", "Legetime")
msg("m11", "c7", True, "Autosvar: Jeg er på ferie til 20. mars", NOW - 4 * DAY + 30,
    "liv@example.com", "Autosvar: Legetime")
msg("m12", "c7", False, "Hei Liv,\n\nEn legetime koster 950 kroner.\n\nMvh\nJulie",
    NOW - 4 * DAY + 600, "julie@amino.no", "Legetime")

# an automated acknowledgement standing between the question and the person
msg("m13", "c8", True, "Kan jeg avbestille timen min?", NOW - 6 * DAY,
    "nils@example.com", "Avbestilling")
msg("m14", "c8", False, "Takk for din henvendelse, vi svarer så snart vi kan.",
    NOW - 6 * DAY + 10, "support@amino.no", "Avbestilling", author_id=None)
msg("m15", "c8", False,
    "Hei Nils,\n\nJa, du kan avbestille inntil 24 timer før.\n\nMvh\nJulie",
    NOW - 6 * DAY + 900, "julie@amino.no", "Avbestilling")

for cid in ("c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8"):
    db.execute("INSERT OR REPLACE INTO conversation_inbox VALUES(?,?,?)",
               (cid, "inb_support", "Support"))

# HTML-only mail: Front leaves `text` empty and the message would vanish.
sync.store_messages(db, "c6", [{
    "id": "h1", "type": "email", "is_inbound": True, "created_at": NOW - DAY,
    "subject": "Timebestilling", "text": "",
    "body": "<div>Hei,</div><div>Kan jeg bestille time p&aring; torsdag?</div>"
            "<blockquote>Den 1. mars skrev support:<br>Hei!</blockquote>",
    "author": None,
    "recipients": [{"role": "from", "handle": "ola@example.com"}],
}], "Timebestilling")
db.commit()
h = db.execute("SELECT clean_text FROM messages WHERE id='h1'").fetchone()["clean_text"]
check("HTML-only mail is not stored empty", "bestille time" in h, repr(h))
check("the HTML quote block is cut", "skrev support" not in h, repr(h))

n = sync.rebuild_pairs(db)
db.commit()
check("only real question-and-answer pairs", n == 5, f"got {n}")

c7 = db.execute("SELECT inbound_text FROM pairs WHERE conversation_id='c7'").fetchone()
check("an auto-reply is not part of the question",
      c7 and "Autosvar" not in c7["inbound_text"], repr(c7 and c7["inbound_text"]))
c8 = db.execute("SELECT reply_text, inbound_text FROM pairs "
                "WHERE conversation_id='c8'").fetchall()
check("an automated acknowledgement is not an exemplar", len(c8) == 1, str(len(c8)))
check("and it does not swallow the question",
      "avbestille timen" in c8[0]["inbound_text"], repr(c8[0]["inbound_text"]))
check("an outreach is not a pair",
      not db.execute("SELECT 1 FROM pairs WHERE conversation_id='c4'").fetchone())
check("a two-word reply is not an example",
      not db.execute("SELECT 1 FROM pairs WHERE conversation_id='c5'").fetchone())

fts = db.execute("SELECT count(*) c FROM pairs_fts").fetchone()["c"]
check("the index matches the table", fts == 5, f"{fts} indexed, 5 rows")

sync.rebuild_pairs(db)
db.commit()
check("rebuilding does not duplicate",
      db.execute("SELECT count(*) c FROM pairs").fetchone()["c"] == 5)
check("rebuilding leaves the index in step",
      db.execute("SELECT count(*) c FROM pairs_fts").fetchone()["c"] == 5)

# An UPDATE with no trigger behind it corrupts an external-content FTS table,
# and the symptom is "database disk image is malformed" on the next read.
db.execute("UPDATE pairs SET subject = 'Pris pa blodprove' WHERE conversation_id='c1'")
db.commit()
check("an UPDATE keeps the index readable",
      db.execute("SELECT count(*) c FROM pairs_fts WHERE pairs_fts MATCH 'blodprøve'"
                 ).fetchone()["c"] >= 1)

# --- retrieval --------------------------------------------------------------

import mail_mcp as M  # noqa: E402  (imports open the db, so after it exists)

res = M.search_replies("Hva koster en blodprøve hos dere?")
# "Hva koster en time hos legen?" is a fair hit on the same words, so the count
# is not the interesting part — which replies come back, and in what order, is.
replies = " | ".join(r["replied"] for r in res)
check("both price threads come back",
      "2 490" in replies and "1 990" in replies, replies[:120])
# The same question asked twice, so bm25 cannot separate them and only age
# can. A *better* old match can still lead — recency flips near-ties, it does
# not overrule relevance — which is why every result carries its date.
check("between equal matches the recent one leads",
      "2 490" in res[0]["replied"], res[0]["replied"][:40])
check("every result carries its date",
      all(len(r["sent_at"]) == 10 for r in res), str([r["sent_at"] for r in res]))
check("a different question goes elsewhere",
      "avinstallere" in M.search_replies("Får ikke logget inn med BankID")[0]["replied"])
check("author narrows to one voice",
      len(M.search_replies("Hva koster en blodprøve?", author="aksel@stadler.no")) == 1)

for probe in ('pris "OR" * NEAR/2', "blodprøve AND (", "pris*"):
    M.search_replies(probe)
check("FTS operators in a question are just words", True)

for probe in ('"""', "*** ()", "a b c"):
    try:
        M.search_replies(probe)
        check("punctuation-only text is refused", False, probe)
    except Exception:
        pass
check("punctuation-only text is refused, not crashed", True)

# Norwegian inflects on the end of the word and fts5 has no stemmer for it,
# so an exact match on the stem alone would return nothing at all.
check("the stem reaches its inflected forms",
      len(M.search_replies("blodprøv")) >= 2, str(len(M.search_replies("blodprøv"))))

check("a thread reads back whole", len(M.get_thread("c1")) == 2)
try:
    M.get_thread("nope")
    check("an unknown id explains itself", False)
except Exception as e:
    check("an unknown id explains itself", "search_replies" in str(e))

import asyncio  # noqa: E402
names = [t.name for t in asyncio.run(M.mcp.list_tools())]
check("the write tool is absent unless asked for",
      "create_draft" not in names and "search_replies" in names, str(names))

os.environ.pop("FRONT_AUTHOR_ID", None)
os.environ["FRONT_API_TOKEN"] = "test"
try:
    M.create_draft("c1", "Hei")
    check("an unsigned draft is refused", False)
except Exception as e:
    check("an unsigned draft is refused", "unsigned" in str(e))

# --- an inbox scope ---------------------------------------------------------

db.execute("INSERT OR REPLACE INTO conversation_inbox VALUES(?,?,?)",
          ("c1", "inb_a", "Aksels inbox"))
db.commit()

check("no scope set means every inbox", corpus.scope() is None)
check("everything is in scope when there is none", corpus.in_scope(db, "c1"))

os.environ["RIGG_VAR_INBOX"] = "Aksels inbox"
check("a pipeline var is the scope", corpus.scope() == "Aksels inbox")
check("a conversation in the scoped inbox is reachable", corpus.in_scope(db, "c1"))
check("one in another inbox is not", not corpus.in_scope(db, "c-elsewhere"))
check("an id works as well as a name",
      corpus.in_scope(db, "c1") and (os.environ.__setitem__("RIGG_VAR_INBOX", "inb_a")
                                    or corpus.in_scope(db, "c1")))

os.environ["RIGG_VAR_INBOX"] = "Aksels inbox"
scoped = corpus.search(db, "blodprøve", limit=50)
os.environ["RIGG_VAR_INBOX"] = "Somebody elses inbox"
elsewhere = corpus.search(db, "blodprøve", limit=50)
check("a search only returns the scoped inbox",
      len(elsewhere) == 0 and len(scoped) >= 0, f"{len(scoped)} vs {len(elsewhere)}")
os.environ.pop("RIGG_VAR_INBOX")
check("dropping the scope restores every inbox", corpus.scope() is None)

# --- a dropped connection is retried, not fatal ------------------------------

import requests  # noqa: E402


class FlakySession:
    """Drops the first two connections, then answers."""

    def __init__(self):
        self.headers = {}
        self.calls = 0

    def get(self, url, params=None, timeout=None):
        self.calls += 1
        if self.calls <= 2:
            raise requests.ConnectionError("Remote end closed connection")

        class R:
            status_code = 200
            ok = True
            headers = {}

            def json(self):
                return {"_results": [{"id": "ok"}]}

        return R()


f = sync.Front.__new__(sync.Front)
f.s = FlakySession()
sync.time.sleep = lambda *_: None
check("a dropped connection is retried rather than fatal",
      f.get("https://example/x")["_results"][0]["id"] == "ok" and f.s.calls == 3,
      f"{f.s.calls} calls")

# --- which channel a draft goes out on --------------------------------------

db.execute("INSERT OR REPLACE INTO conversation_inbox VALUES(?,?,?)",
           ("c-draft", "inb_x", "Some inbox"))
db.commit()

CHANNELS = {"_results": []}
MESSAGES = {"_results": []}


def fake_front_get(path):
    if "/channels" in path:
        return CHANNELS
    if "/messages" in path:
        return MESSAGES
    raise AssertionError(path)


M.front_get = fake_front_get
M.DB = db

def channel_is(types, expect, label):
    CHANNELS["_results"] = types
    try:
        got = M.email_channel_for("c-draft")
    except Exception as e:
        got = f"error: {e}"
    check(label, got == expect, f"got {got}")

channel_is([{"id": "cha_1", "type": "gmail", "address": "a@b.no"}], "cha_1",
           "a gmail channel is email - the type is the transport, not the medium")
channel_is([{"id": "cha_1", "type": "smtp", "address": "a@b.no"}], "cha_1",
           "so is smtp")
channel_is([{"id": "cha_1", "type": "office365", "address": "a@b.no"}], "cha_1",
           "so is office365")
channel_is([{"id": "cha_1", "type": "twilio", "address": "+4712345678"},
            {"id": "cha_2", "type": "gmail", "address": "a@b.no"}], "cha_2",
           "an SMS channel is not picked over an email one")
channel_is([{"id": "cha_1", "type": "something-new", "address": "a@b.no"}], "cha_1",
           "a transport this list has not met still counts if it has an address")
channel_is([{"id": "cha_1", "type": "gmail", "address": "a@b.no", "is_valid": False}],
           "error: inbox inb_x has no channel that sends email - it has gmail",
           "a broken channel is refused, and the error says what it found")

# --- a second draft on the same mail is spam, not help ----------------------

CHANNELS["_results"] = [{"id": "cha_1", "type": "gmail", "address": "a@b.no"}]
os.environ["FRONT_API_TOKEN"] = "test"
os.environ["FRONT_AUTHOR_ID"] = "tea_1"

posted = []


class FakeResp:
    ok = True

    @staticmethod
    def json():
        return {"id": "msg_new"}


M.requests.post = lambda *a, **k: (posted.append(k.get("json")), FakeResp)[1]

MESSAGES["_results"] = [{"id": "msg_old", "is_draft": True}]
out = M.create_draft("c-draft", "Hei igjen")
check("a conversation that already has a draft is left alone",
      out["created"] is False and out["existing_draft_id"] == "msg_old", str(out))
check("and nothing was sent to Front", not posted, str(posted))
check("the reason names the draft that is already there",
      "msg_old" in out["reason"], out["reason"])

MESSAGES["_results"] = [{"id": "msg_sent", "is_draft": False}]
out = M.create_draft("c-draft", "Hei")
check("a conversation with no draft still gets one",
      out["created"] is True and out["draft_id"] == "msg_new", str(out))

# A draft does not mark a conversation answered, so the run after this one sees
# the same mail again - which is exactly when the guard has to hold.
posted.clear()
MESSAGES["_results"] = [{"id": "msg_new", "is_draft": True}]
out = M.create_draft("c-draft", "Hei en tredje gang")
check("the next run over the same window does not stack another",
      out["created"] is False and not posted, str(out))


def blows_up(path):
    if "/messages" in path:
        raise RuntimeError("Front is down")
    return CHANNELS


M.front_get = blows_up
posted.clear()
out = M.create_draft("c-draft", "Hei")
check("not being able to look does not block drafting",
      out["created"] is True, str(out))
M.front_get = fake_front_get

print(f"\n{ok} checks passed")
