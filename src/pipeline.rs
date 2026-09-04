use anyhow::{anyhow, bail, Result};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::PathBuf;

use crate::config::{clear_command, Backend, Config, Step};
use crate::headless;
use crate::herdr;
use crate::util;

/// Slash commands settle instantly; give the TUI a beat before the real prompt.
const CLEAR_SETTLE_MS: u64 = 900;

pub struct Runner {
    pub root: PathBuf,
    pub cfg: Config,
    pub vars: BTreeMap<String, String>,
    pub dry_run: bool,
    force_headless: bool,
    targets: HashMap<String, String>,
}

impl Runner {
    pub fn new(
        root: PathBuf,
        cfg: Config,
        vars: BTreeMap<String, String>,
        dry_run: bool,
        force_headless: bool,
    ) -> Self {
        Self {
            root,
            cfg,
            vars,
            dry_run,
            force_headless,
            targets: HashMap::new(),
        }
    }

    fn backend_for(&self, role: &str) -> Backend {
        if self.force_headless {
            Backend::Headless
        } else {
            self.cfg.backend_for(role)
        }
    }

    /// Map an agent role from the config onto a herdr agent target.
    fn resolve_target(&mut self, role: &str) -> Result<String> {
        if let Some(t) = self.targets.get(role) {
            return Ok(t.clone());
        }
        let acfg = self
            .cfg
            .agents
            .get(role)
            .ok_or_else(|| anyhow!("unknown agent role `{role}`"))?
            .clone();

        if let Some(name) = &acfg.name {
            self.targets.insert(role.into(), name.clone());
            return Ok(name.clone());
        }

        let root_str = self.root.to_string_lossy().to_string();
        // Never target the pane rigg is running in: an agent that launched the
        // pipeline would otherwise be handed its own prompts.
        let own_pane = std::env::var("HERDR_PANE_ID").unwrap_or_default();
        let all = herdr::agents()?;
        let mut matches: Vec<_> = all
            .iter()
            .filter(|a| a.agent == acfg.kind && a.cwd == root_str && a.pane_id != own_pane)
            .collect();

        // Prefer agents in the workspace we were invoked from.
        if let Some(ws) = herdr::current_workspace() {
            if matches.iter().any(|a| a.workspace_id == ws) {
                matches.retain(|a| a.workspace_id == ws);
            }
        }

        let target = match matches.len() {
            1 => matches[0].pane_id.clone(),
            0 => {
                if !acfg.autostart {
                    bail!(
                        "no `{}` agent running in {root_str}. Start one, or set \
                         `autostart = true` for role `{role}`.",
                        acfg.kind
                    );
                }
                println!("  starting {} for role `{role}`...", acfg.kind);
                herdr::start_agent(role, &acfg.kind, &root_str, &acfg.args)?
            }
            _ => {
                let panes: Vec<_> = matches.iter().map(|a| a.pane_id.as_str()).collect();
                bail!(
                    "{} `{}` agents match role `{role}` ({}). Name one with \
                     `herdr agent rename <pane> <name>` and set `name` in rigg.toml.",
                    matches.len(),
                    acfg.kind,
                    panes.join(", ")
                );
            }
        };
        self.targets.insert(role.into(), target.clone());
        Ok(target)
    }

    fn should_run(&self, step: &Step) -> Result<bool> {
        let Some(globs) = &step.when_changed else {
            return Ok(true);
        };
        let base = self.vars.get("base").cloned().unwrap_or_default();
        let files = util::changed_files(&self.root, &base)?;
        Ok(files
            .iter()
            .any(|f| globs.iter().any(|g| util::glob_match(g, f))))
    }

    fn confirm(&self, step: &Step) -> Result<bool> {
        print!("  run step `{}`? [y/N] ", step.id);
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(matches!(line.trim(), "y" | "Y" | "yes"))
    }

    pub fn run_step(&mut self, step: &Step) -> Result<()> {
        if let Some(cmd) = &step.run {
            let cmd = util::render(cmd, &self.vars);
            println!("  $ {cmd}");
            if self.dry_run {
                return Ok(());
            }
            return match &step.capture {
                Some(var) => {
                    let out = util::shell_capture(&self.root, &cmd)?;
                    self.vars.insert(var.clone(), out);
                    Ok(())
                }
                None => util::shell(&self.root, &cmd),
            };
        }

        let role = step.agent.clone().expect("validated: prompt implies agent");
        let acfg = self.cfg.agents[&role].clone();
        let kind = acfg.kind.clone();
        let text = util::render(step.prompt.as_deref().unwrap_or(""), &self.vars);

        if self.backend_for(&role) == Backend::Headless {
            // A one-shot invocation has no session to clear, so `clear` inverts
            // here: it means "do not resume the previous step's session".
            let argv = headless::command_for(&acfg, &text, !step.clear);
            println!("  -> [{role}] {}", describe(&argv, &text));
            if self.dry_run {
                return Ok(());
            }
            return headless::run(&self.root, &argv);
        }

        let until: Vec<String> = step
            .until
            .clone()
            .map(|u| u.into_vec())
            .unwrap_or_default();

        if self.dry_run {
            println!("  -> [{role}] {}", first_line(&text));
            return Ok(());
        }

        let target = self.resolve_target(&role)?;

        if step.clear {
            let cmd = clear_command(&kind);
            println!("  -> [{role}] {cmd}");
            herdr::prompt_nowait(&target, cmd)?;
            std::thread::sleep(std::time::Duration::from_millis(CLEAR_SETTLE_MS));
        }

        println!("  -> [{role}] {}", first_line(&text));
        // An agent parked behind a modal - claude's trust-this-folder dialog is
        // the common one in a fresh worktree - reports `idle`, so the wait can
        // match the state it was already in and the step would pass having done
        // nothing. Require the state counter to actually move.
        let before = herdr::agent_seq(&target).ok();
        herdr::prompt(&target, &text, &until, step.timeout_ms)?;
        let after = herdr::agent_seq(&target).ok();
        if before.is_some() && before == after {
            bail!(
                "agent `{role}` never processed the prompt (state did not change). \
                 It is probably waiting on a dialog - check pane {target}."
            );
        }
        Ok(())
    }

    pub fn run(&mut self, steps: &[Step]) -> Result<()> {
        let total = steps.len();
        for (i, step) in steps.iter().enumerate() {
            let label = step.description.clone().unwrap_or_else(|| step.id.clone());
            println!("\n[{}/{}] {} ({})", i + 1, total, label, step.id);

            if !self.should_run(step)? {
                println!("  skipped: no matching changed files");
                continue;
            }
            if step.confirm && !self.dry_run && !self.confirm(step)? {
                println!("  skipped by user");
                continue;
            }

            match self.run_step(step) {
                Ok(()) => println!("  ok"),
                Err(e) if step.continue_on_error => {
                    println!("  failed (continuing): {e}");
                }
                Err(e) => {
                    herdr::notify(&format!("rigg: step `{}` failed", step.id));
                    return Err(e.context(format!("step `{}` failed", step.id)));
                }
            }
        }
        Ok(())
    }
}

/// Render argv for display, standing the prompt down to its first line.
fn describe(argv: &[String], prompt: &str) -> String {
    argv.iter()
        .map(|a| {
            if a == prompt {
                format!("<{}>", first_line(prompt))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn first_line(s: &str) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if line.chars().count() > 72 {
        format!("{}...", line.chars().take(69).collect::<String>())
    } else {
        line.to_string()
    }
}
