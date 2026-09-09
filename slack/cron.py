"""Recurring jobs, defined from Slack rather than deployed.

    @rigg cron 0 7 * * 1-5 digest          # weekday mornings
    @rigg cron @daily new tidy up the TODOs
    @rigg cron                             # what this channel has
    @rigg cron rm 2

A job belongs to the channel it was defined in: it runs against that channel's
repo and reports back there. The store is one file per instance,
`~/.rigg/instances/<instance>/cron.json`, so two workspaces never see each other's jobs.

The bridge is already the per-instance daemon, so the schedule lives in it
rather than in systemd. That means jobs only fire while the bridge is up —
which is the same window in which anyone could have typed the command anyway.

A scheduled pipeline runs with `rigg run --pipeline`, in the checkout and
without a branch. Interactively a pipeline word means `new`, but a job that
made a branch every morning would leave 365 of them a year; a recurring job
that reads and reports wants neither a branch nor a worktree. Say `new`
explicitly when you do want one.
"""

import json
import os
import re
import threading
import time
import traceback
from datetime import datetime, timedelta
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


def _field(text: str, lo: int, hi: int, label: str) -> set[int]:
    out: set[int] = set()
    for part in text.split(","):
        step = 1
        if "/" in part:
            part, _, s = part.partition("/")
            if not s.isdigit() or int(s) < 1:
                raise ValueError(f"`{s}` is not a step in the {label} field")
            step = int(s)
        if part in ("*", ""):
            start, end = lo, hi
        elif "-" in part.lstrip("-"):
            a, _, b = part.partition("-")
            if not (a.isdigit() and b.isdigit()):
                raise ValueError(f"`{part}` is not a range in the {label} field")
            start, end = int(a), int(b)
        elif part.isdigit():
            start = end = int(part)
        else:
            raise ValueError(f"`{part}` is not a {label}")
        # Sunday is 0, but 7 is the other spelling and people use both.
        if label == "weekday":
            start, end = (0 if start == 7 else start), (0 if end == 7 else end)
        if not (lo <= start <= hi and lo <= end <= hi) or start > end:
            raise ValueError(f"`{part}` is outside {lo}-{hi} in the {label} field")
        out |= set(range(start, end + 1, step))
    return out


def parse(expr: str):
    """A cron expression as five sets, or ValueError saying which field is wrong."""
    expr = ALIASES.get(expr.strip().lower(), expr).strip()
    parts = expr.split()
    if len(parts) != 5:
        raise ValueError(
            f"a schedule is five fields (minute hour day month weekday) or an "
            f"alias like `@daily` — got {len(parts)}"
        )
    sets = [_field(p, lo, hi, name) for p, (name, lo, hi) in zip(parts, FIELDS)]
    # Vixie cron's rule, and the one everybody gets wrong: when *both* day and
    # weekday are restricted the job runs when *either* matches, not both.
    return {
        "minute": sets[0], "hour": sets[1], "day": sets[2],
        "month": sets[3], "weekday": sets[4],
        "day_restricted": parts[2] != "*", "weekday_restricted": parts[4] != "*",
    }


def _date_matches(spec: dict, when: datetime) -> bool:
    """The day part alone, so a day that cannot match can be skipped whole."""
    if when.month not in spec["month"]:
        return False
    dom = when.day in spec["day"]
    dow = (when.isoweekday() % 7) in spec["weekday"]
    if spec["day_restricted"] and spec["weekday_restricted"]:
        return dom or dow
    return dom and dow


def matches(spec: dict, when: datetime) -> bool:
    return (when.minute in spec["minute"] and when.hour in spec["hour"]
            and _date_matches(spec, when))


def next_times(spec: dict, start: datetime, count: int = 3) -> list[datetime]:
    """The next few firings, so a typo in a schedule is visible immediately.

    A minute at a time over the year this is allowed to search is half a
    million steps, and it runs on the Slack path every time someone lists their
    jobs — so a day that cannot match is stepped over whole, and so is an hour.
    """
    out: list[datetime] = []
    t = start.replace(second=0, microsecond=0) + timedelta(minutes=1)
    limit = t + timedelta(days=366)
    while t < limit and len(out) < count:
        if not _date_matches(spec, t):
            t = (t + timedelta(days=1)).replace(hour=0, minute=0)
            continue
        if t.hour not in spec["hour"]:
            t = (t + timedelta(hours=1)).replace(minute=0)
            continue
        if t.minute in spec["minute"]:
            out.append(t)
        t += timedelta(minutes=1)
    return out


# How a run asks for a credential it turns out to need. An unattended job
# cannot stop and wait for someone, so it says what is missing and stops; the
# request is relayed to the channel, and the next firing has it.
#
#   RIGG-NEEDS-SECRET: FRONT_API_TOKEN | a Front token with scopes … | <url>
#
# Only the name is required. This is a line of output rather than an exit code
# so that any step - an agent turn, a shell script, an MCP server - can raise
# it without rigg having to know which.
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

def instance() -> str:
    return os.environ.get("RIGG_INSTANCE") or "default"


def store_path(inst: str | None = None) -> Path:
    # Not cron.json: `rigg cron` owns that file, with a different row shape -
    # an int id here against a string id there, and epoch seconds against
    # "YYYY-MM-DD HH:MM". Sharing the name meant whichever wrote last silently
    # broke the other's reader. The bridge's own scheduler is redundant with
    # `rigg tick` and should go; until it does, it stores its own.
    root = Path(os.environ.get("RIGG_HOME") or (Path.home() / ".rigg"))
    return root / "instances" / (inst or instance()) / "cron-slack.json"


def load(inst: str | None = None) -> list[dict]:
    p = store_path(inst)
    if not p.exists():
        return []
    try:
        return json.loads(p.read_text())
    except Exception:
        return []


_lock = threading.Lock()

# Which jobs are mid-run. `rigg run` takes no lock on the checkout, so nothing
# below this stops two agent turns in one working tree — a `*/5` job whose run
# takes six minutes reaches that within the hour.
_running_lock = threading.Lock()
_running: dict[int, float] = {}


def running() -> dict[int, float]:
    """Job id -> when the firing in flight started."""
    with _running_lock:
        return dict(_running)


def claim(job_id: int) -> bool:
    """True if this thread may run the job, false if the last one still is."""
    with _running_lock:
        if job_id in _running:
            return False
        _running[job_id] = time.time()
        return True


def release(job_id: int) -> None:
    with _running_lock:
        _running.pop(job_id, None)


def update(job_id: int, **fields) -> None:
    """Change one job in place, without losing one added a moment ago.

    The scheduler writes `last_run` while someone in Slack may be adding a job,
    and `save` rewrites the whole file — so the read and the write have to be
    one step.
    """
    with _lock:
        jobs = load()
        for j in jobs:
            if j["id"] == job_id:
                j.update(fields)
        save(jobs)


def save(jobs: list[dict], inst: str | None = None) -> None:
    p = store_path(inst)
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(jobs, indent=2))
    tmp.replace(p)  # atomic, so a crash mid-write cannot lose every job


# The pipeline a free-text job runs. A repo that has one can be told what to do
# in a sentence; one that does not must name a pipeline itself.
FREEFORM_PIPELINE = "do"


def plan(command_text: str, known: list[str]) -> tuple[str, str, str] | None:
    """What a typed command means: (kind, pipeline, task), or None if nothing.

    `new <task>` starts a branch. A known pipeline name runs that pipeline.
    Anything else is taken as plain instructions for the `do` pipeline, which
    is the point — a schedule should be sayable without knowing any of this.
    """
    verb, _, rest = command_text.partition(" ")
    rest = rest.strip()
    if verb == "new":
        return ("new", "", rest) if rest else None
    if verb in known:
        return ("pipeline", verb, rest)
    if FREEFORM_PIPELINE in known and command_text.strip():
        return ("freeform", FREEFORM_PIPELINE, command_text.strip())
    return None


def args_for(job: dict) -> list[str] | None:
    """The rigg command line a job runs, or None if it is not runnable."""
    kind = job.get("kind")
    # `+mail`, `effort=high`, `ai=opencode` — the same modifiers a typed task
    # takes. Stripped when the job was defined rather than every firing, so
    # `cron show` can print the command line the morning will actually run.
    mods = job.get("mods") or []
    if kind == "new":
        task = job.get("task", "")
        return ["new", f"{job['channel']}/cron-{job['id']}", task] + mods if task else None
    if kind in ("pipeline", "freeform"):
        args = ["run", "--pipeline", job["pipeline"]]
        return args + (["--task", job["task"]] if job.get("task") else []) + mods
    return None


# ------------------------------------------------------------------- command

def command(repo, prefix, rest, say, channel_id, *, pipelines, run_rigg,
            split_modifiers, features) -> None:
    """`cron` — list, define or remove this channel's recurring jobs."""
    jobs = load()
    mine = [j for j in jobs if j["channel"] == prefix]
    rest = (rest or "").strip()

    if not rest:
        if not mine:
            say(
                "no recurring jobs here yet.\n"
                "`cron <schedule> <what>` — e.g. `cron 0 7 * * 1-5 digest`, or "
                "`cron @daily digest`.\nThe schedule is five cron fields "
                "(minute hour day month weekday)."
            )
            return
        lines = []
        in_flight = running()
        for j in mine:
            spec = parse(j["schedule"])
            nxt = next_times(spec, datetime.now(), 1)
            when = nxt[0].strftime("%a %d %b %H:%M") if nxt else "never again"
            # The last run matters more than the next one: a job that stopped
            # firing is otherwise invisible until someone notices the silence.
            if j["id"] in in_flight:
                last = "running now"
            elif j.get("last_status"):
                ran = datetime.fromtimestamp(j["last_run"]).strftime("%a %d %b %H:%M")
                last = f"last {j['last_status']} {ran}"
            else:
                last = "never run"
            lines.append(
                f"`{j['id']}`  `{j['schedule']}`  {j['command']}  — {last}, next {when}"
            )
        say("recurring jobs here:\n" + "\n".join(lines)
            + "\n\n`cron show <id>` for the detail, `cron run <id>` to run it now, "
              "`cron rm <id>` to remove it.")
        return

    head, _, tail = rest.partition(" ")

    if head in ("show", "inspect"):
        hit = next((j for j in mine if str(j["id"]) == tail.strip()), None)
        if hit is None:
            say(f"no job `{tail.strip()}` here — `cron` lists them.")
            return
        spec = parse(hit["schedule"])
        nxt = next_times(spec, datetime.now(), 3)
        lines = [
            f"*job `{hit['id']}`* in #{hit['channel']}",
            f"schedule  `{hit['schedule']}`",
            f"next      {', '.join(t.strftime('%a %d %b %H:%M') for t in nxt) or 'never'}",
            f"repo      {hit['repo']}",
            f"asked     {hit.get('task') or hit['command']}",
            f"runs      `rigg {' '.join(args_for(hit) or ['(nothing runnable)'])}`",
        ]
        if hit.get("last_status"):
            ran = datetime.fromtimestamp(hit["last_run"]).strftime("%a %d %b %H:%M")
            lines.append(f"last      {hit['last_status']} at {ran}")
        if hit["id"] in running():
            lines.append("state     running now")
        # The steps are the pipeline's, not the job's — the job names one and
        # rigg decides the rest — so show what rigg says it will do.
        if hit.get("pipeline"):
            # Not `plan` as a name: assigning it here would make the module's
            # plan() a local for the whole function and break every other path.
            code, steps = run_rigg(Path(hit["repo"]),
                                   ["plan", "--pipeline", hit["pipeline"]], timeout=30)
            if code == 0 and steps.strip():
                lines.append(f"\nsteps of `{hit['pipeline']}`:\n```\n{steps.strip()[:1200]}\n```")
        if hit.get("last_output"):
            lines.append(f"\nlast output:\n```\n{hit['last_output'][-1200:]}\n```")
        say("\n".join(lines))
        return

    if head in ("run", "now", "test"):
        hit = next((j for j in mine if str(j["id"]) == tail.strip()), None)
        if hit is None:
            say(f"no job `{tail.strip()}` here — `cron` lists them.")
            return
        if _scheduler is None:
            say("nothing is scheduling here, so there is nothing to run it with.")
            return
        if hit["id"] in running():
            say(f"`{hit['id']}` is already running — it reports here when it lands.")
            return
        _scheduler.fire_now(hit)
        say(f"running `{hit['id']}` now — `{hit.get('task') or hit['command']}`. "
            f"It reports here when it is done; `cron show {hit['id']}` has the detail.")
        return

    if head in ("rm", "remove", "delete"):
        wanted = tail.strip()
        hit = next((j for j in mine if str(j["id"]) == wanted), None)
        if hit is None:
            say(f"no job `{wanted}` here — `cron` lists them.")
            return
        save([j for j in jobs if j is not hit])
        say(f"removed `{hit['id']}` — `{hit['schedule']}` {hit['command']}")
        return

    # A word where a schedule should be is a mistyped sub-verb far more often
    # than it is a schedule, and complaining about cron fields sends people
    # looking in the wrong place. A minute field is only digits and * , - /.
    if not head.startswith("@") and not FIELD_CHARS.match(head):
        say(
            f"`{head}` is neither a schedule nor something I know how to do.\n"
            f"`cron <schedule> <what>` to add one, or "
            f"`cron show <id>`, `cron run <id>`, `cron rm <id>`. "
            f"`cron` on its own lists them."
        )
        return

    # A schedule is an alias or exactly five fields; the rest is the command.
    if head.startswith("@"):
        schedule, command_text = head, tail.strip()
    else:
        parts = rest.split()
        if len(parts) < 6:
            say(
                "that is a schedule with nothing to run. "
                "`cron <minute> <hour> <day> <month> <weekday> <what>` — "
                "e.g. `cron 0 7 * * 1-5 digest`."
            )
            return
        schedule, command_text = " ".join(parts[:5]), " ".join(parts[5:])

    if not command_text:
        say(
            "that is a schedule with nothing to run — "
            "`cron @daily digest`, or `cron 0 7 * * 1-5 digest`."
        )
        return

    try:
        spec = parse(schedule)
    except ValueError as e:
        say(f"{e}. A schedule looks like `0 7 * * 1-5` (07:00, Monday to Friday).")
        return

    # `+mail`, `effort=high`, `ai=opencode` — a schedule takes what a typed
    # task takes. Off the end before the rest is read as a command, or `digest
    # +mail` would schedule a pipeline nobody has, and a freeform job would
    # carry `+mail` into the prompt as two words of instruction.
    command_text, mods = split_modifiers(command_text)

    # A part this repo does not declare is a config error to rigg, so the job
    # would be accepted here and die at 07:00 — which is exactly what the rest
    # of this refuses to do. `+mail` is the one people will get wrong, since
    # registering a corpus and giving a role the tools to search it are two
    # different acts and only the second one makes a part.
    parts = features(repo)
    unknown = [mods[i + 1] for i, m in enumerate(mods)
               if m in ("--with", "--without") and mods[i + 1] not in parts]
    if unknown:
        say(
            f"{', '.join(f'`{u}`' for u in unknown)} "
            f"{'is not a part' if len(unknown) == 1 else 'are not parts'} of this "
            f"repo. It has {', '.join(f'`{p}`' for p in parts) or 'none'} — "
            f"`parts` lists what each one contains.\n"
            f"A corpus is not a part until a role in `.rigg/rigg.toml` holds the "
            f"tools to search it; `corpus show <name>` prints that config."
        )
        return

    known = pipelines(repo)
    made = plan(command_text, known)
    if made is None:
        if command_text.split(" ", 1)[0] == "new":
            say("`new` on a schedule still needs a task: `cron @daily new <task>`")
        else:
            say(
                f"I cannot run that on a schedule. This repo has "
                f"{', '.join(f'`{p}`' for p in known) or 'no pipelines'}, and no "
                f"`{FREEFORM_PIPELINE}` pipeline to take plain instructions — so "
                f"name a pipeline, or `new <task>` to start a branch each time."
            )
        return
    kind, pipeline_name, task = made

    job = {
        "id": max([j["id"] for j in jobs], default=0) + 1,
        "channel": prefix,
        "channel_id": channel_id,
        "repo": str(repo),
        "schedule": schedule,
        "command": command_text,
        "kind": kind,
        "pipeline": pipeline_name,
        "task": task,
        "mods": mods,
        "created_at": int(time.time()),
        # Display only — what stops this minute firing again is the scheduler
        # having already evaluated it, not this field.
        "last_run": int(time.time()),
    }
    jobs.append(job)
    save(jobs)

    when = ", ".join(t.strftime("%a %d %b %H:%M") for t in next_times(spec, datetime.now()))
    how = {
        "new": "a new branch each time",
        "pipeline": f"the `{pipeline_name}` pipeline, no branch",
        "freeform": f"the `{FREEFORM_PIPELINE}` pipeline, no branch",
    }[kind]
    # The modifiers are the half most worth echoing: `+mail` is what decides
    # whether the 07:00 draft has anything to draft from, and a part this repo
    # does not have fails the run rather than the definition.
    extra = f"\nwith: `{' '.join(mods)}`" if mods else ""
    say(f"`{job['id']}` scheduled: `{schedule}` → {how}{extra}"
        f"\n> {task or command_text}\nnext: {when}")


# ----------------------------------------------------------------- scheduler

# Set when the bridge starts one, so `cron run <id>` fires a job through the
# same path a schedule does rather than a second, subtly different one.
_scheduler = None


class Scheduler(threading.Thread):
    """Fires due jobs, one minute at a time.

    A minute is evaluated once. Minutes that pass while the machine is asleep
    are not caught up on: a laptop opened after a long weekend should not fire
    three days of digests at once.
    """

    def __init__(self, run_rigg, post, log=None):
        super().__init__(daemon=True, name="rigg-cron")
        self.run_rigg = run_rigg
        self.post = post
        self.log = log or (lambda m: print(m, flush=True))
        self._last_minute: datetime | None = None
        global _scheduler
        _scheduler = self

    def run(self) -> None:
        while True:
            try:
                self.tick(datetime.now())
            except Exception:
                self.log(traceback.format_exc())
            time.sleep(20)

    def tick(self, now: datetime) -> None:
        minute = now.replace(second=0, microsecond=0)
        if self._last_minute == minute:
            return
        self._last_minute = minute
        jobs = load()
        for job in jobs:
            try:
                spec = parse(job["schedule"])
            except ValueError:
                continue
            if matches(spec, minute):
                threading.Thread(
                    target=self._fire, args=(job,), daemon=True,
                    name=f"rigg-cron-{job['id']}",
                ).start()

    def fire_now(self, job: dict) -> None:
        """Run a job this second, off-schedule, on its own thread."""
        threading.Thread(target=self._fire, args=(job,), daemon=True,
                         name=f"rigg-cron-now-{job['id']}").start()

    def _fire(self, job: dict) -> None:
        args = args_for(job)
        if args is None:
            return
        if not claim(job["id"]):
            self.log(f"cron {job['id']}: last firing still running, skipped")
            return
        try:
            self._run(job, args)
        finally:
            release(job["id"])

    def _run(self, job: dict, args: list[str]) -> None:
        repo = Path(job["repo"])
        # A repo that has been moved or removed fails identically every firing.
        # Say it once, in words, rather than a traceback every five minutes.
        if not repo.is_dir():
            self._settle(
                job, "no repo", str(repo),
                f"`{job['id']}` cannot run: nothing at `{repo}` any more. "
                f"`cron rm {job['id']}` if it has gone for good.")
            return

        self.log(f"cron {job['id']}: rigg {' '.join(args)}")
        try:
            code, out = self.run_rigg(repo, args, timeout=7200)
        except Exception:
            update(job["id"], last_run=int(time.time()), last_status="failed")
            self.post(job["channel_id"], f"`{job['id']}` failed to start:\n```\n"
                                         f"{traceback.format_exc()[-600:]}\n```")
            return

        # Before anything keeps or posts this: a token the run echoed into its
        # own output is otherwise stored in the job file and read back out by
        # `cron show`.
        out = redact(out, creds.current())

        # A run that stopped for want of a credential is not a failure to go
        # and read a log about - it is a question, and it has an answer that
        # takes one message.
        wanted = secret_requests(out)
        if wanted:
            lines = []
            for name, why, where in wanted:
                lines.append(f"*{name}* — {why or 'needed by this job'}")
                if where:
                    lines.append(f"  get one at {where}")
                lines.append(f"  send it with `secret {name} <value>`")
            self._settle(
                job, "needs a secret", ",".join(sorted(n for n, _, _ in wanted)),
                f"`{job['id']}` stopped: it needs a credential it does not have.\n"
                + "\n".join(lines)
                + "\n\nIt will run as scheduled once that is set, and will not ask "
                  "again in the meantime.",
                output=out)
            return

        # The same shape as a missing credential, and for the same reason: a
        # job drafting from a corpus nobody has built is a question with an
        # answer, not a failure to go and read a log about.
        missing = corpora.requests_in(out)
        if missing:
            self._settle(
                job, "needs a corpus", ",".join(sorted(n for n, _ in missing)),
                f"`{job['id']}` stopped: it wants a corpus this instance does "
                f"not have.\n" + corpora.ask_for(missing)
                + "\n\nIt will run as scheduled once that exists, and will not "
                  "ask again in the meantime.",
                output=out)
            return

        noted = self._note(job, out)

        update(job["id"], last_run=int(time.time()),
               last_status="ok" if code == 0 else "failed", last_detail="",
               # Kept so `cron show` can answer "what did it actually do" —
               # a `run` has no branch, so there is no `rigg logs` to read.
               last_output=out[-4000:])
        if code != 0:
            # Not held back the way the two states above are: a failure is
            # rarely the same failure twice, and quiet is how a job that has
            # stopped working goes unnoticed.
            self.post(job["channel_id"],
                      f"`{job['id']}` (`{job['command']}`) failed:\n```\n{out[-1500:]}\n```")
            return

        said = report(out)
        if noted:
            said = ((said or "") + "\n\n_noted for next time:_\n"
                    + "\n".join(f"• {n}" for n in noted[:3])).strip()
        if said:
            # Slack refuses a message over 4000 characters, and a truncated
            # report is better than none at all.
            self.post(job["channel_id"], said[:3800])

    def _settle(self, job: dict, status: str, detail: str, message: str,
                output: str = "") -> None:
        """Record a state a job can be stuck in, and say so the first time.

        A `*/5` job missing one token asked the channel 288 times a day. The
        question does not change until the answer does.
        """
        repeat = (job.get("last_status") == status
                  and job.get("last_detail") == detail)
        fields = {"last_run": int(time.time()), "last_status": status,
                  "last_detail": detail}
        if output:
            fields["last_output"] = output[-4000:]
        update(job["id"], **fields)
        if repeat:
            self.log(f"cron {job['id']}: still {status}, said already")
            return
        self.post(job["channel_id"], message)

    def _note(self, job: dict, out: str) -> list[str]:
        """Keep what the run said was worth keeping. Never fatal to the run."""
        try:
            return note(learned(out), job)
        except Exception:
            self.log(traceback.format_exc())
            return []
