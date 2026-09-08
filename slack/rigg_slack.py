#!/usr/bin/env python3
"""Slack front end for rigg.

    uv run --with slack-bolt slack/rigg_slack.py

Every channel maps to one repository, and every stack a channel starts is named
`<channel>/<slug>`. That is the whole of the per-channel scoping: rigg keeps one
flat namespace of stacks per repo, so the prefix is what keeps two channels
working in the same repo out of each other's way, and what lets this bridge show
a channel only its own work.
"""

from __future__ import annotations

import json
import mimetypes
import os
import random
import re
import string
import shutil
import subprocess
import sys
import traceback
import tempfile
import threading
import urllib.error
import urllib.request
import tomllib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
RIGG = os.environ.get("RIGG_BIN", str(HERE.parent / "target" / "release" / "rigg"))

# Grouped by what you are trying to do, because an alphabetical list of verbs
# is only useful to someone who already knows which one they want.
HELP_GROUPS = [
    ("Start work", [
        ("new <task>", "do it, review it, fix what the review found, and put it on a URL"),
        ("adopt <branch>", "the same on a branch that already exists — preview it, review it"),
        ("add <stack> <task>", "stack another branch on top of one"),
    ]),
    ("While it is running", [
        ("say <stack> <message>", "a follow-up on that branch; commits and pushes it"),
        ("stop <stack>", "cancel it — `cancel` works too"),
        ("continue <stack> +part", "take it further, e.g. `continue foo +copilot`"),
        ("retry <stack> [step]", "run the same thing again"),
    ]),
    ("Ask about the code", [
        ("ask <question>", "no branch, nothing changed; then `@rigg <more>` in its thread"),
    ]),
    ("Have a look", [
        ("stacks", "one message per stack — reply in one to talk to it"),
        ("summary <stack>", "ask it what it did, what was wrong, and what is left"),
        ("logs <stack>", "the tail of a run's log"),
        ("urls <stack>", "links the run printed — previews, PRs"),
        ("parts", "the optional parts you can add"),
    ]),
    ("When it has landed", [
        ("rm <stack>", "what removing it would do"),
        ("rm <stack> yes", "actually remove it, stopping whatever it started"),
    ]),
]

# Put on the end of a task to change one run. Everything here is optional; the
# whole point is that `new <task>` on its own already does the sensible thing.
MODIFIERS = [
    ("+copilot", "also wait for Copilot's review and apply it"),
    ("-push", "on a `say`: change the branch without pushing"),
    ("-preview", "skip the running copy"),
    ("effort=high", "a heavier review — low, medium, high, max"),
    ("ai=opencode", "run it on a different agent"),
]


def short_alias(name: str, names: list[str]) -> str:
    """The shortest obvious way to type a pipeline name."""
    if "-" in name:
        tail = name.rsplit("-", 1)[1]
        if len([n for n in names if tail in n]) == 1:
            return tail
    return name


def help_for(channel: "Channel") -> str:
    lines = [f"*rigg* — #{channel.name} drives `{channel.repo}`"]
    names = pipelines(channel.repo)

    for title, entries in HELP_GROUPS:
        rows = [(c, d) for c, d in entries if channel.may(c.split()[0]) is None]
        # A repo that names variations of its pipeline offers those here too.
        if title == "Start work" and channel.may("new") is None and names:
            rows += [
                (f"{short_alias(n, names)} <task>", f"the same, on `{n}`")
                for n in names
            ]
        if not rows:
            continue
        width = max(len(c) for c, _ in rows)
        lines.append(f"\n*{title}*")
        lines += [f"`{c.ljust(width)}`  {d}" for c, d in rows]

    if channel.may("new") is None:
        width = max(len(m) for m, _ in MODIFIERS)
        lines.append("\n*Put on the end of a task, in any order*")
        lines += [f"`{m.ljust(width)}`  {d}" for m, d in MODIFIERS]
        lines.append("`parts` lists every part this repo has.")

    lines.append(
        "\nStacks here are named `%s/<slug>` and belong to this channel — "
        "refer to them by the short name, and other channels cannot see them."
        % channel.name
    )
    if channel.commands is not None:
        lines.append("This channel is read-only for anything not listed.")
    return "\n".join(lines)


# --------------------------------------------------------------------------
# config


# Commands that put an agent to work, as opposed to looking at what it did.
MUTATING = {"new", "adopt", "add", "say"}


class Channel:
    """One channel's repo, and which commands it may run.

    *Who* may run them is Slack's business: membership of the channel is the
    access control. That is the whole reason an unmapped channel does nothing,
    and the reason a private channel is the natural place to put this - the
    bridge would only be duplicating, and then drifting from, a list Slack
    already keeps.
    """

    def __init__(self, name: str, raw: dict):
        self.name = name
        self.repo = Path(raw["repo"]).expanduser()
        # Absent means every command; a list narrows it, e.g. ["stacks", "logs"]
        # for a channel that should be able to look but not launch.
        allowed = raw.get("commands")
        self.commands: set[str] | None = set(allowed) if allowed is not None else None

    def may(self, verb: str) -> str | None:
        """Why this is refused, or None if it is allowed."""
        if self.commands is not None and verb not in self.commands:
            allowed = ", ".join(f"`{c}`" for c in sorted(self.commands))
            return f"`{verb}` is not allowed in #{self.name} — only {allowed}"
        return None

    def mutating(self) -> bool:
        return self.commands is None or bool(self.commands & MUTATING)


class Config:
    def __init__(self, path: Path):
        self.path = path
        self.mtime = path.stat().st_mtime
        with path.open("rb") as fh:
            raw = tomllib.load(fh)
        self.channels: dict[str, Channel] = {
            name: Channel(name, c) for name, c in raw.get("channels", {}).items()
        }
        for name, ch in self.channels.items():
            if not (ch.repo / ".git").exists():
                raise SystemExit(f"channel #{name}: {ch.repo} is not a git repository")

    def reload_if_changed(self) -> "Config":
        """Pick up an edited channels.toml without a restart.

        A channel still has to be mapped before it can do anything - that
        mapping is what says which repo it drives, and an unmapped channel
        doing nothing is the point of it. Making that cost a restart, though,
        was only friction.
        """
        try:
            if self.path.stat().st_mtime == self.mtime:
                return self
            fresh = Config(self.path)
        # SystemExit is not an Exception, and Config raises it for a repo that
        # is not there - which is exactly the typo this must survive.
        except (Exception, SystemExit) as e:
            # Keep serving the last good config rather than dying on a typo.
            print(f"channels.toml not reloaded: {e}")
            try:
                self.mtime = self.path.stat().st_mtime
            except OSError:
                pass
            return self
        print(f"channels.toml reloaded - {len(fresh.channels)} channel(s): "
              f"{', '.join('#' + n for n in fresh.channels)}")
        return fresh


# --------------------------------------------------------------------------
# running rigg


def run_rigg(repo: Path, args: list[str], timeout: int = 120) -> tuple[int, str]:
    """Run rigg in `repo` and return (exit code, combined output)."""
    try:
        p = subprocess.run(
            [RIGG, *args],
            cwd=repo,
            capture_output=True,
            text=True,
            timeout=timeout,
            stdin=subprocess.DEVNULL,
            # A command line is no use in a message: anything rigg suggests
            # back should be typable where it is being read.
            env={**os.environ, "RIGG_ADDRESSED_AS": "@rigg"},
        )
    except subprocess.TimeoutExpired:
        return 1, f"`rigg {' '.join(args)}` timed out after {timeout}s"
    except FileNotFoundError:
        return 1, f"no rigg binary at {RIGG} — build it, or set RIGG_BIN"
    return p.returncode, (p.stdout + p.stderr).strip()


def first_backticked(text: str) -> str | None:
    """The branch rigg named in its own output."""
    m = re.search(r"`([^`]+)`", text)
    return m.group(1) if m else None


def git_common_dir(repo: Path) -> Path:
    out = subprocess.run(
        ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
        cwd=repo, capture_output=True, text=True,
    )
    return Path(out.stdout.strip())


def ts_of(posted: object) -> str | None:
    """The thread a message just posted belongs to, whatever `say` handed back.

    Not the message's own ts. An answer is posted *into* the thread that asked
    for it, so its ts identifies a reply inside that thread and no later reply
    there ever arrives carrying it - which is why `summary` in a run's own
    thread still asked which stack. `thread_ts` off the message is the thread
    itself, and a message that starts a thread (a listing) has none yet, so
    there its ts is the thread.

    Bolt returns a SlackResponse, which is dict-*like* but not a dict - so an
    isinstance check against dict quietly produced None, and not one of the
    threads this was meant to record ever was.
    """
    try:
        message = posted.get("message") or {}  # type: ignore[attr-defined]
        return message.get("thread_ts") or posted.get("ts")  # type: ignore[attr-defined]
    except (AttributeError, TypeError):
        return None


def remember_thread(repo: Path, branch: str, channel: str, thread_ts: str,
                    primary: bool = True, stack: str | None = None) -> None:
    """Record a thread that belongs to a branch.

    `primary` is where a run reports: the notify hook reads `thread_ts`, which
    is what lets a detached run find the thread that asked for it long after
    this process has forgotten. Every thread is also kept in `threads`, so a
    reply in any of them can be understood as being about this branch -
    a status listing gives a stack a thread without that being where its runs
    should suddenly start reporting.
    """
    d = git_common_dir(repo) / "rigg" / "slack"
    d.mkdir(parents=True, exist_ok=True)
    f = d / f"{branch.replace('/', '-')}.json"
    try:
        data = json.loads(f.read_text())
    except (OSError, ValueError):
        data = {}
    # The branch is stored as well as being the filename, because the filename
    # flattens its slash and cannot be turned back.
    data["branch"] = branch
    data["channel"] = channel
    # Usually the branch is inside the stack's own name and saying it twice
    # would be noise. An adopted branch is not - it was named by whoever made
    # it - so the stack has to be recorded for a reply in the thread to reach
    # it by the name this channel knows.
    if stack and stack != branch:
        data["stack"] = stack
    if primary or not data.get("thread_ts"):
        data["thread_ts"] = thread_ts
    threads = [t for t in data.get("threads", []) if t != thread_ts]
    data["threads"] = (threads + [thread_ts])[-10:]
    f.write_text(json.dumps(data))


def stack_for_thread(repo: Path, channel: str, thread_ts: str | None) -> str | None:
    """The stack whose progress is reported in this thread, if any.

    Replying where a branch already reports is the clearest way of saying which
    branch you mean, and costs no typing at all.
    """
    if not thread_ts:
        return None
    d = git_common_dir(repo) / "rigg" / "slack"
    if not d.is_dir():
        return None
    for f in d.glob("*.json"):
        try:
            data = json.loads(f.read_text())
        except (OSError, ValueError):
            continue
        if data.get("channel") != channel:
            continue
        known = set(data.get("threads", []))
        if data.get("thread_ts"):
            known.add(data["thread_ts"])
        if thread_ts in known:
            return data.get("stack") or data.get("branch") or f.stem
    return None


def download_images(event: dict, token: str) -> tuple[list[str], str | None]:
    """Save any images attached to a Slack message, and say where they went.

    Slack keeps a file behind an authenticated URL, so this needs the bot token
    and the `files:read` scope - a plain fetch returns Slack's sign-in page
    with a 200, which is why the content type is checked rather than trusted.

    Returns the paths and the temp directory holding them, for the caller to
    clean up once rigg has copied them into its own staging.
    """
    files = [
        f for f in event.get("files", [])
        if str(f.get("mimetype", "")).startswith("image/")
    ]
    if not files:
        return [], None

    tmp = tempfile.mkdtemp(prefix="rigg-slack-")
    paths = []
    for i, f in enumerate(files, 1):
        url = f.get("url_private_download") or f.get("url_private")
        if not url:
            continue
        ext = (
            mimetypes.guess_extension(f["mimetype"])
            or os.path.splitext(f.get("name", ""))[1]
            or ".png"
        )
        dest = os.path.join(tmp, f"{i}{'.jpg' if ext == '.jpe' else ext}")
        req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                kind = r.headers.get("Content-Type", "")
                if not kind.startswith("image/"):
                    print(f"slack file {f.get('name')}: got {kind!r}, not an image "
                          f"— is the files:read scope granted?")
                    continue
                with open(dest, "wb") as out:
                    shutil.copyfileobj(r, out)
        except urllib.error.HTTPError as e:
            # 403 is what Slack returns without files:read. The sign-in-page
            # case above is the same cause with a different symptom, so both
            # should name it.
            hint = " — is the files:read scope granted?" if e.code == 403 else ""
            print(f"could not fetch slack file {f.get('name')}: {e}{hint}")
            continue
        except (urllib.error.URLError, OSError) as e:
            print(f"could not fetch slack file {f.get('name')}: {e}")
            continue
        paths.append(dest)
    if not paths:
        shutil.rmtree(tmp, ignore_errors=True)
        return [], None
    return paths, tmp


# Only these are stripped off the end of a task, so "make it work with caching"
# stays a task rather than becoming a request to run on an agent called
# `caching`.
AGENT_KINDS = {"claude", "opencode", "codex", "gemini"}


def split_agent(text: str, keep: int = 1) -> tuple[str, str | None]:
    """Peel a trailing `with opencode` / `on opencode` off a task.

    A word reads better than a flag for the same reason `preview` does, and
    putting it last keeps the task the thing you type first.

    `keep` is how many words have to survive: a task needs at least one, or
    "on opencode" would eat the whole of it. `continue +copilot` carries no
    task at all, and passes 0.
    """
    words = text.split()
    if len(words) >= keep + 2 and words[-2].lower() in ("with", "on", "using") \
            and words[-1].lower() in AGENT_KINDS:
        return " ".join(words[:-2]), words[-1].lower()
    return text, None


def agent_args(kind: str | None) -> list[str]:
    return ["--agent", kind] if kind else []


def split_vars(text: str, keep: int = 1) -> tuple[str, list[str]]:
    """Peel trailing `name=value` settings off a task.

    Only from the end, and only while they keep matching, so an `=` inside the
    task itself is left alone.
    """
    words = text.split()
    found: list[str] = []
    while len(words) > keep and re.fullmatch(r"[a-zA-Z_][\w-]*=\S+", words[-1]):
        found.insert(0, words.pop())
    return " ".join(words), found


# `ai=` is not a prompt placeholder but a choice of agent. Spelling both as
# name=value means there is one thing to learn instead of two.
VAR_ALIASES = {"ai": "--agent", "agent": "--agent"}


def var_args(vars: list[str]) -> list[str]:
    out: list[str] = []
    for v in vars:
        name, _, value = v.partition("=")
        flag = VAR_ALIASES.get(name.lower())
        out += [flag, value] if flag else ["--var", v]
    return out


def split_features(text: str, keep: int = 1) -> tuple[str, list[str]]:
    """Peel trailing `+preview` / `-copilot` off a task.

    `+x` rather than `with x`, because `with` already means the agent and a
    task should not have to be read twice to work out which.
    """
    words = text.split()
    args: list[str] = []
    while len(words) > keep and re.fullmatch(r"[+-][a-zA-Z][\w-]*", words[-1]):
        w = words.pop()
        args = ["--with" if w[0] == "+" else "--without", w[1:]] + args
    return " ".join(words), args


def split_modifiers(text: str, keep: int = 1) -> tuple[str, list[str]]:
    """Strip `+preview`, `effort=high` and `with opencode` off the end.

    In any order, and repeatedly, so nobody has to remember one.

    `keep` is how much has to be left behind. One word by default, because
    these come off the end of a task and a task cannot be nothing. Where there
    is no task to protect - `continue +copilot`, `adopt <branch> +preview` -
    pass 0, or the last modifier stays put and is read as a task.
    """
    args: list[str] = []
    changed = True
    while changed:
        before = text
        text, kind = split_agent(text, keep)
        args += agent_args(kind)
        text, found = split_vars(text, keep)
        args += var_args(found)
        text, feats = split_features(text, keep)
        args += feats
        changed = text != before
    return text, args


def agent_only(mods: list[str]) -> list[str]:
    """Just the agent choice out of a modifier list.

    `ask` has no features to turn on and no placeholders to fill, so `+preview`
    or `effort=high` on the end of a question is a typo rather than a setting.
    """
    return [a for i, m in enumerate(mods) if m == "--agent"
            for a in ("--agent", mods[i + 1])]


def image_args(images: list[str]) -> list[str]:
    """rigg flags for attached images.

    `--no-paste` always: rigg would otherwise read the clipboard of whatever
    desktop this bridge happens to be running on, which has nothing to do with
    the person who sent the message.
    """
    args = ["--no-paste"]
    for p in images:
        args += ["--image", p]
    return args


def slug(task: str) -> str:
    words = re.findall(r"[a-z0-9]+", task.lower())[:4]
    stem = "-".join(words)[:32].strip("-") or "task"
    tail = "".join(random.choices(string.hexdigits.lower()[:16], k=3))
    return f"{stem}-{tail}"


def pipelines(repo: Path) -> list[str]:
    code, out = run_rigg(repo, ["pipelines"])
    return out.splitlines() if code == 0 else []


def match_pipeline(repo: Path, word: str) -> tuple[str | None, str | None]:
    """Resolve a typed word to a pipeline name, or say why it did not.

    Exact first, then substring - so `preview` reaches `full-preview` without
    anyone having to know the full name, which is the point of typing a word
    rather than passing a flag.
    """
    names = pipelines(repo)
    if word in names:
        return word, None
    hits = [n for n in names if word in n]
    if len(hits) == 1:
        return hits[0], None
    if len(hits) > 1:
        return None, f"`{word}` matches {', '.join(f'`{h}`' for h in hits)} — say which"
    return None, None


def own_stacks(repo: Path, prefix: str) -> list[str]:
    code, out = run_rigg(repo, ["stack", "names"])
    if code != 0:
        return []
    return [n for n in out.splitlines() if n.startswith(f"{prefix}/")]


def filter_stack_list(out: str, prefix: str) -> str:
    """Keep only the blocks for stacks belonging to this channel."""
    kept, keeping = [], False
    for line in out.splitlines():
        if not line[:1].isspace() and line.strip():
            keeping = line.strip().startswith(f"{prefix}/")
            if keeping:
                kept.append(line)
        elif keeping:
            kept.append(line)
    return "\n".join(kept)


# --------------------------------------------------------------------------
# commands


def cmd_new(repo: Path, prefix: str, rest: str, say, channel: str,
            pipeline: str | None = None, images: list[str] | None = None) -> None:
    images = images or []
    if not rest and not images:
        say("give me a task: `new <task>`")
        return
    rest, mods = split_modifiers(rest)
    stack = f"{prefix}/{slug(rest or 'from an image')}"
    args = ["new", stack, rest] + image_args(images) + mods
    if pipeline:
        args += ["--pipeline", pipeline]
    code, out = run_rigg(repo, args)
    if code != 0:
        say(f"could not start `{stack}`:\n```\n{out[:2500]}\n```")
        return
    posted = say(f"started `{stack}`\n```\n{out[:1500]}\n```")
    ts = ts_of(posted)
    if ts:
        remember_thread(repo, stack, channel, ts)


def local_branch(repo: Path, name: str) -> str:
    """The branch a name means.

    `origin/feature/login` is how a branch is written when it has just been
    read off a PR page or a `git branch -a`. rigg takes the remote off itself,
    so without this the stack would be named after a name that is not the
    branch - and the thread would be recorded under one no run reports as.
    """
    def has(ref: str) -> bool:
        return subprocess.run(
            ["git", "rev-parse", "--verify", "--quiet", ref],
            cwd=repo, capture_output=True,
        ).returncode == 0

    if "/" not in name or has(f"refs/heads/{name}"):
        return name
    remote, _, rest = name.partition("/")
    remotes = subprocess.run(
        ["git", "remote"], cwd=repo, capture_output=True, text=True,
    ).stdout.split()
    if rest and remote in remotes and has(f"refs/remotes/{name}"):
        return rest
    return name


def cmd_adopt(repo: Path, prefix: str, rest: str, say, channel: str,
              pipeline: str | None = None, images: list[str] | None = None) -> None:
    """Put a pipeline on a branch that exists already.

    `new` cuts a branch and gives an agent something to do on it. Here the
    branch is there before rigg is - a colleague's PR, something started by
    hand - and what is wanted is the rest of the treatment: a preview to look
    at, a review, and then `say` and `add` to carry it on.

    The stack is named `<channel>/<branch>`, so it belongs to this channel like
    any other and the branch is still what you type to reach it.
    """
    parts = rest.split(None, 1)
    if not parts:
        say("`adopt <branch>` — which branch? Add a task after it if there is one.")
        return
    branch, task = local_branch(repo, parts[0]), (parts[1] if len(parts) > 1 else "")
    task, mods = split_modifiers(task, keep=0)
    stack = f"{prefix}/{branch}"
    args = ["adopt", branch]
    # An empty positional is a task of "", which is not the same as no task:
    # a pipeline that only previews the branch must not be asked for one.
    if task:
        args.append(task)
    args += ["--stack", stack] + image_args(images or []) + mods
    if pipeline:
        args += ["--pipeline", pipeline]
    code, out = run_rigg(repo, args)
    if code != 0:
        say(f"could not adopt `{branch}`:\n```\n{out[:2500]}\n```")
        return
    posted = say(f"took over the existing branch `{branch}`\n```\n{out[:1500]}\n```")
    ts = ts_of(posted)
    if ts:
        remember_thread(repo, branch, channel, ts, stack=stack)


def find_stack(repo: Path, prefix: str, short: str) -> tuple[str | None, str | None]:
    """Match what was typed against this channel's stacks.

    Exact first, then a prefix, then anywhere in the name - because the names
    carry a random suffix nobody should have to read back, and `in-b2b` is how
    a person refers to `in-b2b-dashboard-in-fd4`.
    """
    stacks = own_stacks(repo, prefix)
    if not short:
        return None, None
    short = short[len(prefix) + 1:] if short.startswith(f"{prefix}/") else short
    full = f"{prefix}/{short}"
    if full in stacks:
        return full, None
    tails = {s: s.split("/", 1)[1] for s in stacks}
    # A fragment has to be long enough to be a name someone meant. Without
    # this, the first word of a sentence matched: `say i want to change ...`
    # took `i` for a stack, because `i` is inside `b2b-fixes-in-the-a07`, and
    # a question was delivered to an unrelated branch as an instruction.
    tests = []
    if len(short) >= 3:
        tests.append(lambda t: t.startswith(short))
    if len(short) >= 5:
        tests.append(lambda t: short in t)
    for test in tests:
        hits = [s for s, t in tails.items() if test(t)]
        if len(hits) == 1:
            return hits[0], None
        if len(hits) > 1:
            names = ", ".join(f"`{tails[h]}`" for h in sorted(hits))
            return None, f"`{short}` matches {names} — say which"
    return None, None


def resolve(repo: Path, prefix: str, short: str, say) -> str | None:
    """Turn a short name typed in Slack into this channel's full stack name."""
    stack, ambiguous = find_stack(repo, prefix, short)
    if stack:
        return stack
    say(ambiguous or f"no stack `{short}` in this channel. `stacks` lists them.")
    return None


def cmd_add(repo: Path, prefix: str, rest: str, say, channel: str,
            pipeline: str | None = None, images: list[str] | None = None) -> None:
    parts = rest.split(None, 1)
    if len(parts) < 2:
        say("`add <stack> <task>`")
        return
    short, task = parts[0], parts[1]
    stack = resolve(repo, prefix, short, say)
    if stack is None:
        return
    task, mods = split_modifiers(task)
    code, out = run_rigg(repo, ["add", stack, task] + image_args(images or []) + mods)
    if code != 0:
        say(f"could not extend `{stack}`:\n```\n{out[:2500]}\n```")
        return
    branch = first_backticked(out) or stack
    posted = say(f"queued `{branch}`\n```\n{out[:1500]}\n```")
    ts = ts_of(posted)
    if ts:
        remember_thread(repo, branch, channel, ts)


def cmd_say(repo: Path, prefix: str, rest: str, say, channel: str,
            pipeline: str | None = None, images: list[str] | None = None) -> None:
    parts = rest.split(None, 1)
    if len(parts) < 2:
        say("`say <stack> <message>`")
        return
    short, message = parts[0], parts[1]
    stack = resolve(repo, prefix, short, say)
    if stack is None:
        return

    # Pushed unless told otherwise: the containers mount the checkout, so a
    # follow-up is live in the preview the moment it lands. Leaving the PR
    # behind by default meant the visible thing and the reviewable thing
    # quietly disagreed.
    push = True
    words = message.split()
    while words and words[-1].lower() in (
        "+push", "push", "-push", "nopush", "no-push", "and"
    ):
        w = words.pop().lower()
        if w in ("-push", "nopush", "no-push"):
            push = False
        elif w in ("+push", "push"):
            push = True
    message = " ".join(words)
    if not message:
        say("`say <stack> <message>`")
        return

    say(f"passing that to `{stack}`...")
    # A turn takes as long as it takes; the ack above is what keeps Slack happy.
    args = ["say", stack, message] + image_args(images or [])
    if not push:
        args.append("--no-push")
    code, out = run_rigg(repo, args, timeout=3600)
    tail = "\n".join(out.splitlines()[-25:])
    head = f"*{'Done' if code == 0 else 'Failed'}* — `{stack}`"

    extras = ""
    if code == 0:
        # The containers mount the checkout, so the preview already has this
        # change; saying where to look beats leaving it to be remembered.
        urls = preview_urls(repo, stack)
        if urls:
            listed = "\n".join(f"- {l}" for l in urls.splitlines())
            extras += f"\nThe preview has it already:\n{listed}"
        if not push and dirty(repo, stack):
            extras += ("\n_Not pushed_ — the change is in the branch only. "
                       "Say it again without `-push` to get it onto the PR.")

    say(f"{head}\n```\n{tail[:2000]}\n```{extras}")


def preview_urls(repo: Path, stack: str) -> str | None:
    """The URLs a preview is currently serving for this branch, if any.

    A repo that puts a preview up records them where its own teardown looks;
    reading them back is what lets a finished turn say "and here is where to
    look" rather than leaving that to be remembered.
    """
    d = git_common_dir(repo) / "rigg" / "preview" / stack.replace("/", "-")
    f = d / "urls.txt"
    try:
        text = f.read_text().strip()
    except OSError:
        return None
    return text or None


def dirty(repo: Path, stack: str) -> bool:
    """Whether a stack's checkout has uncommitted changes."""
    code, path = run_rigg(repo, ["attach", "--path", stack])
    if code != 0:
        return False
    out = subprocess.run(
        ["git", "status", "--porcelain"], cwd=path.strip(),
        capture_output=True, text=True,
    )
    return bool(out.stdout.strip())


def cmd_pipelines(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    names = pipelines(repo)
    if not names:
        say("This repo has one pipeline and no names for variations of it — "
            "use `+part` / `-part` instead. `parts` lists them.")
        return
    lines = "\n".join(f"• `{short_alias(n, names)} <task>`" for n in names)
    say(f"pipelines here:\n{lines}\n\n`new <task>` runs the default one.")


SUMMARY_PROMPT = """Summarise this branch for someone who has not been following.

Cover, briefly: what you were asked to do, what you have actually changed so
far, anything you found to be wrong and why it was wrong, and what is left.
If there was a bug, say what the root cause turned out to be rather than only
what you changed.

A short paragraph or a few bullets. Do not change any files, and do not run
anything - this is a question, not a task."""


def agent_text(out: str) -> str:
    """The agent's own words, out of a run's output.

    Everything else on the way past belongs to rigg or to the tool stream: the
    step rule, the argv echo, the per-tool `·` lines, the timing.
    """
    keep = []
    for line in out.splitlines():
        t = line.rstrip()
        if not t.strip():
            keep.append("")
            continue
        if t.startswith("----") or t.lstrip().startswith(("· ", "-> [", "$ ")):
            continue
        if re.match(r"^\s*(ok|failed)\s*\(", t) or t.startswith(("total ", "rigg", "  -> ")):
            continue
        keep.append(t)
    return "\n".join(keep).strip()


# Shortcodes do not render inside a code block, where the tables end up, so
# the common marks become characters that do.
CELL_MARKS = {
    ":white_check_mark:": "yes", ":heavy_check_mark:": "yes", ":+1:": "yes",
    ":x:": "no", ":negative_squared_cross_mark:": "no", ":no_entry:": "no",
    ":warning:": "!",
}


def plain_cell(cell: str) -> str:
    """One table cell, with what a code block cannot render taken out."""
    for code, mark in CELL_MARKS.items():
        cell = cell.replace(code, mark)
    cell = re.sub(r"\*\*(.+?)\*\*", r"\1", cell)
    cell = re.sub(r"__(.+?)__", r"\1", cell)
    return cell.strip()


def to_slack(text: str) -> str:
    """Turn an agent's Markdown into what Slack actually renders.

    Slack's mrkdwn is not Markdown: no headings, no tables, and bold is one
    asterisk rather than two. Left alone, a good answer arrives as literal
    `##` and a wall of pipes.
    """
    out: list[str] = []
    fenced = False
    table: list[str] = []

    def flush_table() -> None:
        # Slack has no tables, so lay the columns out by hand in a code block.
        # Markup does not render in there, which is why the cells are stripped
        # of it rather than left showing literal asterisks.
        if not table:
            return
        rows = []
        for r in table:
            if re.fullmatch(r"\s*\|[\s|:-]+\|\s*", r):
                continue  # the |---|---| rule
            cells = [c.strip() for c in r.strip().strip("|").split("|")]
            rows.append([plain_cell(c) for c in cells])
        table.clear()
        if not rows:
            return
        width = max(len(r) for r in rows)
        rows = [r + [""] * (width - len(r)) for r in rows]
        pads = [max(len(r[i]) for r in rows) for i in range(width)]
        laid = [
            "  ".join(c.ljust(p) for c, p in zip(row, pads)).rstrip()
            for row in rows
        ]
        out.append("```\n" + "\n".join(laid) + "\n```")

    for line in text.splitlines():
        if line.lstrip().startswith("```"):
            flush_table()
            fenced = not fenced
            # A language tag renders as part of the block in Slack.
            out.append("```")
            continue
        if fenced:
            out.append(line)
            continue

        if line.strip().startswith("|") and line.strip().endswith("|"):
            table.append(line)
            continue
        flush_table()

        # Headings become a bold line; Slack has no heading of its own.
        h = re.match(r"^\s*#{1,6}\s+(.*)$", line)
        if h:
            out.append(f"*{h.group(1).strip().rstrip('#').strip()}*")
            continue

        line = re.sub(r"\[([^\]]+)\]\((https?://[^)]+)\)", r"<\2|\1>", line)
        line = re.sub(r"\*\*(.+?)\*\*", r"*\1*", line)
        line = re.sub(r"__(.+?)__", r"*\1*", line)
        line = re.sub(r"^(\s*)[-*]\s+", r"\1• ", line)
        out.append(line)

    flush_table()
    return "\n".join(out)


def chunks(text: str, size: int) -> list[str]:
    """Split for Slack, on paragraph then line breaks rather than mid-word."""
    out, cur = [], ""
    for para in text.split("\n\n"):
        for piece in ([para] if len(para) <= size else para.splitlines()):
            if len(cur) + len(piece) + 2 > size and cur:
                out.append(cur.rstrip())
                cur = ""
            cur += piece + "\n\n"
    if cur.strip():
        out.append(cur.rstrip())
    return out or [text[:size]]


def ask_session(repo: Path, thread_ts: str | None) -> Path | None:
    """Where the conversation id for a Slack thread is kept."""
    if not thread_ts:
        return None
    d = git_common_dir(repo) / "rigg" / "slack" / "ask"
    d.mkdir(parents=True, exist_ok=True)
    return d / f"{thread_ts}.id"


def cmd_ask(repo: Path, prefix: str, rest: str, say, channel: str,
            thread_ts: str | None = None) -> None:
    """A question about the code: no branch, no worktree, nothing changed.

    A new message starts a fresh conversation and a reply inside its thread
    carries that one on, which is how Slack already reads - so nobody has to
    learn a flag for it.
    """
    if not rest:
        say("`ask <question>` — about the code; nothing is changed")
        return
    # A thread is one conversation. Resuming it by id rather than by
    # "whichever spoke last in this checkout" is what keeps two threads from
    # answering each other's questions.
    # `ai=opencode` on the end of a question, the same as on a task. rigg
    # takes a kind here as well as a role, so both spellings reach an agent.
    rest, mods = split_modifiers(rest)
    store = ask_session(repo, thread_ts)
    prior = None
    if store is not None:
        try:
            prior = store.read_text().strip() or None
        except OSError:
            prior = None

    args = ["ask", rest] + agent_only(mods)
    if prior:
        args += ["--session", prior]
    else:
        args.append("--new")
    say("_carrying on…_" if prior else "_reading the code…_")
    code, out = run_rigg(repo, args, timeout=1800)

    # Record which conversation this became, so the next reply here finds it.
    m = re.search(r"^session:\s*(\S+)", out, re.M)
    if store is not None and m:
        try:
            store.write_text(m.group(1))
        except OSError:
            pass
    if code != 0:
        say(f"could not ask:\n```\n{out[-1500:]}\n```")
        return
    text = re.sub(r"^session:.*$", "", agent_text(out), flags=re.M).strip()
    if not text:
        say(f"```\n{out[-1500:]}\n```")
        return
    for part in chunks(to_slack(text), 3500):
        say(part)


def cmd_summary(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    """Ask the agent what it has done and what is left.

    A turn rather than a log: the session already holds the task, the review
    and every change, so this needs no re-reading - which is what makes it
    cheap enough to ask casually.
    """
    if not rest:
        say("`summary <stack>`")
        return
    stack = resolve(repo, prefix, rest.split()[0], say)
    if stack is None:
        return
    say(f"asking `{stack}`...")
    # No --push: this changes nothing, and a summary is not a commit.
    code, out = run_rigg(repo, ["say", stack, SUMMARY_PROMPT], timeout=1800)
    if code != 0:
        say(f"could not ask it:\n```\n{out[-1500:]}\n```")
        return
    text = agent_text(out)
    if not text:
        say(f"```\n{out[-1500:]}\n```")
        return
    # A real summary runs past Slack's per-message limit, and truncating it
    # loses the end - which is where "what is left" lives.
    for part in chunks(to_slack(text), 3500):
        say(part)


def cmd_stop(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    if not rest:
        say("`stop <stack>` — which one?")
        return
    stack = resolve(repo, prefix, rest.split()[0], say)
    if stack is None:
        return
    code, out = run_rigg(repo, ["stop", stack])
    say(out or ("stopped" if code == 0 else "could not stop it"))


def cmd_retry(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    """Run the pipeline again on a branch, optionally from a named step."""
    # A retry is a run, so it takes what a task takes - the agent above all,
    # since retrying on the other one is half the reason to retry at all.
    rest, mods = split_modifiers(rest, keep=1)
    parts = rest.split()
    if not parts:
        say("`retry <stack> [step]` — which one?")
        return
    stack = resolve(repo, prefix, parts[0], say)
    if stack is None:
        return
    # `run` works on the checkout it is in, so find that first.
    code, dir_out = run_rigg(repo, ["attach", "--path", stack])
    if code != 0:
        say(f"no checkout for `{stack}`:\n```\n{dir_out[:800]}\n```")
        return
    args = ["run", "--detach"]
    # Carry the original pipeline forward. Without this a retry silently runs
    # the default one, whose steps may not even include the one being resumed
    # from - and the branch would come back having done something else.
    pipeline = pipeline_of(repo, stack)
    if pipeline:
        args += ["--pipeline", pipeline]
    if len(parts) > 1:
        args += ["--from", parts[1]]
    args += mods
    code, out = run_rigg(Path(dir_out.strip()), args)
    posted = say(f"```\n{out[:1500]}\n```" if out else
                 ("restarted" if code == 0 else "failed"))
    # So the run reports back here, which it cannot do for a branch that was
    # started from a terminal and has no thread recorded.
    ts = ts_of(posted)
    if ts and code == 0:
        remember_thread(repo, stack, channel, ts)


def pipeline_of(repo: Path, stack: str) -> str | None:
    """Which pipeline a branch was last run with, from its log header."""
    code, out = run_rigg(repo, ["logs", stack])
    if code != 0:
        return None
    m = re.search(r"^rigg\s+\S+\s+\(base \S+, pipeline (\S+),", out, re.M)
    if not m:
        return None
    # It may since have been renamed or removed, in which case the default is
    # a better answer than a run that refuses to start.
    return m.group(1) if m.group(1) in pipelines(repo) else None


def cmd_continue(repo: Path, prefix: str, rest: str, say, channel: str) -> None:  # noqa: C901
    """Take a branch further than the pipeline it was started with.

    Distinct from `retry`, which runs the same pipeline again: this one picks
    up after the last step that finished, so a branch run with `preview` can
    be given the Copilot round it never had.
    """
    parts = rest.split()
    if not parts:
        say("`continue <stack> [pipeline]`")
        return
    stack = resolve(repo, prefix, parts[0], say)
    if stack is None:
        return
    # --wait: forgetting a part is something you notice while it is running,
    # which is exactly when a refusal is least useful.
    args = ["continue", stack, "--wait"]
    # `continue foo +copilot` is the usual shape now that parts replaced
    # pipeline names; a bare name still works where a repo has them.
    rest_after = " ".join(parts[1:])
    _, mods = split_modifiers(rest_after, keep=0)
    if mods:
        args += mods
    elif len(parts) > 1:
        name, ambiguous = match_pipeline(repo, parts[1])
        if ambiguous:
            say(ambiguous)
            return
        if name is None:
            say(f"`{parts[1]}` is neither a part nor a pipeline — try `parts`")
            return
        args += ["--pipeline", name]
    code, out = run_rigg(repo, args, timeout=300)
    posted = say(f"```\n{out[:1500]}\n```" if out else
                 ("continuing" if code == 0 else "could not continue it"))
    # Point progress at this thread, so the answer lands where it was asked.
    ts = ts_of(posted)
    if ts and code == 0:
        remember_thread(repo, stack, channel, ts)


def cmd_urls(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    """Whatever links the last run printed - preview URLs, PRs."""
    if not rest:
        say("`urls <stack>` — which one?")
        return
    stack = resolve(repo, prefix, rest.split()[0], say)
    if stack is None:
        return
    code, out = run_rigg(repo, ["logs", stack])
    seen: list[str] = []
    for m in re.findall(r"https?://[^\s`\"'<>)]+", out):
        if m not in seen:
            seen.append(m)
    if not seen:
        say(f"no links in `{stack}`'s log yet")
        return
    say("\n".join(f"• {u}" for u in seen[-12:]))


def cmd_rm(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    """Remove a stack. A dry run unless the word `yes` follows, as in the CLI."""
    parts = rest.split()
    if not parts:
        say("`rm <stack>` to see what would go, `rm <stack> yes` to do it")
        return
    stack = resolve(repo, prefix, parts[0], say)
    if stack is None:
        return
    args = ["stack", "rm", stack]
    doing = len(parts) > 1 and parts[1].lower() in ("yes", "y", "confirm")
    if doing:
        args.append("--yes")
        # The teardown stops a dev stack and its tunnels, which is not quick.
        say(f"_removing `{stack.split('/', 1)[-1]}` and stopping what it started…_")
    code, out = run_rigg(repo, args, timeout=600)
    say(f"```\n{out[:2500]}\n```" if out else "nothing to say")


def cmd_parts(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    """The optional parts, so `+preview` is discoverable rather than folklore."""
    code, out = run_rigg(repo, ["features"])
    if code != 0 or not out.strip():
        say(f"```\n{out[:2000] or 'no optional parts'}\n```")
        return
    # The first line tells a terminal user about --with; in here the way to
    # change one is `+name`, so that line is replaced rather than shown.
    body = "\n".join(out.splitlines()[1:]).strip("\n")
    say(f"```\n{body[:2500]}\n```\n"
        f"Add `+name` or `-name` to the end of a task to change one — "
        f"`redo the payment screen +preview`.")


def branch_state(line: str) -> tuple[str, str]:
    """A branch and its state, out of one line of `rigg stack list`."""
    body = re.sub(r"^\s*\*?\s*\d+\.\s*", "", line.strip())
    state = ""
    m = re.search(r"\[(.+)\]\s*$", body)
    if m:
        state = m.group(1)
        body = body[: m.start()]
    branch = body.split("<-")[0].strip()
    return branch, state


def ever_committed(repo: Path, branch: str) -> bool:
    """Whether a branch has ever had a commit of its own.

    A branch that committed nothing sits exactly where it was cut from, which
    is a commit on the trunk - so `merge-base --is-ancestor` calls it merged
    and a listing clears away a stack that has done nothing. A question asked
    as a task is the usual way to get one.

    The reflog is what remembers where a branch started. Where it cannot say,
    this answers no and the stack is left alone: not removing something is the
    safe way to be wrong.
    """
    out = subprocess.run(
        ["git", "reflog", "show", "--format=%H", branch],
        cwd=repo, capture_output=True, text=True,
    )
    shas = out.stdout.split()
    # Oldest entry is where the branch was created; anything above it is work.
    return out.returncode == 0 and len(shas) > 1 and shas[0] != shas[-1]


def merged_into_trunk(repo: Path, branch: str) -> bool:
    """Whether a branch's work is already contained in the trunk.

    Its *work*. A branch that never committed is contained in the trunk as
    well, trivially, and clearing that away is not the same thing at all.
    """
    if not ever_committed(repo, branch):
        return False
    for trunk in ("origin/main", "main"):
        r = subprocess.run(
            ["git", "merge-base", "--is-ancestor", branch, trunk],
            cwd=repo, capture_output=True,
        )
        if r.returncode == 0:
            return True
        # 1 is a clean "no"; anything else means the ref was not resolvable.
        if r.returncode == 1:
            return False
    return False


def cmd_stacks(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    """One message per stack, so each gets a thread to be talked to in."""
    code, out = run_rigg(repo, ["stack", "list"])
    if code != 0:
        say(f"```\n{out[:2500]}\n```")
        return
    mine = filter_stack_list(out, prefix)
    if not mine.strip():
        say("no stacks in this channel yet — `new <task>` starts one")
        return

    # Split the listing back into a block per stack: a stack's name is the
    # only unindented line, and its branches follow indented under it.
    blocks: list[tuple[str, list[str]]] = []
    for line in mine.splitlines():
        if not line[:1].isspace() and line.strip():
            blocks.append((line.strip(), []))
        elif blocks and line.strip():
            blocks[-1][1].append(line.strip())

    # A stack whose every branch has landed is finished with: the next piece
    # of work wants a new one, not this.
    # Deciding is a few git ancestry checks; removing means stopping a dev
    # stack and its tunnels, tens of seconds each - so the listing says which
    # are going, shows what is live, and lets the removal happen behind it.
    live, merged = [], []
    for name, branches in blocks:
        heads = [branch_state(b)[0] for b in branches]
        running = any("running" in branch_state(b)[1] for b in branches)
        if heads and not running and all(merged_into_trunk(repo, h) for h in heads):
            merged.append(name)
        else:
            live.append((name, branches))

    if merged:
        listed = ", ".join(f"`{m.split('/', 1)[-1]}`" for m in merged)
        say(f"_Clearing {len(merged)} merged stack(s): {listed}_")

        def clear():
            failed = []
            for name in merged:
                code, out = run_rigg(repo, ["stack", "rm", name, "--yes"], timeout=900)
                if code != 0:
                    failed.append((name, out))
            if failed:
                lines = "\n".join(
                    f"`{n.split('/', 1)[-1]}`: {o.strip().splitlines()[-1][:120]}"
                    for n, o in failed
                )
                say(f"_could not clear:_\n{lines}")
            else:
                say(f"_cleared {len(merged)} merged stack(s)_")

        # Daemon: if the bridge goes away mid-teardown the worst case is a
        # tunnel left running, which the next listing clears.
        threading.Thread(target=clear, daemon=True).start()

    blocks = live
    if not blocks:
        say("nothing live in this channel — `new <task>` starts something")
        return

    say(f"*{len(blocks)} stack(s)* — open one to see it or talk to it.")
    for name, branches in blocks:
        short = name.split("/", 1)[1] if "/" in name else name
        # The name carries where it has got to, so the channel answers "what
        # is happening" without anything having to be opened.
        states = [branch_state(b)[1] for b in branches]
        state = next((x for x in states if "running" in x), "") or states[-1] or "pending"
        posted = say(f"*{short}* — {state}")
        ts = ts_of(posted)
        if not ts:
            continue
        # Not primary: a listing should give this stack somewhere to be
        # replied to, without moving where its runs report.
        remember_thread(repo, name, channel, ts, primary=False)

        card = list(branches)
        urls = preview_urls(repo, name)
        if urls:
            card += [""] + urls.splitlines()
        say("```\n" + "\n".join(card) + "\n```", thread_ts=ts)


def cmd_logs(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    if not rest:
        say("`logs <stack>`")
        return
    short = rest.split()[0]
    stack = short if short.startswith(f"{prefix}/") else f"{prefix}/{short}"
    code, out = run_rigg(repo, ["logs", stack])
    tail = "\n".join(out.splitlines()[-40:])
    say(f"```\n{tail[:2800]}\n```" if tail.strip() else f"no log for `{stack}`")


# Commands whose first word is a stack, and which therefore can take it from
# the thread instead.
STACK_FIRST = {"add", "say", "stop", "cancel", "retry", "continue", "urls",
               "logs", "rm", "remove", "summary"}

# Answered in the channel rather than in a thread. A listing exists to be
# glanced at, and a message per stack is no use if all of them are folded
# inside one thread that has to be opened first - and each needs to be
# top-level anyway to have a thread of its own to be replied in.
CHANNEL_LEVEL = {"stacks", "status", "list"}

COMMANDS = {
    "new": cmd_new,
    "adopt": cmd_adopt,
    "add": cmd_add,
    "say": cmd_say,
    "stacks": cmd_stacks,
    "status": cmd_stacks,
    "list": cmd_stacks,
    "logs": cmd_logs,
    "pipelines": cmd_pipelines,
    "parts": cmd_parts,
    "features": cmd_parts,
    "help": None,  # answered in dispatch, which knows the channel
    "stop": cmd_stop,
    "cancel": cmd_stop,
    "retry": cmd_retry,
    "continue": cmd_continue,
    "summary": cmd_summary,
    "ask": cmd_ask,
    "q": cmd_ask,
    "urls": cmd_urls,
    "rm": cmd_rm,
    "remove": cmd_rm,
}


def parse(text: str) -> tuple[str, str]:
    """Strip any mention, then split into verb and the rest."""
    text = re.sub(r"<@[A-Z0-9]+>", " ", text).strip()
    if not text:
        return "help", ""
    verb, _, rest = text.partition(" ")
    return verb.lower(), rest.strip()


# --------------------------------------------------------------------------
# wiring


def check(cfg: Config, bot_token: str, app_token: str) -> int:
    """Verify the setup and say precisely what is wrong with it.

    Each failure here is one someone hits once while wiring Slack up, and each
    has a different fix - worth telling apart rather than letting the bridge
    fall over on the first message instead.
    """
    from slack_sdk import WebClient
    from slack_sdk.errors import SlackApiError

    problems = 0
    web = WebClient(token=bot_token)
    try:
        me = web.auth_test()
        print(f"bot token      ok - {me['user']} in {me['team']}")
    except SlackApiError as e:
        print(f"bot token      REJECTED ({e.response['error']}) - it should "
              f"start with xoxb- and come from Install App")
        return 1

    try:
        WebClient(token=app_token).api_call("apps.connections.open")
        print("app token      ok")
    except SlackApiError as e:
        print(f"app token      REJECTED ({e.response['error']}) - it should "
              f"start with xapp- and carry connections:write")
        problems += 1

    found: dict[str, dict] = {}
    try:
        cursor = None
        while True:
            page = web.conversations_list(
                types="public_channel,private_channel", limit=200, cursor=cursor
            )
            for c in page["channels"]:
                if c["name"] in cfg.channels:
                    found[c["name"]] = c
            cursor = page.get("response_metadata", {}).get("next_cursor")
            if not cursor:
                break
    except SlackApiError as e:
        print(f"channels       could not list ({e.response['error']})")
        problems += 1

    for name, ch in cfg.channels.items():
        info = found.get(name)
        if info is None:
            print(f"#{name:<14} NOT FOUND - no such channel, or the bot cannot "
                  f"see it. Check the name, then `/invite @rigg`.")
            problems += 1
        elif not info.get("is_member"):
            print(f"#{name:<14} found, but the bot is not in it - `/invite @rigg`")
            problems += 1
        else:
            kind = "private" if info.get("is_private") else "PUBLIC"
            note = ""
            if not info.get("is_private") and ch.mutating():
                note = "  (anyone who joins can run agents)"
            print(f"#{name:<14} ok - {kind}, {ch.repo}{note}")

    print("\nall good" if problems == 0 else f"\n{problems} problem(s) to fix")
    return 1 if problems else 0


def main() -> int:
    args = [a for a in sys.argv[1:] if a != "--check"]
    checking = "--check" in sys.argv
    cfg_path = Path(args[0]) if args else HERE / "channels.toml"
    if not cfg_path.exists():
        raise SystemExit(f"no config at {cfg_path} (copy channels.example.toml)")
    cfg = Config(cfg_path)

    from slack_bolt import App
    from slack_bolt.adapter.socket_mode import SocketModeHandler

    bot_token = os.environ.get("SLACK_BOT_TOKEN")
    app_token = os.environ.get("SLACK_APP_TOKEN")
    if not bot_token or not app_token:
        raise SystemExit("set SLACK_BOT_TOKEN (xoxb-) and SLACK_APP_TOKEN (xapp-)")

    if checking:
        return check(cfg, bot_token, app_token)

    # Bolt verifies the bot token here. Left alone it fails with a stack trace
    # out of slack_sdk, which is a poor way to learn you pasted the wrong one.
    try:
        app = App(token=bot_token)
    except Exception as e:
        detail = str(e)
        if "invalid_auth" in detail or "not_authed" in detail:
            raise SystemExit(
                "Slack rejected SLACK_BOT_TOKEN. It should start with `xoxb-` "
                "and come from Install App -> Install to Workspace."
            )
        raise SystemExit(f"could not start: {detail}")

    pool = ThreadPoolExecutor(max_workers=4)
    names: dict[str, str] = {}
    warned: set[str] = set()

    def channel_name(client, channel_id: str) -> str | None:
        if channel_id not in names:
            try:
                info = client.conversations_info(channel=channel_id)["channel"]
            except Exception:
                return None
            names[channel_id] = info["name"]
            # Membership is the access control now, so a public channel means
            # anyone in the workspace who joins it can put an agent to work.
            # Worth saying once, where it will be seen.
            ch = cfg.channels.get(info["name"])
            if (
                ch is not None
                and ch.mutating()
                and not info.get("is_private")
                and info["name"] not in warned
            ):
                warned.add(info["name"])
                print(
                    f"WARNING: #{info['name']} is a public channel and can run "
                    f"agents. Anyone who joins it gets that. Make it private, "
                    f"or narrow it with `commands`."
                )
        return names[channel_id]

    def handle(event, client, say):
        nonlocal cfg
        user = event.get("user")
        if not user or event.get("bot_id"):
            return
        cfg = cfg.reload_if_changed()
        name = channel_name(client, event["channel"])
        channel = cfg.channels.get(name) if name else None
        if channel is None:
            mapped = ", ".join(f"#{n}" for n in cfg.channels) or "none"
            say(
                f"#{name} is not mapped to a repo. A channel has to be listed "
                f"in `{cfg_path.name}` before I will do anything in it — that "
                f"mapping is what says which repo it drives. Mapped now: "
                f"{mapped}."
            )
            return

        def in_thread_say(text: str, thread_ts: str | None = None):
            """Answer in the thread the message came from."""
            return say(text=text, thread_ts=thread_ts or event.get("thread_ts") or event["ts"])

        verb, rest = parse(event.get("text", ""))
        if verb == "help":
            say(help_for(channel))
            return
        fn = COMMANDS.get(verb)
        pipeline = None

        # Inside a thread that is already a conversation, anything that is not
        # a command is the next question. Retyping `ask` there says nothing the
        # thread has not already said.
        if fn is None:
            store = ask_session(channel.repo, event.get("thread_ts"))
            if store is not None and store.exists() and channel.may("ask") is None:
                question = f"{verb} {rest}".strip()
                pool.submit(
                    lambda: cmd_ask(channel.repo, name, question, in_thread_say,
                                    event["channel"], event.get("thread_ts"))
                )
                return

        if fn is None:
            # Not a command, so it may be a pipeline: `preview <task>` reads
            # better than a flag, and non-coders do not have to learn one.
            pipeline, ambiguous = match_pipeline(channel.repo, verb)
            if ambiguous:
                say(ambiguous)
                return
            if pipeline is None:
                say(help_for(channel))
                return
            fn = cmd_new
            # A pipeline word in front of a verb, the shape the CLI takes:
            # `preview adopt <branch>` puts that pipeline on that branch.
            head, _, tail = rest.partition(" ")
            if head.lower() == "adopt":
                fn, rest, verb = cmd_adopt, tail.strip(), "adopt"
        refusal = channel.may(verb)
        if refusal:
            say(refusal)
            return

        images, tmp = download_images(event, bot_token)
        if images:
            reply_ts = event.get("thread_ts") or event["ts"]
            say(text=f"got {len(images)} image(s)", thread_ts=reply_ts)

        def reply(text: str, thread_ts: str | None = None):
            # Threading keeps a channel with several stacks in flight
            # readable; a listing is the exception, being the thing you look
            # at to decide which thread to open. `thread_ts` lets a listing
            # put the detail under the title it just posted.
            if thread_ts:
                return say(text=text, thread_ts=thread_ts)
            if verb in CHANNEL_LEVEL:
                return say(text=text)
            return say(text=text, thread_ts=event.get("thread_ts") or event["ts"])

        # A reply in a stack's own thread already says which stack, so the
        # name does not have to be typed again.
        if verb in STACK_FIRST:
            first = rest.split(None, 1)[0] if rest else ""
            found, _ = find_stack(channel.repo, name, first)
            if found:
                # Naming it here says which stack this thread is about, so the
                # next message in the thread does not have to name it again.
                # Not primary: this must not move where the run reports.
                remember_thread(channel.repo, found, event["channel"],
                                event.get("thread_ts") or event["ts"], primary=False)
            if not found:
                in_thread = stack_for_thread(
                    channel.repo, event["channel"], event.get("thread_ts")
                )
                if in_thread:
                    rest = f"{in_thread.split('/', 1)[-1]} {rest}".strip()
                elif ask_session(channel.repo, event.get("thread_ts")) is not None and \
                        ask_session(channel.repo, event.get("thread_ts")).exists():
                    # This thread is a question, not a branch. Guessing a stack
                    # from the first word of a sentence is how a question ends
                    # up being carried out on an unrelated branch.
                    say(
                        f"This thread is a question, not a stack — `{verb}` needs "
                        f"one. Name it (`{verb} <stack> …`), or drop the verb to "
                        f"ask a follow-up here."
                    )
                    return

        def work():
            try:  # noqa: SIM105
                if fn is cmd_ask:
                    # The thread this lands in - its own ts when the question
                    # starts one - is the conversation's identity.
                    fn(channel.repo, name, rest, reply, event["channel"],
                       event.get("thread_ts") or event["ts"])
                elif fn in (cmd_new, cmd_adopt, cmd_add, cmd_say):
                    fn(channel.repo, name, rest, reply, event["channel"],
                       pipeline, images)
                else:
                    fn(channel.repo, name, rest, reply, event["channel"])
            except Exception:
                # A discarded Future swallows this entirely: the command
                # simply stops half-done and nothing anywhere says why.
                detail = traceback.format_exc()
                print(detail, file=sys.stderr)
                try:
                    reply(f"that failed:\n```\n{detail.strip().splitlines()[-1][:400]}\n```")
                except Exception:
                    pass
            finally:
                # rigg copies them into its own staging while it runs, so they
                # are only needed for the length of the call.
                if tmp:
                    shutil.rmtree(tmp, ignore_errors=True)

        pool.submit(work)

    app.event("app_mention")(lambda event, client, say: handle(event, client, say))

    @app.event("message")
    def on_message(event, client, say):
        # Direct messages need no mention; channel posts are handled above.
        if event.get("channel_type") == "im":
            handle(event, client, say)

    print(f"rigg slack bridge — {len(cfg.channels)} channel(s):")
    for n, c in cfg.channels.items():
        what = ",".join(sorted(c.commands)) if c.commands is not None else "all commands"
        print(f"  #{n:<20} {c.repo}  [{what}]")
    print("Anyone in these channels can run what the channel allows.")
    try:
        SocketModeHandler(app, app_token).start()
    except Exception as e:
        detail = str(e)
        if "invalid_auth" in detail or "not_authed" in detail:
            raise SystemExit(
                "Slack rejected SLACK_APP_TOKEN. It should start with `xapp-` "
                "and come from Basic Information -> App-Level Tokens, with the "
                "connections:write scope."
            )
        raise
    return 0


if __name__ == "__main__":
    sys.exit(main())
