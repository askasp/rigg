# rigg

Pipeline runner for AI coding agents.

You describe your repo's workflow once in `rigg.toml`; `rigg` drives the agents
through it — prompting them, waiting for each turn to actually finish, running
shell steps in between, and managing stacked branches.

## How a step runs

Each agent step is one non-interactive turn — `claude -p "..."`,
`opencode run "..."` — spawned in the branch's checkout with its output
streamed to the run log. Process exit *is* the end of the turn, so there is no
agent state to watch and nothing to install: the same pipeline works for Claude
Code, opencode, Codex and anything else with a print mode.

## Setup

```sh
cargo build --release
rigg init                             # writes a starter rigg.toml
rigg doctor                           # checks the whole chain
```

## Use

```sh
rigg run --task "Add rate limiting to the upload endpoint"
rigg run --from self-review           # resume partway through
rigg run --only test --dry-run        # see what would happen, touch nothing
rigg status
```

The short form, which is how you will normally start work. Everything is
addressed **by stack name**, so it works from any directory:

```sh
rigg new billing "Add proration to subscription changes"
rigg new "Add proration to subscription changes"   # name generated for you
rigg quick new billing "..."      # use the pipeline named `quick`
rigg add billing "Expose it in the API"   # next branch, auto-named billing-2
rigg say billing "also handle refunds"    # another turn in that stack's session
rigg attach billing               # open claude/opencode on that stack's session
rigg attach --path billing        # just the path: cd "$(rigg attach --path billing)"
rigg logs billing -f              # follow a run
rigg stack names                  # for shell completion
```

A step marked `confirm` cannot run detached - there is no terminal to answer it -
so `new` and `add` say so up front rather than skipping the step silently.

`new` and `add` return the terminal immediately and run the pipeline detached,
logging to `.git/rigg/logs/<branch>.log`; pass `--fg` to run in the terminal
instead. Given a single argument that reads like a sentence, `new` treats it as
the task and generates a stack name.

`new` starts a stack; `add` appends the next branch to one and runs the pipeline
there; `say` continues the agent session in that stack's tip rather than running
a pipeline. `attach` opens an interactive agent on the stack's checkout, resuming the
session the pipeline was using (`--continue`), so you can take over by hand. It
refuses while a run is still in flight - a step owns its conversation for as
long as it runs, and claude will not resume one another process still has
open. Use `rigg logs -f` to watch, `--wait` to open the session the moment the
run finishes, or `--force` to start a second session alongside it. Stacks
with a run in flight are marked `[running]` by `rigg stack list`.
It resumes by session id and prints which one, rather than relying on
`--continue`, and says plainly when a checkout has no previous session instead
of leaving claude to report that it found nothing.

`--new` starts a fresh session instead, and `--path` only prints the directory.

A branch name resolves to itself. A stack name resolves to the branch you most
likely mean - the one being worked on now, else the newest that exists on disk -
because the tip of a queued stack is usually still waiting and has no checkout
yet. So `rigg attach billing` reaches whichever branch of that stack is running.

### Shell integration

`completions/_rigg` completes stack names (zsh). Keybindings and aliases come
from the binary, so add to `~/.zshrc`:

```sh
source <(rigg keys --shell zsh)
```

`rigg keys` prints them at any time, and says so if the integration is not
loaded in the current shell.

| key | does |
| --- | --- |
| `^X n` | start a new stack |
| `^X d` | add to a stack |
| `^X m` | `rigg say ""`, likewise |
| `^X a` | attach to a stack |
| `^X l` | follow a run's log |
| `^X s` | list stacks, keeping what you were typing |
| `^X r` | remove a stack |

Aliases: `rn`, `rad`, `ra`, `rl`, `rsay`, `rs`, `rrm`.

`say --push` commits and pushes whatever the turn changed, using the request as
the commit subject, so a follow-up reaches the PR without a second command.

### Images

A terminal cannot receive a pasted image, so rigg reads the system clipboard
itself. Copy a screenshot, then write the task as usual — `new`, `add` and
`say` pick the image up and tell you they did:

```
$ rigg new billing
task> the spacing in this dialog is wrong
  attached clipboard image (48 KB)
  copy another image and press enter, or just press enter to start>
```

The clipboard holds one image at a time, so after taking one it offers to take
another: copy the next screenshot and press enter. Enter on an unchanged
clipboard starts the run. `--image <path>` attaches a file instead and can be
repeated; `--no-paste` leaves the clipboard alone.

Images land in `.rigg/media/` inside the branch's checkout and are named at the
end of the prompt, so the agent reads them as files. That directory is added to
`.git/info/exclude`, so it never shows up in `git status` or reaches a commit.

Needs `wl-paste` (Wayland) or `xclip` (X11) for the clipboard; `--image` works
without either.

`rigg stop <stack>` cancels a run in flight. The run is its own process group,
so the agent and any shell step under it go down with it rather than being
orphaned; the branch is then marked `stopped` rather than failed.

`say` with no arguments picks a stack and then asks what to send, the same shape
as `attach` and `logs`. `stack rm` removes a whole stack, refusing any branch
that is still running, has uncommitted work, or holds commits that are not on
the trunk, unless `--force`; like `prune` it is a dry run until `--yes`.

Where a stack name is omitted, `add`, `attach`, `logs` and `say` choose one: silently
when there is only one, through fzf when it is installed, otherwise from a
numbered list. `attach` prefers the stack you are standing in.

### Reading a log

A run's log opens with the branch, the pipeline and the whole task, then rules
off each step with a timestamp and reports how long it took:

```
rigg  billing  (base main, pipeline full, 9 steps)
task  Add proration to subscription changes so a mid-cycle upgrade
      bills the difference rather than the full period

---- [1/9] implement ------------------------------------------- 20:34:44
  -> [impl] claude -p --continue ...
  · Edit backend/src/billing/proration.ts
  ok (2m14s)
```

A multi-line `run` step shows its first line and a line count rather than its
whole body.

### Watching a run

`claude -p` prints nothing until its turn ends, which makes a long step look
hung. rigg runs it with `--output-format stream-json` instead and prints a line
per event, so `rigg logs <stack> -f` shows the text and each tool call as they
happen:

```
I'll read the file first.
  · Read calc.py
  · Edit calc.py
```

### Queueing a stack

`rigg add` returns immediately and queues, so a stack can be filled in one go
and left to run - each branch its own small PR:

```sh
rigg new billing   "Add proration to subscription changes"
rigg add billing   "Expose it in the API"
rigg add billing   "Show it in the invoice view"
```

```
billing
  1. billing   <- main      [running 3/9 apply-review]
  2. billing-2 <- billing   [queued behind billing]
  3. billing-3 <- billing-2 [queued behind billing-2]
```

There is no daemon. Each `add` reserves its entry in `stack.json`, writes a pid
file, and detaches a worker under `setsid` that polls until the run on its base
finishes; then it creates the worktree from that base's final commit and runs
the pipeline. So the queue is a chain of waiters, each watching the one below.

A worker refuses to branch when its base's run did not succeed, or left work
uncommitted - either way the new branch would not contain what it should. Runs
record their outcome in `.git/rigg/run/<branch>.status`.

Everything lives in `.git/rigg/`: `stack.json`, `logs/<branch>.log`,
`run/<branch>.pid` and `run/<branch>.status`. Queued workers do not survive a
reboot; re-run `rigg add` for anything that was still waiting.

### Stacking needs commits

Each branch is cut from the previous branch's last commit, so a pipeline that
stacks must commit its work - otherwise the next branch silently starts without
it. `rigg add` refuses when the base branch's checkout is dirty.

Stacked PRs — each branch is a git worktree rooted on the one below it, so
every PR is reviewable on its own. Stacks are named, and several can sit on the
trunk at once:

```sh
rigg stack push billing-1     # not in a stack: starts one, rooted on trunk
rigg stack push billing-2     # run from the billing-1 worktree: continues it
rigg stack push docs-1        # run from the trunk: starts a separate stack
rigg stack push x --stack billing-1   # target a stack explicitly
rigg stack push y --base main         # force a new stack rooted here
rigg stack list               # every stack, current branch marked
rigg stack pr                 # PR against its own base, labelled if frontend
rigg stack prune              # dry run: worktrees whose branch already landed
rigg stack prune --yes        # remove the checkouts
```

`stack push` picks its target this way: an explicit `--stack` wins; otherwise it
continues the stack holding the current branch; otherwise it starts a new stack
named after the branch. `--base` always means "root a new stack here". So
running it twice from the trunk gives two independent stacks rather than
accidentally piling the second onto the first.

`stack pr` adds the preview label through the REST API rather than
`gh pr edit --add-label`, which still queries Projects (classic) and therefore
fails outright on GitHub today.

### Tearing a branch down

A pipeline that starts something — a dev stack, a tunnel — leaves it running
after the run ends. `teardown` is what stops it, and runs in the checkout just
before its worktree is removed by either `stack rm` or `prune`:

```toml
[stack]
teardown = "./dev.sh stop"
```

It supports `{{branch}}`, `{{base}}` and `{{repo}}` (the checkout). A dry run
prints what it would run without running it. A failure is reported but does not
stop the removal you asked for — though it is worth reading, since a leaked
docker stack is invisible until the disk fills.

`stack prune` deletes the merged branches along with their checkouts, since
`git branch -d` refuses anything not actually merged; pass `--keep-branches` to
keep them. It only touches a worktree whose branch is an ancestor of the trunk
(`origin/<trunk>`, else `<trunk>`), whose checkout is clean, and which is not
any process's working directory. The main checkout and the worktree you run it
from are never candidates.

## Config

`.rigg/rigg.toml` in the repo (a plain `rigg.toml` at the root still works, and
`.rigg/` wins if both exist). Keeping it in a directory leaves somewhere for the
prompts to live:

```
.rigg/
  rigg.toml
  reviewer.md
```

### Chaining pipelines

A pipeline can start from another's steps rather than repeating them. So one
variant can exist with and without an extra stage:

```toml
[[pipelines.full.steps]]
id = "implement"
# ... commit, push-and-pr, wait-copilot

# Everything `full` does, plus a preview brought up as soon as the PR exists.
[pipelines.full-preview]
extends = "full"
insert_after = "push-and-pr"     # default is to append at the end

[[pipelines.full-preview.steps]]
id = "preview-up"
run = "./dev.sh quick -d"
```

`rigg full-preview new billing "..."` then runs six steps, with the two new ones
sitting between `push-and-pr` and `wait-copilot` — placement matters when the
step it would otherwise follow waits 20 minutes for a review.

`extends` chains, so a pipeline may extend one that itself extends another. A
cycle, an `insert_after` that names no inherited step, and two steps ending up
with the same id are all config errors caught by `rigg doctor` rather than
surprises on the first run.

### Choosing a pipeline

The pipeline that runs when none is named is the top-level `[[steps]]`. To make
a named one the default instead:

```toml
default_pipeline = "quick"

[[pipelines.quick.steps]]
id = "implement"
```

`--pipeline <name>` and `rigg <name> new ...` still override it. Note that
setting `default_pipeline` leaves the top-level `[[steps]]` with no way to be
selected, so put every pipeline under `[pipelines]` if you use it.

A step takes either an inline `prompt` or a `prompt_file` relative to the config
directory - both get the same `{{task}}` / `{{branch}}` / `{{base}}` / `{{repo}}`
substitution:

```toml
[[steps]]
id = "review"
agent = "impl"
clear = true
prompt_file = "reviewer.md"
```

```toml
[agents.impl]
kind = "claude"        # agent kind: claude, opencode, codex, ...
args = ["--permission-mode", "acceptEdits"]   # added to every turn

[[steps]]
id = "implement"
agent = "impl"
prompt = "{{task}}"    # also {{branch}}, {{base}}, {{repo}}

[[steps]]
id = "self-review"
agent = "impl"
clear = true           # a new session, so review runs on fresh context

[[steps]]
id = "test"
run = "npm test"       # shell step
continue_on_error = true
```

`capture = "name"` on a shell step stores its stdout as `{{name}}`, so a script's
output can be fed straight into a later prompt - that is how PR review comments
reach the agent.

Other step keys: `when_changed` (globs — used for frontend-only steps),
`confirm`, `description`.

## Sessions

Each step is a separate process with no memory of the last one, so continuity
comes from the agent's own resume flag: rigg passes `--continue` to every step
that did not ask for a fresh context, and `clear = true` is what leaves it off.

`--continue` resumes the most recent session in the checkout, so the first step
of a run picks up where the last one left off — give it `clear = true` when a
run should start clean.

Unknown agent kinds get `<kind> "<prompt>"` and no resume flag. Give them a
real command line instead:

```toml
[agents.reviewer]
kind = "my-agent"
command = ["my-agent", "--print", "{{prompt}}"]
```

A `command` is used verbatim — rigg adds no continue flag to it, so put one in
the template if the tool has one.

## Running on another agent

A role carries a `kind` and the `args` for it as a matched set, so `--agent`
swaps both:

```sh
rigg run --agent opencode                # every role
rigg run --agent reviewer=opencode       # just that one - a second opinion
rigg new billing "..." --agent opencode
```

`args` and `command` are dropped when the kind changes, and it says so. They
have to be: claude takes `--permission-mode`, opencode has no such flag and
takes `-m/--model` instead, so carrying them across would produce an agent that
cannot start. A role you switch often is better written out twice in the config,
with the right flags on each.

## Reporting progress

```toml
[notify]
command = "./slack/notify.sh"
```

Runs at the start of a run, after every step, and when the run ends. Details
arrive as environment variables rather than arguments, so nothing needs
quoting: `RIGG_EVENT` (start, step, done, failed), `RIGG_MESSAGE` (a ready-made
one-line summary), plus `RIGG_BRANCH`, `RIGG_STEP`, `RIGG_STATUS`,
`RIGG_INDEX`, `RIGG_TOTAL`, `RIGG_ELAPSED`, `RIGG_REPO` and `RIGG_TASK`.

A failing hook never fails a run — it prints a line and the run carries on.
`--dry-run` does not fire it.

`slack/` uses this to drive [rigg from a Slack channel](slack/README.md), one
repo per channel.

## Things worth knowing

**An agent needs a permission mode or it will edit nothing.** Run unattended,
`claude -p` describes the change it would make and stops rather than doing it.
Give the role `args = ["--permission-mode", "acceptEdits"]`, or
`bypassPermissions` to allow commands too. This is the one thing that will
silently produce a run that looks like it worked and changed nothing, which is
why `rigg init` puts it in the starter config.
