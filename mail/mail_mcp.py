#!/usr/bin/env python3
"""An MCP server over this instance's mail corpus.

Three tools: find how questions like this one were answered before, read one
whole thread, and put a draft back on a Front conversation. Nothing here takes
SQL or a URL — the agent can reach the corpus and the one conversation it names,
and nothing else.

Run it by hand to check it:
    RIGG_INSTANCE=default FRONT_API_TOKEN=... mail/mail_mcp.py
"""

import os
import sys
import time

import corpus
import requests
from mcp.server.mcpserver import MCPServer
from mcp.server.mcpserver.exceptions import ToolError

mcp = MCPServer("rigg-mail")

DB = corpus.connect(corpus.db_path())
FRONT = "https://api2.frontapp.com"


@mcp.tool()
def search_replies(
    question: str, limit: int = 5, author: str | None = None
) -> list[dict]:
    """Find how questions like this one were answered before.

    Searches past inbound mail for ones resembling `question` and returns each
    with the reply that was actually sent, newest and closest first. Use these
    as examples of house style and of what the answer usually is — not as facts
    to repeat, since an old reply can be out of date.

    Searches the whole corpus, not just the inbox this run is scoped to: a
    colleague's answer to the same question is precedent, and every result
    names its `author` so you can see whose it is.

    Args:
        question: The text of the mail being answered. Paste it whole.
        limit: How many examples to return. Default 5.
        author: Prefer this person's replies. Not a filter — when they have
            answered nothing like it, everyone else's replies come back
            instead, which is usually what you want.
    """
    try:
        return corpus.search(DB, question, limit, author)
    except ValueError as e:
        # ToolError rather than a bare exception: the SDK passes its message
        # back to the agent as the tool result, where anything else becomes
        # "Error executing tool X" and teaches it nothing. Same reason
        # amino-mcp.ts returns isError text instead of throwing.
        raise ToolError(str(e)) from None


@mcp.tool()
def get_thread(conversation_id: str) -> list[dict]:
    """Read one whole conversation, oldest message first.

    Quoted history and sign-offs are already stripped. Use it when a search
    result looks relevant but you need the exchange around it.

    Args:
        conversation_id: A Front conversation id, as returned by search_replies.
    """
    if not corpus.in_scope(DB, conversation_id):
        raise ToolError(
            f"{conversation_id} is not in {corpus.scope()}, which is the only "
            "inbox this run may read"
        )
    rows = DB.execute(
        "SELECT is_inbound, author_email, created_at, subject, clean_text"
        " FROM messages WHERE conversation_id = ? ORDER BY created_at",
        (conversation_id,),
    ).fetchall()
    if not rows:
        raise ToolError(
            f"no conversation {conversation_id} in the corpus — "
            "ids come from search_replies, and the corpus only holds what has been synced"
        )
    return [
        {
            "direction": "in" if r["is_inbound"] else "out",
            "author": r["author_email"],
            "at": time.strftime("%Y-%m-%d %H:%M", time.gmtime(r["created_at"])),
            "subject": r["subject"],
            "text": r["clean_text"],
        }
        for r in rows
    ]


def front_get(path: str) -> dict:
    token = os.environ.get("FRONT_API_TOKEN")
    if not token:
        raise ToolError("FRONT_API_TOKEN is not set for this instance")
    r = requests.get(
        FRONT + path, headers={"Authorization": f"Bearer {token}"}, timeout=30
    )
    if not r.ok:
        raise ToolError(f"Front {r.status_code} on {path}: {r.text[:200]}")
    return r.json()


EMAIL_CHANNELS = frozenset(
    # Front names the transport, not the medium, so every way of putting an
    # address on a mailbox is its own type. Guessing at this list is how a
    # draft fails on a working inbox: a `gmail` channel is email, and testing
    # for "smtp" said it was not.
    {"smtp", "imap", "gmail", "office365", "office365_shared", "exchange"}
)


def inbox_of(conversation_id: str) -> tuple[str, str]:
    """Which inbox a conversation is in, as (id, name).

    The corpus knows this for anything it has synced, and Front knows it for
    everything. Asking Front when the corpus has not caught up is what stops
    drafting from inheriting the sync's staleness: mail that arrived twenty
    minutes ago is exactly the mail worth answering, and it is never in the
    corpus yet.
    """
    row = DB.execute(
        "SELECT inbox_id, inbox_name FROM conversation_inbox WHERE conversation_id = ?",
        (conversation_id,),
    ).fetchone()
    if row and row["inbox_id"]:
        return row["inbox_id"], row["inbox_name"] or ""
    found = front_get(f"/conversations/{conversation_id}/inboxes").get("_results", [])
    if not found:
        raise ToolError(f"Front says {conversation_id} is in no inbox")
    return found[0]["id"], found[0].get("name") or ""


def draftable(conversation_id: str) -> str:
    """The inbox id, once this run is allowed to draft in it."""
    inbox_id, inbox_name = inbox_of(conversation_id)
    want = corpus.scope()
    if want and want not in (inbox_id, inbox_name):
        raise ToolError(
            f"{conversation_id} is in {inbox_name or inbox_id}, and this run may "
            f"only draft in {want}. Widen it with --var inbox on the schedule if "
            "that is really what you want."
        )
    return inbox_id


def email_channel_for(conversation_id: str) -> str:
    """The channel a reply on this conversation should go out on.

    Front wants one explicitly, and the right one is an email channel on the
    inbox the conversation already lives in.
    """
    inbox_id = draftable(conversation_id)
    channels = front_get(f"/inboxes/{inbox_id}/channels").get("_results", [])
    usable = [c for c in channels if c.get("is_valid", True)]
    for ch in usable:
        if ch.get("type") in EMAIL_CHANNELS:
            return ch["id"]
    # A transport this list has not met yet, but which clearly sends mail.
    for ch in usable:
        if "@" in str(ch.get("address") or ""):
            return ch["id"]
    saw = ", ".join(sorted({str(c.get("type")) for c in channels})) or "none at all"
    raise ToolError(
        f"inbox {inbox_id} has no channel that sends email - it has {saw}"
    )


def existing_draft(conversation_id: str) -> str | None:
    """The id of a draft already waiting on this conversation, if any.

    A draft does not mark a conversation answered, so every overlapping run
    would otherwise stack another one on the same thread - and the person who
    has to clear them is the one this is supposed to be helping.
    """
    try:
        d = front_get(f"/conversations/{conversation_id}/messages")
    except Exception:
        # Not being able to look is not permission to duplicate, but it is
        # also not a reason to refuse: report it and let the caller decide.
        return None
    for m in d.get("_results", []):
        if m.get("is_draft"):
            return m.get("id")
    return None


def create_draft(conversation_id: str, body: str, author_id: str | None = None) -> dict:
    """Put a draft reply on a Front conversation, unsent.

    It appears in Front for a person to edit and send. Nothing is sent by this
    tool. A conversation that already has a draft is left alone.

    Args:
        conversation_id: The Front conversation to reply on.
        body: The reply, as HTML or plain text.
        author_id: The teammate the draft is from. Defaults to FRONT_AUTHOR_ID.
    """
    token = os.environ.get("FRONT_API_TOKEN")
    if not token:
        raise ToolError("FRONT_API_TOKEN is not set for this instance")
    author = author_id or os.environ.get("FRONT_AUTHOR_ID")
    if not author:
        # Front attributes an author-less draft to the API token itself, which
        # means it goes out with no signature and from nobody. Better to stop.
        raise ToolError(
            "no author for the draft: set FRONT_AUTHOR_ID for this instance, or "
            "pass author_id. Without one Front attributes the draft to the API "
            "token and it is unsigned."
        )
    # Before anything else: the list step only ever offers this run's inbox,
    # but that is a habit of the pipeline, not a rule. Joining a shared mailbox
    # should not silently turn a drafting job loose in it.
    draftable(conversation_id)
    already = existing_draft(conversation_id)
    if already:
        return {
            "created": False,
            "conversation_id": conversation_id,
            "existing_draft_id": already,
            "reason": (
                f"{conversation_id} already has an unsent draft ({already}); "
                "left alone rather than adding a second one"
            ),
        }
    r = requests.post(
        f"{FRONT}/conversations/{conversation_id}/drafts",
        headers={"Authorization": f"Bearer {token}"},
        json={
            "body": body,
            "channel_id": email_channel_for(conversation_id),
            "author_id": author,
            "mode": "private",
        },
        timeout=30,
    )
    if not r.ok:
        raise ToolError(f"Front {r.status_code} creating the draft: {r.text[:300]}")
    out = r.json()
    return {"created": True, "draft_id": out.get("id"), "conversation_id": conversation_id}


# Registered only where writing is actually wanted, which is the `mailer` role
# and nowhere else. The read tools exist to put customer mail in front of a
# model, and that mail is written by strangers: a server that both reads it and
# holds a tool taking a conversation id to write to is one injected instruction
# away from drafting whatever the sender asked for, signed by a named
# colleague. `email_channel_for` already refuses a conversation this tenant's
# corpus has never seen, so an invented id cannot reach another inbox either.
if os.environ.get("MAIL_MCP_WRITE") == "1":
    mcp.tool()(create_draft)


if __name__ == "__main__":
    if not corpus.db_path().exists():
        print(
            f"no corpus at {corpus.db_path()} — run mail/run.sh --backfill first",
            file=sys.stderr,
        )
    mcp.run("stdio")
