#!/usr/bin/env python3
"""Checks for the corpus registry, its search and the protocol a run asks with.

    python3 slack/test_corpora.py

Needs nothing installed — a fixture database is built here rather than fetched.
The part worth testing is the reading: a `corpus search` that ranks differently
from the tool an agent gets would be a lie told confidently, and nothing about
the output would show it.
"""
import os
import sqlite3
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
MAIL_DIR = tempfile.mkdtemp(prefix="rigg-mail-test-")
os.environ["RIGG_HOME"] = tempfile.mkdtemp(prefix="rigg-home-test-")
os.environ["RIGG_MAIL_DIR"] = MAIL_DIR
# So a run of these never reads - or writes - the real credentials.
os.environ["RIGG_INSTANCE"] = "test"

import corpora  # noqa: E402
import creds  # noqa: E402

ok = 0


def check(label, cond, detail=""):
    global ok
    if not cond:
        print(f"FAIL  {label} {detail}", file=sys.stderr)
        sys.exit(1)
    ok += 1
    print(f"ok    {label}")


said = []


def say(text):
    said.append(text)


def run(rest):
    said.clear()
    corpora.command("/repo", "rigg-tasks", rest, say, "C1")
    return said[0] if said else ""


DAY = 86400


def fixture(pairs):
    """A corpus the way the sync leaves one, without going near Front."""
    path = corpora.Mail.path()
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        path.unlink()
    db = sqlite3.connect(path)
    # The sidecar's own schema, so a column added there fails here rather than
    # being silently absent from what these checks think they are searching.
    db.executescript(corpora._ranking().SCHEMA)
    for i, (asked, replied, author, age_days) in enumerate(pairs):
        db.execute(
            "INSERT INTO pairs(reply_id, conversation_id, subject, inbound_text,"
            " reply_text, author_email, sent_at) VALUES(?,?,?,?,?,?,?)",
            (f"r{i}", f"cnv_{i}", "Spørsmål", asked, replied, author,
             time.time() - age_days * DAY),
        )
        db.execute(
            "INSERT INTO messages(id, conversation_id, is_inbound, author_email,"
            " created_at, subject, text, clean_text) VALUES(?,?,?,?,?,?,?,?)",
            (f"m{i}", f"cnv_{i}", 1, "kari@example.com",
             time.time() - age_days * DAY, "Spørsmål", asked, asked),
        )
    db.commit()
    db.close()


# --- an instance with nothing in it ----------------------------------------
corpora.save([])
out = run("")
check("an empty register says what a corpus is for", "past work" in out, out)
check("...and names what it can build", "corpus add mail" in out, out)

out = run("search hva koster en blodprøve")
check("searching nothing says so, rather than an empty list",
      "nothing to search" in out, out)

out = run("show mail")
check("showing one at an empty register offers to build it",
      "corpus add mail" in out, out)

out = run("add sharepoint")
check("an unknown kind is refused by name", "no `sharepoint`" in out, out)
check("...listing what there is", "`mail`" in out, out)

# --- adding one, before its credential exists ------------------------------
out = run("add mail")
check("a corpus that needs a token asks for it first",
      "FRONT_API_TOKEN" in out, out)
check("...naming the scopes, since someone has to tick them",
      "shared:conversations" in out, out)
check("...and where to get one", "frontapp.com" in out, out)
check("...and nothing is registered on the way past", corpora.load() == [],
      str(corpora.load()))

# --- with the credential, but nothing to run the import --------------------
creds._write_local({"FRONT_API_TOKEN": "fr_live_test"})
out = run("add mail")
check("with the token it registers", [e["name"] for e in corpora.load()] == ["mail"],
      str(corpora.load()))
check("...and says so rather than pretending it is ready",
      "fill it by hand" in out or "importing" in out, out)
check("...and is not yet marked imported", corpora.load()[0]["imported"] is False)

out = run("add mail")
check("adding the same one twice is not an error", "already here" in out, out)

# --- what a listing says while it is still empty ---------------------------
out = run("")
check("a registered but unfilled corpus says so", "not imported yet" in out, out)

# --- with something in it --------------------------------------------------
fixture([
    ("Hva koster en blodprøve hos dere?",
     "Hei! En blodprøve koster 450 kroner, og du trenger ikke bestille time.",
     "ola@example.com", 5),
    ("Hva koster en blodprøve?",
     "Hei! Blodprøve koster 300 kroner. Drop-in mellom 08 og 15.",
     "kari@example.com", 900),
    ("Kan jeg parkere gratis utenfor?",
     "Ja, det er to timer gratis parkering rett utenfor inngangen.",
     "ola@example.com", 30),
])
corpora.update("mail", imported=True, last_sync=time.time())

stats = corpora.Mail.stats()
check("a filled corpus reports its size", stats["rows"] == 3, str(stats))
check("...and its newest reply", time.time() - stats["newest"] < 6 * DAY, str(stats))

out = run("")
check("the listing counts what is in it", "3 replies" in out, out)
check("...and how fresh it is", "newest reply" in out, out)

out = run("search hva koster blodprøve")
check("a search finds the matching replies", "450 kroner" in out, out)
check("...and shows the question that got them", "Hva koster" in out, out)
check("...dated, since an old price is not a fact to repeat", "202" in out, out)
check("...with the recent one first",
      out.index("450 kroner") < out.index("300 kroner"), out)
check("...and not the unrelated one", "parkering" not in out, out)

out = run("hva koster blodprøve")
check("the verb can be left off a search", "450 kroner" in out, out)

out = run("search ????")
check("a search with nothing searchable in it is a sentence",
      "nothing searchable" in out, out)

out = run("search kolonoskopi")
check("no match says the corpus has nothing like it, with its size",
      "nothing in `mail`" in out and "3 replies" in out, out)

# The one thing this file exists to check: `corpus search` must be the same
# ranking the agent's tool runs, or it is a preview of something else.
db = corpora._read_only(corpora.Mail.path())
direct = corpora._ranking().search(db, "hva koster blodprøve", 3)
db.close()
via = corpora.Mail.search("hva koster blodprøve", 3)
check("Slack search and the agent's tool are one implementation",
      [d["conversation_id"] for d in direct] == [v["conversation_id"] for v in via],
      f"{direct} != {via}")

check("a name that is not here says so once there is something",
      "no corpus `handbook`" in run("show handbook"), run("show handbook"))
check("a search naming a corpus and nothing else asks what to look for",
      "say what to look for" in run("search mail"), run("search mail"))
check("...and one naming it does not search for its own name",
      "450 kroner" in run("search mail hva koster blodprøve"),
      run("search mail hva koster blodprøve"))

out = run("show mail")
check("show gives the file, so it can be looked at on the box",
      MAIL_DIR in out, out)
check("...and how often it refreshes", "every 15 minutes" in out, out)
check("...and the config that lets a run actually search it",
      "front-mcp.json" in out and "+mail" in out, out)
check("...warning off a role that can both read and draft",
      "written by strangers" in out, out)

# --- what a run asks for ---------------------------------------------------
check("a run asking for a corpus that exists is not a request",
      corpora.requests_in("RIGG-NEEDS-CORPUS: mail | past replies") == [])
wanted = corpora.requests_in("RIGG-NEEDS-CORPUS: handbook | how we phrase things")
check("one that does not exist is", wanted == [("handbook", "how we phrase things")],
      str(wanted))
check("...and is answered with what I do have",
      "`mail`" in corpora.ask_for(wanted), corpora.ask_for(wanted))
check("a request without a reason still parses",
      corpora.requests_in("RIGG-NEEDS-CORPUS: handbook") == [("handbook", "")])
check("the same one twice is one request",
      len(corpora.requests_in("RIGG-NEEDS-CORPUS: handbook\n"
                              "RIGG-NEEDS-CORPUS: handbook | again")) == 1)
check("a line that only mentions the marker is not a request",
      corpora.requests_in("it would print RIGG-NEEDS-CORPUS: if it wanted one") == [])

# --- the syncer ------------------------------------------------------------
# `sync` itself shells out to the sidecar; what is worth checking here is when
# it is called and in which mode, since a corpus that stops being refreshed is
# invisible until an agent quotes last year's price.
ran = []
corpora.sync = lambda name, mode="": (ran.append((name, mode)), (0, ""))[1]
posted = []
syncer = corpora.Syncer(post=lambda ch, text: posted.append(text), log=lambda m: None)

now = time.time()
corpora.save([{"name": "mail", "kind": "mail", "added_at": int(now),
               "added_in": "c", "added_in_id": "C1", "every_minutes": 15,
               "imported": False, "last_sync": 0, "last_reconcile": 0}])
syncer.tick(now)
time.sleep(0.2)
check("nothing is refreshed before the first import lands", ran == [], str(ran))

corpora.update("mail", imported=True, last_sync=now - 60)
syncer.tick(now)
time.sleep(0.2)
check("...nor before it is due", ran == [], str(ran))

corpora.update("mail", last_sync=now - 20 * 60, last_reconcile=now)
syncer.tick(now)
time.sleep(0.3)
check("a due corpus is refreshed", [n for n, _ in ran] == ["mail"], str(ran))
check("...incrementally, not by walking Front again", ran[0][1] == "", str(ran))
check("...and the refresh is recorded",
      corpora.load()[0]["last_sync"] > now - 5, str(corpora.load()))

ran.clear()
corpora.update("mail", last_sync=now - 20 * 60, last_reconcile=now - 8 * DAY)
syncer.tick(now)
time.sleep(0.3)
check("a week on, it walks the last month instead",
      ran and ran[0][1] == "reconcile", str(ran))
check("...and does not do it again tomorrow",
      corpora.load()[0]["last_reconcile"] > now - 5, str(corpora.load()))

ran.clear()
corpora.claim("mail", "backfill")
corpora.update("mail", last_sync=0)
syncer.tick(now)
time.sleep(0.2)
check("a sync never overlaps the one still running", ran == [], str(ran))
check("...and a listing says what it is doing", "importing now" in run(""), run(""))
corpora.release("mail")

corpora.sync = lambda name, mode="": (0, "")

# --- removing one ----------------------------------------------------------
out = run("rm mail")
check("removing deregisters it", corpora.load() == [], str(corpora.load()))
check("...and says the file is still there", MAIL_DIR in out, out)
check("...rather than deleting hours of import", corpora.Mail.path().exists())

print(f"\n{ok} checks passed")
