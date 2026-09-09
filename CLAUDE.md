# Working on rigg

rigg drives coding agents through a pipeline of steps. Everything else here —
Slack, cron, secrets, corpora, approvals — is scaffolding for running that
pipeline unattended.

## The rule that decides where things go

> **A repo holds capability. An instance holds identity. A checkout holds
> activity.**

rigg is the only thing that reads all three. Nothing else may reach across.

| plane | where | holds | lifetime |
| --- | --- | --- | --- |
| capability | `<repo>/.rigg/` | pipelines, roles, prompts, mcp surfaces, approvals, schedules | versioned, reviewed in a PR |
| identity | `~/.rigg/instances/<inst>/` | secrets, notes, corpora, outbox, logs, the repos it covers, and the *state* of declared schedules | never committed, one dir per tenant |
| activity | `<git-common-dir>/rigg/` | stacks, run status, logs, thread map | disposable, rebuildable |

Before adding state anywhere, name its plane. If it does not fit one, it is
probably two things.

**Worked examples.** A pipeline that ingests mail → capability, so the repo. The
token it needs → identity, so the instance; the repo names it and never holds
it. Which branch is running → activity. A draft waiting for someone to approve
it → identity, because it outlives the run that made it and belongs to a
tenant, not to a repo.

## Two more rules that follow from it

**Every Slack command is `rigg` plus the same words.** The bridge translates a
message into an argv and renders what comes back. It owns no logic. If the
bridge needs a verb rigg does not have, that is a bug in rigg — add the verb.

**A repo declares its schedules; the instance holds their state.** `[[schedules]]`
in `.rigg/rigg.toml` is read from the repo's **main checkout only** — never a
worktree, for the reason GitHub schedules only from the default branch: rigg cuts
a worktree per branch, and reading all of them would let any branch change when
things run. An instance runs a repo's schedules only once `rigg repo add`
registers it, and a repo belongs to exactly one instance — two would double-fire
with neither able to say why.

`rigg cron add` is unchanged and is not deprecated: typed jobs need no PR and are
the right answer for what you want this week. `rigg cron export` turns one into a
`[[schedules]]` block when it has earned permanence. `rigg cron edit`/`rm` refuse
a declared job and name the file instead.

**Nothing in a repo's config may name an absolute path.** A tool a pipeline
runs is vendored into `<repo>/.rigg/tools/` and named relatively, so a clone
works on somebody else's machine and the checkout is a complete description of
what rigg can do there. `rigg corpus enable` does the copying. The duplication
is deliberate: with one consumer it costs nothing, and a second one is when the
shared interface becomes knowable rather than guessed. There is no tool search
path, no install step and no `$PATH` convention to learn — that was considered
and rejected as a third way to find things.

**Identity is typed and takes effect now; capability is written and reviewed.**
`rigg secret set` and `rigg cron add` mutate instance state immediately.
Anything that changes what an agent *can do* writes a diff into `<repo>/.rigg/`
and leaves it uncommitted. Never silently mutate a repo's capability.

## Where to put a new thing

| you are adding | it goes in | rebuild? |
| --- | --- | --- |
| a pipeline, role, prompt, feature, var | `<repo>/.rigg/rigg.toml` | no |
| a corpus kind | a sidecar directory with a `corpus.toml` | no |
| a tool a pipeline runs | `<repo>/.rigg/tools/` — `rigg corpus enable` copies it there | no |
| a tool surface for an agent | an MCP server + `.rigg/mcp/*.json` | no |
| an approval action | `[approvals.<name>]` in the repo config | no |
| a schedule that must survive a rebuild | `[[schedules]]` in the repo | no |
| an ad-hoc schedule | `rigg cron add` — no PR, fires next tick | no |
| a secret | `rigg secret set` | no |
| an agent kind | `[agents.x] command = [...]` | no |
| **a step key, a subcommand, a flag** | `src/` | **yes** |
| **a manifest field** | `src/corpus.rs` (`deny_unknown_fields`) | **yes** |

The binary changes when the *language* gains a word. Everything you would add
weekly is outside it. If a task seems to need a Rust change, check this table
first — usually it does not.

## No daemon

There is none, and adding one needs a real argument. Cron knocks once a minute
per instance (`cron.sh <inst> tick`); rigg owns the expressions, the job store,
the overlap guard and the dispatch. A background thread inside a long-lived
process is how the scheduler and the corpus syncer ended up inside the Slack
bridge, which made a chat-defined job strictly weaker than a crontab line — it
died with the bridge and could only be tested by waiting.

The honest argument for a daemon is the branch queue: `rigg add` detaches a
`setsid` worker per branch that polls until its base finishes, and none survive
a reboot. That is a supervisor problem. Cron is not.

## Safety rules that are not negotiable

- **An agent needs a permission mode or it edits nothing.** `claude -p` with
  none describes the change, writes nothing and exits 0 — the step reports ok
  and the run dies later at `git push` with no commits.
- **A plain reply is feedback, never a send.** In the approval loop the two
  signals that send are both explicit (`:+1:`, `send: ...`). Getting this the
  wrong way round sends a half-written sentence to a customer.
- **Executing is never an MCP tool.** An approved action runs in the poll,
  after a person has said so, where no prompt can reach it. What it does is
  `[approvals.<name>] run = ...` in the repo — capability, reviewed.
- **An empty value is not a value.** `FRONT_API_TOKEN=` is how a wrapper
  exports one it failed to read; treating it as set is how a run reaches a 401.
- **A job that could do nothing must fail, not pass quietly.** A poll that read
  zero items exits non-zero. Silence is not success.

## Style

Commit subjects are imperative and say what changed for the reader, not what
was edited: "Push what a run leaves behind", not "update main.rs". The body
says what was going wrong and why this is the fix.

Comments explain the failure they prevent, and are rare. Do not narrate what
the code says. Prefer a test that asserts the rule over a comment describing
it.

Errors name the fix: `rigg secret set FRONT_API_TOKEN <value>`, not
`missing_scope`.

## Layout

```
src/            the binary
  main.rs       CLI and every subcommand handler
  config.rs     rigg.toml schema, pipeline resolution, `rigg init` templates
  pipeline.rs   the step driver
  headless.rs   argv for an agent turn, and spawning it
  cron.rs       expressions, the job store, tick
  instance.rs   the identity plane, secrets, migration
  corpus.rs     sidecar manifests, `rigg corpus enable`
  model.rs      which model a role will run, for `rigg plan`
  stack.rs util.rs media.rs

mail/           a corpus sidecar: Front -> SQLite, and an MCP server over it
outbox/         proposals waiting on a person: Slack, the rules, the executors
slack/          the bridge, one process per instance
cron.sh         what a crontab line calls
```

## Testing

Rust: `cargo test`. Python suites are self-contained scripts, one per sidecar:
`./mail/test.sh`, `./outbox/test.sh`, `slack/test_*.py`. They must not touch
the real home directory — set `RIGG_HOME` to a temp dir.

Assert the rule, not the implementation. The checks worth having here are the
ones for failures that are silent: a tool that fails to register looks exactly
like an agent that chose not to call it, so registration is asserted.
