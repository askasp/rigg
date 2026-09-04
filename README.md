# rigg

Pipeline runner for AI coding agents, built on [Herdr](https://herdr.dev).

You describe your repo's workflow once in `rigg.toml`; `rigg` drives the agents
through it — prompting them, waiting for each turn to actually finish, running
shell steps in between, and managing stacked branches.

## Why Herdr rather than a shell script

The hard part of this workflow is not ordering commands, it is knowing when an
agent is *done*. Herdr tracks agent lifecycle state through integration hooks,
so `herdr agent prompt <pane> "..." --wait` blocks until the turn genuinely
settles instead of polling terminal output. `rigg` is a thin layer over that.

Because Herdr owns the terminals, this works the same for Claude Code, opencode,
Codex and the rest, and you can still watch and interrupt every step by hand.

For unattended runs there is a second backend that skips Herdr entirely and
drives the agents' own non-interactive modes — see [Backends](#backends).

## Setup

```sh
cargo build --release
herdr integration install claude      # required: enables accurate agent state
herdr plugin link .                   # optional: adds "Rigg: run pipeline"
rigg init                             # writes a starter rigg.toml
rigg doctor                           # checks the whole chain
```

## Use

```sh
rigg run --task "Add rate limiting to the upload endpoint"
rigg run --from self-review           # resume partway through
rigg run --only test --dry-run        # see what would happen, touch nothing
rigg run --headless --task "..."      # no herdr: claude -p / opencode run
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
refuses while a run is still in flight - a headless step owns its conversation
for as long as it runs, and claude will not resume one another process still has
open. Use `rigg logs -f` to watch, `--wait` to open the session the moment the
run finishes, or `--force` to start a second session alongside it. Stacks
with a run in flight are marked `[running]` by `rigg stack list`.
`--new` starts a fresh session instead, `--path` only prints the directory, and
under herdr it focuses the existing workspace rather than starting a second
agent on the same checkout.

A stack name resolves to its tip, and a branch name to itself, so both
`rigg say billing` and `rigg say billing-2` work.

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
| `^X n` | `rigg new ""` with the cursor inside the quotes |
| `^X m` | `rigg say ""`, likewise |
| `^X a` | attach to a stack |
| `^X l` | follow a run's log |
| `^X s` | list stacks, keeping what you were typing |

Aliases: `rn`, `ra`, `rl`, `rsay`, `rs`.

Where a stack name is omitted, `attach`, `logs` and `say` choose one: silently
when there is only one, through fzf when it is installed, otherwise from a
numbered list. `attach` prefers the stack you are standing in.

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

### Queueing the next branch

The next branch is cut from the tip's last commit, so it cannot be created while
a run is still working on that tip. `rigg add` says so, and `--wait` queues:

```sh
rigg add billing "Expose it in the API" --wait
```

It waits for the tip's run to finish - which, with a pipeline that opens the PR,
means the PR is up - then branches from the result and starts the next task
detached.

### Stacking needs commits

Each branch is cut from the previous branch's last commit, so a pipeline that
stacks must commit its work - otherwise the next branch silently starts without
it. `rigg add` refuses when the base branch's checkout is dirty.

Stacked PRs — each branch is a Herdr worktree rooted on the one below it, so
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
rigg stack prune --yes        # close their workspaces and remove the checkouts
```

`stack push` picks its target this way: an explicit `--stack` wins; otherwise it
continues the stack holding the current branch; otherwise it starts a new stack
named after the branch. `--base` always means "root a new stack here". So
running it twice from the trunk gives two independent stacks rather than
accidentally piling the second onto the first.

`stack prune` deletes the merged branches along with their checkouts, since
`git branch -d` refuses anything not actually merged; pass `--keep-branches` to
keep them. It only touches a worktree whose branch is an ancestor of the trunk
(`origin/<trunk>`, else `<trunk>`), whose checkout is clean, and which is either
owned by a herdr workspace it can close first or is not any process's working
directory. The main checkout and the worktree you run it from are never
candidates.

## Config

`.rigg/rigg.toml` in the repo (a plain `rigg.toml` at the root still works, and
`.rigg/` wins if both exist). Keeping it in a directory leaves somewhere for the
prompts to live:

```
.rigg/
  rigg.toml
  reviewer.md
```

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
kind = "claude"        # herdr agent kind
autostart = true       # split a pane and start it if not already running

[[steps]]
id = "implement"
agent = "impl"
prompt = "{{task}}"    # also {{branch}}, {{base}}, {{repo}}

[[steps]]
id = "self-review"
agent = "impl"
clear = true           # /clear first, so review runs on fresh context

[[steps]]
id = "test"
run = "npm test"       # shell step
continue_on_error = true
```

`capture = "name"` on a shell step stores its stdout as `{{name}}`, so a script's
output can be fed straight into a later prompt - that is how PR review comments
reach the agent.

Other step keys: `until`, `timeout_ms`, `when_changed` (globs — used for
frontend-only steps), `confirm`, `description`.

## Backends

```toml
backend = "herdr"      # default; per-role override in [agents.<role>]
```

- **`herdr`** — prompt a long-lived agent in a pane and wait for the turn to
  settle. Interactive: you watch it, and you can interrupt it.
- **`headless`** — run the agent's own non-interactive mode once per step
  (`claude -p "..."`, `opencode run "..."`) in the repo root, streaming its
  output. Process exit *is* the end of the turn, so there is no state to wait
  on and no Herdr, no session and no integration hooks are needed. `rigg run
  --headless` forces it for one run; `rigg doctor --headless` checks it.

**`clear` inverts between the two**, because the sessions are opposite. A Herdr
session carries context forward on its own and `clear = true` wipes it with
`/clear`. A headless invocation starts with nothing, so rigg resumes the
previous one with `--continue`; `clear = true` is what leaves that flag off and
starts a fresh session.

`--continue` resumes the most recent session in the repo directory, so the
first step of a headless run picks up where the last one left off — give it
`clear = true` when a run should start clean.

Unknown agent kinds get `<kind> "<prompt>"` and no resume flag. Give them a
real command line instead:

```toml
[agents.reviewer]
kind = "my-agent"
command = ["my-agent", "--print", "{{prompt}}"]
```

A `command` is used verbatim — rigg adds no continue flag to it, so put one in
the template if the tool has one. `until` and `timeout_ms` are Herdr-only.

## Things worth knowing

These were found the hard way while building this, and are encoded in the
defaults:

- **A headless agent needs a permission mode or it will edit nothing.** Run
  unattended, `claude -p` describes the change it would make and stops. Give the
  role `args = ["--permission-mode", "acceptEdits"]` (or `bypassPermissions` to
  allow commands too). This does not apply in herdr mode, where the live session
  already has its own permission setting.
- **Claude finishes a turn in state `done`, not `idle`.** Waiting on `idle`
  alone hangs forever. `until` is therefore unset by default, which makes Herdr
  match its own set of `idle`, `done` and `blocked`.
- **Slash commands are sent without `--wait`.** `/clear` settles instantly and
  never enters a working state, so `--wait` would fail with
  `agent_prompt_stalled` after 5s.
- **rigg never targets its own pane.** An agent that starts a pipeline would
  otherwise be handed its own prompts.
- **opencode's TUI cannot currently be driven** (as of opencode 1.18.27): Herdr
  delivers the prompt text into the composer but no synthetic key — `enter`,
  `ctrl+m`, `return`, `ctrl+j`, a literal CR — submits it. Until that is fixed,
  a second-model reviewer has to run on another agent kind — or on the headless
  backend, where `opencode run` submits the prompt fine.
- **`herdr pane run` re-parses its command through a shell**, so anything passed
  to it needs quoting rather than argv splitting.
# rigg
