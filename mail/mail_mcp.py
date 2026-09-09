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

    Args:
        question: The text of the mail being answered. Paste it whole.
        limit: How many examples to return. Default 5.
        author: Only replies written by this email address, if you want one
            person's voice rather than the team's.
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


def email_channel_for(conversation_id: str) -> str:
    """The channel a reply on this conversation should go out on.

    Front wants one explicitly, and the right one is an email channel on the
    inbox the conversation already lives in.
    """
    row = DB.execute(
        "SELECT inbox_id FROM conversation_inbox WHERE conversation_id = ?",
        (conversation_id,),
    ).fetchone()
    if not row or not row["inbox_id"]:
        raise ToolError(
            f"no inbox recorded for {conversation_id}; run the sync before drafting"
        )
    for ch in front_get(f"/inboxes/{row['inbox_id']}/channels").get("_results", []):
        if ch.get("types") == "smtp" or ch.get("type") in ("smtp", "email"):
            return ch["id"]
    raise ToolError(f"inbox {row['inbox_id']} has no email channel to send from")


def create_draft(conversation_id: str, body: str, author_id: str | None = None) -> dict:
    """Put a draft reply on a Front conversation, unsent.

    It appears in Front for a person to edit and send. Nothing is sent by this
    tool.

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
    return {"draft_id": out.get("id"), "conversation_id": conversation_id}


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
