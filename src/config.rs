use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub agents: BTreeMap<String, AgentCfg>,
    /// The default pipeline.
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Additional named pipelines, selected with `rigg <name> new ...`.
    #[serde(default)]
    pub pipelines: BTreeMap<String, Pipeline>,
    /// Which pipeline runs when none is named. Defaults to the top-level
    /// `[[steps]]`; set it to run one of `[pipelines]` instead.
    #[serde(default)]
    pub default_pipeline: Option<String>,
    #[serde(default)]
    pub stack: StackCfg,
    #[serde(default)]
    pub notify: NotifyCfg,
    /// Defaults for placeholders a prompt uses beyond the built-in {{task}},
    /// {{branch}}, {{base}} and {{repo}} - `--var name=value` overrides one
    /// for a single run. Having the default here is what lets a prompt written
    /// against {{effort}} still work when nobody passes one.
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    /// Optional parts of the pipeline, and whether each is on by default.
    /// A step tagged `feature = "preview"` runs only when `preview` resolves
    /// true. Listing them here in one table is what makes the defaults
    /// something you can read rather than infer.
    #[serde(default)]
    pub features: BTreeMap<String, bool>,
    /// What a person may approve in Slack, and what runs when they do. rigg
    /// never executes these - the outbox does - but they are repo config, so
    /// they are typed here and `rigg doctor` can say which are ready.
    #[serde(default)]
    pub approvals: BTreeMap<String, Approval>,
    /// Directory the config was loaded from; `prompt_file` paths resolve
    /// against it.
    #[serde(skip)]
    pub dir: PathBuf,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    #[serde(default)]
    pub description: Option<String>,
    /// The command. The draft arrives on its stdin, the proposal's fields as
    /// RIGG_APPROVAL_*.
    #[serde(default)]
    pub run: Option<String>,
    /// Secrets it cannot run without. Missing ones make it a dry run rather
    /// than a failure after somebody has clicked.
    #[serde(default)]
    pub needs: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Pipeline {
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Start from another pipeline's steps, then add this one's. Chains, so a
    /// pipeline may extend one that itself extends another.
    #[serde(default)]
    pub extends: Option<String>,
    /// Put this pipeline's own steps directly after the inherited step with
    /// this id, rather than at the end. Only meaningful with `extends`.
    #[serde(default)]
    pub insert_after: Option<String>,
    /// Inherited steps to leave out, by id. The counterpart to `extends`: it
    /// lets the fuller pipeline be the one written down, and the shorter one
    /// say what it drops, rather than either repeating the other.
    #[serde(default)]
    pub without: Vec<String>,
    /// Which features this pipeline turns on or off, over the `[features]`
    /// defaults. This is how a named pipeline becomes an alias for a
    /// combination rather than a copy of one.
    #[serde(default)]
    pub features: BTreeMap<String, bool>,
    /// Placeholder values this pipeline sets, over the top-level `[vars]`.
    /// So a `thorough` alias can mean a heavier review as well as more steps.
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AgentCfg {
    /// Agent kind: claude, opencode, codex, ...
    pub kind: String,
    /// Extra CLI args, added to every turn.
    #[serde(default)]
    pub args: Vec<String>,
    /// Argv for an agent rigg has no built-in command for.
    /// `{{prompt}}` is replaced with the prompt; without it the prompt is
    /// appended as the last argument.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// Secrets this role cannot work without, by name. The value lives in the
    /// instance; naming it here lets `rigg doctor` answer before a run.
    #[serde(default)]
    pub needs: Vec<String>,
}

impl Step {
    /// The step's prompt, read from `prompt_file` when that is what it uses.
    pub fn prompt_text(&self, dir: &Path) -> Result<Option<String>> {
        if let Some(p) = &self.prompt {
            return Ok(Some(p.clone()));
        }
        if let Some(f) = &self.prompt_file {
            let path = dir.join(f);
            let text = std::fs::read_to_string(&path).with_context(|| {
                format!("step `{}`: reading prompt file {}", self.id, path.display())
            })?;
            return Ok(Some(text));
        }
        Ok(None)
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Agent role key from [agents]. Required unless `run` is set.
    #[serde(default)]
    pub agent: Option<String>,
    /// Prompt text. Supports {{task}}, {{branch}}, {{base}}, {{repo}}.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Markdown file holding the prompt, relative to the config directory.
    /// The same placeholders apply.
    #[serde(default)]
    pub prompt_file: Option<String>,
    /// Shell command instead of an agent prompt.
    #[serde(default)]
    pub run: Option<String>,
    /// Store this step's stdout as a variable later prompts can interpolate,
    /// e.g. `capture = "review"` makes it available as `{{review}}`.
    #[serde(default)]
    pub capture: Option<String>,
    /// Start a fresh session instead of resuming the previous step's.
    #[serde(default)]
    pub clear: bool,
    /// This step belongs to an optional feature and runs only when it is on.
    /// Unset means the step always runs.
    #[serde(default)]
    pub feature: Option<String>,
    /// Only run when changed files match one of these globs.
    #[serde(default)]
    pub when_changed: Option<Vec<String>>,
    /// Run only when this renders to something non-empty, so a step can be
    /// skipped by an earlier step's `capture` finding nothing to do.
    #[serde(default)]
    pub when: Option<String>,
    /// Keep going when this step fails.
    #[serde(default)]
    pub continue_on_error: bool,
    /// Pause for confirmation before running.
    #[serde(default)]
    pub confirm: bool,
}

/// A command run as a run moves through its steps, so something outside rigg -
/// a chat bot, a desktop notifier - can follow along without tailing the log.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct NotifyCfg {
    /// Shell command run at the start of a run, after every step, and when the
    /// run ends. Details arrive as `RIGG_*` environment variables rather than
    /// as arguments, so nothing has to be quoted: RIGG_EVENT (start, step,
    /// done, failed), RIGG_MESSAGE (a ready-made one-line summary),
    /// RIGG_BRANCH, RIGG_STEP, RIGG_STATUS, RIGG_INDEX, RIGG_TOTAL,
    /// RIGG_ELAPSED, RIGG_REPO, RIGG_TASK.
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct StackCfg {
    /// Trunk branch new stacks are rooted on.
    #[serde(default = "default_trunk")]
    pub trunk: String,
    /// Label applied to PRs that touch frontend paths.
    #[serde(default)]
    pub preview_label: Option<String>,
    /// Globs that mark a change as frontend.
    #[serde(default)]
    pub frontend_paths: Vec<String>,
    /// Where to put worktrees. Defaults to ~/.rigg/worktrees/<repo>.
    #[serde(default)]
    pub worktree_dir: Option<String>,
    /// Command run in a checkout just before its worktree is removed, so a
    /// branch that started services can stop them again. Supports {{branch}},
    /// {{base}} and {{repo}} (the checkout). A failure is reported but does
    /// not stop the removal that was asked for.
    #[serde(default)]
    pub teardown: Option<String>,
    /// Commit and push whatever a run leaves in the checkout when it finishes.
    ///
    /// On, because a branch is worked on somewhere else - a preview mounts the
    /// checkout, a PR is what gets reviewed - and a change that was not pushed
    /// is in neither. A pipeline that applies a review after it has pushed
    /// leaves the fixes behind otherwise, and nothing says so.
    #[serde(default = "yes")]
    pub push_when_done: bool,
}

fn yes() -> bool {
    true
}

impl Default for StackCfg {
    fn default() -> Self {
        Self {
            trunk: default_trunk(),
            preview_label: None,
            frontend_paths: Vec::new(),
            worktree_dir: None,
            teardown: None,
            push_when_done: true,
        }
    }
}

fn default_trunk() -> String {
    "main".into()
}

impl Config {
    /// Preferred config location, and the legacy one beside it.
    pub fn candidates(root: &Path) -> [PathBuf; 2] {
        [root.join(".rigg").join("rigg.toml"), root.join("rigg.toml")]
    }

    pub fn path_for(root: &Path) -> PathBuf {
        let [preferred, legacy] = Self::candidates(root);
        if !preferred.exists() && legacy.exists() {
            legacy
        } else {
            preferred
        }
    }

    pub fn load(root: &Path, explicit: Option<&str>) -> Result<Self> {
        let mut path = match explicit {
            Some(p) => PathBuf::from(p),
            None => Self::path_for(root),
        };
        // A worktree checked out before the config was committed will not have
        // one, so fall back to the primary checkout's.
        if !path.exists() && explicit.is_none() {
            if let Ok(main) = crate::util::main_checkout(root) {
                if let Some(alt) = Self::candidates(&main).into_iter().find(|p| p.exists()) {
                    path = alt;
                }
            }
        }
        if !path.exists() {
            bail!(
                "no config at {}. Run `rigg init` to create one.",
                path.display()
            );
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: Config =
            toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        cfg.dir = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| root.to_path_buf());
        cfg.validate()?;
        Ok(cfg)
    }

    /// Steps for a pipeline by name, or the default pipeline.
    pub fn steps_for(&self, name: Option<&str>) -> Result<Vec<Step>> {
        // An explicit choice wins, then `default_pipeline`, then the top-level
        // steps.
        let name = name.or(self.default_pipeline.as_deref());
        match name {
            None => Ok(self.steps.clone()),
            Some(n) => self.resolve_pipeline(n, &mut Vec::new()),
        }
    }

    /// Flatten a pipeline and everything it extends into one list of steps.
    fn resolve_pipeline(&self, name: &str, seen: &mut Vec<String>) -> Result<Vec<Step>> {
        if seen.iter().any(|s| s == name) {
            seen.push(name.to_string());
            bail!("pipeline `{}` extends itself: {}", seen[0], seen.join(" -> "));
        }
        seen.push(name.to_string());

        let p = self.pipelines.get(name).ok_or_else(|| {
            let known: Vec<&str> = self.pipelines.keys().map(|s| s.as_str()).collect();
            anyhow::anyhow!(
                "no pipeline `{name}`; known pipelines: {}",
                if known.is_empty() { "(none)".into() } else { known.join(", ") }
            )
        })?;

        let mut steps = match &p.extends {
            Some(parent) => self.resolve_pipeline(parent, seen)?,
            // A pipeline that adds no steps of its own and extends nothing is
            // a name for a set of features, so it starts from the top-level
            // steps. That is what lets an alias be two lines.
            None if p.steps.is_empty() => self.steps.clone(),
            None => Vec::new(),
        };

        for id in &p.without {
            let before = steps.len();
            steps.retain(|s| &s.id != id);
            if steps.len() == before {
                let ids: Vec<&str> = steps.iter().map(|s| s.id.as_str()).collect();
                bail!(
                    "pipeline `{name}`: without = \"{id}\" names no inherited step; \
                     it has {}",
                    if ids.is_empty() { "none".into() } else { ids.join(", ") }
                );
            }
        }

        match &p.insert_after {
            Some(id) => {
                let at = steps
                    .iter()
                    .position(|s| &s.id == id)
                    .with_context(|| {
                        let ids: Vec<&str> = steps.iter().map(|s| s.id.as_str()).collect();
                        format!(
                            "pipeline `{name}`: insert_after = \"{id}\" names no inherited \
                             step; it has {}",
                            if ids.is_empty() { "none".into() } else { ids.join(", ") }
                        )
                    })?;
                for (i, step) in p.steps.iter().enumerate() {
                    steps.insert(at + 1 + i, step.clone());
                }
            }
            None => steps.extend(p.steps.iter().cloned()),
        }

        // `--from` and `--only` address steps by id, so a duplicate would make
        // them ambiguous rather than merely untidy.
        for (i, step) in steps.iter().enumerate() {
            if steps[..i].iter().any(|e| e.id == step.id) {
                bail!("pipeline `{name}` ends up with two steps called `{}`", step.id);
            }
        }
        Ok(steps)
    }


    /// Which features are on: the defaults, then the pipeline's own choices,
    /// then whatever this run asked for.
    pub fn features_for(
        &self,
        pipeline: Option<&str>,
        with: &[String],
        without: &[String],
    ) -> Result<BTreeMap<String, bool>> {
        let mut on = self.features.clone();
        let name = pipeline.or(self.default_pipeline.as_deref());
        if let Some(p) = name.and_then(|n| self.pipelines.get(n)) {
            for (k, v) in &p.features {
                on.insert(k.clone(), *v);
            }
        }
        for (list, value) in [(with, true), (without, false)] {
            for f in list {
                if !on.contains_key(f) {
                    let known: Vec<&str> = on.keys().map(|s| s.as_str()).collect();
                    bail!(
                        "no feature `{f}`; this config has {}",
                        if known.is_empty() { "none".into() } else { known.join(", ") }
                    );
                }
                on.insert(f.clone(), value);
            }
        }
        Ok(on)
    }

    /// Placeholder values: the defaults, then the pipeline's own.
    /// `--var` is applied by the caller on top of these.
    pub fn vars_for(&self, pipeline: Option<&str>) -> BTreeMap<String, String> {
        let mut vars = self.vars.clone();
        let name = pipeline.or(self.default_pipeline.as_deref());
        if let Some(p) = name.and_then(|n| self.pipelines.get(n)) {
            for (k, v) in &p.vars {
                vars.insert(k.clone(), v.clone());
            }
        }
        vars
    }

    fn validate(&self) -> Result<()> {
        // Resolve every pipeline now, so a broken `extends` is a config error
        // rather than a surprise the first time that pipeline is chosen.
        for name in self.pipelines.keys() {
            self.resolve_pipeline(name, &mut Vec::new())?;
        }
        if let Some(d) = &self.default_pipeline {
            if !self.pipelines.contains_key(d) {
                let known: Vec<&str> = self.pipelines.keys().map(|s| s.as_str()).collect();
                bail!(
                    "default_pipeline = \"{d}\" names no pipeline; known: {}",
                    if known.is_empty() { "(none)".into() } else { known.join(", ") }
                );
            }
        }
        // A feature named nowhere in [features] is a typo that would silently
        // drop a step, so it is worth refusing up front.
        let all_features = self
            .steps
            .iter()
            .chain(self.pipelines.values().flat_map(|p| p.steps.iter()))
            .filter_map(|s| s.feature.clone())
            .chain(self.pipelines.values().flat_map(|p| p.features.keys().cloned()));
        for f in all_features {
            if !self.features.contains_key(&f) {
                let known: Vec<&str> = self.features.keys().map(|s| s.as_str()).collect();
                bail!(
                    "feature `{f}` is used but not declared in [features]; declared: {}",
                    if known.is_empty() { "none".into() } else { known.join(", ") }
                );
            }
        }

        let all = self
            .steps
            .iter()
            .chain(self.pipelines.values().flat_map(|p| p.steps.iter()));
        for step in all {
            let prompts = step.prompt.is_some() as u8 + step.prompt_file.is_some() as u8;
            if step.run.is_none() && prompts == 0 {
                bail!(
                    "step `{}` has none of `prompt`, `prompt_file` or `run`",
                    step.id
                );
            }
            if prompts == 2 {
                bail!(
                    "step `{}` sets both `prompt` and `prompt_file`",
                    step.id
                );
            }
            if step.run.is_some() && prompts > 0 {
                bail!("step `{}` sets both a prompt and `run`", step.id);
            }
            if let Some(role) = &step.agent {
                if !self.agents.contains_key(role) {
                    bail!(
                        "step `{}` refers to unknown agent role `{}`",
                        step.id,
                        role
                    );
                }
            } else if prompts > 0 {
                bail!("step `{}` has a prompt but no `agent`", step.id);
            }
        }
        Ok(())
    }
}

pub const TEMPLATE: &str = r#"# rigg pipeline
# Steps run top to bottom. Start them with: rigg run --task "..."

[agents.impl]
kind = "claude"
# An agent running unattended needs a permission mode or it will describe
# changes instead of making them. `acceptEdits` allows file edits;
# `bypassPermissions` also allows commands.
args = ["--permission-mode", "acceptEdits"]
# command = ["my-agent", "--print", "{{prompt}}"]   # argv for an agent rigg
                                 # has no built-in command for

# A second model is the point of a Copilot-style review, without the round trip.
# [agents.reviewer]
# kind = "opencode"

[stack]
trunk = "main"
preview_label = "preview"
frontend_paths = ["src/**/*.tsx", "src/**/*.css", "app/**"]

[[steps]]
id = "implement"
agent = "impl"
prompt = "{{task}}"

[[steps]]
id = "self-review"
agent = "impl"
clear = true                     # fresh context, as you do by hand
prompt = "/code-review"
# Each step is one non-interactive turn, so rigg resumes the previous step's
# session (--continue) to carry context forward. `clear` leaves that flag off
# and starts a new session instead.

[[steps]]
id = "test"
run = "npm test"
continue_on_error = true

[[steps]]
id = "apply-review"
agent = "impl"
prompt = "Address every finding from the review above. If a finding is wrong, say why instead."

[[steps]]
id = "push"
run = "git push -u origin {{branch}}"
confirm = true

# --- things you will want next, and where they go --------------------------
# `rigg corpus enable <name>` writes the first two for you.

# A role holding an API surface. The repo says which tools; the instance holds
# the token, and `needs` is what lets `rigg doctor` answer before a run does.
# [agents.mailer]
# kind = "claude"
# needs = ["FRONT_API_TOKEN"]
# args = ["--permission-mode", "auto",
#         "--mcp-config", ".rigg/mcp/mail.json", "--strict-mcp-config",
#         "--allowedTools", "mcp__mail"]

# A shell step's stdout becomes a placeholder, and `when` skips the step after
# it when there is nothing there - so a quiet poll costs no agent turn.
# [[pipelines.check.steps]]
# id = "look"
# run = "./scripts/whats-pending.sh"
# capture = "pending"
#
# [[pipelines.check.steps]]
# id = "act"
# agent = "impl"
# when = "{{pending}}"
# prompt = "Deal with these:\n{{pending}}"

# An action a person approves in Slack before it happens. The draft arrives on
# stdin; the item's fields arrive as RIGG_APPROVAL_*.
# [approvals.reply]
# description = "Reply to a customer"
# run = "./scripts/send-reply.sh"
# needs = ["API_TOKEN"]

# Progress into the Slack thread that asked for the branch.
# [notify]
# command = "/path/to/rigg/slack/notify.sh"
"#;

pub const README: &str = r#"# .rigg

What this repo lets an agent do. Three places hold everything, and rigg is the
only thing that reads all three:

| plane | where | holds |
| --- | --- | --- |
| capability | `.rigg/` (here) | pipelines, roles, prompts, mcp surfaces, approvals |
| identity | `~/.rigg/instances/<inst>/` | secrets, schedules, notes, corpora |
| activity | `.git/rigg/` | stacks, run status, logs |

The planes never reach into each other. **This directory names a secret; it
never contains one.** An instance holds a schedule; it never holds a pipeline.

## What goes here

```
.rigg/
  rigg.toml     pipelines, roles, features, vars
  prompts/      anything a step names with prompt_file
  mcp/          an API surface a role may hold - `rigg corpus enable` writes these
```

Changing anything here changes what an agent *can do*, so it goes through a
diff and a review. Changing a token or a schedule does not — those are typed:

```sh
rigg secret set API_TOKEN <value>     # identity, takes effect now
rigg cron add "0 7 * * *" --pipeline morning
```

## The commands you will actually use

```sh
rigg doctor                  is the chain intact - binaries, config, secrets
rigg plan                    the steps a run would take, and on which model
rigg corpus                  bodies of past work a role could be given
rigg cron                    what runs unattended, and when it next does
rigg run --task "..."        run the pipeline here
rigg new <stack> "..."       cut a branch and run it there
```

## Adding to this file

- a new step: add to `[[steps]]`, or a named `[pipelines.x]`
- a role that needs a token: give it `needs = ["NAME"]` so `rigg doctor` can
  answer the day you write it, instead of the run finding out at 07:04
- a corpus: `rigg corpus enable <name>` — it writes the mcp json and the
  pipelines and leaves them uncommitted for you to read
- something on a schedule: `rigg cron add`, not a line in here

An agent running unattended needs a permission mode or it will describe
changes instead of making them, report the step ok, and fail three steps later
at `git push` with a branch that has no commits.
"#;

pub const REVIEWER_PROMPT: &str = r#"Review the diff on this branch against {{base}}.

Look for defects that would reach a user: wrong behaviour, an unhandled case, a
silent failure. Then look for the ones that are silent by nature — something
that reports success having done nothing, an error that names no fix, state
written to the wrong place.

Skip style. Skip anything the tests already assert.

For each finding: the file and line, what breaks, and the smallest change that
fixes it. If you find nothing worth changing, say so in one line rather than
inventing something.
"#;
