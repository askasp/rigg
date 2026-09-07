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

/// rigg's per-repository state directory, inside the shared git dir.
pub fn state_dir(root: &std::path::Path) -> Result<PathBuf> {
    let common = git(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    Ok(PathBuf::from(common).join("rigg"))
}

/// A readable random branch name, for when none was given.
pub fn random_name() -> String {
    const ADJ: [&str; 16] = [
        "swift", "quiet", "brave", "silver", "rapid", "calm", "lucky", "green",
        "amber", "clear", "bold", "warm", "keen", "still", "bright", "plain",
    ];
    const NOUN: [&str; 16] = [
        "harbor", "meadow", "forest", "river", "stone", "field", "cloud", "valley",
        "ridge", "delta", "grove", "beacon", "anchor", "summit", "hollow", "quarry",
    ];
    // /dev/urandom never reaches EOF, so take a fixed number of bytes.
    let mut r = [0u8; 4];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut r))
        .is_ok();
    if !ok {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        r = n.to_le_bytes();
    }
    format!(
        "{}-{}-{:02x}{:02x}",
        ADJ[r[0] as usize % ADJ.len()],
        NOUN[r[1] as usize % NOUN.len()],
        r[2],
        r[3]
    )
}

/// Create a worktree with plain git.
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

/// Create a worktree on a branch that already exists, so an existing branch
/// can be worked on without a new one being cut from it.
///
/// A branch only on a remote is materialised as a local one tracking it -
/// which is what `git checkout <branch>` does by hand, and what makes
/// adopting someone else's pushed branch a single command.
pub fn git_worktree_checkout(
    main: &std::path::Path,
    branch: &str,
    remote_ref: Option<&str>,
    dest: &std::path::Path,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dest = dest.to_string_lossy().to_string();
    match remote_ref {
        Some(r) => git(main, &["worktree", "add", "--track", "-b", branch, &dest, r])?,
        None => git(main, &["worktree", "add", &dest, branch])?,
    };
    Ok(())
}

/// Whether a branch has ever had a commit of its own.
///
/// A branch that committed nothing sits exactly where it was cut from, which
/// is a commit on the trunk - so `merge-base --is-ancestor` calls it merged,
/// and it would be pruned as landed work when it has landed nothing. A run
/// that changed nothing, or was asked a question rather than given a task,
/// leaves one behind.
///
/// The reflog is what remembers where a branch started. Where it cannot say,
/// this answers no: not removing something is the safe way to be wrong.
pub fn ever_committed(root: &std::path::Path, branch: &str) -> bool {
    let Ok(out) = git(root, &["reflog", "show", "--format=%H", branch]) else {
        return false;
    };
    let shas: Vec<&str> = out.split_whitespace().collect();
    // The oldest entry is where the branch was created; anything above is work.
    shas.len() > 1 && shas.first() != shas.last()
}

/// The remote-tracking ref for a branch that has no local ref yet, if exactly
/// one remote has it. Two remotes carrying the same name is a question rather
/// than something to guess at, so that returns nothing.
pub fn remote_branch(root: &std::path::Path, branch: &str) -> Option<String> {
    let remotes = git(root, &["remote"]).ok()?;
    let mut hits: Vec<String> = remotes
        .lines()
        .map(|r| format!("{}/{branch}", r.trim()))
        .filter(|r| {
            git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/{r}")]).is_ok()
        })
        .collect();
    match hits.len() {
        1 => hits.pop(),
        _ => None,
    }
}

/// The repository's primary checkout. Worktrees are created from there, so
/// every checkout hangs off one place rather than chaining off each other.
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

/// Run a shell command with extra environment, discarding its output.
///
/// Used for the notify hook, where the command is someone else's and its
/// chatter does not belong in the run log.
pub fn shell_quiet(dir: &std::path::Path, cmd: &str, env: &[(&str, String)]) -> Result<()> {
    let mut c = Command::new("sh");
    c.current_dir(dir).arg("-c").arg(cmd);
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c
        .output()
        .with_context(|| format!("failed to spawn shell for `{cmd}`"))?;
    if !out.status.success() {
        bail!(
            "exit {}: {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Run a shell command, streaming output to the terminal.
///
/// stderr is teed rather than inherited so that a failure can say *why*. A
/// multi-line script otherwise reported itself in full and the one line that
/// explained it - "nothing to push: no commits beyond main" - was left in the
/// log for someone to go and find.
pub fn shell(dir: &std::path::Path, cmd: &str) -> Result<()> {
    use std::io::{BufRead, Write};

    let mut child = Command::new("sh")
        .current_dir(dir)
        .arg("-c")
        .arg(cmd)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn shell for `{cmd}`"))?;

    let mut tail: Vec<String> = Vec::new();
    if let Some(err) = child.stderr.take() {
        for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
            let _ = writeln!(std::io::stderr(), "{line}");
            if !line.trim().is_empty() {
                tail.push(line);
                if tail.len() > 3 {
                    tail.remove(0);
                }
            }
        }
    }

    let status = child.wait()?;
    if !status.success() {
        // The first line that does something: `set -eu` and the comment above
        // it name the failure no better than the exit code does.
        let what = cmd
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("set "))
            .unwrap_or(cmd.trim());
        let what: String = what.chars().take(60).collect();
        let why = if tail.is_empty() {
            String::new()
        } else {
            format!(" - {}", tail.join("; "))
        };
        bail!("`{what}` failed (exit {}){why}", status.code().unwrap_or(-1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn random_name_is_bounded_and_shaped() {
        let n = super::random_name();
        assert_eq!(n.matches('-').count(), 2, "{n}");
        assert!(n.len() < 32, "{n}");
    }
}
