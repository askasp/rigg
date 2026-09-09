#!/usr/bin/env python3
"""Checks for the cron store and its schedule parsing.

    python3 slack/test_cron.py

Needs nothing installed. The schedule parser is the part worth testing: a
wrong field is silent — the job simply never fires, or fires every minute.
"""
import os
import sys
import tempfile
from datetime import datetime

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
os.environ["RIGG_HOME"] = tempfile.mkdtemp(prefix="rigg-home-test-")
# So a run of these never reads - or writes - the real credentials.
os.environ["RIGG_INSTANCE"] = "test"

# A job whose repo has gone is now a state of its own, so the fixtures need one
# that is actually there.
REPO = tempfile.mkdtemp(prefix="rigg-repo-test-")

import cron  # noqa: E402
# For the real modifier parser: `slack_bolt` is imported inside main(), so this
# costs nothing here, and a stub would only prove the stub works.
import rigg_slack  # noqa: E402

ok = 0


def check(label, cond, detail=""):
    global ok
    if not cond:
        print(f"FAIL  {label} {detail}", file=sys.stderr)
        sys.exit(1)
    ok += 1
    print(f"ok    {label}")


def at(s):
    return datetime.strptime(s, "%Y-%m-%d %H:%M")


# --- parsing ---------------------------------------------------------------
w = cron.parse("0 7 * * 1-5")
check("weekday morning fires Monday 07:00", cron.matches(w, at("2026-09-07 07:00")))
check("...and not Saturday", not cron.matches(w, at("2026-09-12 07:00")))
check("...and not 07:01", not cron.matches(w, at("2026-09-07 07:01")))

check("@daily is midnight", cron.matches(cron.parse("@daily"), at("2026-09-07 00:00")))
check("@hourly is on the hour", cron.matches(cron.parse("@hourly"), at("2026-09-07 13:00")))

s = cron.parse("*/15 * * * *")
check("*/15 fires at :00 :15 :30 :45",
      all(cron.matches(s, at(f"2026-09-07 09:{m:02d}")) for m in (0, 15, 30, 45)))
check("...and not :07", not cron.matches(s, at("2026-09-07 09:07")))

s = cron.parse("0 9,17 * * *")
check("a list of hours", cron.matches(s, at("2026-09-07 17:00"))
      and cron.matches(s, at("2026-09-07 09:00"))
      and not cron.matches(s, at("2026-09-07 12:00")))

check("Sunday is 0", cron.matches(cron.parse("0 8 * * 0"), at("2026-09-06 08:00")))
check("...and 7 is the same Sunday", cron.matches(cron.parse("0 8 * * 7"), at("2026-09-06 08:00")))

# Vixie's rule: both day-of-month and weekday restricted means OR, not AND.
s = cron.parse("0 0 1 * 1")
check("day-or-weekday: the 1st, whatever weekday", cron.matches(s, at("2026-09-01 00:00")))
check("day-or-weekday: any Monday", cron.matches(s, at("2026-09-07 00:00")))
check("day-or-weekday: not a plain Tuesday", not cron.matches(s, at("2026-09-08 00:00")))
# ...but with only one restricted it is a plain AND.
s = cron.parse("0 0 15 * *")
check("only the 15th", cron.matches(s, at("2026-09-15 00:00"))
      and not cron.matches(s, at("2026-09-16 00:00")))

# --- errors say which field ------------------------------------------------
for expr, word in [("0 7 * *", "five fields"), ("60 7 * * *", "minute"),
                   ("0 25 * * *", "hour"), ("0 7 * * abc", "weekday"),
                   ("*/0 * * * *", "step")]:
    try:
        cron.parse(expr)
        check(f"{expr!r} rejected", False)
    except ValueError as e:
        check(f"{expr!r} rejected, naming the problem", word in str(e), str(e))

# --- next_times ------------------------------------------------------------
n = cron.next_times(cron.parse("0 7 * * 1-5"), at("2026-09-11 12:00"), 3)
check("next three weekday mornings skip the weekend",
      [t.strftime("%a %H:%M") for t in n] == ["Mon 07:00", "Tue 07:00", "Wed 07:00"],
      str([t.strftime("%a %d %H:%M") for t in n]))

# --- store -----------------------------------------------------------------
check("no jobs to begin with", cron.load() == [])
cron.save([{"id": 1, "channel": "rigg-tasks", "command": "digest"}])
check("a job survives a round trip", cron.load()[0]["command"] == "digest")
check("the store is per instance", cron.load("other") == [])

# --- what a typed command means --------------------------------------------
KNOWN = ["digest", "full", "do"]
check("a pipeline name is that pipeline",
      cron.plan("digest", KNOWN) == ("pipeline", "digest", ""))
check("a pipeline can carry a task",
      cron.plan("digest siste døgn", KNOWN) == ("pipeline", "digest", "siste døgn"))
check("`new` starts a branch",
      cron.plan("new tidy the TODOs", KNOWN) == ("new", "", "tidy the TODOs"))
check("plain instructions go to the `do` pipeline",
      cron.plan("read my Front inbox and summarise it", KNOWN)
      == ("freeform", "do", "read my Front inbox and summarise it"))
check("...but only where the repo has one",
      cron.plan("read my Front inbox", ["digest"]) is None)
check("`new` with nothing to do is nothing", cron.plan("new", KNOWN) is None)

check("a pipeline job runs in the checkout, no branch",
      cron.args_for({"id": 1, "channel": "c", "kind": "pipeline",
                     "pipeline": "digest", "task": ""})
      == ["run", "--pipeline", "digest"])
check("a freeform job passes the sentence as the task",
      cron.args_for({"id": 1, "channel": "c", "kind": "freeform",
                     "pipeline": "do", "task": "read my Front inbox"})
      == ["run", "--pipeline", "do", "--task", "read my Front inbox"])
check("`new` makes a branch named for the job",
      cron.args_for({"id": 2, "channel": "rigg-tasks", "kind": "new",
                     "pipeline": "", "task": "tidy the TODOs"})
      == ["new", "rigg-tasks/cron-2", "tidy the TODOs"])

# --- how a run asks for a credential ---------------------------------------
out = """thinking about it
RIGG-NEEDS-SECRET: FRONT_API_TOKEN | a Front token with scopes shared:conversations | https://app.frontapp.com/settings/api
stopping here."""
check("a secret request is picked out of the output",
      cron.secret_requests(out)
      == [("FRONT_API_TOKEN", "a Front token with scopes shared:conversations",
           "https://app.frontapp.com/settings/api")], str(cron.secret_requests(out)))
check("the name alone is enough",
      cron.secret_requests("RIGG-NEEDS-SECRET: FOO_TOKEN") == [("FOO_TOKEN", "", "")])
check("asking twice is asking once",
      len(cron.secret_requests("RIGG-NEEDS-SECRET: A\nRIGG-NEEDS-SECRET: A")) == 1)
check("ordinary output asks for nothing", cron.secret_requests("all fine") == [])

check("a report is what follows the marker",
      cron.report("step 1/1 ok\nRIGG-REPORT\n7 henvendelser, 2 ubesvarte.")
      == "7 henvendelser, 2 ubesvarte.")
check("the last marker wins",
      cron.report("RIGG-REPORT\nfirst\nRIGG-REPORT\nsecond") == "second")
check("no marker means nothing to say", cron.report("just some log output") is None)
check("an empty report is nothing to say", cron.report("RIGG-REPORT\n   ") is None)

# --- the Slack command -----------------------------------------------------
cron.save([])
said = []
PIPELINES = ["digest", "full", "do"]
PARTS = ["mail", "preview", "copilot"]


def run(rest):
    said.clear()
    cron.command("/repo", "rigg-tasks", rest, said.append, "C1",
                 pipelines=lambda repo: PIPELINES,
                 run_rigg=lambda repo, args, timeout=None: (0, "1   do  impl  claude"),
                 split_modifiers=rigg_slack.split_modifiers,
                 features=lambda repo: PARTS)
    return said[0] if said else ""


out = run("")
check("empty channel explains the syntax", "cron <schedule>" in out, out)

out = run("0 7 * * 1-5 digest")
check("a good job is accepted", "scheduled" in out, out)
check("...and echoes the next firings", "next:" in out, out)
check("...and says it makes no branch", "no branch" in out, out)
check("...and is stored", len(cron.load()) == 1)

out = run("0 7 * * 1-5 read my Front inbox and summarise yesterday")
check("plain instructions are accepted", "scheduled" in out, out)
check("...and say they use the do pipeline", "`do`" in out, out)
check("...and echo back what was asked for", "Front inbox" in out, out)
check("...and are stored", len(cron.load()) == 2)

out = run("0 99 * * * digest")
check("a bad schedule is refused with the field named", "hour" in out, out)

out = run("@daily")
check("a schedule with no command is refused", "nothing to run" in out, out)

out = run("")
check("the listing shows the job and its next run", "digest" in out and "next" in out, out)

# --- inspecting and firing on demand ---------------------------------------
out = run("show 1")
check("show gives the schedule and what it will run", "`0 7 * * 1-5`" in out, out)
check("...the exact rigg command", "rigg run --pipeline digest" in out, out)
check("...and what was asked for", "asked" in out, out)
check("...and the pipeline's actual steps", "steps of `digest`" in out, out)
out = run("show 99")
check("showing a job that is not here says so", "no job" in out, out)

fired_now = []


class _Sched:
    def fire_now(self, job):
        fired_now.append(job["id"])


cron._scheduler = _Sched()
out = run("run 1")
check("a job can be run on demand", fired_now == [1], str(fired_now))
check("...and says so", "running `1` now" in out, out)
out = run("run 99")
check("running a job that is not here says so", "no job" in out, out)
cron._scheduler = None
out = run("run 1")
check("with no scheduler it says so rather than pretending",
      "nothing to run it with" in out, out)

for typo in ("run 1", "shwo 1", "delete-all", "digest"):
    out = run(typo)
    check(f"{typo!r} is not answered with a lecture about cron fields",
          "cron fields" not in out and "five fields" not in out, out)
check("...and a mistyped verb lists the real ones",
      "cron show" in run("shwo 1") and "cron run" in run("shwo 1"), run("shwo 1"))

out = run("rm 99")
check("removing a job that is not here says so", "no job" in out, out)
out = run("rm 1")
check("removing one works", "removed" in out, out)
check("...and leaves the other alone", len(cron.load()) == 1)
run("rm 2")
check("...and the store is empty again", cron.load() == [])

# --- the scheduler ---------------------------------------------------------
fired = []
sched = cron.Scheduler(run_rigg=lambda repo, args, timeout=None: fired.append(args) or (0, ""),
                       post=lambda ch, text: None, log=lambda m: None)
cron.save([{"id": 1, "channel": "c", "channel_id": "C1", "repo": REPO,
            "schedule": "0 7 * * *", "command": "digest",
            "kind": "pipeline", "pipeline": "digest", "task": ""}])
sched.tick(at("2026-09-07 07:00"))
import time as _t; _t.sleep(0.3)
check("a due job fires", fired == [["run", "--pipeline", "digest"]], str(fired))
fired.clear()
sched.tick(at("2026-09-07 07:00"))
_t.sleep(0.2)
check("the same minute does not fire twice", fired == [], str(fired))
sched.tick(at("2026-09-07 08:00"))
_t.sleep(0.2)
check("an hour later it does not fire", fired == [], str(fired))

check("a successful run is recorded",
      cron.load()[0].get("last_status") == "ok", str(cron.load()[0]))

told = []
teller = cron.Scheduler(
    run_rigg=lambda repo, args, timeout=None: (0, "[1/1] do ok\nRIGG-REPORT\n7 henvendelser siste døgn, 2 ubesvarte."),
    post=lambda ch, text: told.append(text), log=lambda m: None)
teller.tick(at("2026-09-09 07:00"))
_t.sleep(0.3)
check("a run that answered says so in the channel",
      told and "7 henvendelser" in told[0], str(told))
check("...without rigg's own step chrome", told and "[1/1]" not in told[0], str(told))

posted = []
asked = cron.Scheduler(
    run_rigg=lambda repo, args, timeout=None: (
        1, "RIGG-NEEDS-SECRET: FRONT_API_TOKEN | a Front token with scopes "
           "shared:conversations | https://app.frontapp.com/settings/api"),
    post=lambda ch, text: posted.append(text), log=lambda m: None)
asked.tick(at("2026-09-08 07:00"))
_t.sleep(0.3)
check("a run that needs a credential asks for it, in the channel",
      posted and "FRONT_API_TOKEN" in posted[0], str(posted))
check("...and says how to send it", "secret FRONT_API_TOKEN" in posted[0], str(posted))
check("...and links where to get one", "frontapp.com" in posted[0], str(posted))
check("...and is not reported as a plain failure",
      cron.load()[0]["last_status"] == "needs a secret", str(cron.load()[0]))

# --- what a run learns is kept, and read back ------------------------------
check("a learned line is picked out",
      cron.learned("thinking\nRIGG-LEARNED: /events reaches back 30 days.\ndone")
      == ["/events reaches back 30 days."], str(cron.learned("RIGG-LEARNED: x")))
check("learning it twice is learning it once",
      cron.learned("RIGG-LEARNED: a\nRIGG-LEARNED: a\nRIGG-LEARNED: b") == ["a", "b"])
check("ordinary output learns nothing", cron.learned("all fine") == [])

JOB = {"id": 4, "channel": "rigg-tasks"}
check("a note is appended", cron.note(["/events reaches back 30 days."], JOB)
      == ["/events reaches back 30 days."])
body = cron.notes_path().read_text()
check("...under a header saying what the file is", "worked out" in body, body[:80])
check("...dated and attributed", "(job 4, #rigg-tasks)" in body, body)
check("the same note twice is written once",
      cron.note(["/events reaches back 30 days."], JOB) == [])
check("a new one still lands", cron.note(["Front pages with _pagination.next"], JOB)
      == ["Front pages with _pagination.next"])
check("...after the first, not instead of it",
      cron.notes_path().read_text().count("- 20") == 2)

# --- the report is the report, not rigg's frame ----------------------------
check("the step result and the run total are not part of the report",
      cron.report("---- do ---- 12:03\nRIGG-REPORT\n7 ubesvarte.\n  ok (12s)\n\ntotal 1m3s")
      == "7 ubesvarte.",
      repr(cron.report("RIGG-REPORT\n7 ubesvarte.\n  ok (12s)\n\ntotal 1m3s")))
check("...nor is a learned line that landed after the marker",
      cron.report("RIGG-REPORT\n7 ubesvarte.\nRIGG-LEARNED: noted\n  ok (2s)")
      == "7 ubesvarte.")
check("a report that is only chrome is nothing to say",
      cron.report("RIGG-REPORT\n  ok (12s)\ntotal 1m3s") is None)
# Taken from a real run: the parting line arrives after a blank one, so an
# end-of-output trim stops before it.
REAL = ("---- do ---- 12:03\nRIGG-REPORT\n7 ubesvarte.\n  ok (8s)\n\ntotal 8s"
        "\n\nrigg: pipeline complete\n")
check("...including the line rigg signs off with", cron.report(REAL) == "7 ubesvarte.",
      repr(cron.report(REAL)))
check("a report may still start a line with the word total",
      cron.report("RIGG-REPORT\ntotal 3 ubesvarte\n  ok (8s)") == "total 3 ubesvarte",
      repr(cron.report("RIGG-REPORT\ntotal 3 ubesvarte\n  ok (8s)")))

# --- a token the run echoed does not get stored or posted ------------------
check("a credential value is replaced by its name",
      cron.redact("curl -H 'Bearer fr_live_abcdef123456'",
                  {"FRONT_API_TOKEN": "fr_live_abcdef123456"})
      == "curl -H 'Bearer <FRONT_API_TOKEN>'")
check("...and something too short to be one is left alone",
      cron.redact("it was ok", {"X": "ok"}) == "it was ok")

# --- two runs of one job do not overlap ------------------------------------
import threading as _th
gate = _th.Event()
started = []


def slow(repo, args, timeout=None):
    started.append(args)
    gate.wait(3)
    return 0, ""


cron.save([{"id": 1, "channel": "c", "channel_id": "C1", "repo": REPO,
            "schedule": "0 7 * * *", "command": "digest",
            "kind": "pipeline", "pipeline": "digest", "task": ""}])
slowly = cron.Scheduler(run_rigg=slow, post=lambda ch, text: None, log=lambda m: None)
slowly.tick(at("2026-09-07 07:00"))
_t.sleep(0.2)
check("a job that is running says so", 1 in cron.running(), str(cron.running()))
slowly.fire_now(cron.load()[0])
_t.sleep(0.2)
check("...and a second firing while it runs is skipped", len(started) == 1, str(started))
gate.set()
_t.sleep(0.3)
check("...and it is free again once it lands", cron.running() == {}, str(cron.running()))

cron.save([{"id": 1, "channel": "rigg-tasks", "channel_id": "C1", "repo": REPO,
            "schedule": "0 7 * * *", "command": "digest",
            "kind": "pipeline", "pipeline": "digest", "task": ""}])
cron._scheduler = _Sched()
cron.claim(1)
out = run("run 1")
check("`cron run` on one already going says so rather than starting a second",
      "already running" in out, out)
check("...and starts nothing", fired_now == [1], str(fired_now))
cron.release(1)
out = run("")
check("the listing shows a job in flight as running",
      "running now" not in out, out)
cron.claim(1)
out = run("")
check("...when it is", "running now" in out, out)
check("`cron show` says so too", "running now" in run("show 1"), run("show 1"))
cron.release(1)

# --- a job stuck in one state asks once, not every firing ------------------
cron.save([{"id": 1, "channel": "c", "channel_id": "C1", "repo": REPO,
            "schedule": "0 7 * * *", "command": "read the inbox",
            "kind": "freeform", "pipeline": "do", "task": "read the inbox"}])
posted = []
stuck = cron.Scheduler(
    run_rigg=lambda repo, args, timeout=None: (
        1, "RIGG-NEEDS-SECRET: FRONT_API_TOKEN | a Front token | https://front"),
    post=lambda ch, text: posted.append(text), log=lambda m: None)
stuck.tick(at("2026-09-08 07:00"))
_t.sleep(0.3)
check("the first missing credential is asked for", len(posted) == 1, str(len(posted)))
stuck.tick(at("2026-09-09 07:00"))
_t.sleep(0.3)
check("...and the same one, next firing, is not asked for again",
      len(posted) == 1, str(posted[1:]))

# --- a repo that has gone is a sentence, not a traceback -------------------
cron.save([{"id": 1, "channel": "c", "channel_id": "C1", "repo": "/gone",
            "schedule": "0 7 * * *", "command": "digest",
            "kind": "pipeline", "pipeline": "digest", "task": ""}])
gone = []
missing = cron.Scheduler(run_rigg=lambda repo, args, timeout=None: (0, ""),
                         post=lambda ch, text: gone.append(text), log=lambda m: None)
missing.tick(at("2026-09-08 07:00"))
_t.sleep(0.3)
check("a missing repo is reported in words", gone and "/gone" in gone[0], str(gone))
check("...without running anything", cron.load()[0]["last_status"] == "no repo")
missing.tick(at("2026-09-09 07:00"))
_t.sleep(0.3)
check("...and not again the next morning", len(gone) == 1, str(gone))

# --- modifiers survive a schedule ------------------------------------------
cron.save([])
out = run("0 7 * * 1-5 digest +mail effort=high")
job = cron.load()[0]
check("a modifier is taken off the command", job["command"] == "digest", job["command"])
check("...and kept as flags", job["mods"] == ["--var", "effort=high", "--with", "mail"],
      str(job["mods"]))
check("...and reaches the command line",
      cron.args_for(job) == ["run", "--pipeline", "digest",
                             "--var", "effort=high", "--with", "mail"],
      str(cron.args_for(job)))
check("...and is echoed back when it is defined", "--with mail" in out, out)

cron.save([])
run("@daily draft replies to anything unanswered +mail")
job = cron.load()[0]
check("a freeform job keeps its task clean",
      job["task"] == "draft replies to anything unanswered", job["task"])
check("...and still carries the part",
      cron.args_for(job)[-2:] == ["--with", "mail"], str(cron.args_for(job)))

out = run("@daily digest +handbok")
check("a part this repo does not have is refused when it is defined",
      "`handbok` is not a part" in out, out)
check("...listing what it does have", "`mail`" in out, out)
check("...saying where a corpus becomes one", "corpus show" in out, out)
check("...and storing nothing", len(cron.load()) == 1, str(cron.load()))

cron.save([])
run("@daily new tidy the TODOs +preview")
job = cron.load()[0]
check("`new` on a schedule takes them too",
      cron.args_for(job) == ["new", "rigg-tasks/cron-1", "tidy the TODOs",
                             "--with", "preview"], str(cron.args_for(job)))

check("a job written before any of this still runs",
      cron.args_for({"kind": "pipeline", "pipeline": "digest", "task": ""})
      == ["run", "--pipeline", "digest"])

# --- a run that wants a corpus ---------------------------------------------
cron.save([{"id": 1, "channel": "c", "channel_id": "C1", "repo": REPO,
            "schedule": "0 7 * * *", "command": "draft replies",
            "kind": "freeform", "pipeline": "do", "task": "draft replies"}])
asked = []
wants = cron.Scheduler(
    run_rigg=lambda repo, args, timeout=None: (
        0, "RIGG-NEEDS-CORPUS: mail | past replies to draft from\n"),
    post=lambda ch, text: asked.append(text), log=lambda m: None)
wants.tick(at("2026-09-08 07:00"))
_t.sleep(0.3)
check("a run that wants a corpus asks for one", len(asked) == 1, str(asked))
check("...naming it and what it is for",
      "*mail*" in asked[0] and "draft from" in asked[0], asked[0])
check("...with the command that builds it", "corpus add mail" in asked[0], asked[0])
check("...and is not called a failure",
      cron.load()[0]["last_status"] == "needs a corpus", str(cron.load()[0]))
wants.tick(at("2026-09-09 07:00"))
_t.sleep(0.3)
check("...and does not ask again the next morning", len(asked) == 1, str(asked))


print(f"\n{ok} checks passed")
