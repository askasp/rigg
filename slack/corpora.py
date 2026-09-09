"""Corpora a run can search, set up from a channel rather than on the box.

    @rigg corpus                              # what exists, how big, how fresh
    @rigg corpus add mail                     # token, first import, and the sync that keeps it current
    @rigg corpus search hva koster blodprøve  # read what an agent would be shown
    @rigg corpus sync mail
    @rigg corpus rm mail

Named `corpora` and not `corpus` deliberately: `mail/corpus.py` is the sidecar's
own module and this directory is on `sys.path`, so two modules of that name in
one process would end up opening each other's database. Same reason `creds.py`
is not called `secrets.py`.

A corpus belongs to the **instance**, not to a channel and not to a repo —
`~/.rigg/mail/<instance>.db` is one workspace's mail and nobody else's — so the
registry is `~/.rigg/corpus/<instance>.json` and every channel of an instance
sees the same corpora. Which is the point: the corpus outlives the repo that
happens to be drafting from it this month.

What this does **not** do is hand an agent the tools to search one. Tools stay
where rigg already puts them — a role's `args` in the repo's own config, and a
step gated on a feature — so `+mail` on a task, `parts` and `plan` go on meaning
exactly what they mean for everything else. `corpus add` prints the config to
paste; `wiring()` is that message.

The sync lives here for the reason the schedule does: this process is already
the per-instance daemon, and a corpus nobody is drafting from does not need to
be fresh. It is a thread, not a crontab line, so `corpus rm` actually stops it.
"""

import json
import os
import re
import sqlite3
import subprocess
import threading
import time
import traceback
from datetime import datetime
from pathlib import Path

import creds

ROOT = Path(__file__).resolve().parent.parent

# Front's rate limit is per company rather than per token, and the incremental
# path is a handful of requests. Fifteen minutes is what `mail/README.md`
# already recommends as a timer, and the same number here means the two agree.
DEFAULT_EVERY = 15

# `/events` is what the incremental sync trusts, and Front's own forums report
# that it can miss inbound messages outright. A weekly walk of the last month
# over `/conversations` reconciles whatever the stream dropped. Every write is
# an upsert, so the sweep costs requests and nothing else.
RECONCILE_DAYS = 7
RECONCILE_WINDOW_DAYS = 31


def instance() -> str:
    return os.environ.get("RIGG_INSTANCE") or "default"


# ----------------------------------------------------------------------- kinds


class Mail:
    """Front conversations, paired question-to-reply. The only kind so far.

    A kind knows three things and nothing else: where its data sits, how to
    describe what is in it, and the command line that fills it. Anything else —
    Slack, the registry, the schedule — is the same for every kind, which is
    what makes a second one cheap.
    """

    name = "mail"
    summary = "past replies from Front, to draft new ones from"
    secret = "FRONT_API_TOKEN"
    secret_why = ("a Front API token with scopes shared:conversations and "
                  "shared:inboxes, to read the inboxes this corpus is built from")
    secret_where = "https://app.frontapp.com/settings/api"
    tool = "mcp__front"
    mcp_command = str(ROOT / "mail" / "mcp.sh")

    @staticmethod
    def path(inst: str | None = None) -> Path:
        # The twin of `mail/corpus.py:db_path`. Two lines rather than an import:
        # the bridge has no business opening the sidecar's schema, and this
        # never writes.
        root = Path(os.environ.get("RIGG_MAIL_DIR") or (Path.home() / ".rigg" / "mail"))
        return root / f"{inst or instance()}.db"

    @staticmethod
    def argv(mode: str = "") -> list[str]:
        run = [str(ROOT / "mail" / "run.sh"), instance()]
        if mode == "backfill":
            return run + ["--backfill"]
        if mode == "reconcile":
            since = datetime.fromtimestamp(
                time.time() - RECONCILE_WINDOW_DAYS * 86400
            ).strftime("%Y-%m-%d")
            return run + ["--backfill", "--since", since]
        return run

    @classmethod
    def stats(cls, inst: str | None = None) -> dict | None:
        """How much is in it and how recent, or None if it does not exist yet."""
        p = cls.path(inst)
        if not p.exists():
            return None
        try:
            db = _read_only(p)
        except sqlite3.Error:
            return None
        try:
            row = db.execute(
                "SELECT count(*) AS n, max(sent_at) AS newest FROM pairs"
            ).fetchone()
            msgs = db.execute("SELECT count(*) AS n FROM messages").fetchone()["n"]
            return {"rows": row["n"], "newest": row["newest"], "messages": msgs,
                    "bytes": p.stat().st_size}
        except sqlite3.Error:
            # A database mid-backfill has the tables; one that is half-created
            # by something else does not, and that is worth saying rather than
            # raising into a channel.
            return None
        finally:
            db.close()

    @classmethod
    def search(cls, question: str, limit: int, inst: str | None = None) -> list[dict]:
        p = cls.path(inst)
        if not p.exists():
            raise LookupError("nothing imported yet")
        db = _read_only(p)
        try:
            return _ranking().search(db, question, limit)
        finally:
            db.close()


KINDS = {"mail": Mail}


def _read_only(path: Path) -> sqlite3.Connection:
    """The corpus, opened so that reading it cannot create or change it.

    `mode=ro` rather than `corpus.connect`: that one runs the schema and would
    leave an empty database behind for a name nobody has imported yet.
    """
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    db.row_factory = sqlite3.Row
    db.execute("PRAGMA busy_timeout = 5000")  # the sync may be writing
    return db


_mail_corpus = None


def _ranking():
    """`mail/corpus.py`, for its ranking and nothing else.

    Imported late and by path: it is the sidecar's module, `mail/` is not on
    the bridge's `sys.path`, and a channel that never searches should not pay
    for it. The alternative — a second bm25-and-recency here — would be a
    second answer to "what would the agent see", which is the one question
    `corpus search` exists to answer.
    """
    global _mail_corpus
    if _mail_corpus is None:
        import importlib.util

        spec = importlib.util.spec_from_file_location(
            "rigg_mail_corpus", ROOT / "mail" / "corpus.py"
        )
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        _mail_corpus = mod
    return _mail_corpus


# ----------------------------------------------------------------------- store


def store_path(inst: str | None = None) -> Path:
    root = Path(os.environ.get("RIGG_CORPUS_DIR") or (Path.home() / ".rigg" / "corpus"))
    return root / f"{inst or instance()}.json"


def load(inst: str | None = None) -> list[dict]:
    p = store_path(inst)
    if not p.exists():
        return []
    try:
        return json.loads(p.read_text())
    except Exception:
        return []


def save(entries: list[dict], inst: str | None = None) -> None:
    p = store_path(inst)
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(entries, indent=2))
    tmp.replace(p)  # atomic, so a crash mid-write cannot lose the registry


_lock = threading.Lock()


def update(name: str, **fields) -> None:
    """Change one entry in place, without losing one added a moment ago."""
    with _lock:
        entries = load()
        for e in entries:
            if e["name"] == name:
                e.update(fields)
        save(entries)


def find(name: str) -> dict | None:
    return next((e for e in load() if e["name"] == name), None)


def names() -> list[str]:
    return [e["name"] for e in load()]


# What is mid-sync, so two never overlap on one database and a listing can say
# "importing" rather than "never synced" for the first hour of a backfill.
_busy_lock = threading.Lock()
_busy: dict[str, str] = {}


def busy() -> dict[str, str]:
    with _busy_lock:
        return dict(_busy)


def claim(name: str, what: str) -> bool:
    with _busy_lock:
        if name in _busy:
            return False
        _busy[name] = what
        return True


def release(name: str) -> None:
    with _busy_lock:
        _busy.pop(name, None)


# ------------------------------------------------------------------ the syncer


def sync(name: str, mode: str = "") -> tuple[int, str]:
    """Run one corpus's own fill command. Blocks; caller decides where."""
    entry = find(name)
    if entry is None:
        return 1, f"no corpus `{name}` registered here"
    kind = KINDS.get(entry.get("kind", name))
    if kind is None:
        return 1, f"`{name}` is of a kind I no longer know how to sync"
    argv = kind.argv(mode)
    try:
        p = subprocess.run(
            argv,
            cwd=ROOT,
            capture_output=True,
            text=True,
            stdin=subprocess.DEVNULL,
            # A backfill walks every inbox at 50 requests a minute; hours is
            # the honest number. The incremental path returns in seconds.
            timeout=12 * 3600 if mode else 1800,
            # Read now rather than inherited at boot, so a token set in Slack a
            # minute ago reaches this sync without restarting the bridge.
            env={**os.environ, **creds.current(), "RIGG_INSTANCE": instance()},
        )
    except subprocess.TimeoutExpired:
        return 1, f"`{' '.join(argv)}` timed out"
    except FileNotFoundError:
        return 1, f"nothing to run at {argv[0]}"
    return p.returncode, (p.stdout + p.stderr).strip()


class Syncer(threading.Thread):
    """Keeps every registered corpus current, one minute at a time.

    A timer rather than a step in each pipeline that drafts: two jobs drafting
    from the same corpus would otherwise each pay for the sync, and a corpus
    nobody synced is the failure that looks like the agent inventing a price.
    """

    def __init__(self, post, log=None):
        super().__init__(daemon=True, name="rigg-corpus")
        self.post = post
        self.log = log or (lambda m: print(m, flush=True))
        global _syncer
        _syncer = self

    def run(self) -> None:
        while True:
            try:
                self.tick(time.time())
            except Exception:
                self.log(traceback.format_exc())
            time.sleep(60)

    def tick(self, now: float) -> None:
        for entry in load():
            # Nothing to keep current until the first import has been through.
            if not entry.get("imported"):
                continue
            every = max(1, int(entry.get("every_minutes") or DEFAULT_EVERY)) * 60
            if now - float(entry.get("last_sync") or 0) < every:
                continue
            due = ""
            if now - float(entry.get("last_reconcile") or 0) > RECONCILE_DAYS * 86400:
                due = "reconcile"
            self.start_sync(entry["name"], due)

    def start_sync(self, name: str, mode: str = "", announce=None) -> bool:
        """Sync on its own thread. False if one is already in flight."""
        what = mode or "sync"
        if not claim(name, what):
            return False
        threading.Thread(
            target=self._sync, args=(name, mode, announce), daemon=True,
            name=f"rigg-corpus-{name}",
        ).start()
        return True

    def _sync(self, name: str, mode: str, announce) -> None:
        try:
            started = time.time()
            code, out = sync(name, mode)
            fields = {"last_sync": time.time(),
                      "last_status": "ok" if code == 0 else "failed",
                      "last_output": out[-2000:]}
            if code == 0 and mode:
                fields["last_reconcile"] = time.time()
            if code == 0 and mode == "backfill":
                fields["imported"] = True
            update(name, **fields)
            self.log(f"corpus {name}: {mode or 'sync'} "
                     f"{'ok' if code == 0 else 'failed'} in {time.time() - started:.0f}s")
            if announce:
                announce(code, out)
            elif code != 0:
                self._say_failed(name, out)
        except Exception:
            self.log(traceback.format_exc())
        finally:
            release(name)

    def _say_failed(self, name: str, out: str) -> None:
        """A sync that has started failing is otherwise a corpus quietly aging.

        Only on the change, the way a stuck job is: `*/15` failing all night is
        one problem, not ninety-six.
        """
        entry = find(name) or {}
        if entry.get("said_failed"):
            return
        update(name, said_failed=True)
        where = entry.get("added_in_id")
        if where:
            self.post(where, f"`{name}` has stopped syncing:\n```\n{out[-1200:]}\n```\n"
                             f"It is still searchable, but it is not getting newer. "
                             f"`corpus show {name}` has the detail.")


_syncer = None


# ------------------------------------------------------------------- reporting


def freshness(entry: dict, stats: dict | None) -> str:
    """One phrase for the state a corpus is actually in."""
    doing = busy().get(entry["name"])
    if doing == "backfill":
        return "importing now"
    if doing:
        return "syncing now"
    if not entry.get("imported"):
        return "not imported yet"
    if entry.get("last_status") == "failed":
        return "sync failing"
    if stats and stats.get("newest"):
        return f"newest reply {_ago(stats['newest'])}"
    if entry.get("last_sync"):
        return f"synced {_ago(entry['last_sync'])}"
    return "never synced"


def _ago(when: float) -> str:
    secs = max(0, time.time() - float(when))
    if secs < 3600:
        return f"{int(secs // 60)}m ago"
    if secs < 86400:
        return f"{int(secs // 3600)}h ago"
    return f"{int(secs // 86400)}d ago"


def wiring(kind) -> str:
    """How to let a run actually search it.

    Printed rather than written into the repo: this is a channel talking about
    somebody's git repository, and the config belongs in a commit a person
    made. It is also where the read/write split gets explained, which is the
    one part of this nobody should be left to work out.
    """
    return (
        f"A corpus is not a tool until a role holds one. In the repo:\n"
        f"```\n"
        f"// .rigg/front-mcp.json\n"
        f'{{ "mcpServers": {{ "front": {{ "command": "{kind.mcp_command}" }} }} }}\n'
        f"```\n"
        f"```\n"
        f"# .rigg/rigg.toml\n"
        f"[features]\n"
        f"{kind.name} = false          # off unless a task asks for it\n\n"
        f"[agents.mailer]\n"
        f"kind = \"claude\"\n"
        f"args = [\"--permission-mode\", \"bypassPermissions\",\n"
        f"        \"--mcp-config\", \".rigg/front-mcp.json\", \"--strict-mcp-config\",\n"
        f"        \"--allowedTools\", \"{kind.tool}\"]\n\n"
        f"[[pipelines.do.steps]]\n"
        f"id = \"answer-mail\"\n"
        f"agent = \"mailer\"\n"
        f"feature = \"{kind.name}\"\n"
        f"```\n"
        f"Then `+{kind.name}` on the end of any task or schedule turns it on, "
        f"`parts` lists it and `plan` shows it. Leave the draft tool out of that "
        f"config unless the role is meant to answer mail — the corpus is written "
        f"by strangers, and a role that can read it and write to a conversation "
        f"is one injected sentence from drafting whatever a sender asked for."
    )


# --------------------------------------------------------------- the protocol

# How a run asks for a corpus it turns out to want. The twin of
# RIGG-NEEDS-SECRET, and for the same reason: an unattended job cannot stop and
# wait for someone, so it says what is missing in one line and stops.
#
#   RIGG-NEEDS-CORPUS: mail | past replies to draft from
#
# Only the name is required. A line of output rather than an exit code so that
# any step — an agent turn, a shell step, an MCP server — can raise it without
# rigg having to know which.
NEEDS_CORPUS = re.compile(
    r"^RIGG-NEEDS-CORPUS:\s*([a-z][a-z0-9_-]*)\s*(?:\|([^|\n]*))?", re.M
)


def requests_in(output: str) -> list[tuple[str, str]]:
    """Corpora a run said it wanted, minus the ones it already has."""
    have = set(names())
    seen: dict[str, tuple[str, str]] = {}
    for name, why in NEEDS_CORPUS.findall(output or ""):
        if name not in have:
            seen[name] = (name, (why or "").strip())
    return list(seen.values())


def ask_for(wanted: list[tuple[str, str]]) -> str:
    """What a channel is told when a run wants one. Reads like `secret` does."""
    lines = []
    for name, why in wanted:
        kind = KINDS.get(name)
        lines.append(f"*{name}* — {why or (kind.summary if kind else 'wanted by this job')}")
        if kind is None:
            lines.append(f"  I have no `{name}` to build — "
                         f"I know {', '.join(f'`{k}`' for k in KINDS)}")
        else:
            lines.append(f"  build it with `corpus add {name}`")
    return "\n".join(lines)


# ------------------------------------------------------------------- command


def command(repo, prefix, rest, say, channel_id) -> None:
    """`corpus` — what this instance can search, and what keeps it current."""
    rest = (rest or "").strip()
    entries = load()

    if not rest:
        if not entries:
            say(
                "no corpus here yet. A corpus is a body of past work an agent "
                "can search while it drafts — so an answer sounds like the ones "
                "that went before it.\n"
                + "\n".join(f"`corpus add {n}` — {k.summary}" for n, k in KINDS.items())
            )
            return
        lines = []
        for e in entries:
            kind = KINDS.get(e.get("kind", e["name"]))
            stats = kind.stats() if kind else None
            size = f"{stats['rows']:,} replies" if stats else "nothing yet"
            lines.append(f"`{e['name']}`  {size}  — {freshness(e, stats)}")
        say("corpora this instance can search:\n" + "\n".join(lines)
            + "\n\n`corpus search <text>` to see what an agent would be shown, "
              "`corpus show <name>` for the detail, `corpus sync <name>` to "
              "fetch now.")
        return

    head, _, tail = rest.partition(" ")
    tail = tail.strip()

    if head in ("add", "new"):
        _add(head, tail, say, channel_id, prefix)
        return

    if head in ("search", "find", "q"):
        _search(tail, say)
        return

    if head in ("show", "inspect"):
        _show(tail, say)
        return

    if head in ("sync", "refresh", "update"):
        _sync_now(tail, say)
        return

    if head in ("rm", "remove", "delete", "forget"):
        _rm(tail, say)
        return

    # A bare `corpus <words>` is a search far more often than it is a typo —
    # nobody types the verb when they are looking something up — but only when
    # there is something to search.
    if entries:
        _search(rest, say)
        return
    say(f"`{head}` is not something I know how to do with a corpus. "
        f"`corpus add <kind>`, `corpus search <text>`, `corpus show <name>`, "
        f"`corpus sync <name>`, `corpus rm <name>`. `corpus` on its own lists them.")


def _add(verb: str, tail: str, say, channel_id: str, prefix: str) -> None:
    name = tail.split()[0] if tail else ""
    if not name:
        say("`corpus add <kind>` — I know "
            + ", ".join(f"`{n}` ({k.summary})" for n, k in KINDS.items()))
        return
    kind = KINDS.get(name)
    if kind is None:
        say(f"I have no `{name}` to build. I know "
            + ", ".join(f"`{n}` — {k.summary}" for n, k in KINDS.items())
            + ".\nA new kind is a class in `slack/corpora.py`: where its data "
              "sits, and the command that fills it.")
        return
    if find(name) is not None:
        say(f"`{name}` is already here — `corpus show {name}`.")
        return

    # The token before the registration, so a corpus never exists in a state
    # where nothing can fill it. Same words the cron protocol uses, because it
    # is the same answer: one message, and the next run has it.
    if kind.secret and not creds.current().get(kind.secret):
        say(
            f"`{name}` needs a credential first.\n"
            f"*{kind.secret}* — {kind.secret_why}\n"
            f"  get one at {kind.secret_where}\n"
            f"  send it with `secret {kind.secret} <value>` in a private channel\n\n"
            f"Then `corpus add {name}` again."
        )
        return

    entry = {
        "name": name,
        "kind": name,
        "added_at": int(time.time()),
        "added_in": prefix,
        "added_in_id": channel_id,
        "every_minutes": DEFAULT_EVERY,
        "imported": False,
        "last_sync": 0,
        "last_reconcile": 0,
    }
    entries = load()
    entries.append(entry)
    save(entries)

    if _syncer is None:
        say(f"`{name}` registered, but nothing here is running syncs — "
            f"fill it by hand with `{' '.join(kind.argv('backfill'))}`.")
        return

    def done(code: int, out: str) -> None:
        stats = kind.stats()
        if code != 0:
            _syncer.post(channel_id,
                         f"`{name}` could not be imported:\n```\n{out[-1500:]}\n```")
            return
        got = f"{stats['rows']:,} replies from {stats['messages']:,} messages" \
            if stats else "no pairs — worth checking the token's inbox scopes"
        _syncer.post(
            channel_id,
            f"`{name}` is ready: {got}. It refreshes every "
            f"{entry['every_minutes']} minutes from here on.\n\n"
            f"Look at it with `corpus search <text>` — that runs the same "
            f"ranking an agent gets, so what comes back is what it would be "
            f"shown.\n\n" + wiring(kind))

    _syncer.start_sync(name, "backfill", announce=done)
    say(
        f"importing `{name}` — {kind.summary}.\n"
        f"The first pass walks every inbox at Front's rate limit, so this takes "
        f"a while; I will say when it is searchable. After that it refreshes "
        f"every {entry['every_minutes']} minutes, and reconciles the last month "
        f"weekly — `/events` is known to drop inbound mail.\n"
        f"`corpus` shows how far it has got."
    )


def _pick(name: str) -> tuple[dict | None, str]:
    """The corpus a command means, or why it means none.

    A name is optional while there is one corpus, which is the case that
    matters: nobody types `mail` at a register that holds only `mail`.
    """
    entries = load()
    if not entries:
        return None, "nothing here yet — `corpus add mail` builds one."
    if not name:
        if len(entries) == 1:
            return entries[0], ""
        return None, ("which one? " + ", ".join(f"`{e['name']}`" for e in entries))
    hit = find(name)
    if hit is None:
        return None, f"no corpus `{name}` here — `corpus` lists them."
    return hit, ""


def _search(question: str, say) -> None:
    if not question:
        say("`corpus search <text>` — paste the mail you are answering, or just "
            "the question in it.")
        return
    if not load():
        say("nothing to search yet — `corpus add mail` builds one.")
        return
    # A leading corpus name picks one; anything else is all search text. Which
    # means `corpus search mail` is a name and no question, and says so rather
    # than searching the corpus for its own name.
    named = ""
    if question.split()[0] in names():
        named, _, question = question.partition(" ")
        question = question.strip()
    entry, why = _pick(named)
    if entry is None:
        say(why)
        return
    kind = KINDS.get(entry.get("kind", entry["name"]))
    if kind is None:
        say(f"`{entry['name']}` is of a kind I no longer know how to search.")
        return
    if not question:
        say(f"`corpus search {entry['name']} <text>` — say what to look for.")
        return

    try:
        hits = kind.search(question, 3)
    except LookupError:
        say(f"`{entry['name']}` has nothing in it yet — "
            f"{freshness(entry, None)}.")
        return
    except ValueError as e:
        say(f"{e}.")
        return

    if not hits:
        say(f"nothing in `{entry['name']}` resembles that. It holds "
            f"{(kind.stats() or {}).get('rows', 0):,} replies, so either this is "
            f"new ground or the wording is very different from how it was asked "
            f"before.")
        return

    out = [(f"top {len(hits)} of what `{entry['name']}` would show an agent "
            f"answering that:")]
    for h in hits:
        out.append(
            f"\n*{h['sent_at']}*  {h['author'] or 'unattributed'}"
            f"{'  ·  ' + h['subject'] if h['subject'] else ''}\n"
            f"> {_clip(h['asked'], 220)}\n"
            f"```\n{_clip(h['replied'], 700)}\n```"
        )
    out.append("\n_Ranked closest-and-newest first. Recency is weighted hard: "
               "what goes stale in a mailbox is what a draft repeats._")
    say("\n".join(out)[:3800])


def _clip(text: str, n: int) -> str:
    text = " ".join((text or "").split())
    return text if len(text) <= n else text[: n - 1] + "…"


def _show(name: str, say) -> None:
    entry, why = _pick(name)
    if entry is None:
        say(why)
        return
    kind = KINDS.get(entry.get("kind", entry["name"]))
    stats = kind.stats() if kind else None
    if stats:
        holds = (f"{stats['rows']:,} replies, {stats['messages']:,} messages, "
                 f"{stats['bytes'] // (1 << 20)} MB")
    else:
        holds = "nothing yet"
    lines = [
        f"*corpus `{entry['name']}`* — {kind.summary if kind else 'unknown kind'}",
        f"holds     {holds}",
        f"state     {freshness(entry, stats)}",
        (f"refresh   every {entry.get('every_minutes', DEFAULT_EVERY)} minutes, "
         f"reconciling the last {RECONCILE_WINDOW_DAYS} days weekly"),
        f"file      {kind.path() if kind else '?'}",
        (f"added     {datetime.fromtimestamp(entry['added_at']).strftime('%d %b %Y')} "
         f"in #{entry.get('added_in', '?')}"),
    ]
    if entry.get("last_reconcile"):
        lines.append(f"reconciled {_ago(entry['last_reconcile'])}")
    if entry.get("last_status") == "failed" and entry.get("last_output"):
        lines.append(f"\nlast sync failed:\n```\n{entry['last_output'][-1000:]}\n```")
    if kind:
        lines.append("\n" + wiring(kind))
    say("\n".join(lines))


def _sync_now(name: str, say) -> None:
    entry, why = _pick(name)
    if entry is None:
        say(why)
        return
    if _syncer is None:
        say("nothing here is running syncs, so there is nothing to run it with.")
        return
    doing = busy().get(entry["name"])
    if doing:
        say(f"`{entry['name']}` is already {doing}ing — it lands on its own.")
        return
    mode = "" if entry.get("imported") else "backfill"
    _syncer.start_sync(entry["name"], mode)
    say(f"{'importing' if mode else 'syncing'} `{entry['name']}` now. "
        f"`corpus` says when it is done.")


def _rm(tail: str, say) -> None:
    entry, why = _pick(tail.split()[0] if tail.split() else "")
    if entry is None:
        say(why)
        return
    name = entry["name"]
    kind = KINDS.get(entry.get("kind", name))
    save([e for e in load() if e["name"] != name])
    where = kind.path() if kind else None
    say(f"`{name}` is off the register — nothing syncs it any more, and a run "
        f"that searches it will find whatever was there when it stopped.\n"
        + (f"The file is still at `{where}`; delete it yourself if you mean to "
           f"be rid of it, since re-importing is hours."
           if where else ""))
