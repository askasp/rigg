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

Mention the bot in a channel, or DM it. `help` prints the list that *this*
channel is allowed to use, with the repo's own pipeline names filled in.

**Start work**

| | |
| --- | --- |
| `new <task>` | a stack, on the default pipeline |
| `<pipeline> <task>` | a stack, on a named one: `preview fix the button` |
| `add <stack> <task>` | stack another branch on top of one |

**While it is running**

| | |
| --- | --- |
| `say <stack> <message>` | a follow-up turn in that stack's session |
| `stop <stack>` | cancel it (`cancel` also works) |
| `retry <stack> [step]` | run it again, from a step if you name one |

**Have a look**

| | |
| --- | --- |
| `stacks` | this channel's stacks and how they are doing |
| `logs <stack>` | the tail of a run's log |
| `urls <stack>` | links the run printed — previews, PRs |
| `pipelines` | what this repo offers |

**When it has landed**

| | |
| --- | --- |
| `rm <stack>` | what removing it would do |
| `rm <stack> yes` | actually remove it, running the repo's `teardown` first |

Stacks are referred to by their short name — `add proration-e4f "..."`, not the
full `billing/proration-e4f`.

A pipeline is chosen by typing its name instead of `new`, and any unambiguous
part of it will do — `preview fix the button` reaches `full-preview`. A flag
would have been fewer lines here and worse to explain to anyone who does not
write code for a living. `commands` gates these like any other verb, so a
channel limited to `stacks` and `logs` cannot start one.

`rm` mirrors the CLI in being a dry run until you add `yes`, which matters
because it also stops whatever the branch had running — a dev stack, a tunnel.

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

Put the two tokens in `slack/.env` (gitignored, so they stay out of shell
history):

```sh
cp slack/env.example slack/.env    # then paste the tokens in
```

Check the wiring before starting anything — it tells the failures apart rather
than letting the first message fall over:

```sh
./slack/run.sh --check
```

```
bot token      ok - rigg in Amino
app token      ok
#aksel-dev     ok - private, /home/aksel/git/amino-monorepo
#support       found, but the bot is not in it - `/invite @rigg`

1 problem(s) to fix
```

Then start it:

```sh
./slack/run.sh
```

It prints the channel→repo map and stays in the foreground. Socket Mode dials
out to Slack, so there is no port to open and nothing to expose.

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
- `say` holds a worker thread for the whole turn; four can run at once.
