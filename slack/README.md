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

```
@rigg new redo the payment screen
```

That is the whole common case: it does the work, reviews it, fixes what the
review found, and hands back a URL to look at. Everything below is optional.

**Start work**

| | |
| --- | --- |
| `new <task>` | the above |
| `add <stack> <task>` | stack another branch on top of one |

**While it is running**

| | |
| --- | --- |
| `say <stack> <message>` | a follow-up on that branch — committed and pushed |
| `stop <stack>` | cancel it (`cancel` also works) |
| `continue <stack> +part` | take it further than it was started with |
| `retry <stack> [step]` | run the same thing again |

**Have a look**

| | |
| --- | --- |
| `stacks` | a name per stack in the channel; open one for its card (`list`, `status`) |
| `logs <stack>` | the tail of a run's log |
| `urls <stack>` | links the run printed — previews, PRs |
| `parts` | the optional parts this repo has |
| `pipelines` | named pipelines, where a repo has any |

**When it has landed**

| | |
| --- | --- |
| `rm <stack>` | what removing it would do |
| `rm <stack> yes` | actually remove it, running the repo's `teardown` first |

`say` continues the same branch and the agent's own session — use it for
"also do X" on work already done. `add` starts the *next* branch on top, with
its own PR, for something that should be reviewed separately.

When a `say` finishes, the reply says so and lists the preview URLs if one is
up — the containers mount the checkout, so the change is already live there.

A `say` is committed and pushed, using your message as the commit subject.
That is the default because the containers mount the checkout: a follow-up is
live in the preview the moment it lands, so leaving the PR behind would mean
the visible thing and the reviewable thing quietly disagree. End with `-push`
to change the branch without pushing.

### Changing one run

Put any of these on the end of a task, in any order:

| | |
| --- | --- |
| `+copilot` / `-preview` | turn an optional part on or off — `parts` lists them |
| `effort=high` | set a prompt placeholder |
| `ai=opencode` | run it on a different agent |

```
@rigg new redo the payment screen +copilot effort=high ai=opencode
```

They are only taken off the end, and `+x` / `-x` only when they look like a
part — so "make the button 2+2 wide" and "set FOO=bar in the config" stay
tasks.

### Naming a stack

Any unambiguous part of the name will do — `say in-b2b ...` reaches
`in-b2b-dashboard-in-fd4`, matching a prefix first and then anywhere in the
name, since the random suffix is not something to read back. An ambiguous
fragment says what it matched.

Better still, **leave the name out and reply in the stack's own thread**.
`stacks` posts one message per stack for exactly this — reply under the one
you mean:

```
@rigg say make the bars blue
@rigg urls
@rigg continue +copilot
```

That works in any thread belonging to a stack: the one `stacks` posted, or the
one a run reports into. It applies to `add`, `say`, `stop`, `retry`,
`continue`, `urls`, `logs` and `rm`.

A listing puts names in the channel and everything else - branches, state,
preview URLs - one level down in each name's thread, so it stays glanceable
however many stacks there are. It answers in the channel rather than in a
thread, since it is what you look at to decide which thread to open — and each stack needs to be top-level
to have a thread of its own. Everything else replies in the thread it was
asked in.

A listing gives a stack a thread to be addressed in without moving where its
runs report — that stays wherever the run was started.

## Channels, and who is allowed

A channel does nothing until it is listed in `channels.toml`. That mapping is
what says which repo it drives, and an unmapped channel being inert is the
access control — but it costs no restart: the file is re-read when it changes,
so adding a channel is an edit plus `/invite @rigg`. A broken edit is reported
and the last good config keeps serving.

*Who* may use a mapped channel is Slack's business: whoever is in it. A second
list of user IDs here would only duplicate what Slack already keeps, and then
drift from it. Which means a channel you map is a channel whose members you are
handing an agent to — a message becomes a prompt for an agent running with
`bypassPermissions`, so **keep it private**. `--check` says which mapped
channels are public, and the bridge repeats it the first time it sees a message
in one.

`commands` narrows what a channel may do, which is how a wider channel can
watch without being able to start work:

```toml
# Private, just you: everything.
[channels.rigg-tasks]
repo = "~/git/amino-monorepo"

# The team can look, not launch. They see this channel's stacks, not yours.
[channels.bugs_feil]
repo = "~/git/amino-monorepo"
commands = ["stacks", "logs", "urls", "help"]
```

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

**Ignore the App Credentials block** on the Basic Information page — App ID,
Client ID, Client Secret, Signing Secret, Verification Token. None of them is
used here, and copying them anywhere is only a way to leak them:

| | |
| --- | --- |
| Client ID / Secret | the OAuth flow for installing into *other* workspaces. This app is installed directly into yours. |
| Signing Secret | verifies inbound HTTP webhooks. Socket Mode has no inbound HTTP. |
| Verification Token | the deprecated predecessor of the signing secret. |
| App ID | an identifier, not a secret. |

The token you want is further down that same page, under **App-Level Tokens**.

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

## Images

Paste a screenshot into the message and the agent gets it:

```
@rigg preview [screenshot] this banner is misaligned, fix the spacing
```

The bridge downloads anything image-shaped attached to the message, hands it to
rigg with `--image`, and rigg puts it in `.rigg/media/` inside the branch's
checkout with the prompt naming the files. Works with `new`, a pipeline word,
`add` and `say`, and several images at once.

This needs the **`files:read`** scope. Slack keeps files behind an
authenticated URL, and without the scope a fetch returns Slack's sign-in page
as a `200` — so the bridge checks the content type rather than trusting the
status, and says so when the scope is missing.

The bridge always passes `--no-paste`. rigg reads the clipboard when a person
runs it in a terminal; the clipboard of whatever desktop the bridge happens to
run on has nothing to do with whoever sent the message.

## Sending a file back

A pipeline step can put a file into the thread the run reports in — Playwright
screenshots, a coverage report, anything worth looking at rather than reading
in a log:

```toml
[[steps]]
id = "e2e"
run = """
npx playwright test
for f in test-results/**/*.png; do
  /home/aksel/git/rigg/slack/upload.sh "$f" "failure shot"
done
"""
```

`RIGG_BRANCH` is already in the environment of a step, which is how the script
finds the thread. It needs the **`files:write`** scope, and exits quietly when
there is no token or no thread — so a run started by hand is unaffected.

It uses Slack's external-upload flow rather than `files.upload`, which is
deprecated and being removed.

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
