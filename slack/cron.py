"""`@rigg cron` — this channel's schedules.

rigg owns the schedules: the expressions, the store, the overlap guard and the
tick. This module used to keep its own copy of all four, on the same file with
a different row shape, so a job typed in Slack and a job declared in a repo
were different things that could not see each other and quietly corrupted one
another's store.

Now it translates and renders, like every other verb in the bridge.
"""
import os
import shlex
import re
import time
import traceback
from pathlib import Path

import corpora
import creds

# Standard cron, plus the @ aliases cron itself defines. Nothing invented: a
# schedule someone can paste into crontab and get the same behaviour.
ALIASES = {
    "@hourly": "0 * * * *",
    "@daily": "0 0 * * *",
    "@midnight": "0 0 * * *",
    "@weekly": "0 0 * * 0",
    "@monthly": "0 0 1 * *",
    "@yearly": "0 0 1 1 *",
    "@annually": "0 0 1 1 *",
}

FIELDS = [("minute", 0, 59), ("hour", 0, 23), ("day", 1, 31),
          ("month", 1, 12), ("weekday", 0, 6)]

# What a cron field can be made of, used to tell a schedule from a typo.
FIELD_CHARS = re.compile(r"^[0-9*,\-/]+$")


NEEDS_SECRET = re.compile(
    r"^RIGG-NEEDS-SECRET:\s*([A-Z][A-Z0-9_]*)\s*(?:\|([^|\n]*))?(?:\|([^|\n]*))?",
    re.M,
)


# A corpus a run turns out to want lives in `corpora`, which owns both the
# marker and the wording — the answer is a `corpus add`, not a `secret`.


# What a run wants said back in the channel. Everything after the last such
# line is the message; without one, a successful run says nothing, which is
# right for a job that pushes a branch and wrong for one that answers a
# question. A marker rather than "post the tail" because the output also
# carries rigg's own step chrome, and nobody wants that in Slack.
REPORT = re.compile(r"^\s*RIGG-REPORT\s*:?\s*$", re.M)


# rigg's own frame around a step, printed *after* the agent has spoken: the
# step result, the run total, the next step's banner, its parting line.
# Everything after the marker is the report, so without this they all ride along
# on the end of it. Narrow on purpose - the step lines are indented by exactly
# two spaces and a total is a duration, so a report of its own that opens
# "total 3 ubesvarte" is not mistaken for one.
CHROME = re.compile(
    r"^(?:\s{2}(?:ok \(|failed after |skipped\b|notify hook failed)"
    r"|total \d+[a-z]|rigg: |-{4} )"
)


def report(output: str) -> str | None:
    hits = list(REPORT.finditer(output or ""))
    if not hits:
        return None
    # Dropped wherever they land, not just off the end: stdout and stderr are
    # read as one stream, so rigg's parting line can arrive after a blank one.
    lines = [x for x in output[hits[-1].end():].splitlines()
             if not CHROME.match(x) and not LEARNED.match(x)]
    return "\n".join(lines).strip() or None


# What a run wants remembered. A scheduled run starts from a cleared context
# every time, so whatever it worked out today is gone by tomorrow unless it
# says so on a line of its own:
#
#   RIGG-LEARNED: Front's /events only reaches back about 30 days.
#
# These go to ~/.rigg/instances/<instance>/notes.md, which the prompt reads back — not to
# the target repo's own capabilities file. An unattended writer must not leave a
# dirty working tree in the checkout branches are cut from, and what a run works
# out is usually about this workspace's accounts rather than that repo's code. A
# line that turns out to be durable and repo-shaped is promoted by a person.
LEARNED = re.compile(r"^RIGG-LEARNED:\s*(\S[^\n]*?)\s*$", re.M)


# The RIGG-* markers an unattended run uses to talk back. rigg parses agent
# output already and these belong there; until they move, this is the only
# implementation, and `redact` below is load-bearing - `rigg cron` output is
# posted verbatim and a job's log can quote a token.

def report(output: str) -> str | None:
    hits = list(REPORT.finditer(output or ""))
    if not hits:
        return None
    # Dropped wherever they land, not just off the end: stdout and stderr are
    # read as one stream, so rigg's parting line can arrive after a blank one.
    lines = [x for x in output[hits[-1].end():].splitlines()
             if not CHROME.match(x) and not LEARNED.match(x)]
    return "\n".join(lines).strip() or None


# What a run wants remembered. A scheduled run starts from a cleared context
# every time, so whatever it worked out today is gone by tomorrow unless it
# says so on a line of its own:
#
#   RIGG-LEARNED: Front's /events only reaches back about 30 days.
#
# These go to ~/.rigg/instances/<instance>/notes.md, which the prompt reads back — not to
# the target repo's own capabilities file. An unattended writer must not leave a
# dirty working tree in the checkout branches are cut from, and what a run works
# out is usually about this workspace's accounts rather than that repo's code. A
# line that turns out to be durable and repo-shaped is promoted by a person.
LEARNED = re.compile(r"^RIGG-LEARNED:\s*(\S[^\n]*?)\s*$", re.M)

NOTES_HEADER = """\
# What scheduled runs here have worked out

Appended from `RIGG-LEARNED:` lines, oldest first, and read back by the `do`
prompt. A scratchpad, not a record: prune it. A line that holds for a repo
rather than for this workspace belongs in that repo's `.rigg/capabilities.md`
instead — move it there by hand.
"""


def learned(output: str) -> list[str]:
    seen: list[str] = []
    for line in LEARNED.findall(output or ""):
        if line not in seen:
            seen.append(line)
    return seen


def notes_path(inst: str | None = None) -> Path:
    root = Path(os.environ.get("RIGG_HOME") or (Path.home() / ".rigg"))
    return root / "instances" / (inst or instance()) / "notes.md"


def note(lines: list[str], job: dict) -> list[str]:
    """Append what is new to this instance's notes; return what was added."""
    if not lines:
        return []
    p = notes_path()
    p.parent.mkdir(parents=True, exist_ok=True)
    body = p.read_text() if p.exists() else NOTES_HEADER
    # Said before is not worth saying again — a job that learns the same thing
    # every morning would otherwise write a page a week.
    added = [line for line in lines if line not in body]
    if not added:
        return []
    stamp = datetime.now().strftime("%Y-%m-%d")
    body = body.rstrip("\n") + "\n" + "".join(
        f"\n- {stamp} (job {job['id']}, #{job['channel']}): {line}" for line in added
    ) + "\n"
    tmp = p.with_suffix(".md.tmp")
    tmp.write_text(body)
    tmp.replace(p)
    return added


def redact(text: str, secrets: dict[str, str]) -> str:
    """Credential values out of a run's own output, by name.

    An agent that inlines a token instead of naming the variable puts it in the
    output — and the output is stored in the job file and posted back by
    `cron show`. Short values are left alone: blanking an eight-character one
    would take half the report with it.
    """
    for name, value in (secrets or {}).items():
        if value and len(value) >= 8:
            text = text.replace(value, f"<{name}>")
    return text


def secret_requests(output: str) -> list[tuple[str, str, str]]:
    seen: dict[str, tuple[str, str, str]] = {}
    for name, why, where in NEEDS_SECRET.findall(output or ""):
        seen[name] = (name, (why or "").strip(), (where or "").strip())
    return list(seen.values())


# --------------------------------------------------------------------- store

def command(repo, prefix, rest, say, channel_id, *, pipelines, run_rigg,
            split_modifiers, features) -> None:
    """`cron` — hand the words to `rigg cron` and render what comes back.

    rigg owns the schedules: the expressions, the store, the overlap guard and
    the tick. This used to keep its own copy of all four, which meant a job
    typed here and a job declared in a repo were different things that could
    not see each other. Now it translates, like every other verb.
    """
    args = shlex.split(rest or "")

    # A job made here reports here. Without this a digest would go to whatever
    # the instance's default channel is, which is somebody else's channel.
    if args and args[0] == "add" and "--channel" not in args:
        args += ["--channel", f"#{prefix}"]
    # And a listing here is this channel's jobs, not the whole instance's.
    if not args or args[0] in ("list", "ls"):
        args = ["list", "--channel", f"#{prefix}"]

    # A run is the one verb here that takes minutes. Without a word first,
    # Slack looks like it swallowed the message, and the reflex is to send it
    # again - which is how you get two of whatever it was doing.
    if args and args[0] == "run":
        which = f"`{args[1]}`" if len(args) > 1 else "it"
        say(f"running {which} now — this takes a minute or two, result lands here.")

    code, out = run_rigg(repo, ["cron", *args], timeout=300)
    out = redact(out, creds.current()).strip()
    if not out:
        out = "done." if code == 0 else f"`rigg cron` failed ({code}) and said nothing."
    say(out if len(out) < 3500 else out[:3500] + "\n…")
