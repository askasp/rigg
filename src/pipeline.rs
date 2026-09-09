use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use crate::config::{Config, Step};
use crate::headless;
use crate::util;

pub struct Runner {
    pub root: PathBuf,
    pub cfg: Config,
    pub vars: BTreeMap<String, String>,
    pub dry_run: bool,
    /// Resume this exact conversation rather than the checkout's most recent.
    pub session: Option<String>,
    /// The id of the step last attempted, so a caller can record where the
    /// run got to without having to read its own log back.
    pub at: Option<String>,
    /// Where these steps sit in the pipeline they were cut from, 1-based, so a
    /// run picking up in the middle still reports `[8/9]` - the number a
    /// reader can find in the plan they were given when the branch started.
    /// Empty means these are the whole pipeline.
    pub numbers: Vec<usize>,
    /// How many steps that pipeline has. 0 means the same.
    pub total: usize,
    /// Whether to tell the notify hook about this run. Off for a single turn
    /// asked for by hand: a `say` is not a pipeline, and reporting it as
    /// "1 step(s)" three times says nothing and loses where the branch is.
    pub announce: bool,
}

impl Runner {
    pub fn new(
        root: PathBuf,
        cfg: Config,
        vars: BTreeMap<String, String>,
        dry_run: bool,
    ) -> Self {
        Self {
            root,
            cfg,
            vars,
            dry_run,
            session: None,
            at: None,
            numbers: Vec::new(),
            total: 0,
            announce: true,
        }
    }

    /// Tell the notify hook where the run has got to.
    ///
    /// Never fails a run: the hook is someone else's command, and a chat bot
    /// being down is not a reason to abandon work the agent already did.
    fn notify(&self, event: &str, message: &str, extra: &[(&str, String)]) {
        let Some(cmd) = self.cfg.notify.command.clone() else {
            return;
        };
        if self.dry_run || !self.announce {
            return;
        }
        let mut env: Vec<(&str, String)> = vec![
            ("RIGG_EVENT", event.to_string()),
            ("RIGG_MESSAGE", message.to_string()),
            ("RIGG_BRANCH", self.var("branch")),
            ("RIGG_REPO", self.var("repo")),
            ("RIGG_TASK", self.var("task")),
        ];
        env.extend(extra.iter().map(|(k, v)| (*k, v.clone())));
        if let Err(e) = util::shell_quiet(&self.root, &cmd, &env) {
            println!("  notify hook failed: {e}");
        }
    }

    fn var(&self, key: &str) -> String {
        self.vars.get(key).cloned().unwrap_or_default()
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
        // With no terminal the read returns EOF immediately. Treating that as
        // "no" would silently skip the step and report it as the user's choice.
        if std::io::stdin().read_line(&mut line)? == 0 {
            bail!(
                "step `{}` asks for confirmation but there is no terminal to ask. \
                 Run with --fg, or drop `confirm` from the step.",
                step.id
            );
        }
        Ok(matches!(line.trim(), "y" | "Y" | "yes"))
    }

    pub fn run_step(&mut self, step: &Step) -> Result<()> {
        if let Some(cmd) = &step.run {
            let cmd = util::render(cmd, &self.vars);
            // A multi-line script would otherwise dump its whole body into the
            // log before running.
            match cmd.lines().count() {
                0 | 1 => println!("  $ {}", cmd.trim()),
                n => println!(
                    "  $ {}  ... ({n} lines)",
                    cmd.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim()
                ),
            }
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
        let raw = step.prompt_text(&self.cfg.dir)?.unwrap_or_default();
        let text = util::render(&raw, &self.vars);

        // A one-shot invocation has no session of its own, so continuity comes
        // from the agent's resume flag; `clear` is what leaves it off.
        let argv =
            headless::command_for(&acfg, &text, !step.clear, self.session.as_deref());
        println!("  -> [{role}] {}", describe(&argv, &text));
        if self.dry_run {
            return Ok(());
        }
        headless::run(&self.root, &argv)
    }

    pub fn run(&mut self, steps: &[Step]) -> Result<()> {
        let count = steps.len();
        let total = if self.total > 0 { self.total } else { count };
        // Which step of the pipeline each of these is. A whole run numbers
        // itself; a resumed one was told.
        let number: Vec<usize> = (0..count)
            .map(|i| self.numbers.get(i).copied().unwrap_or(i + 1))
            .collect();
        let started = std::time::Instant::now();
        let branch = self.var("branch");
        self.notify(
            "start",
            &match steps.first() {
                Some(s) if count < total => format!(
                    "*{branch}* picking up at [{}/{total}] `{}` - {count} step(s) left",
                    number[0], s.id
                ),
                _ => format!("*{branch}* started - {total} step(s)"),
            },
            &[("RIGG_TOTAL", total.to_string())],
        );
        for (i, step) in steps.iter().enumerate() {
            let label = step.description.clone().unwrap_or_else(|| step.id.clone());
            let head = format!("[{}/{}] {}", number[i], total, label);
            // A rule per step, so the boundaries stay findable once the agent
            // output is streaming past.
            println!("\n{:-<4} {head} {:-<width$} {}", "", "", clock(),
                width = 64usize.saturating_sub(head.len()));
            let step_started = std::time::Instant::now();
            self.at = Some(step.id.clone());

            if !self.should_run(step)? {
                println!("  skipped: no matching changed files");
                continue;
            }
            if step.confirm && !self.dry_run && !self.confirm(step)? {
                println!("  skipped by user");
                continue;
            }

            let took = || elapsed(step_started);
            let mut at = vec![
                ("RIGG_STEP", step.id.clone()),
                ("RIGG_INDEX", number[i].to_string()),
                ("RIGG_TOTAL", total.to_string()),
            ];
            let outcome = self.run_step(step);
            at.push(("RIGG_ELAPSED", took()));
            let head = format!("*{branch}* [{}/{total}] `{}`", number[i], step.id);

            match outcome {
                Ok(()) => {
                    println!("  ok ({})", took());
                    at.push(("RIGG_STATUS", "ok".into()));
                    self.notify("step", &format!("{head} ok ({})", took()), &at);
                }
                Err(e) if step.continue_on_error => {
                    println!("  failed after {} (continuing): {e}", took());
                    at.push(("RIGG_STATUS", "failed".into()));
                    self.notify("step", &format!("{head} failed, continuing: {e}"), &at);
                }
                Err(e) => {
                    println!("  failed after {}", took());
                    at.push(("RIGG_STATUS", "failed".into()));
                    self.notify("failed", &format!("{head} failed: {e}"), &at);
                    return Err(e.context(format!(
                        "step `{}` failed after {}",
                        step.id,
                        took()
                    )));
                }
            }
        }
        println!("\ntotal {}", elapsed(started));
        self.notify(
            "done",
            &if count < total {
                format!(
                    "*{branch}* finished - {count} of {total} step(s) in {}",
                    elapsed(started)
                )
            } else {
                format!("*{branch}* finished - {total} step(s) in {}", elapsed(started))
            },
            &[("RIGG_TOTAL", total.to_string()), ("RIGG_ELAPSED", elapsed(started))],
        );
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

/// Local wall-clock time. Shelling out to `date` avoids a timezone dependency
/// for what is only a log annotation.
fn clock() -> String {
    std::process::Command::new("date")
        .arg("+%H:%M:%S")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn elapsed(from: std::time::Instant) -> String {
    let s = from.elapsed().as_secs();
    if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn first_line(s: &str) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if line.chars().count() > 72 {
        format!("{}...", line.chars().take(69).collect::<String>())
    } else {
        line.to_string()
    }
}
