#!/usr/bin/env python3
"""The outbox as an MCP server: what an agent may do to a waiting proposal.

Proposing and redrafting only. Sending is not a tool, and deliberately: it
happens in `poll.py` after a person has said so, where no prompt can reach it.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mcp.server.fastmcp import FastMCP  # noqa: E402
from mcp.server.fastmcp.exceptions import ToolError  # noqa: E402

import outbox  # noqa: E402

mcp = FastMCP("rigg-outbox")


@mcp.tool()
def propose_reply(conversation_id: str, subject: str, sender: str, draft: str,
                  why: str = "") -> str:
    """Post a mail and a suggested reply to Slack, and wait for a person.

    conversation_id is the `id:` from the inbox listing. `why` is one line on
    why this one is safe to answer from a template.
    """
    try:
        item = outbox.propose(conversation_id, subject, sender, draft, why)
    except (ValueError, outbox.SlackError) as e:
        raise ToolError(str(e)) from e
    return f"{item['id']} posted, waiting for approval"


@mcp.tool()
def post_digest(text: str) -> str:
    """Post the morning summary to the channel."""
    try:
        outbox.post(text)
    except outbox.SlackError as e:
        raise ToolError(str(e)) from e
    return "posted"


@mcp.tool()
def redraft(item_id: str, draft: str) -> str:
    """Replace a waiting draft after feedback, and post the new one."""
    try:
        outbox.redraft(item_id, draft)
    except (KeyError, outbox.SlackError) as e:
        raise ToolError(str(e)) from e
    return f"{item_id} redrafted, waiting for approval"


@mcp.tool()
def waiting() -> list[dict]:
    """Everything still waiting on a person."""
    return [
        {k: i[k] for k in ("id", "conversation_id", "subject", "status", "feedback")}
        for i in outbox.load()
        if i["status"] in ("pending", "redraft")
    ]


if __name__ == "__main__":
    if os.environ.get("OUTBOX_MCP_READONLY") == "1":
        mcp._tool_manager._tools.pop("propose_reply", None)
        mcp._tool_manager._tools.pop("redraft", None)
    mcp.run()
