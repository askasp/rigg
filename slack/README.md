# rigg in Slack

A channel drives a repo. Stacks a channel starts are named `<channel>/<slug>`,
so two channels can share a repo without colliding and each sees only its own
work. Progress reports back into the thread that asked for it.

```
#billing  @rigg new add proration to subscription changes
          rigg  started `billing/add-proration-to-subscri-e4f`
                1  implement     impl      opencode vllm/qwen3-coder-next
                2  review        reviewer  opencode vllm/qwen3-coder-next
                3  apply-review  impl      opencode vllm/qwen3-coder-next
                …
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
| `adopt <branch>` | all of that on a branch that exists already |
| `add <stack> <task>` | stack another branch on top of one |

`adopt` is for a branch nobody here started - a colleague's PR, something cut
by hand. It gets the same treatment as anything else: a preview to look at, a
review, and then `say` and `add` to carry it on.

```
@rigg adopt feature/login
@rigg preview adopt feature/login        # only put it on a URL
@rigg adopt feature/login also fix the spacing
```

The stack is named `<channel>/<branch>`, so it belongs to this channel like any
other and the branch name is still what you type to reach it. `rm` never
removes the branch itself - rigg did not make it.

**While it is running**

| | |
| --- | --- |
| `say <stack> <message>` | a follow-up on that branch — committed and pushed |
| `stop <stack>` | cancel it (`cancel` also works) |
| `continue <stack> +part` | take it further; queues if a run is still going |
| `retry <stack> [step]` | run the same thing again |

`say` is a sentence to the agent; `continue` is more of the pipeline. So a part
belongs to `continue` — `say run +copilot` would spend a turn reading
`+copilot` as a word and move no step, and is answered with the `continue` that
was meant instead. A `say` reports once, when it is done: it is one turn, not a
run, and numbering it `[1/1]` would only lose the branch's real place.

`continue` numbers its steps where they sit in the pipeline the branch started
with, so `[8/9] audit` lines up with line 8 of the plan posted above.

**Ask about the code**

| | |
| --- | --- |
| `ask <question>` | answered from the repo; no branch, nothing changed (`q`) |

A question takes `ai=opencode` on the end like a task does, and `retry` takes
everything a task does. `say` and `summary` do not: they carry on the session
the branch already has, and that session belongs to the agent that ran it.

An answer is rewritten into what Slack actually renders: `##` headings become
bold lines, `**bold**` becomes `*bold*`, links become Slack links, and a
Markdown table is laid out as aligned columns in a code block, since Slack has
no table of its own.

A question is not a task, so `ask` makes no stack, no worktree and no PR — it
answers in the main checkout with the agent in a read-only mode, and the
checkout is untouched afterwards.

**A thread is a conversation.** A new message starts one; every reply inside
its thread carries that same one on — and once a thread is a conversation you
can drop the verb, since `@rigg what about the fee?` in there can only mean
the next question. The mention is still needed: Slack only tells the bot about
messages that name it.

A new message starts one; every reply inside its thread carries that same one on, and two threads never answer each other's
questions — the conversation is resumed by id, not by "whichever spoke last in
this checkout", which is what `--continue` alone would mean.

**Have a look**

| | |
| --- | --- |
| `stacks` | live stacks and where each has got to; clears merged ones (`list`, `status`) |
| `summary <stack>` | ask the agent what it did, what was wrong, and what is left |
| `logs <stack>` | the tail of a run's log |
| `urls <stack>` | links the run printed — previews, PRs |
| `parts` | the optional parts this repo has |
| `pipelines` | named pipelines, where a repo has any |

**What an agent can draw on**

| | |
| --- | --- |
| `corpus` | bodies of past work it can search, and how fresh each is |
| `corpus search <text>` | what an agent drafting that would be shown |
| `corpus add mail` | build one from Front, and keep it current |

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

So is anything a run leaves behind. `continue +copilot` applies the review
after the branch has already been pushed, so without this the fixes sat in the
checkout - live in the preview, absent from the PR - and nothing said so.

### What it answers with

`new`, `adopt` and `add` reply with the branch and then the plan: every step
the run will go through, in order, and for each agent step the role, the agent
and the model it will actually run on. The worktree path and the pid rigg
prints on a terminal are left out — they belong to the machine, not the
channel.

The modifiers below move both halves of that, which is the other reason it is
worth printing: `+copilot` adds three steps, `ai=reviewer=claude` changes what
line 2 says it will run on. `rigg plan` prints the same listing on a terminal.

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

`summary` is a turn, not a log: the session already holds the task, the review
and every change, so the agent answers from what it knows rather than reading
anything back. That is what makes it quick — around half a minute — and why it
can say what a bug's root cause turned out to be, which no log contains.

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

That works in any thread belonging to a stack: the one `stacks` posted, the one
a run reports into, or the one you started the work in. It applies to `add`,
`say`, `summary`, `stop`, `retry`, `continue`, `urls`, `logs` and `rm`.

A thread is recorded by its own id rather than by the id of the message the
answer happened to be. An answer is posted *into* the thread that asked for it,
so its id belongs to a reply inside that thread and no later reply there
arrives carrying it - which is why this used to work only in the threads
`stacks` posts, those being the only ones the bridge starts itself.

A listing leaves out any stack in this channel whose every branch has landed
on the trunk — a merged stack is finished with, and the next piece of work
wants a new one. Landed means its *work* is on the trunk: a branch that never
committed is contained in the trunk as well, trivially, so a stack that only
answered a question stays where it is rather than being cleared as finished. It names them as going and removes them behind you: each goes
through its own teardown, which stops a dev stack and its tunnels and takes
tens of seconds, so waiting for it would make a listing something you avoid
running. A stack with a run in flight is never touched.

A listing puts names in the channel and everything else - branches, state,
preview URLs - one level down in each name's thread, so it stays glanceable
however many stacks there are. It answers in the channel rather than in a
thread, since it is what you look at to decide which thread to open — and each stack needs to be top-level
to have a thread of its own. Everything else replies in the thread it was
asked in.

A listing gives a stack a thread to be addressed in without moving where its
runs report — that stays wherever the run was started.

### Recurring jobs

A schedule you say, rather than one you deploy:

```
@rigg cron 0 7 * * 1-5 digest        # weekday mornings
@rigg cron @daily new tidy the TODOs
@rigg cron                           # what this channel has
@rigg cron show 1                    # everything about one
@rigg cron run 1                     # run it now, off-schedule
@rigg cron rm 2
```

`cron run <id>` is how you find out whether a job works without waiting until
07:00 — it goes through exactly the path a schedule does, so what you see is
what the morning will do.

`cron show <id>` answers "what will this actually do". The job stores what to
run, not a copy of the steps — the steps belong to the pipeline and rigg
resolves them at the time — so `show` asks rigg and prints both:

```
*job `1`* in #rigg-tasks
schedule  `0 7 * * 1-5`
next      Thu 10 Sep 07:00, Fri 11 Sep 07:00, Mon 14 Sep 07:00
repo      /home/aksel/git/amino-monorepo
asked     read my Front inbox and summarise yesterday
runs      `rigg run --pipeline do --task read my Front inbox and summarise yesterday`
last      ok at Wed 09 Sep 12:57

steps of `do`:
    1   do  auditor  claude opus[1m]

last output:
    …
```

The last run's output is kept with the job, because a `run` has no branch and
so there is no `rigg logs` to go and read.

The schedule is five cron fields — minute, hour, day, month, weekday — or one
of cron's own aliases (`@daily`, `@hourly`, `@weekly`, `@monthly`). Nothing
invented: paste the same expression into `crontab` and it means the same thing.
Because a fumbled field is otherwise silent — the job simply never fires — the
reply says when it will next run, three times over:

```
`1` scheduled: `0 7 * * 1-5` → `digest` (in this checkout, no branch)
next: Thu 10 Sep 07:00, Fri 11 Sep 07:00, Mon 14 Sep 07:00
```

**Or just say what you want.** If the repo has a pipeline called `do`, anything
that is not a pipeline name is taken as instructions for it:

```
@rigg cron 0 7 * * 1-5 read my Front inbox and summarise yesterday
```

```
`1` scheduled: `0 7 * * 1-5` → the `do` pipeline, no branch
> read my Front inbox and summarise yesterday
next: Thu 10 Sep 07:00, Fri 11 Sep 07:00, Mon 14 Sep 07:00
```

That pipeline is four lines, written once per repo — one agent turn on the task,
with a prompt file that sets the boundaries:

```toml
[[pipelines.do.steps]]
id = "do"
agent = "impl"
clear = true
prompt_file = "do.md"
```

`slack/do.example.md` is a `do.md` to start from — it sets the reporting style,
forbids committing and sending, points the agent at tooling this machine
already has before it reaches for raw HTTP, and defines the credential protocol
below. Copy it to the repo's `.rigg/do.md`.

Without a `do` pipeline a job must name one, or say `new <task>`; either way it
is refused when you define it rather than failing quietly at 07:00, and the
refusal lists what the repo actually has.

The listing shows the **last** run as well as the next, because a job that
quietly stopped is otherwise invisible:

```
`1`  `0 7 * * 1-5`  read my Front inbox and summarise  — last ok Thu 07:00, next Fri 07:00
`2`  `@daily`       digest  — never run, next 00:00
```

A job already in flight reads `running now` there, and its next firing is
skipped rather than started beside it — `rigg run` takes no lock on the
checkout, so two turns of a `*/5` job that takes six minutes would otherwise be
two agents editing one working tree. `cron run <id>` says so instead of
starting a second copy.

### When a job needs a credential

A run that turns out to need something it has not been given does not guess, and
does not fail into a log nobody reads. It prints one line and stops:

```
RIGG-NEEDS-SECRET: FRONT_API_TOKEN | a Front API token with scopes shared:conversations and shared:inboxes, to read the support inbox | https://app.frontapp.com/settings/api
```

and that arrives in the channel that scheduled it, as a question with an answer:

```
`1` stopped: it needs a credential it does not have.
*FRONT_API_TOKEN* — a Front API token with scopes shared:conversations and
shared:inboxes, to read the support inbox
  get one at https://app.frontapp.com/settings/api
  send it with `secret FRONT_API_TOKEN <value>`

It will run as scheduled once that is set.
```

A job that *succeeds* says nothing unless it asks to. Everything after a line
containing exactly `RIGG-REPORT` is posted to the channel, and nothing else is
— the run's output also carries rigg's own step chrome, which nobody wants in
Slack. A job that pushes a branch wants silence; one that answers a question
does not.

Both are lines of output rather than exit codes, so that anything in a pipeline
can raise them — an agent turn, a shell step, an MCP server — without rigg
having to know which. The `do.md` prompt is what tells an agent to use it, and to name
the scopes precisely, since whoever reads the message has to tick those boxes.

The question is asked **once**. A job that fires every five minutes and lacks
one token would otherwise put the same request in the channel 288 times a day,
so a job stuck in a state — waiting for a credential, or pointed at a repo that
has been moved — says so the first time and then only in the log, until the
state changes. A plain failure is not held back that way: it is rarely the same
failure twice, and quiet is how a job that stopped working goes unnoticed.

Credential values are taken out of what a run printed before any of it is
stored or posted. An agent that inlines a token instead of naming the variable
would otherwise leave it in the job file and read it back out on `cron show`.

### When a run learns something

A scheduled run starts from a cleared context every time, so what it works out
today is gone by tomorrow. One line says otherwise:

```
RIGG-LEARNED: Front's /events only reaches back about 30 days.
```

Those are appended, dated and attributed to the job, to
`~/.rigg/instances/<instance>/notes.md` — which the `do.md` prompt reads back at the start
of the next run — and the report says what was noted. Written the same way a
secret is: by the bridge, on the run's say-so, never by the run itself.

Not in the target repo's `.rigg/capabilities.md`, deliberately. That file is
hand-written and reviewed like code; an unattended writer would leave a dirty
working tree in the checkout branches are cut from, and most of what a run
works out is about *this workspace's* accounts rather than that repo. A note
that turns out to hold for the repo is promoted by a person moving the line —
which is the review you would want anyway. Prune the notes; it is a scratchpad,
not a record.

**A pipeline on a schedule runs `rigg run --pipeline`** — in the checkout, no
branch. Typed interactively a pipeline word means `new`, but a job that made a
branch every morning would leave a few hundred a year, and a recurring job that
reads and reports wants neither a branch nor a worktree. Say `new` when you do
want one; the branch is then named `<channel>/cron-<id>`.

Jobs belong to the channel they were defined in — they run against that
channel's repo and report failures back there — and the store is one file per
instance, `~/.rigg/instances/<instance>/cron.json`, so two workspaces never see each
other's. Defining one counts as starting work, so a look-but-not-launch channel
cannot.

They fire from the bridge, not from systemd, because this process is already
the per-instance daemon. Two consequences worth knowing: **a job only fires
while the bridge is up**, which is the same window in which anyone could have
typed the command anyway; and minutes that pass while the machine is asleep are
not caught up on, so a laptop opened after a long weekend does not fire three
days of digests at once.

`python3 slack/test_cron.py` checks the parsing and the store — 126 assertions,
no dependencies. Worth running after touching the schedule fields, since a
wrong one is invisible until the job does not fire.

### What a job can draw on

A job that drafts wants examples. `corpus` is the body of past work an agent
can search while it works — today, the mail corpus [`mail/`](../mail/README.md)
builds out of Front:

```
@rigg corpus                              # what exists, how big, how fresh
@rigg corpus add mail                     # token, first import, and the sync that keeps it current
@rigg corpus search hva koster blodprøve  # what an agent drafting that would be shown
@rigg corpus show mail
@rigg corpus sync mail                    # fetch now, off-schedule
@rigg corpus rm mail
```

`corpus add` is the whole setup. It asks for the token the way a job does —
same words, same `secret NAME <value>` answer — registers the corpus, starts
the first import in the background, and says when it is searchable:

```
@rigg corpus add mail
rigg  `mail` needs a credential first.
      *FRONT_API_TOKEN* — a Front API token with scopes shared:conversations
      and shared:inboxes, to read the inboxes this corpus is built from
        get one at https://app.frontapp.com/settings/api
        send it with `secret FRONT_API_TOKEN <value>` in a private channel

      Then `corpus add mail` again.
```

After that there is no crontab line to write: the bridge refreshes it every 15
minutes and walks the last month once a week, because Front's `/events` is
known to drop inbound mail and a corpus nobody reconciles is one that quietly
disagrees with the mailbox.

**`corpus search` is the point.** It runs the same ranking the agent's own tool
runs — one implementation, in `mail/corpus.py`, called from both — so what
comes back is what a run would be shown, and you can find out whether the 07:00
draft has anything worth drafting from without waiting until 07:00:

```
@rigg corpus search hva koster en blodprøve
rigg  top 3 of what `mail` would show an agent answering that:

      *2026-09-04*  ola@example.com  ·  Pris på blodprøve
      > Hva koster en blodprøve hos dere?
      ```
      Hei! En blodprøve koster 450 kroner, og du trenger ikke bestille time.
      ```
      …
      _Ranked closest-and-newest first. Recency is weighted hard: what goes
      stale in a mailbox is what a draft repeats._
```

A corpus belongs to the **instance**, not to a channel and not to a repo —
`~/.rigg/instances/<instance>/corpus/<instance>.db` is one workspace's mail and nobody else's — so
every channel of an instance sees the same corpora, and the register is
`~/.rigg/instances/<instance>/corpus.json`. Which is the point: the corpus outlives the
repo that happens to be drafting from it this month.

#### Letting a run search one

Registering a corpus does not hand an agent a tool. That stays where rigg
already puts tools — a role's `args` in the repo's own config, and a step
gated on a feature — so `+mail` on a task, `parts` and `plan` go on meaning
what they mean for everything else. `corpus add` prints the config to paste:

```toml
[features]
mail = false          # off unless a task asks for it

[agents.mailer]
kind = "claude"
args = ["--permission-mode", "bypassPermissions",
        "--mcp-config", ".rigg/front-mcp.json", "--strict-mcp-config",
        "--allowedTools", "mcp__front"]

[[pipelines.do.steps]]
id = "answer-mail"
agent = "mailer"
feature = "mail"
```

Then it is a part like any other, on a task or on a schedule:

```
@rigg new draft replies to anything unanswered overnight +mail
@rigg cron 0 7 * * 1-5 draft replies to anything unanswered +mail
```

```
`1` scheduled: `0 7 * * 1-5` → the `do` pipeline, no branch
with: `--with mail`
> draft replies to anything unanswered
next: Thu 10 Sep 07:00, Fri 11 Sep 07:00, Mon 14 Sep 07:00
```

A schedule takes every modifier a typed task takes — `+mail`, `effort=high`,
`ai=opencode` — and they are stripped when the job is *defined* rather than at
every firing, so `cron show` prints the command line the morning will actually
run. Without that, `digest +mail` would schedule a pipeline nobody has, and a
freeform job would carry `+mail` into the prompt as two more words of
instruction.

A part the repo does not declare is refused there and then, the way an
unrunnable pipeline is — rigg treats an undeclared `feature` as a config error,
so the job would otherwise be accepted here and die at 07:00:

```
@rigg cron @daily draft replies +mail
rigg  `mail` is not a part of this repo. It has `preview`, `copilot` —
      `parts` lists what each one contains.
      A corpus is not a part until a role in `.rigg/rigg.toml` holds the tools
      to search it; `corpus show <name>` prints that config.
```

Leave the draft tool out of that config unless the role is meant to answer
mail. The corpus is written by strangers: a role that can read it *and* write
to a conversation is one injected sentence from drafting whatever a sender
asked for, signed by a colleague. [`mail/README.md`](../mail/README.md) has the
two configs side by side.

#### When a job wants one

The twin of the credential protocol. A run that finds it has nothing to draft
from prints one line and stops:

```
RIGG-NEEDS-CORPUS: mail | past replies to draft from
```

and the channel that scheduled it gets a question with an answer:

```
`1` stopped: it wants a corpus this instance does not have.
*mail* — past replies to draft from
  build it with `corpus add mail`

It will run as scheduled once that exists, and will not ask again in the
meantime.
```

Like the secret one, it is a line of output rather than an exit code, so any
step can raise it — an agent turn, a shell step, an MCP server — without rigg
having to know which. And like the secret one it is said once: a `*/15` job
missing a corpus all night is one problem, not ninety-six.

`python3 slack/test_corpora.py` — 53 checks on the register, the search and the
protocol, no dependencies and no network. The one it exists for is that Slack's
search and the agent's tool come back with the same rows.

### Credentials

```
@rigg secret FRONT_API_TOKEN fr_live_…
@rigg secret                      # what is set — never what it is
@rigg secret rm FRONT_API_TOKEN
```

Stored per instance, `0600` in a `0700` directory, and picked up by the *next*
run without restarting anything — the bridge reads the file when it starts a
run rather than inheriting it once at boot.

Two files, deliberately:

| | |
| --- | --- |
| `~/.rigg/instances/<instance>/secrets.env` | Ansible templates this from the vault |
| `~/.rigg/instances/<instance>/secrets.local.env` | this writes this one |

Anything written from chat into the first would vanish on the next converge, so
it goes in the second, which Ansible never touches and which wins when both set
the same name. The listing says which source each name came from.

Values never come back — a listing shows `…4f2d`, and removing a name the vault
owns tells you to go and do that in Ansible instead.

**Slack keeps the message.** A bot cannot delete someone else's, so a token sent
this way sits in the workspace history until a person removes it. That is the
trade for setting one without a deploy: it is refused outside a private channel
or DM, and the reply says to delete the message and treat the credential as one
that gets rotated. For anything long-lived, the vault is the better home.

`python3 slack/test_creds.py` — 28 checks, no dependencies.

### Saying it here, or declaring it in Ansible

Both exist and they are for different things. `cron` here is for what you want
now and may drop next week — it needs no deploy and no root, and it is gone
when you say `cron rm`. The `schedules:` block in the Ansible role is for a job
that has to be there after a rebuild: it is in git, it is reviewed, and it is
recreated on a fresh machine. A morning digest you are still tuning belongs
here; the one you rely on belongs there.

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

## A second workspace

One process is one workspace. There is no `team_id` check anywhere in the bridge
and no installation store — the two tokens it holds simply *are* a workspace — so
a second Slack means a second app, a second pair of tokens and a second process.
The channel→repo map is already per-instance and already multi-repo, so nothing
about the first one changes.

An **instance** names that set, and `run.sh` takes one as its first argument:

| | env file | channels |
| --- | --- | --- |
| `./slack/run.sh` | `slack/.env` | `slack/channels.toml` |
| `./slack/run.sh acme` | `slack/acme.env` | `slack/acme-channels.toml` |

Both are gitignored (`slack/*.env`, `slack/*channels.toml`). Set the second one
up exactly like the first — a new app from `manifest.yaml` in that workspace, its
own app-level token, `/invite @rigg` — and then:

```sh
./slack/run.sh acme --check
./slack/run.sh acme
```

Run the two side by side. They share nothing: Socket Mode dials out so there is
no port, and every file either one writes lives in the target repo's own git
directory. Two things do need keeping apart — two channels of the *same name*
pointed at one repo would share a stack namespace, since stacks are
`<channel>/<slug>`; and two repos whose directory basename matches would share a
worktree directory, since those are `~/.rigg/worktrees/<basename>`.

Each instance exports `RIGG_INSTANCE` into everything it starts (`default` when
unnamed). rigg inherits its whole environment, so the agent, its shell steps and
any MCP server they spawn all know which workspace they are working for — which
is what keeps per-workspace state apart without a registry of workspaces.

Anything else an instance needs goes in its env file beside the tokens. It is
sourced before rigg starts and inherited all the way down, which is already how
`SLACK_BOT_TOKEN` reaches `notify.sh`.

## Several on one machine

Instances share nothing of their own: tokens, channels, stacks, corpus and
`RIGG_INSTANCE` are all per-instance by construction. But three things on disk
are keyed by something *other* than the instance, and each is silent when two
land on the same one:

| keyed by | shared when |
| --- | --- |
| the stack namespace — stacks are `<channel>/<slug>` | two instances have a channel of the **same name** on the same repo |
| `stack.json`, in the repo's shared git dir | two instances drive the **same repo** at all |
| `~/.rigg/worktrees/<basename>` | two repos have the same directory **basename** |

```sh
python3 slack/instances.py
```

prints every instance on this box with its channels, repos and unit state, and
names any of the three. It exits non-zero when it finds one, so it can gate a
deploy. Nothing to install — it is stdlib only.

The first is the one that actually corrupts: two `#rigg-tasks` on one repo see
each other's stacks, and `stop` in one workspace can kill the other's run.
Rename the channel in one. The second is a race that already exists between two
channels of one instance — `stack.json` is rewritten whole — so keep two
instances off one repo unless the work never overlaps.

### Keeping them out of each other's way

One slice per instance, so a busy tenant cannot starve the rest. This machine
has cgroup v2 with `cpu`, `memory` and `pids` delegated to the user manager, so
these are enforced rather than decorative — check with
`cat /sys/fs/cgroup/user.slice/user-$(id -u).slice/cgroup.controllers`.

```ini
# ~/.config/systemd/user/rigg-acme.slice
[Slice]
MemoryMax=10G
CPUQuota=300%
TasksMax=4096
```

```ini
# ~/.config/systemd/user/rigg-slack@.service
[Unit]
Description=rigg Slack bridge (%i)
After=network-online.target

[Service]
Type=simple
WorkingDirectory=%h/git/rigg
ExecStart=%h/git/rigg/slack/run.sh %i
Restart=on-failure
RestartSec=5
KillMode=process
Slice=rigg-%i.slice

[Install]
WantedBy=default.target
```

```sh
systemctl --user daemon-reload
loginctl enable-linger "$USER"          # or nothing starts at boot
systemctl --user enable --now rigg-slack@acme
```

**`KillMode=process` is load-bearing.** `rigg new` detaches its run under
`setsid`, so in-flight runs sit in the unit's cgroup; with systemd's default
`control-group`, `systemctl restart` kills every agent mid-edit. The `Slice=`
still applies to them, so the caps hold either way.

Stagger the mail syncs rather than having every instance hit Front at once —
the rate limit is per company, not per token:

```ini
# ~/.config/systemd/user/rigg-mail@.timer
[Timer]
OnCalendar=*:0/15
RandomizedDelaySec=300
Persistent=true
```

And under a timer, schedule `run --pipeline <name>` or pass `--fg` to `new`: a
detached run under `Type=oneshot` is killed the moment the unit's main process
exits.

### What actually limits how many

Not rigg — it is a 2 MB binary that spawns agents. It is what the pipelines do:
amino's brings up a compose stack (postgres + four services) per run. Two or
three instances idling cost nothing; two or three *running* pipelines is several
full dev stacks at once, and that is what the box has to fit.

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
