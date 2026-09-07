//! Images attached to a task.
//!
//! A terminal cannot receive a pasted image, so rigg reads the system
//! clipboard itself rather than expecting one to arrive as input.
//!
//! Images are staged in the state dir when the task is written and copied into
//! the branch's checkout when its run starts. The detour is necessary: `add`
//! queues, so the worktree does not exist yet at the point the task is typed -
//! and an agent's working directory is its checkout, so anything under the main
//! checkout's `.git` is somewhere a linked worktree's agent is refused.

use anyhow::{Context, Result};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::util;

/// Where images live inside a checkout, relative to its root.
const MEDIA: &str = ".rigg/media";

/// Keeps the media directory out of `git status`, so it neither trips the
/// dirty-checkout guard nor lands in a commit.
const EXCLUDE_RULE: &str = "/.rigg/media/";

/// Image types worth carrying, in the order we ask the clipboard for them.
const TYPES: [(&str, &str); 4] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
    ("image/gif", "gif"),
];

/// Staging directory for a branch, in the shared git dir so a queued worker
/// finds what the shell that queued it put there.
fn stage_dir(root: &Path, branch: &str) -> Result<PathBuf> {
    Ok(util::state_dir(root)?
        .join("media")
        .join(branch.replace('/', "-")))
}

/// Stage every image for a task: explicit files first, then the clipboard.
///
/// Returns the number staged. The clipboard holds one image at a time, so
/// after taking one we offer to take another - copy the next, press enter.
/// Enter on an unchanged clipboard ends it.
pub fn collect(root: &Path, branch: &str, files: &[String], paste: bool) -> Result<usize> {
    let dir = stage_dir(root, branch)?;
    let start = next_index(&dir);
    let mut n = start;

    for f in files {
        let src = PathBuf::from(f);
        let bytes = std::fs::read(&src)
            .with_context(|| format!("reading image {}", src.display()))?;
        let ext = src
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png")
            .to_lowercase();
        write_one(&dir, n, &ext, &bytes)?;
        println!("  attached {} ({})", src.display(), human(bytes.len()));
        n += 1;
    }

    if paste {
        let mut last: Option<Vec<u8>> = None;
        while let Some((bytes, ext)) = clipboard_image() {
            // The same image still sitting there means the user is done.
            if last.as_deref() == Some(bytes.as_slice()) {
                break;
            }
            write_one(&dir, n, ext, &bytes)?;
            println!("  attached clipboard image ({})", human(bytes.len()));
            n += 1;
            last = Some(bytes);
            if !std::io::stdin().is_terminal() || !ask_for_another()? {
                break;
            }
        }
    }
    Ok(n - start)
}

/// Copy a branch's staged images into its checkout and return their paths
/// relative to it, ready to name in a prompt. Staging is cleared, so a second
/// `say` does not resend the first one's images.
pub fn materialize(root: &Path, checkout: &Path, branch: &str) -> Result<Vec<String>> {
    let dir = match stage_dir(root, branch) {
        Ok(d) if d.is_dir() => d,
        _ => return Ok(Vec::new()),
    };
    let mut staged: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    if staged.is_empty() {
        return Ok(Vec::new());
    }
    staged.sort();

    exclude_media(checkout)?;
    let dest = checkout.join(MEDIA);
    std::fs::create_dir_all(&dest)
        .with_context(|| format!("creating {}", dest.display()))?;

    // Continue the checkout's own numbering rather than overwriting what an
    // earlier turn already put there.
    let mut n = next_index(&dest);
    let mut rels = Vec::new();
    for src in &staged {
        let ext = src
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png")
            .to_string();
        let name = format!("{n}.{ext}");
        std::fs::copy(src, dest.join(&name))
            .with_context(|| format!("copying {} into {}", src.display(), dest.display()))?;
        rels.push(format!("{MEDIA}/{name}"));
        n += 1;
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(rels)
}

/// Point the agent at the images, since a prompt is only text.
pub fn decorate(task: &str, paths: &[String]) -> String {
    if paths.is_empty() {
        return task.to_string();
    }
    let (s, it) = if paths.len() > 1 { ("s", "them") } else { ("", "it") };
    let mut out = task.trim_end().to_string();
    out.push_str(&format!("\n\nAttached image{s} - read {it} first:\n"));
    for p in paths {
        out.push_str(&format!("  {p}\n"));
    }
    out
}

/// Drop a branch's staged images, for when the branch itself is going.
pub fn discard(root: &Path, branch: &str) {
    if let Ok(dir) = stage_dir(root, branch) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Read an image off the system clipboard, if one is on it.
pub fn clipboard_image() -> Option<(Vec<u8>, &'static str)> {
    let offered = offered_types()?;
    for (mime, ext) in TYPES {
        if !offered.iter().any(|t| t == mime) {
            continue;
        }
        match read_clipboard(mime) {
            Some(bytes) if !bytes.is_empty() => return Some((bytes, ext)),
            _ => {}
        }
    }
    None
}

/// What the clipboard is currently offering, via whichever tool is installed.
fn offered_types() -> Option<Vec<String>> {
    let attempts: [(&str, &[&str]); 2] = [
        ("wl-paste", &["--list-types"]),
        ("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"]),
    ];
    for (bin, args) in attempts {
        if let Some(out) = Command::new(bin).args(args).output().ok().filter(|o| o.status.success()) {
            return Some(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect(),
            );
        }
    }
    None
}

fn read_clipboard(mime: &str) -> Option<Vec<u8>> {
    let wl = ["--type", mime, "--no-newline"];
    let x = ["-selection", "clipboard", "-t", mime, "-o"];
    for (bin, args) in [("wl-paste", &wl[..]), ("xclip", &x[..])] {
        if let Some(out) = Command::new(bin).args(args).output().ok().filter(|o| o.status.success()) {
            return Some(out.stdout);
        }
    }
    None
}

/// One write covers every worktree: info/exclude lives in the common git dir.
fn exclude_media(checkout: &Path) -> Result<()> {
    let common = util::git(
        checkout,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let path = PathBuf::from(common).join("info").join("exclude");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if text.lines().any(|l| l.trim() == EXCLUDE_RULE) {
        return Ok(());
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(EXCLUDE_RULE);
    text.push('\n');
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn write_one(dir: &Path, n: usize, ext: &str, bytes: &[u8]) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{n}.{ext}"));
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// One past the highest number already used in `dir`, so nothing is clobbered.
fn next_index(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 1;
    };
    entries
        .flatten()
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<usize>().ok())
        })
        .max()
        .map_or(1, |m| m + 1)
}

fn ask_for_another() -> Result<bool> {
    use std::io::{BufRead, Write};
    print!("  copy another image and press enter, or just press enter to start> ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    // EOF means no terminal is answering, so stop rather than spin.
    Ok(std::io::stdin().lock().read_line(&mut line)? > 0)
}

fn human(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn decorate_names_every_image() {
        let out = super::decorate("fix this", &[".rigg/media/1.png".into(), ".rigg/media/2.png".into()]);
        assert!(out.starts_with("fix this"));
        assert!(out.contains("Attached images - read them first:"));
        assert!(out.contains("  .rigg/media/1.png"));
        assert!(out.contains("  .rigg/media/2.png"));
    }

    #[test]
    fn decorate_leaves_a_bare_task_alone() {
        assert_eq!(super::decorate("fix this", &[]), "fix this");
    }
}
