"""The corpus file, and the little that both the sync and the server need to
know about it.

One file per instance — `~/.rigg/mail/<instance>.db` — so two workspaces share
nothing at all, not even a `WHERE` clause. `RIGG_INSTANCE` is exported by
`slack/run.sh`, so a server an agent starts from a channel opens the right
corpus without being told which one.

Messages are kept as Front gave them *and* cleaned. Cleaning is the step most
likely to need improving, and re-deriving the pairs from stored messages takes
seconds where re-fetching them from Front takes hours at 50 requests a minute.
"""

import math
import os
import re
import sqlite3
import time
from pathlib import Path

SCHEMA = """
CREATE TABLE IF NOT EXISTS messages (
  id              TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  is_inbound      INTEGER NOT NULL,
  -- Front sets no author on a message a rule or the API token sent. That is
  -- how an automated acknowledgement is told from a person's reply, and only
  -- a person's reply is worth learning from.
  author_id       TEXT,
  author_email    TEXT,
  created_at      REAL NOT NULL,
  subject         TEXT,
  text            TEXT NOT NULL,   -- as Front gave it
  clean_text      TEXT NOT NULL    -- quoted history and signature removed
);
CREATE INDEX IF NOT EXISTS messages_thread ON messages(conversation_id, created_at);

-- Which inbox a conversation belongs to. Its own table because the incremental
-- path learns it with an extra request, and only ever needs to once.
CREATE TABLE IF NOT EXISTS conversation_inbox (
  conversation_id TEXT PRIMARY KEY,
  inbox_id        TEXT,
  inbox_name      TEXT
);

-- One row per reply: what was asked, and what was sent back.
CREATE TABLE IF NOT EXISTS pairs (
  reply_id        TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  inbox_id        TEXT,
  subject         TEXT,
  inbound_text    TEXT NOT NULL,
  reply_text      TEXT NOT NULL,
  author_email    TEXT,
  sent_at         REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS pairs_sent_at ON pairs(sent_at);
CREATE INDEX IF NOT EXISTS pairs_thread ON pairs(conversation_id);

-- Retrieval is on the question, because that is what you have when a new mail
-- arrives; the reply is the payload. `remove_diacritics 0` keeps å, ø and æ
-- as themselves — they are letters here, not decorated vowels, and folding
-- them makes `får` and `far` the same word.
CREATE VIRTUAL TABLE IF NOT EXISTS pairs_fts USING fts5(
  inbound_text, subject,
  content='pairs', content_rowid='rowid',
  tokenize="unicode61 remove_diacritics 0"
);

CREATE TRIGGER IF NOT EXISTS pairs_ai AFTER INSERT ON pairs BEGIN
  INSERT INTO pairs_fts(rowid, inbound_text, subject)
  VALUES (new.rowid, new.inbound_text, new.subject);
END;
CREATE TRIGGER IF NOT EXISTS pairs_ad AFTER DELETE ON pairs BEGIN
  INSERT INTO pairs_fts(pairs_fts, rowid, inbound_text, subject)
  VALUES ('delete', old.rowid, old.inbound_text, old.subject);
END;
-- The sync only ever deletes and re-inserts, so this trigger fires for nobody
-- today. It is here because an external-content FTS table whose content is
-- updated behind its back does not degrade, it reports the whole database as
-- malformed — a landmine for the first UPDATE anyone adds later.
CREATE TRIGGER IF NOT EXISTS pairs_au AFTER UPDATE ON pairs BEGIN
  INSERT INTO pairs_fts(pairs_fts, rowid, inbound_text, subject)
  VALUES ('delete', old.rowid, old.inbound_text, old.subject);
  INSERT INTO pairs_fts(rowid, inbound_text, subject)
  VALUES (new.rowid, new.inbound_text, new.subject);
END;

CREATE TABLE IF NOT EXISTS state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
"""


def instance() -> str:
    return os.environ.get("RIGG_INSTANCE") or "default"


def db_path(inst: str | None = None) -> Path:
    root = Path(os.environ.get("RIGG_MAIL_DIR") or (Path.home() / ".rigg" / "mail"))
    return root / f"{inst or instance()}.db"


def connect(path: Path) -> sqlite3.Connection:
    path.parent.mkdir(parents=True, exist_ok=True)
    # The MCP server runs each tool call on a worker thread, so a connection
    # bound to the thread that opened it fails every call with "SQLite objects
    # created in a thread can only be used in that same thread". Sharing is safe
    # here: the server only reads, and the sync writes from one thread.
    db = sqlite3.connect(path, check_same_thread=False)
    db.row_factory = sqlite3.Row
    db.execute("PRAGMA journal_mode=WAL")
    # The sync writes while the server reads. Without this the reader raises
    # "database is locked" instead of waiting the moment a cron sync overlaps.
    db.execute("PRAGMA busy_timeout = 5000")
    db.executescript(SCHEMA)
    return db


def get_state(db: sqlite3.Connection, key: str, default: str | None = None) -> str | None:
    row = db.execute("SELECT value FROM state WHERE key = ?", (key,)).fetchone()
    return row["value"] if row else default


def set_state(db: sqlite3.Connection, key: str, value: str) -> None:
    db.execute(
        "INSERT INTO state(key, value) VALUES(?, ?) "
        "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (key, str(value)),
    )


# ------------------------------------------------------------------ retrieval

# Words shorter than this carry no signal and only slow the match down.
MIN_TERM = 3
# How many bm25 rows to re-rank in Python. Wide enough that recency can lift a
# good older answer, narrow enough to stay instant.
CANDIDATES = 60


def fts_query(text: str) -> str:
    """A question turned into something FTS5 will accept.

    User text goes nowhere near the MATCH syntax: every word is taken out,
    quoted, and OR-ed, so a stray `"` or `*` is a word rather than an operator.
    """
    terms = [t for t in re.findall(r"\w+", text, flags=re.UNICODE) if len(t) >= MIN_TERM]
    if not terms:
        raise ValueError("nothing searchable in that text — it is all short words")
    # Prefix-matched, because Norwegian inflects on the end of the word and
    # fts5 has no stemmer for it: `blodprøve*` reaches blodprøven, blodprøver
    # and blodprøvene, which an exact match would all miss.
    return " OR ".join('"' + t.replace('"', "") + '"*' for t in terms[:60])


def search(db: sqlite3.Connection, question: str, limit: int = 5,
           author: str | None = None) -> list[dict]:
    """Past mail resembling `question`, each with the reply that was sent.

    Here rather than in the MCP server because two callers need the same
    answer: the tool an agent searches with, and `corpus search` in Slack,
    which exists to show what that agent will be shown. Two rankings would
    make the second one a lie the first time either was tuned.
    """
    rows = db.execute(
        "SELECT p.*, bm25(pairs_fts, 1.0, 2.0) AS bm"
        " FROM pairs_fts JOIN pairs p ON p.rowid = pairs_fts.rowid"
        " WHERE pairs_fts MATCH ?"
        + (" AND p.author_email = ?" if author else "")
        + " ORDER BY bm LIMIT ?",
        ([fts_query(question)] + ([author] if author else []) + [CANDIDATES]),
    ).fetchall()

    now = time.time()
    scored = []
    for r in rows:
        # bm25 is negative and better the lower it goes; flip it so the boost
        # below multiplies in the direction anyone would expect.
        relevance = -r["bm"]
        age_years = max(0.0, (now - r["sent_at"]) / (365 * 24 * 3600))
        # Recency is weighted hard on purpose. What goes stale in a support
        # mailbox is exactly what a draft repeats — prices, product names, the
        # current policy — so a fresh answer to a slightly less similar question
        # beats a two-year-old answer to the same one. The 2.5x spread flips
        # near-ties and nothing more: a genuinely weak match scores several
        # times lower on bm25 and stays down where it belongs.
        recency = 1.0 + 1.5 * math.exp(-age_years)
        scored.append((relevance * recency, r))

    scored.sort(key=lambda x: x[0], reverse=True)
    return [
        {
            "asked": r["inbound_text"],
            "replied": r["reply_text"],
            "subject": r["subject"],
            "author": r["author_email"],
            "sent_at": time.strftime("%Y-%m-%d", time.gmtime(r["sent_at"])),
            "conversation_id": r["conversation_id"],
        }
        for _, r in scored[: max(1, min(limit, 25))]
    ]
