# mail

A corpus of past replies, and an MCP server that searches it — so an agent
drafting an answer can be shown how questions like this one were answered
before.

The twin of `slack/`: a sidecar with its own env file and its own README. rigg
itself stays a Rust binary that knows nothing about email.

## What it stores

One row per reply: what someone asked, and what was sent back. Both cleaned of
quoted history and sign-offs, because a corpus of quoted threads and
`Med vennlig hilsen` matches every query equally well and helps with none.

Messages are kept as Front gave them *as well as* cleaned. Cleaning is the part
most likely to need improving, and `--rebuild` re-derives every pair from stored
messages in seconds where re-fetching them is hours at 50 requests a minute.

One corpus per instance — `~/.rigg/mail/<instance>.db` — so two workspaces share
nothing, not even a `WHERE` clause. `RIGG_INSTANCE` picks it, and `slack/run.sh`
already exports that, so a server an agent starts from a channel opens the right
one without being told.

## Setting it up

**From Slack, this is one command.** `@rigg corpus add mail` asks for the token,
runs the backfill in the background, and keeps it current afterwards without a
crontab line — see [what a job can draw on](../slack/README.md#what-a-job-can-draw-on).
Everything below is the same thing done by hand, which is what you want when
there is no bridge running or the import needs an argument.

A Front API token in the instance's env file:

```sh
mkdir -p ~/.rigg/secrets && chmod 700 ~/.rigg/secrets
cp mail/env.example ~/.rigg/secrets/default.env    # then fill it in
```

Then the first import, which walks every inbox:

```sh
./mail/run.sh --backfill                 # the unnamed instance
./mail/run.sh acme --backfill            # a named one
./mail/run.sh --backfill --since 2024-01-01
```

After that, everything new since last time:

```sh
./mail/run.sh
```

`--backfill` walks conversations because Front's `/events` only reaches so far
back; the incremental run uses `/events` from a stored watermark. Both stop and
wait when the rate limit is spent rather than failing, so a backfill can be left
alone. The watermark only moves once a page is committed — a crash halfway
repeats work rather than leaving a hole.

## Keeping it current

Registered from Slack, the bridge does this itself: every 15 minutes, and a
walk of the last month once a week. `@rigg corpus` says how fresh it is and
`@rigg corpus sync mail` fetches now. What follows is the same schedule for a
machine with no bridge on it.

A timer, not a daemon:

```
*/15 * * * * /home/aksel/git/rigg/mail/run.sh amino >/dev/null 2>&1
```

Worth a second, slower entry beside it:

```
17 4 * * 0 /home/aksel/git/rigg/mail/run.sh amino --backfill --since 2026-08-01
```

The incremental run trusts `/events` to say what changed. Front's own forums
report that it can miss inbound messages outright and that it sorts and filters
on different timestamps, so a weekly walk of the last month over `/conversations`
reconciles whatever the event stream dropped. Every write is an upsert, so the
sweep costs requests and nothing else — and it means the corpus does not depend
on `/events` being complete.

Or make the incremental run the first step of any pipeline that answers mail, so
a run never drafts from a stale corpus:

```toml
[[steps]]
id = "sync-mail"
run = "~/git/rigg/mail/run.sh {{instance}}"
```

## A morning digest

Reading the corpus by *date* rather than by similarity is a command, not a
tool — a digest wants everything that arrived, answered or not, in order:

```sh
./mail/run.sh amino --list --since-hours 24
```

```
7 message(s) in the last 24 hours.

--- 2026-09-09 07:12  kari@example.com  [UNANSWERED]
inbox: Support   subject: Pris på blodprøve
id: cnv_9f2a
Hei! Hva koster en blodprøve hos dere?
```

Four steps in the repo's `.rigg/rigg.toml` turn that into a summary in Slack.
`rigg run` uses the checkout it is in and makes no branch, which is what a
digest wants:

```toml
[[pipelines.digest.steps]]
id = "sync"
run = "$HOME/git/rigg/mail/run.sh $RIGG_INSTANCE"

[[pipelines.digest.steps]]
id = "collect"
run = "$HOME/git/rigg/mail/run.sh $RIGG_INSTANCE --list --since-hours 24"
capture = "inbox"          # stdout becomes {{inbox}} for later steps

[[pipelines.digest.steps]]
id = "summarize"
agent = "impl"
clear = true               # a digest carries nothing over from yesterday
prompt = """
Below is every mail that reached our Front inboxes in the last 24 hours.

Write a short morning digest in Norwegian, as Slack-ready plain text - no
Markdown headings, no tables. Lead with anything still UNANSWERED, grouped by
what it is about, then one line per theme for the rest. Counts, not every
message. If nothing needs a person, say so in one sentence.

Write it to /home/aksel/.rigg/mail/digest.md and nothing else. Do not reply to
anyone and do not commit anything.

{{inbox}}
"""

[[pipelines.digest.steps]]
id = "post"
run = """
RIGG_BRANCH=digest RIGG_MESSAGE="$(cat /home/aksel/.rigg/mail/digest.md)" \
  $HOME/git/rigg/slack/notify.sh
"""
```

The last step is `notify.sh` used for what it already does — post `RIGG_MESSAGE`
to a channel. With no Slack thread behind the run it falls back to
`RIGG_NOTIFY_CHANNEL`, so put that in the instance's env file. For a digest too
long for one message, `slack/upload.sh <file>` posts it as a snippet instead and
takes the same fallback.

Then one crontab line:

```
0 7 * * * /home/aksel/git/rigg/cron.sh amino ~/git/amino-monorepo \
          run --pipeline digest
```

Build it up a step at a time before trusting it — `--only` runs one:

```sh
rigg run --pipeline digest --only collect     # is the mail there?
rigg run --pipeline digest --only post        # does Slack get it?
rigg plan --pipeline digest                   # what will run, and on what
```

Two things to know. The digest only sees what the sync has pulled, which is why
`sync` is the first step rather than a separate timer. And the agent step reads
mail written by strangers — give the role no MCP config at all, as above, so a
sentence in an email has no tool to reach for.

## The tools

`mail/mcp.sh` is the server. Point a repo at it:

```json
// <repo>/.rigg/front-mcp.json
{ "mcpServers": { "front": { "command": "/home/aksel/git/rigg/mail/mcp.sh" } } }
```

```toml
# <repo>/.rigg/rigg.toml
[agents.mailer]
kind = "claude"
args = ["--permission-mode", "bypassPermissions",
        "--mcp-config", ".rigg/front-mcp.json", "--strict-mcp-config",
        "--allowedTools", "mcp__front"]
```

With `--strict-mcp-config` and that allowlist the role has these three tools and
nothing else — no shell, no files, no web.

| tool | |
| --- | --- |
| `search_replies(question, limit, author)` | Past mail resembling `question`, each with the reply that was sent. `author` narrows it to one person's voice. |
| `get_thread(conversation_id)` | One whole conversation, cleaned, oldest first. |
| `create_draft(conversation_id, body, author_id)` | A private draft on the conversation. Nothing is ever sent. |

The ranking lives in `corpus.py` rather than in the server, because two callers
need the same answer: this tool, and `@rigg corpus search` in Slack, which
exists to show what an agent will be shown. Two rankings would make the second
one a lie the first time either was tuned.

### How results are ranked

bm25 over the question text and subject, then weighted hard towards recent
replies — up to 2.5× for something from this month, falling to 1× over a few
years. What goes stale in a support mailbox is exactly what a draft repeats:
prices, product names, the current policy. The spread flips near-ties and
nothing more, so a genuinely weak match stays down where it belongs.

A *much* closer old match still leads — recency breaks near-ties, it does not
overrule relevance. So every result carries its date and author, and the tool
description tells the agent to treat old replies as examples of *style* rather
than facts to repeat.

### Reading and writing are separate configs

The read tools exist to put customer mail in front of a model, and that mail is
written by strangers. A server that reads it *and* holds a tool that writes to a
conversation id is one injected instruction away from drafting whatever the
sender asked for, signed by a named colleague. So the draft tool is not
registered unless the config asks for it:

```json
// .rigg/front-mcp.json — searching, and nothing else
{ "mcpServers": { "front": { "command": "/home/aksel/git/rigg/mail/mcp.sh" } } }

// .rigg/front-draft-mcp.json — the role that is meant to answer mail
{ "mcpServers": { "front": {
    "command": "/home/aksel/git/rigg/mail/mcp.sh",
    "env": { "MAIL_MCP_WRITE": "1" } } } }
```

`create_draft` also refuses any conversation this tenant's corpus has never
seen, so an id an email talked the model into using cannot reach another inbox.

### Drafts are attributed, or refused

Front credits an author-less draft to the API token itself, which means it goes
out signed by nobody. `create_draft` refuses rather than doing that: set
`FRONT_AUTHOR_ID` for the instance, or pass `author_id`.

## Choices worth knowing

**Only email, never drafts.** An SMS answers a different kind of question in a
different register, and a draft is something nobody has said yet.

**Machines are not exemplars.** An outbound message Front records no author for
was sent by a rule or by an API token, not by a colleague. An inbound one whose
subject reads `Autosvar:`, `Out of office`, `Undeliverable` and the like is a
holiday responder or a bounce, not a question. Both are dropped *before* pairing
rather than after, so an automated acknowledgement cannot stand between a
question and the person who actually answered it.

**HTML when there is no text.** Front leaves `text` empty on some HTML-only
messages, and storing "" for those would drop them from the corpus without a
word. The fallback flattens `body` instead, cutting at the first quote container
— `blockquote`, Gmail's `gmail_quote`, Outlook's `divRplyFwdMsg`, Apple's
`messageReplySection` — since quoting is explicit in HTML and guesswork in text.

**Searches are prefix matches.** Norwegian inflects on the end of the word and
fts5 has no stemmer for it, so `blodprøve` is searched as `blodprøve*` and
reaches *blodprøven*, *blodprøver*, *blodprøvene*. Without it the stem alone
matches nothing.

**A reply with nothing pending is not a pair.** Outbound with no unanswered
inbound before it is an outreach that opened the thread, or a second message
from the same person — it teaches nothing about how a question gets answered.

**Replies under 20 characters are skipped.** "Bare hyggelig!" is not an example
of anything.

**Norwegian had to be added.** `mail-parser-reply` ships Danish and Swedish but
not Norwegian, so `sync.py` registers a language built from the Danish one:
`skrev` survives, the Outlook header words and the sign-offs do not. Improving
it means editing that block and running `--rebuild` — no re-fetching.

**FTS5 first, vectors later.** Keyword search with recency and author filters
answers "how did we answer this before" well enough to be worth having now.
`search_replies` keeps its signature when an embedding column is added behind
it, so nothing that calls it has to change.

## Checking it

The step most likely to be quietly wrong is the cleaning, and the way to see it
is to read some:

```sh
sqlite3 ~/.rigg/mail/default.db \
  "SELECT reply_text FROM pairs ORDER BY random() LIMIT 10;"
```

Quoted history and sign-offs should be gone and the answer itself intact. When
it is not, fix the language block and `./mail/run.sh --rebuild`.

`./mail/test.sh` runs the checks on a fixture instead of on Front — cleaning,
pairing, the FTS index staying in step with its table, and the ranking rule.
Worth running after editing the language block, since most of what goes wrong
there is silent.

## What it does not do

- **No embeddings.** Keyword search only, for now.
- **No retention rule.** The corpus keeps everything the backfill reached, and
  nothing propagates a deletion in Front back to it.
- **No deduplication.** Several near-identical canned replies can fill every
  slot a search returns.
- **No attachments.** A reply saying "se vedlagt" arrives without knowing what
  was attached.
- **Nothing is ever sent.** `create_draft` leaves a draft for a person to send.
