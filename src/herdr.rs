use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::process::Command;

pub fn bin() -> String {
    std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into())
}

pub fn inside_session() -> bool {
    std::env::var("HERDR_ENV").is_ok()
}

pub fn current_workspace() -> Option<String> {
    std::env::var("HERDR_WORKSPACE_ID").ok()
}

/// Invoke the herdr CLI and capture stdout.
pub fn call<S: AsRef<str>>(args: &[S]) -> Result<String> {
    let args: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();
    let out = Command::new(bin())
        .args(&args)
        .output()
        .with_context(|| format!("failed to run `{} {}`", bin(), args.join(" ")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        bail!("herdr {} failed: {msg}", args.join(" "));
    }
    Ok(stdout)
}

fn call_json<S: AsRef<str>>(args: &[S]) -> Result<serde_json::Value> {
    let raw = call(args)?;
    serde_json::from_str(&raw).with_context(|| format!("could not parse herdr JSON: {raw}"))
}

#[derive(Debug, Clone, Deserialize)]
pub struct Agent {
    /// Agent kind, e.g. "claude" or "opencode".
    pub agent: String,
    pub agent_status: String,
    pub cwd: String,
    pub pane_id: String,
    pub workspace_id: String,
}

pub fn agents() -> Result<Vec<Agent>> {
    let v = call_json(&["agent", "list"])?;
    let arr = v
        .pointer("/result/agents")
        .ok_or_else(|| anyhow!("unexpected shape from `herdr agent list`"))?;
    serde_json::from_value(arr.clone()).context("decoding agent list")
}

/// Submit a prompt and block until the agent settles.
pub fn prompt(target: &str, text: &str, until: &[String], timeout_ms: Option<u64>) -> Result<()> {
    let mut args: Vec<String> = vec![
        "agent".into(),
        "prompt".into(),
        target.into(),
        text.into(),
        "--wait".into(),
    ];
    // With no --until, herdr matches idle, done or blocked - the right default.
    for state in until {
        args.push("--until".into());
        args.push(state.clone());
    }
    if let Some(ms) = timeout_ms {
        args.push("--timeout".into());
        args.push(ms.to_string());
    }
    call(&args)?;
    Ok(())
}

/// Submit a prompt without waiting.
///
/// Slash commands such as `/clear` settle instantly and never enter a working
/// state, so `--wait` would return `agent_prompt_stalled` after 5s. Those must
/// be sent fire-and-forget.
pub fn prompt_nowait(target: &str, text: &str) -> Result<()> {
    call(&["agent", "prompt", target, text])?;
    Ok(())
}

/// The agent's state-change counter, used to prove a prompt was actually
/// processed rather than matched against a state it was already sitting in.
pub fn agent_seq(target: &str) -> Result<u64> {
    let v = call_json(&["agent", "get", target])?;
    v.pointer("/result/agent/state_change_seq")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| anyhow!("no state_change_seq for {target}"))
}

/// Resolve a pane's working directory.
pub fn pane_cwd(pane_id: &str) -> Result<String> {
    let v = call_json(&["pane", "get", pane_id])?;
    Ok(v.pointer("/result/pane/cwd")
        .and_then(|p| p.as_str())
        .ok_or_else(|| anyhow!("could not read cwd for pane {pane_id}"))?
        .to_string())
}

fn pane_id_from(v: &serde_json::Value) -> Result<String> {
    Ok(v.pointer("/result/pane/pane_id")
        .or_else(|| v.pointer("/result/pane_id"))
        .and_then(|p| p.as_str())
        .ok_or_else(|| anyhow!("could not read new pane id from `herdr pane split`"))?
        .to_string())
}

/// Split a pane in `cwd` and run a command in it.
pub fn split_and_run(cwd: &str, cmd: &[&str]) -> Result<String> {
    let v = call_json(&["pane", "split", "--direction", "down", "--cwd", cwd, "--focus"])?;
    let pane_id = pane_id_from(&v)?;
    let mut args: Vec<&str> = vec!["pane", "run", &pane_id];
    args.extend_from_slice(cmd);
    call(&args)?;
    Ok(pane_id)
}

/// Split the current pane and start an agent of `kind` in it.
pub fn start_agent(name: &str, kind: &str, cwd: &str, extra: &[String]) -> Result<String> {
    let v = call_json(&[
        "pane",
        "split",
        "--current",
        "--direction",
        "right",
        "--cwd",
        cwd,
    ])?;
    let pane_id = pane_id_from(&v)?;

    let mut args: Vec<String> = vec![
        "agent".into(),
        "start".into(),
        name.into(),
        "--kind".into(),
        kind.into(),
        "--pane".into(),
        pane_id.clone(),
    ];
    if !extra.is_empty() {
        args.push("--".into());
        args.extend(extra.iter().cloned());
    }
    call(&args)?;
    Ok(pane_id)
}

pub fn worktree_create(branch: &str, base: &str, cwd: &str) -> Result<String> {
    let v = call_json(&[
        "worktree", "create", "--branch", branch, "--base", base, "--cwd", cwd, "--focus",
        "--json",
    ])?;
    Ok(v.pointer("/result/workspace/path")
        .or_else(|| v.pointer("/result/path"))
        .and_then(|p| p.as_str())
        .unwrap_or_default()
        .to_string())
}

/// Workspaces paired with the checkout they are open on.
pub fn workspaces() -> Result<Vec<(String, String)>> {
    let v = call_json(&["workspace", "list"])?;
    let arr = v
        .pointer("/result/workspaces")
        .and_then(|x| x.as_array())
        .ok_or_else(|| anyhow!("unexpected shape from `herdr workspace list`"))?;
    Ok(arr
        .iter()
        .filter_map(|w| {
            let id = w.get("workspace_id")?.as_str()?.to_string();
            let path = w.pointer("/worktree/checkout_path")?.as_str()?.to_string();
            Some((id, path))
        })
        .collect())
}

pub fn workspace_focus(id: &str) -> Result<()> {
    call(&["workspace", "focus", id])?;
    Ok(())
}

pub fn workspace_close(id: &str) -> Result<()> {
    call(&["workspace", "close", id])?;
    Ok(())
}

pub fn notify(text: &str) {
    let _ = call(&["notification", "show", text]);
}
