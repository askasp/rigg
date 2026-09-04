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
        // stream-json emits an event per step, so the log shows progress while
        // the turn runs instead of nothing until it ends.
        "claude" => (
            vec![
                "claude".into(),
                "-p".into(),
                "--output-format".into(),
                "stream-json".into(),
                "--verbose".into(),
            ],
            true,
        ),
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
    if argv.iter().any(|a| a == "stream-json") {
        return run_streaming(dir, bin, args);
    }
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

/// Run an agent that emits NDJSON events, printing a readable line per event.
fn run_streaming(dir: &Path, bin: &str, args: &[String]) -> Result<()> {
    use std::io::BufRead;

    let mut child = Command::new(bin)
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("could not start `{bin}`"))?;

    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines() {
            let line = line?;
            match serde_json::from_str::<serde_json::Value>(&line) {
                Ok(ev) => report(&ev),
                // Anything that is not an event is still worth seeing.
                Err(_) if !line.trim().is_empty() => println!("{line}"),
                Err(_) => {}
            }
        }
    }

    let status = child.wait()?;
    if !status.success() {
        bail!("agent exited with status {}", status.code().unwrap_or(-1));
    }
    Ok(())
}

/// Turn one stream event into at most one line of output.
fn report(ev: &serde_json::Value) {
    if ev.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return;
    }
    let Some(content) = ev.pointer("/message/content").and_then(|c| c.as_array()) else {
        return;
    };
    for block in content {
        match block.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("").trim();
                if !text.is_empty() {
                    println!("{text}");
                }
            }
            Some("tool_use") => {
                let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                let hint = ["file_path", "command", "pattern", "path", "description"]
                    .iter()
                    .find_map(|k| block.pointer(&format!("/input/{k}")).and_then(|v| v.as_str()))
                    .unwrap_or("");
                let hint: String = hint.lines().next().unwrap_or("").chars().take(70).collect();
                println!("  · {name} {hint}");
            }
            _ => {}
        }
    }
}
