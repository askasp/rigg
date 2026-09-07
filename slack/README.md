# rigg in Slack

A channel drives a repo. Stacks a channel starts are named `<channel>/<slug>`,
so two channels can share a repo without colliding and each sees only its own
work. Progress reports back into the thread that asked for it.

```
#billing  @rigg new add proration to subscription changes
          rigg  started `billing/add-proration-to-subscri-e4f`
          └─ *billing/add-proration-to-subscri-e4f* started - 9 step(s)
          └─ *…* [1/9] `implement` ok (2m14s)
          └─ *…* [2/9] `review` ok (48s)
```

## Commands

Mention the bot in a channel, or DM it.

| | |
| --- | --- |
| `new <task>` | start a stack and run the pipeline on it |
| `add <stack> <task>` | stack the next branch on top of one |
| `say <stack> <message>` | another turn in that stack's session |
| `stacks` | this channel's stacks and their state |
| `logs <stack>` | the tail of a run's log |
| `help` | the above |

Stacks are referred to by their short name — `add proration-e4f "..."`, not the
full `billing/proration-e4f`.

## Setting up the Slack app

1. **Create the app** at <https://api.slack.com/apps> → *Create New App* →
   *From an app manifest* → pick the workspace → paste `slack/manifest.yaml`.
   That sets every scope and event in one go.
2. **Basic Information → App-Level Tokens → Generate**, scope
   `connections:write`. This is `SLACK_APP_TOKEN` (`xapp-…`). Slack will not
   mint this from a manifest, so it has to be done here.
3. **Install App → Install to Workspace**. This is `SLACK_BOT_TOKEN`
   (`xoxb-…`).
4. In Slack, invite the bot to each channel: `/invite @rigg`.

## Running it

```sh
cp slack/channels.example.toml slack/channels.toml   # then edit
export SLACK_BOT_TOKEN=xoxb-...
export SLACK_APP_TOKEN=xapp-...
uv run --with slack-bolt slack/rigg_slack.py
```

## Who is allowed

Whoever is in the channel. Slack already decides that, and a second list of
user IDs here would only duplicate it and then drift from it — so the channel
*is* the boundary, and an unmapped channel does nothing at all.

Which means a channel you map is a channel whose members you are handing an
agent to. A message becomes a prompt for an agent running with
`bypassPermissions`, so put it in a **private** channel. If you map a public
one the bridge says so the first time it sees a message there.

`commands` narrows what a channel may do, which is how a wider channel can
watch without being able to start work:

```toml
# Private, just you: everything.
[channels.aksel-dev]
repo = "~/git/amino-monorepo"

# The team can look, not launch. They see this channel's stacks, not yours.
[channels.support]
repo = "~/git/amino-monorepo"
commands = ["stacks", "logs", "help"]
```

## Reporting back

Progress arrives through rigg's notify hook, which the bridge does not have to
be running to serve. Add to the repo's `rigg.toml`:

```toml
[notify]
command = "/home/aksel/git/rigg/slack/notify.sh"
```

rigg runs that at the start of a run, after every step, and when the run ends,
passing the details as `RIGG_*` environment variables. `notify.sh` looks up
which channel and thread the branch belongs to — the bridge records that in
`.git/rigg/slack/<branch>.json` when it starts the stack — and posts there.

`SLACK_BOT_TOKEN` reaches the hook because a detached run inherits the bridge's
environment. A run you start by hand has no token, and the hook exits quietly.

## Testing it without Slack

`notify.sh` honours `SLACK_API_URL`, so the whole reporting path can be pointed
at a local server:

```sh
SLACK_API_URL=http://127.0.0.1:8998/ SLACK_BOT_TOKEN=x rigg new demo "..." --fg
```

The command handling is importable and takes its `say` as an argument, so it
can be exercised against a scratch repo with no Slack credentials at all.

## What it does not do yet

- **A `confirm` step blocks a detached run**, so a pipeline with one cannot be
  started from Slack at all. rigg says so rather than skipping the step.
- No approval buttons — every message in an allowed channel runs immediately.
- Always runs the repo's default pipeline; no way to pick `full-preview` from
  Slack yet.
- `say` holds a worker thread for the whole turn; four can run at once.
