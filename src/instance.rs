
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// The instance this process is acting as.
pub fn name() -> String {
    match std::env::var("RIGG_INSTANCE") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => "default".to_string(),
    }
}

/// Everything rigg keeps outside a repo.
pub fn root() -> PathBuf {
    if let Ok(v) = std::env::var("RIGG_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".rigg")
}

pub fn dir_for(inst: &str) -> PathBuf {
    root().join("instances").join(inst)
}

pub fn dir() -> PathBuf {
    dir_for(&name())
}

/// Templated by Ansible from a vault, and rewritten whole by a converge.
pub fn managed_path() -> PathBuf {
    dir().join("secrets.env")
}

/// Set from Slack or the command line.
pub fn local_path() -> PathBuf {
    dir().join("secrets.local.env")
}

pub fn cron_path() -> PathBuf {
    dir().join("cron.json")
}

/// The main checkouts this instance covers. A repo belongs to exactly one
/// instance, which is what lets a repo declare its own schedules.
pub fn repos_path() -> PathBuf {
    dir().join("repos.json")
}

pub fn repos() -> Vec<PathBuf> {
    let Ok(text) = fs::read_to_string(repos_path()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&text)
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

fn write_repos(list: &[PathBuf]) -> Result<()> {
    ensure_dir()?;
    let out: Vec<String> = list.iter().map(|p| p.display().to_string()).collect();
    let path = repos_path();
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&out)? + "\n")?;
    fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Returns false when it was already registered.
pub fn add_repo(path: &std::path::Path) -> Result<bool> {
    let mut list = repos();
    if list.iter().any(|p| p == path) {
        return Ok(false);
    }
    // Another instance holding the same repo would fire its schedules twice,
    // with neither side able to say why.
    for other in all() {
        if other == name() {
            continue;
        }
        let f = dir_for(&other).join("repos.json");
        if let Ok(t) = fs::read_to_string(&f) {
            if serde_json::from_str::<Vec<String>>(&t)
                .unwrap_or_default()
                .iter()
                .any(|p| std::path::Path::new(p) == path)
            {
                bail!("{} is already registered to instance `{other}`", path.display());
            }
        }
    }
    list.push(path.to_path_buf());
    list.sort();
    write_repos(&list)?;
    Ok(true)
}

pub fn rm_repo(path: &std::path::Path) -> Result<bool> {
    let mut list = repos();
    let before = list.len();
    list.retain(|p| p != path);
    if list.len() == before {
        return Ok(false);
    }
    write_repos(&list)?;
    Ok(true)
}

pub fn notes_path() -> PathBuf {
    dir().join("notes.md")
}

/// Where an ingested corpus lives.
pub fn corpus_dir() -> PathBuf {
    dir().join("corpus")
}

pub fn all() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(root().join("instances")) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                if let Some(n) = e.file_name().to_str() {
                    out.push(n.to_string());
                }
            }
        }
    }
    out.sort();
    out
}

/// Read a `KEY=value` file the way a shell sourcing it would, minus the shell.
pub fn parse_env(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        if k.is_empty() {
            continue;
        }
        let v = v.trim();
        let v = if v.len() >= 2
            && ((v.starts_with('"') && v.ends_with('"'))
                || (v.starts_with('\'') && v.ends_with('\'')))
        {
            &v[1..v.len() - 1]
        } else {
            v
        };
        out.insert(k.to_string(), v.to_string());
    }
    out
}

fn read_env(path: &std::path::Path) -> BTreeMap<String, String> {
    fs::read_to_string(path)
        .map(|t| parse_env(&t))
        .unwrap_or_default()
}

/// This instance's secrets, local over managed.
pub fn secrets() -> BTreeMap<String, String> {
    let mut out = read_env(&managed_path());
    out.extend(read_env(&local_path()));
    out
}

/// Where a name got its value, for a listing that can be acted on: a token from the vault is fixed by editing the vault, one set here by setting it again.
pub fn provenance(key: &str) -> &'static str {
    if read_env(&local_path()).contains_key(key) {
        "set here"
    } else if read_env(&managed_path()).contains_key(key) {
        "from the vault"
    } else {
        "unset"
    }
}

/// A name that could be exported by a shell.
pub fn valid_name(key: &str) -> bool {
    !key.is_empty()
        && key.starts_with(|c: char| c.is_ascii_uppercase())
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn ensure_dir() -> Result<()> {
    let d = dir();
    fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
    let _ = fs::set_permissions(&d, fs::Permissions::from_mode(0o700));
    Ok(())
}

fn write_local(map: &BTreeMap<String, String>) -> Result<()> {
    ensure_dir()?;
    let mut body = String::from(
        "# Set through rigg. Ansible does not template this file, so what is\n\
         # here survives a converge and wins over secrets.env on a clash.\n\
         # `rigg secret rm <NAME>` removes one.\n\n",
    );
    for (k, v) in map {
        body.push_str(&format!("{k}={v}\n"));
    }
    let path = local_path();
    let tmp = path.with_extension("env.tmp");
    fs::write(&tmp, &body).with_context(|| format!("writing {}", tmp.display()))?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

pub fn set_secret(key: &str, value: &str) -> Result<()> {
    if !valid_name(key) {
        bail!("`{key}` is not a name a shell could export - use A-Z, 0-9 and _");
    }
    let mut map = read_env(&local_path());
    map.insert(key.to_string(), value.to_string());
    write_local(&map)
}

/// Returns false when there was nothing to remove here, which is worth saying out loud: the name may still be set, from the vault, where rigg cannot reach it.
pub fn rm_secret(key: &str) -> Result<bool> {
    let mut map = read_env(&local_path());
    if map.remove(key).is_none() {
        return Ok(false);
    }
    write_local(&map)?;
    Ok(true)
}

/// Show enough of a value to recognise it and not enough to use it.
pub fn mask(value: &str) -> String {
    let n = value.chars().count();
    if n <= 4 {
        return "…".to_string();
    }
    let tail: String = value.chars().skip(n.saturating_sub(4)).collect();
    format!("…{tail}")
}

/// Put this instance's identity where a child process will find it.
/// An empty value is not a value: `FRONT_API_TOKEN=` is how a wrapper
/// exports one it failed to read, and treating it as present is how a run
/// gets as far as a 401.
pub fn set_in_env(key: &str) -> bool {
    std::env::var(key).map(|v| !v.trim().is_empty()).unwrap_or(false)
}

pub fn export() -> Result<()> {
    migrate()?;
    std::env::set_var("RIGG_INSTANCE", name());
    std::env::set_var("RIGG_MAIL_DIR", corpus_dir());
    std::env::set_var("RIGG_NOTES", notes_path());
    for (k, v) in secrets() {
        if !set_in_env(&k) {
            std::env::set_var(&k, &v);
        }
    }
    Ok(())
}

/// Move a pre-plane layout into the instance directory, once.
pub fn migrate() -> Result<Vec<String>> {
    let inst = name();
    let r = root();
    let d = dir_for(&inst);
    let moves: Vec<(PathBuf, PathBuf)> = vec![
        (r.join("secrets").join(format!("{inst}.env")), d.join("secrets.env")),
        (
            r.join("secrets").join(format!("{inst}.local.env")),
            d.join("secrets.local.env"),
        ),
        (r.join("cron").join(format!("{inst}.json")), d.join("cron.json")),
        (r.join("notes").join(format!("{inst}.md")), d.join("notes.md")),
        (
            r.join("mail").join(format!("{inst}.db")),
            d.join("corpus").join(format!("{inst}.db")),
        ),
    ];
    let mut moved = Vec::new();
    for (from, to) in moves {
        if !from.exists() || to.exists() {
            continue;
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::rename(&from, &to).is_ok() {
            moved.push(format!("{} -> {}", from.display(), to.display()));
        }
    }
    for old in ["secrets", "cron", "notes", "mail", "corpus"] {
        let _ = fs::remove_dir(r.join(old));
    }
    if !moved.is_empty() {
        let _ = fs::set_permissions(&d, fs::Permissions::from_mode(0o700));
        for f in [d.join("secrets.env"), d.join("secrets.local.env")] {
            if f.exists() {
                let _ = fs::set_permissions(&f, fs::Permissions::from_mode(0o600));
            }
        }
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_env_file_is_read_the_way_a_shell_would() {
        let m = parse_env(
            "# a comment\n\nexport A=1\nB=\"two words\"\nC='three'\nD=\nbad line\n",
        );
        assert_eq!(m.get("A").map(String::as_str), Some("1"));
        assert_eq!(m.get("B").map(String::as_str), Some("two words"));
        assert_eq!(m.get("C").map(String::as_str), Some("three"));
        assert_eq!(m.get("D").map(String::as_str), Some(""));
        assert!(!m.contains_key("bad line"));
    }

    #[test]
    fn a_mask_shows_the_tail_and_nothing_else() {
        assert_eq!(mask("abcdefghij"), "…ghij");
        assert_eq!(mask("abc"), "…");
    }

    #[test]
    fn only_a_name_a_shell_could_export_is_a_secret() {
        assert!(valid_name("FRONT_API_TOKEN"));
        assert!(valid_name("A1"));
        assert!(!valid_name("front_api_token"));
        assert!(!valid_name("1A"));
        assert!(!valid_name("A-B"));
        assert!(!valid_name(""));
    }
}
