#!/usr/bin/env python3
"""The outbox as an MCP server: what an agent may do to a waiting proposal.

Proposing and redrafting only. Sending is not a tool, and deliberately: it
happens in `poll.py` after a person has said so, where no prompt can reach it.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mcp.server.mcpserver import MCPServer  # noqa: E402
from mcp.server.mcpserver.exceptions import ToolError  # noqa: E402

import outbox  # noqa: E402

mcp = MCPServer("rigg-outbox")


def propose(action: str, key: str, title: str, draft: str, why: str = "",
            subtitle: str = "", params: dict | None = None) -> str:
    """Post something to Slack for a person to approve.

    action  which [approvals.*] runs on approval - `approvals()` lists them
    key     what makes it idempotent, e.g. a conversation id
    title   the bold line in Slack
    draft   the text a person approves
    why     one line on the precedent you are leaning on
    params  fields the action needs, e.g. {"conversation_id": "cnv_1"}
    """
    try:
        item = outbox.propose(action, key, title, draft, why, params, subtitle)
    except (ValueError, outbox.SlackError) as e:
        raise ToolError(str(e)) from e
    return f"{item['id']} posted, waiting for approval"


def approvals() -> dict:
    """What this repo allows to be approved, and what each one does."""
    return {
        name: {"description": spec.get("description", ""),
               "ready": outbox.blocked(spec) is None,
               "blocked_by": outbox.blocked(spec)}
        for name, spec in outbox.approvals().items()
    }


@mcp.tool()
def post_digest(text: str) -> str:
    """Post the morning summary to the channel."""
    try:
        outbox.post(text)
    except outbox.SlackError as e:
        raise ToolError(str(e)) from e
    return "posted"


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
        {k: i[k] for k in ("id", "action", "key", "title", "status", "feedback")}
        for i in outbox.load()
        if i["status"] in ("pending", "redraft")
    ]


# Proposing and redrafting post to Slack, so they are only registered when a
# role is meant to do that. `waiting` and `post_digest` are always safe.
mcp.tool()(approvals)
if os.environ.get("OUTBOX_MCP_READONLY") != "1":
    mcp.tool()(propose)
    mcp.tool()(redraft)

if __name__ == "__main__":
    mcp.run()
