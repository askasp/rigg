use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

/// Run a git command in `dir`, returning trimmed stdout.
pub fn git(dir: &std::path::Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `git {}`", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Repository root for the current working directory.
pub fn repo_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let root = git(&cwd, &["rev-parse", "--show-toplevel"])
        .context("not inside a git repository")?;
    Ok(PathBuf::from(root))
}

pub fn current_branch(root: &std::path::Path) -> Result<String> {
    // symbolic-ref also resolves an unborn branch (a repo with no commits yet);
    // rev-parse does not.
    git(root, &["symbolic-ref", "--short", "HEAD"])
        .or_else(|_| git(root, &["rev-parse", "--abbrev-ref", "HEAD"]))
}

/// Files changed between `base` and HEAD, including uncommitted work.
pub fn changed_files(root: &std::path::Path, base: &str) -> Result<Vec<String>> {
    let mut files = Vec::new();
    if let Ok(out) = git(root, &["diff", "--name-only", &format!("{base}...HEAD")]) {
        files.extend(out.lines().map(str::to_string));
    }
    if let Ok(out) = git(root, &["status", "--porcelain"]) {
        for line in out.lines() {
            if line.len() > 3 {
                files.push(line[3..].trim().to_string());
            }
        }
    }
    files.retain(|f| !f.is_empty());
    files.sort();
    files.dedup();
    Ok(files)
}

/// Create a worktree with plain git, for setups with no herdr.
pub fn git_worktree_add(
    main: &std::path::Path,
    branch: &str,
    base: &str,
    dest: &std::path::Path,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    git(
        main,
        &["worktree", "add", "-b", branch, &dest.to_string_lossy(), base],
    )?;
    Ok(())
}

/// The repository's primary checkout, which is where worktree creation has to
/// run from - herdr refuses to branch a new worktree off a linked one.
pub fn main_checkout(root: &std::path::Path) -> Result<PathBuf> {
    let common = git(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    Ok(std::path::Path::new(&common)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| root.to_path_buf()))
}

/// Working directories of every process we can see, used to avoid pulling a
/// checkout out from under something still running in it.
pub fn cwds_in_use() -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return set;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Ok(target) = std::fs::read_link(format!("/proc/{name}/cwd")) {
            set.insert(target.to_string_lossy().to_string());
        }
    }
    set
}

/// Checkout path for a branch's worktree, from git rather than from the
/// creating tool's JSON.
pub fn worktree_path(root: &std::path::Path, branch: &str) -> Option<String> {
    let out = git(root, &["worktree", "list", "--porcelain"]).ok()?;
    let mut current: Option<String> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            current = Some(p.to_string());
        } else if let Some(b) = line.strip_prefix("branch ") {
            if b.trim_start_matches("refs/heads/") == branch {
                return current;
            }
        }
    }
    None
}

/// Minimal `*` / `**` glob match, enough for path patterns like `src/**/*.tsx`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    fn go(p: &[char], t: &[char]) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        if p[0] == '*' {
            let mut rest = p;
            while !rest.is_empty() && rest[0] == '*' {
                rest = &rest[1..];
            }
            if rest.is_empty() {
                return true;
            }
            for i in 0..=t.len() {
                if go(rest, &t[i..]) {
                    return true;
                }
            }
            return false;
        }
        if !t.is_empty() && (p[0] == '?' || p[0] == t[0]) {
            return go(&p[1..], &t[1..]);
        }
        false
    }
    go(&p, &t)
}

/// Substitute `{{key}}` placeholders.
pub fn render(template: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    out
}

/// Run a shell command and return its stdout, echoing it so the run stays
/// readable. Used by steps that feed their output into a later prompt.
pub fn shell_capture(dir: &std::path::Path, cmd: &str) -> Result<String> {
    let out = Command::new("sh")
        .current_dir(dir)
        .arg("-c")
        .arg(cmd)
        .output()
        .with_context(|| format!("failed to spawn shell for `{cmd}`"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    print!("{stdout}");
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!(
            "command failed (exit {}): {cmd}\n{}",
            out.status.code().unwrap_or(-1),
            stderr.trim()
        );
    }
    Ok(stdout)
}

/// Run a shell command, streaming output to the terminal.
pub fn shell(dir: &std::path::Path, cmd: &str) -> Result<()> {
    let status = Command::new("sh")
        .current_dir(dir)
        .arg("-c")
        .arg(cmd)
        .status()
        .with_context(|| format!("failed to spawn shell for `{cmd}`"))?;
    if !status.success() {
        bail!("command failed (exit {}): {cmd}", status.code().unwrap_or(-1));
    }
    Ok(())
}
