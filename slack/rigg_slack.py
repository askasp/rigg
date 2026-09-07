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
import os
import random
import re
import string
import subprocess
import sys
import tomllib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
RIGG = os.environ.get("RIGG_BIN", str(HERE.parent / "target" / "release" / "rigg"))

HELP = """*rigg* — one repo per channel, stacks scoped to this channel.

• `new <task>` — start a stack and run the pipeline on it
• `add <stack> <task>` — stack the next branch on top of one
• `say <stack> <message>` — another turn in that stack's session
• `stacks` — this channel's stacks and their state
• `logs <stack>` — the tail of a run's log
• `help` — this

Stacks are named `<channel>/<slug>`; refer to them by the short name."""


# --------------------------------------------------------------------------
# config


class Config:
    def __init__(self, path: Path):
        with path.open("rb") as fh:
            raw = tomllib.load(fh)
        self.channels: dict[str, Path] = {
            name: Path(c["repo"]).expanduser()
            for name, c in raw.get("channels", {}).items()
        }
        # Empty means anyone in a mapped channel may drive an agent, which is
        # code execution on this machine. Startup says so out loud.
        self.allowed_users: set[str] = set(raw.get("allowed_users", []))
        for name, repo in self.channels.items():
            if not (repo / ".git").exists():
                raise SystemExit(f"channel #{name}: {repo} is not a git repository")


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


def remember_thread(repo: Path, branch: str, channel: str, thread_ts: str) -> None:
    """Record where a branch's progress should be reported.

    The notify hook reads this; keying by branch is what lets a detached run
    find its way back to the thread that asked for it, long after this process
    has forgotten the request.
    """
    d = git_common_dir(repo) / "rigg" / "slack"
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{branch.replace('/', '-')}.json").write_text(
        json.dumps({"channel": channel, "thread_ts": thread_ts})
    )


def slug(task: str) -> str:
    words = re.findall(r"[a-z0-9]+", task.lower())[:4]
    stem = "-".join(words)[:32].strip("-") or "task"
    tail = "".join(random.choices(string.hexdigits.lower()[:16], k=3))
    return f"{stem}-{tail}"


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


def cmd_new(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    if not rest:
        say("give me a task: `new <task>`")
        return
    stack = f"{prefix}/{slug(rest)}"
    code, out = run_rigg(repo, ["new", stack, rest])
    if code != 0:
        say(f"could not start `{stack}`:\n```\n{out[:2500]}\n```")
        return
    posted = say(f"started `{stack}`\n```\n{out[:1500]}\n```")
    ts = posted.get("ts") if isinstance(posted, dict) else None
    if ts:
        remember_thread(repo, stack, channel, ts)


def cmd_add(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    parts = rest.split(None, 1)
    if len(parts) < 2:
        say("`add <stack> <task>`")
        return
    short, task = parts[0], parts[1]
    stack = short if short.startswith(f"{prefix}/") else f"{prefix}/{short}"
    if stack not in own_stacks(repo, prefix):
        say(f"no stack `{stack}` in this channel. `stacks` lists them.")
        return
    code, out = run_rigg(repo, ["add", stack, task])
    if code != 0:
        say(f"could not extend `{stack}`:\n```\n{out[:2500]}\n```")
        return
    branch = first_backticked(out) or stack
    posted = say(f"queued `{branch}`\n```\n{out[:1500]}\n```")
    ts = posted.get("ts") if isinstance(posted, dict) else None
    if ts:
        remember_thread(repo, branch, channel, ts)


def cmd_say(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    parts = rest.split(None, 1)
    if len(parts) < 2:
        say("`say <stack> <message>`")
        return
    short, message = parts[0], parts[1]
    stack = short if short.startswith(f"{prefix}/") else f"{prefix}/{short}"
    if stack not in own_stacks(repo, prefix):
        say(f"no stack `{stack}` in this channel. `stacks` lists them.")
        return
    say(f"passing that to `{stack}`...")
    # A turn takes as long as it takes; the ack above is what keeps Slack happy.
    code, out = run_rigg(repo, ["say", stack, message], timeout=3600)
    tail = "\n".join(out.splitlines()[-25:])
    say(f"{'done' if code == 0 else 'failed'} — `{stack}`\n```\n{tail[:2500]}\n```")


def cmd_stacks(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    code, out = run_rigg(repo, ["stack", "list"])
    if code != 0:
        say(f"```\n{out[:2500]}\n```")
        return
    mine = filter_stack_list(out, prefix)
    say(f"```\n{mine}\n```" if mine.strip() else "no stacks in this channel yet")


def cmd_logs(repo: Path, prefix: str, rest: str, say, channel: str) -> None:
    if not rest:
        say("`logs <stack>`")
        return
    short = rest.split()[0]
    stack = short if short.startswith(f"{prefix}/") else f"{prefix}/{short}"
    code, out = run_rigg(repo, ["logs", stack])
    tail = "\n".join(out.splitlines()[-40:])
    say(f"```\n{tail[:2800]}\n```" if tail.strip() else f"no log for `{stack}`")


COMMANDS = {
    "new": cmd_new,
    "add": cmd_add,
    "say": cmd_say,
    "stacks": cmd_stacks,
    "status": cmd_stacks,
    "logs": cmd_logs,
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


def main() -> int:
    cfg_path = Path(sys.argv[1]) if len(sys.argv) > 1 else HERE / "channels.toml"
    if not cfg_path.exists():
        raise SystemExit(f"no config at {cfg_path} (copy channels.example.toml)")
    cfg = Config(cfg_path)

    from slack_bolt import App
    from slack_bolt.adapter.socket_mode import SocketModeHandler

    bot_token = os.environ.get("SLACK_BOT_TOKEN")
    app_token = os.environ.get("SLACK_APP_TOKEN")
    if not bot_token or not app_token:
        raise SystemExit("set SLACK_BOT_TOKEN (xoxb-) and SLACK_APP_TOKEN (xapp-)")

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

    def channel_name(client, channel_id: str) -> str | None:
        if channel_id not in names:
            try:
                info = client.conversations_info(channel=channel_id)
                names[channel_id] = info["channel"]["name"]
            except Exception:
                return None
        return names[channel_id]

    def handle(event, client, say):
        user = event.get("user")
        if not user or event.get("bot_id"):
            return
        if cfg.allowed_users and user not in cfg.allowed_users:
            say("you are not on this bridge's allowlist")
            return
        name = channel_name(client, event["channel"])
        repo = cfg.channels.get(name) if name else None
        if repo is None:
            say(f"#{name} is not mapped to a repo in {cfg_path.name}")
            return

        verb, rest = parse(event.get("text", ""))
        fn = COMMANDS.get(verb)
        if fn is None:
            say(HELP)
            return

        def reply(text: str):
            # Threading every reply under the request keeps a channel with
            # several stacks in flight readable.
            return say(text=text, thread_ts=event.get("thread_ts") or event["ts"])

        pool.submit(fn, repo, name, rest, reply, event["channel"])

    app.event("app_mention")(lambda event, client, say: handle(event, client, say))

    @app.event("message")
    def on_message(event, client, say):
        # Direct messages need no mention; channel posts are handled above.
        if event.get("channel_type") == "im":
            handle(event, client, say)

    print(f"rigg slack bridge — {len(cfg.channels)} channel(s):")
    for n, r in cfg.channels.items():
        print(f"  #{n:<20} {r}")
    if not cfg.allowed_users:
        print("WARNING: allowed_users is empty — anyone in these channels can "
              "run an agent on this machine.")
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
