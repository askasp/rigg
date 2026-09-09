#!/usr/bin/env python3
"""Every instance configured on this machine, and what they collide over.

    python3 slack/instances.py

One instance is one Slack workspace. Running several on one box is fine — they
share no state of their own — but three things on disk are keyed by something
other than the instance, and each is silent when two instances land on the same
one. This says so before a run does.

Needs nothing installed: tomllib is stdlib on 3.11+.
"""

import subprocess
import sys
import tomllib
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
HOME = Path.home()


def instances() -> dict[str, Path]:
    """Name -> channels file. `channels.toml` is the unnamed one, `default`."""
    found: dict[str, Path] = {}
    for f in sorted(HERE.glob("*channels.toml")):
        if f.name.endswith(".example.toml"):
            continue
        name = f.name[: -len("-channels.toml")] if f.name != "channels.toml" else "default"
        found[name] = f
    return found


def channels_of(path: Path) -> dict[str, Path]:
    try:
        cfg = tomllib.loads(path.read_text())
    except Exception as e:
        print(f"  ! {path.name} does not parse: {e}", file=sys.stderr)
        return {}
    out = {}
    for name, entry in (cfg.get("channels") or {}).items():
        repo = entry.get("repo")
        if repo:
            out[name] = Path(repo).expanduser()
    return out


def unit_state(inst: str) -> str:
    try:
        r = subprocess.run(
            ["systemctl", "--user", "is-active", f"rigg-slack@{inst}.service"],
            capture_output=True, text=True, timeout=5,
        )
        return r.stdout.strip() or "unknown"
    except Exception:
        return "-"


def main() -> int:
    found = instances()
    if not found:
        print("no channels files in slack/ — copy channels.example.toml first")
        return 1

    by_instance = {name: channels_of(p) for name, p in found.items()}
    problems: list[str] = []

    print(f"{'instance':<12} {'unit':<10} {'channel':<22} repo")
    for inst, chans in by_instance.items():
        env = HERE / (f"{inst}.env" if inst != "default" else ".env")
        state = unit_state(inst)
        if not chans:
            print(f"{inst:<12} {state:<10} (no channels mapped)")
        for i, (ch, repo) in enumerate(sorted(chans.items())):
            mark = "" if repo.is_dir() else "   <- no such directory"
            print(f"{inst if i == 0 else '':<12} {state if i == 0 else '':<10} "
                  f"#{ch:<21} {repo}{mark}")
            if not repo.is_dir():
                problems.append(f"{inst}: #{ch} points at {repo}, which does not exist")
            elif not (repo / ".git").exists():
                problems.append(f"{inst}: #{ch} points at {repo}, which is not a git repo")
        if not env.exists():
            problems.append(f"{inst}: no {env.relative_to(HERE.parent)} — it cannot start")
        secrets = HOME / ".rigg" / "secrets" / f"{inst}.env"
        if not secrets.exists():
            print(f"{'':<12} {'':<10} (no {secrets}, so no Front token for this instance)")

    # 1. The stack namespace is the channel name alone (rigg_slack.py:594), and
    #    stack.json lives in the repo's shared git dir. Same channel name plus
    #    same repo in two instances means one namespace and one file.
    per_pair = defaultdict(list)
    for inst, chans in by_instance.items():
        for ch, repo in chans.items():
            per_pair[(ch, repo.resolve() if repo.is_dir() else repo)].append(inst)
    for (ch, repo), insts in per_pair.items():
        if len(insts) > 1:
            problems.append(
                f"#{ch} is mapped in {' and '.join(insts)}, both to {repo}. "
                f"Stacks are named <channel>/<slug>, so those two share one "
                f"namespace and one stack.json — rename the channel in one."
            )

    # 2. Two instances on one repo share stack.json even with different channel
    #    names. Allowed - two channels already do it - but worth knowing.
    per_repo = defaultdict(set)
    for inst, chans in by_instance.items():
        for repo in chans.values():
            per_repo[repo.resolve() if repo.is_dir() else repo].add(inst)
    for repo, insts in per_repo.items():
        if len(insts) > 1:
            problems.append(
                f"{repo} is driven by {' and '.join(sorted(insts))}. They share "
                f"its stack.json, which is rewritten whole — two runs starting at "
                f"once can lose one another's stack."
            )

    # 3. Worktrees are keyed by the repo's directory *basename*.
    per_base = defaultdict(set)
    for chans in by_instance.values():
        for repo in chans.values():
            per_base[repo.name].add(str(repo))
    for base, paths in per_base.items():
        if len(paths) > 1:
            problems.append(
                f"{' and '.join(sorted(paths))} share the basename '{base}', so "
                f"both check out into ~/.rigg/worktrees/{base}."
            )

    print()
    if problems:
        print(f"{len(problems)} problem(s):")
        for p in problems:
            print(f"  - {p}")
    else:
        print("nothing shared between instances that should not be.")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
