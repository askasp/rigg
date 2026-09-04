use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::AgentCfg;

/// Argv for one non-interactive turn.
///
/// There is no live session to hold context between steps, so continuity has to
/// come from the agent's own resume flag: `continue_session` is set for every
/// step that did not ask for a fresh context. Both `claude -p` and
/// `opencode run` take `--continue`, and both start a new session instead of
/// failing when there is nothing to continue.
pub fn command_for(cfg: &AgentCfg, prompt: &str, continue_session: bool) -> Vec<String> {
    if let Some(template) = &cfg.command {
        let mut argv: Vec<String> = template
            .iter()
            .map(|a| a.replace("{{prompt}}", prompt))
            .collect();
        if !template.iter().any(|a| a.contains("{{prompt}}")) {
            argv.push(prompt.to_string());
        }
        return argv;
    }

    let (mut argv, resumable): (Vec<String>, bool) = match cfg.kind.as_str() {
        "claude" => (vec!["claude".into(), "-p".into()], true),
        "opencode" => (vec!["opencode".into(), "run".into()], true),
        // Anything else gets the bare binary and the prompt; `command` in
        // rigg.toml is how you say more than that.
        other => (vec![other.to_string()], false),
    };
    if continue_session && resumable {
        argv.push("--continue".into());
    }
    // Without something like `--permission-mode acceptEdits`, a headless agent
    // will describe the change it would make and edit nothing.
    argv.extend(cfg.args.iter().cloned());
    argv.push(prompt.to_string());
    argv
}

/// The executable a headless turn would spawn, for `rigg doctor`.
pub fn binary(cfg: &AgentCfg) -> String {
    cfg.command
        .as_ref()
        .and_then(|c| c.first())
        .cloned()
        .unwrap_or_else(|| cfg.kind.clone())
}

/// Run one turn to completion in `dir`, streaming its output to the terminal.
///
/// Process exit is the whole completion signal - there is no agent state to
/// wait on, which is the point of this backend.
pub fn run(dir: &Path, argv: &[String]) -> Result<()> {
    let (bin, args) = argv.split_first().context("empty agent command")?;
    // The prompt is an argument, and claude -p otherwise waits 3s for piped
    // stdin before warning and giving up on it.
    let status = Command::new(bin)
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("failed to spawn `{bin}`"))?;
    if !status.success() {
        bail!("`{bin}` exited with status {}", status.code().unwrap_or(-1));
    }
    Ok(())
}
