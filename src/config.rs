use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How agent steps are driven.
#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// Prompt a live agent through herdr and wait for its turn to settle.
    #[default]
    Herdr,
    /// Spawn the agent's own non-interactive mode once per step.
    Headless,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Herdr => "herdr",
            Backend::Headless => "headless",
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Default backend for every agent step.
    #[serde(default)]
    pub backend: Backend,
    #[serde(default)]
    pub agents: BTreeMap<String, AgentCfg>,
    /// The default pipeline.
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Additional named pipelines, selected with `rigg <name> new ...`.
    #[serde(default)]
    pub pipelines: BTreeMap<String, Pipeline>,
    #[serde(default)]
    pub stack: StackCfg,
    /// Directory the config was loaded from; `prompt_file` paths resolve
    /// against it.
    #[serde(skip)]
    pub dir: PathBuf,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Pipeline {
    #[serde(default)]
    pub steps: Vec<Step>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AgentCfg {
    /// Herdr agent kind: claude, opencode, codex, ...
    pub kind: String,
    /// Explicit herdr agent name to target, if you name your panes.
    #[serde(default)]
    pub name: Option<String>,
    /// Split a pane and start this agent when it is not already running.
    #[serde(default)]
    pub autostart: bool,
    /// Extra CLI args for this agent: passed on autostart in herdr mode, and
    /// added to every turn in headless mode.
    #[serde(default)]
    pub args: Vec<String>,
    /// Backend for this role, overriding the global `backend`.
    #[serde(default)]
    pub backend: Option<Backend>,
    /// Headless argv for an agent rigg has no built-in command for.
    /// `{{prompt}}` is replaced with the prompt; without it the prompt is
    /// appended as the last argument.
    #[serde(default)]
    pub command: Option<Vec<String>>,
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

/// Accepts either `until = "done"` or `until = ["done", "blocked"]`.
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    pub fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s],
            OneOrMany::Many(v) => v,
        }
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
    /// Clear the agent's context before prompting.
    #[serde(default)]
    pub clear: bool,
    /// State(s) to wait for. Defaults to herdr's own set: idle, done, blocked.
    /// Claude ends a turn in `done`, not `idle`, so pinning a single state is
    /// usually wrong.
    #[serde(default)]
    pub until: Option<OneOrMany>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Only run when changed files match one of these globs.
    #[serde(default)]
    pub when_changed: Option<Vec<String>>,
    /// Keep going when this step fails.
    #[serde(default)]
    pub continue_on_error: bool,
    /// Pause for confirmation before running.
    #[serde(default)]
    pub confirm: bool,
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
    /// Where to put worktrees when not going through herdr. Defaults to
    /// ~/.rigg/worktrees/<repo>.
    #[serde(default)]
    pub worktree_dir: Option<String>,
}

impl Default for StackCfg {
    fn default() -> Self {
        Self {
            trunk: default_trunk(),
            preview_label: None,
            frontend_paths: Vec::new(),
            worktree_dir: None,
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

    /// Backend for a role: its own override, else the global default.
    pub fn backend_for(&self, role: &str) -> Backend {
        self.agents
            .get(role)
            .and_then(|a| a.backend)
            .unwrap_or(self.backend)
    }

    /// Steps for a pipeline by name, or the default pipeline.
    pub fn steps_for(&self, name: Option<&str>) -> Result<Vec<Step>> {
        match name {
            None => Ok(self.steps.clone()),
            Some(n) => self
                .pipelines
                .get(n)
                .map(|p| p.steps.clone())
                .ok_or_else(|| {
                    let known: Vec<&str> = self.pipelines.keys().map(|s| s.as_str()).collect();
                    anyhow::anyhow!(
                        "no pipeline `{n}`; known pipelines: {}",
                        if known.is_empty() { "(none)".into() } else { known.join(", ") }
                    )
                }),
        }
    }


    fn validate(&self) -> Result<()> {
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

/// Context-clearing command per agent kind.
pub fn clear_command(kind: &str) -> &'static str {
    match kind {
        "opencode" | "codex" | "gemini" => "/new",
        _ => "/clear",
    }
}

pub const TEMPLATE: &str = r#"# rigg pipeline
# Steps run top to bottom. Start them with: rigg run --task "..."

# How agent steps are driven. "herdr" prompts a live agent in a pane and waits
# for its turn to settle; "headless" runs the agent's own non-interactive mode
# once per step (claude -p, opencode run) and needs no herdr at all.
# `rigg run --headless` forces headless for one run.
backend = "herdr"

[agents.impl]
kind = "claude"
autostart = true
# Headless agents run unattended, so they need a permission mode or they will
# describe changes instead of making them. `acceptEdits` allows file edits;
# `bypassPermissions` also allows commands.
args = ["--permission-mode", "acceptEdits"]
# backend = "headless"           # per-role override
# command = ["my-agent", "--print", "{{prompt}}"]   # headless argv for an
                                 # agent rigg has no built-in command for

# A second model is the point of a Copilot-style review, without the round trip.
# NOTE: as of opencode 1.18.27 herdr cannot submit prompts to opencode - the
# text lands in the composer but Enter never fires. Until that is fixed, run the
# reviewer on claude too, or point `kind` at another working agent.
# [agents.reviewer]
# kind = "opencode"
# autostart = true

[stack]
trunk = "main"
preview_label = "preview"
frontend_paths = ["src/**/*.tsx", "src/**/*.css", "app/**"]

[[steps]]
id = "implement"
agent = "impl"
prompt = "{{task}}"
# `until` defaults to herdr's own set (idle, done, blocked). Claude finishes a
# turn in `done`, so do not pin this to `idle` - it will wait forever.

[[steps]]
id = "self-review"
agent = "impl"
clear = true                     # fresh context, as you do by hand
prompt = "/code-review"
# `clear` means the opposite thing per backend, because the underlying sessions
# are opposite: herdr sends /clear to a session that would otherwise carry
# context on, while headless steps resume the previous session (--continue)
# unless `clear` tells them to start a new one.

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
"#;
